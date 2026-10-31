//! 一个待处理请求、递增序号，以及本地结果和文件结果的合并。
//!
//! 组合中的预编辑不要调用 [`Dispatch::submit`]。起点时间是已提交文本变化的时刻。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::apps::{
    AppHit, AppIndex, HiddenApp, LaunchTarget, UserCatalog, WallEntry, launch_key,
    load_user_catalog, save_user_catalog,
};
use crate::files::{
    FILE_INDEX_UNAVAILABLE, FileCommands, FileHit, FileKind, FileQueryResult, FileSource,
};
use crate::notes::NoteCommands;
use crate::search::{
    DISPLAY_LIMIT, FieldRole, Hit, HitKind, OPEN_FILE_LABEL, OPEN_WEBSITE_LABEL, PathRead,
    READ_TIMEOUT_LABEL, RankedGroup, SearchGroup, WebSearchEngine, allocate_display,
    classify_prefix, load_system, looks_like_local_path, open_website, present, rank_hits,
};
use crate::shelves::{ShelfCommands, normalize_path, normalize_web_url};
use crate::storage::{self, EntityKind, Error as StorageError, StartupError, Store};
use crate::todos::TodoCommands;

use super::icon::{IconCache, IconLoader, UserBatch};
use super::icon_disk::ICON_DISK_DIR;
use super::index::MemoryIndex;
use super::model::{
    DispatchError, FILE_DELAY_NS, FileProgress, LATENCY_METRIC_RESULT, LATENCY_SOURCE_EVERYTHING,
    LATENCY_SOURCE_LOCAL, LATENCY_SOURCE_WINDOWS_SEARCH, LatencyRecord, QueryView, RowDetail,
    SearchRow, Surface, UsageKey, ViewPhase,
};
use super::usage::{UsageHandle, UsageKeep, rank_query_hash, rank_weight};

/// 单调时钟的纳秒。测量工具只做减法，不把它当成 Unix 时间。
pub trait Monotonic: Send {
    fn now_ns(&self) -> u64;
}

/// 进程启动后的单调时间。
#[derive(Debug, Default)]
pub struct SystemClock;

impl Monotonic for SystemClock {
    fn now_ns(&self) -> u64 {
        static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let origin = ORIGIN.get_or_init(std::time::Instant::now);
        u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// 应用来源。正式构建传 [`AppIndex`]。
pub trait AppLookup: Send {
    fn query(&mut self, text: &str) -> Vec<AppHit>;

    /// 应用墙。[`crate::apps::AppIndex`] 只列开始菜单来源里未隐藏的条目。
    /// 默认实现没有整表，只把非空过滤交给 [`Self::query`]。
    fn wall(&mut self, filter: &str) -> Vec<WallEntry> {
        let filter = filter.trim();
        if filter.is_empty() {
            Vec::new()
        } else {
            self.query(filter)
                .into_iter()
                .map(|hit| WallEntry {
                    entry: hit.entry,
                    kind: Some(hit.kind),
                })
                .collect()
        }
    }

    fn hide(&mut self, _key: &str) -> Result<(), String> {
        Err("应用索引不支持隐藏".to_owned())
    }

    fn restore(&mut self, _key: &str) -> bool {
        false
    }

    fn hidden_apps(&mut self) -> Vec<HiddenApp> {
        Vec::new()
    }

    fn user_catalog(&mut self) -> UserCatalog {
        UserCatalog::default()
    }

    /// 导入替换 `user-apps.json` 之后，用磁盘上的那份换掉内存。默认不支持。
    fn replace_user_catalog(&mut self, _catalog: UserCatalog) -> Result<(), String> {
        Err("应用索引不支持更换用户目录".to_owned())
    }
}

impl AppLookup for AppIndex {
    fn query(&mut self, text: &str) -> Vec<AppHit> {
        AppIndex::query(self, text)
    }

    fn wall(&mut self, filter: &str) -> Vec<WallEntry> {
        AppIndex::wall(self, filter)
    }

    fn hide(&mut self, key: &str) -> Result<(), String> {
        AppIndex::hide(self, key).map_err(|err| err.to_string())
    }

    fn restore(&mut self, key: &str) -> bool {
        AppIndex::restore(self, key)
    }

    fn hidden_apps(&mut self) -> Vec<HiddenApp> {
        AppIndex::hidden_apps(self)
    }

    fn user_catalog(&mut self) -> UserCatalog {
        AppIndex::user_catalog(self)
    }

    fn replace_user_catalog(&mut self, catalog: UserCatalog) -> Result<(), String> {
        AppIndex::set_user_catalog(self, catalog)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

/// 文件来源。正式构建传 [`FileCommands`]。序号原样带回。
pub trait FileLookup: Send {
    fn query(&mut self, sequence: u64, text: &str) -> FileQueryResult;
}

impl FileLookup for FileCommands {
    fn query(&mut self, sequence: u64, text: &str) -> FileQueryResult {
        FileCommands::query(self, sequence, text)
    }
}

/// 待办、便签和收纳服务。索引在启动完成之后读它们的内存。
///
/// 收纳打不开时 `shelves` 为 `None`，搜索里的「网页」组为空。
pub struct Services {
    pub todos: TodoCommands,
    pub notes: NoteCommands,
    pub shelves: Option<ShelfCommands>,
}

/// 可替换的来源和时钟。测试用它换掉文件查询和时钟。
pub struct Sides<A, F, C, I> {
    pub apps: A,
    pub files: F,
    pub clock: C,
    pub icons: I,
}

/// 搜索条和面板共用的查询调度。
///
/// 调用顺序：`TodoCommands::boot` 成功，再 `NoteCommands::open`（导入恢复会换文件），
/// 然后 [`Dispatch::build`]。这几份服务必须就是正在写入的那一份；克隆仍共享内存。
/// 查询时不读盘。变更消息发出时，待办、便签和收纳的内存已经是新值。
pub struct Dispatch {
    store: Store,
    inner: Mutex<Inner>,
    usage: Arc<UsageHandle>,
    apps: Mutex<Box<dyn AppLookup>>,
    files: Mutex<Box<dyn FileLookup>>,
    clock: Mutex<Box<dyn Monotonic>>,
    icons: Arc<Mutex<IconCache>>,
    /// `None` 时用系统读取。测试放进一个替身，避免碰真实磁盘。
    paths: Mutex<Option<Box<dyn PathLookup>>>,
}

struct Inner {
    todos: TodoCommands,
    notes: NoteCommands,
    shelves: Option<ShelfCommands>,
    index: MemoryIndex,
    changes: std::sync::mpsc::Receiver<crate::storage::EntityChanged>,
    built: bool,
    generation: u64,
    sequence: u64,
    mode: Mode,
    latency: Vec<LatencyRecord>,
    web_engine: WebSearchEngine,
}

enum Mode {
    Empty,
    Capture,
    Search(SearchState),
}

struct SearchState {
    sequence: u64,
    generation: u64,
    text: String,
    start_ns: u64,
    /// 本地分组已经放进这次搜索。文件请求要等它为真，避免文件行先于应用、待办和便签出现。
    local_ready: bool,
    local_groups: Vec<RankedGroup<SearchRow>>,
    file_hits: Vec<FileHit>,
    file: FileSlot,
    unavailable: Option<&'static str>,
    /// 本机路径这一组。接在其它分组前面，不占 20 条名额。
    path_rows: Vec<SearchRow>,
    /// Ctrl+Backspace 写回的上一层。
    path_up: Option<String>,
    path: PathSlot,
}

enum FileSlot {
    Waiting { not_before_ns: u64 },
    InFlight,
    Done,
    Skipped,
}

enum PathSlot {
    Idle,
    Pending,
    InFlight,
    Done,
}

/// 后台线程要读的一次路径。序号过期的结果丢掉。
pub struct PathRequest {
    sequence: u64,
    generation: u64,
    text: String,
}

/// 测试替换本机路径的读取。正式构建不设置，走 [`load_system`]。
pub(crate) trait PathLookup: Send {
    fn load(&mut self, text: &str) -> PathRead;
}

struct SearchPlan {
    sequence: u64,
    generation: u64,
    text: String,
}

struct FileJob {
    sequence: u64,
    generation: u64,
    start_ns: u64,
    text: String,
}

enum Begun {
    Ready(QueryView),
    NeedApps(SearchPlan),
}

/// `poll` 的结果。`sent` 只表示文件请求已经发出。
pub struct PollReport {
    pub sent: bool,
    pub accepted: bool,
    pub view: QueryView,
}

fn save_catalog(store: &crate::storage::Store, apps: &mut dyn AppLookup) -> Result<(), String> {
    let catalog = apps.user_catalog();
    store.with_write_lock(|| save_user_catalog(&store.user_apps_path(), &catalog))
}

impl Dispatch {
    /// 用系统时钟和壳层图标缓存打开。
    #[must_use]
    pub fn open(store: Store, services: Services, apps: AppIndex, files: FileCommands) -> Self {
        Self::new(
            store,
            services,
            Sides {
                apps,
                files,
                clock: SystemClock,
                icons: super::icon::ShellIcons,
            },
        )
    }

    #[must_use]
    pub fn new<A, F, C, I>(store: Store, services: Services, sides: Sides<A, F, C, I>) -> Self
    where
        A: AppLookup + 'static,
        F: FileLookup + 'static,
        C: Monotonic + 'static,
        I: IconLoader + 'static,
    {
        let changes = store.subscribe();
        let usage = UsageHandle::open(store.clone());
        let icon_dir = store.cache_dir().join(ICON_DISK_DIR);
        Self {
            store,
            usage,
            inner: Mutex::new(Inner {
                todos: services.todos,
                notes: services.notes,
                shelves: services.shelves,
                index: MemoryIndex::empty(),
                changes,
                built: false,
                generation: 0,
                sequence: 0,
                mode: Mode::Empty,
                latency: Vec::new(),
                web_engine: WebSearchEngine::default(),
            }),
            apps: Mutex::new(Box::new(sides.apps)),
            files: Mutex::new(Box::new(sides.files)),
            clock: Mutex::new(Box::new(sides.clock)),
            icons: Arc::new(Mutex::new(IconCache::with_disk(sides.icons, icon_dir))),
            paths: Mutex::new(None),
        }
    }

    /// 加载修复和导入恢复完成之后才建索引。失败时索引仍不可用。
    pub fn build(&self) -> Result<(), DispatchError> {
        let built = self.store.build_index(|_| {
            let mut inner = storage::lock_mutex(&self.inner);
            inner
                .rebuild()
                .map_err(|message| StorageError::Startup(StartupError::Load { message }))?;
            inner.built = true;
            Ok(())
        });
        if built.is_ok() {
            self.flush_usage();
        }
        match built {
            Ok(()) => Ok(()),
            Err(StorageError::NotReady) => Err(DispatchError::NotReady),
            Err(err) => Err(DispatchError::Index(err.to_string())),
        }
    }

    /// 搜索和收纳页共用的那一份收纳命令。收纳服务没有打开时为 `None`。
    pub fn shelf_commands(&self) -> Option<ShelfCommands> {
        storage::lock_mutex(&self.inner).shelves.clone()
    }

    /// 按启动目标隐藏，并写入 `user-apps.json`。写盘失败时把内存改回去。
    pub fn hide_app(&self, key: &str) -> Result<(), String> {
        let mut apps = storage::lock_mutex(&self.apps);
        apps.hide(key)?;
        if let Err(err) = save_catalog(&self.store, &mut **apps) {
            let _ = apps.restore(key);
            return Err(err);
        }
        Ok(())
    }

    /// 从隐藏名单去掉这个键，并写入 `user-apps.json`。本来就没有时不写盘。
    /// 写盘失败时把这一项加回内存。
    pub fn restore_hidden_app(&self, key: &str) -> Result<(), String> {
        let mut apps = storage::lock_mutex(&self.apps);
        if !apps.restore(key) {
            return Ok(());
        }
        if let Err(err) = save_catalog(&self.store, &mut **apps) {
            let _ = apps.hide(key);
            return Err(err);
        }
        Ok(())
    }

    /// 应用墙上的条目。正式索引只列开始菜单来源里未隐藏的应用，过滤与搜索里的应用同一套匹配。
    #[must_use]
    pub fn wall_apps(&self, filter: &str) -> Vec<WallEntry> {
        storage::lock_mutex(&self.apps).wall(filter)
    }

    /// 设置页要列出的隐藏项。
    #[must_use]
    pub fn hidden_apps(&self) -> Vec<HiddenApp> {
        storage::lock_mutex(&self.apps).hidden_apps()
    }

    /// 从磁盘重新读 `user-apps.json`。文件不在时换成空目录。
    pub fn reload_user_catalog(&self) -> Result<(), String> {
        let catalog = load_user_catalog(&self.store.user_apps_path())?;
        storage::lock_mutex(&self.apps).replace_user_catalog(catalog)
    }

    /// 已提交的文本变化。
    ///
    /// 搜索条会先分类。收集前缀不入查询队列，并使旧序号失效。面板不分类。
    /// 预编辑期间不要调用。
    pub fn submit(&self, surface: Surface, text: &str) -> Result<QueryView, DispatchError> {
        let now = self.now_ns();
        let begun = {
            let mut inner = storage::lock_mutex(&self.inner);
            inner.sync()?;
            inner.begin(surface, text, now)?
        };
        match begun {
            Begun::Ready(view) => Ok(view),
            Begun::NeedApps(plan) => {
                let apps = storage::lock_mutex(&self.apps).query(&plan.text);
                let mut inner = storage::lock_mutex(&self.inner);
                Ok(inner.finish_local(plan, apps, &self.usage))
            }
        }
    }

    /// 60ms 到了才调用文件来源。调用期间不持有调度锁，迟到结果按序号丢掉。
    pub fn poll(&self) -> Result<PollReport, DispatchError> {
        let now = self.now_ns();
        let job = {
            let mut inner = storage::lock_mutex(&self.inner);
            inner.sync()?;
            inner.take_file_job(now)
        };
        let Some(job) = job else {
            return Ok(PollReport {
                sent: false,
                accepted: false,
                view: self.view(),
            });
        };
        let result = storage::lock_mutex(&self.files).query(job.sequence, &job.text);
        let mut inner = storage::lock_mutex(&self.inner);
        let accepted = inner.finish_file(&job, result);
        let view = inner.view();
        Ok(PollReport {
            sent: true,
            accepted,
            view,
        })
    }

    /// 有本机路径要读时取出这一次。界面线程只调用 [`Self::submit`]，不在这里读目录。
    pub fn take_path_request(&self) -> Option<PathRequest> {
        let mut inner = storage::lock_mutex(&self.inner);
        let Mode::Search(state) = &mut inner.mode else {
            return None;
        };
        if !matches!(state.path, PathSlot::Pending) {
            return None;
        }
        let request = PathRequest {
            sequence: state.sequence,
            generation: state.generation,
            text: state.text.clone(),
        };
        state.path = PathSlot::InFlight;
        Some(request)
    }

    /// 在后台线程里读路径并写入这次搜索。序号已经变了就丢掉，返回 `false`。
    ///
    /// 不返回当时的结果。文件阶段可能在这之后才合并，界面投递时再读 [`Self::view`]。
    pub fn complete_path(&self, request: PathRequest) -> bool {
        let read = self.read_path(&request.text);
        let mut inner = storage::lock_mutex(&self.inner);
        let current = matches!(
            &inner.mode,
            Mode::Search(state)
                if state.sequence == request.sequence && state.generation == request.generation
        );
        if !current {
            return false;
        }
        let query = match &inner.mode {
            Mode::Search(state) => state.text.clone(),
            Mode::Empty | Mode::Capture => String::new(),
        };
        let hash = rank_query_hash(&query);
        self.usage.with_book(|book| {
            inner.apply_path(read, |path, folder| {
                let key = if folder {
                    UsageKey::Folder(path.to_owned())
                } else {
                    UsageKey::File(path.to_owned())
                };
                rank_weight(book, hash.as_deref(), &key)
            });
        });
        true
    }

    /// 测试用。替换之后，[`Self::complete_path`] 不再访问系统目录。
    #[cfg(test)]
    pub(crate) fn set_path_source(&self, source: Box<dyn PathLookup>) {
        *storage::lock_mutex(&self.paths) = Some(source);
    }

    fn read_path(&self, text: &str) -> PathRead {
        let mut slot = storage::lock_mutex(&self.paths);
        if let Some(custom) = slot.as_mut() {
            return custom.load(text);
        }
        drop(slot);
        load_system(text)
    }

    #[must_use]
    pub fn view(&self) -> QueryView {
        storage::lock_mutex(&self.inner).view()
    }

    /// 界面只接受当前序号。收集、空输入和更新的输入之后，旧序号为假。
    #[must_use]
    pub fn accepts(&self, sequence: u64) -> bool {
        storage::lock_mutex(&self.inner).accepts(sequence)
    }

    /// 网页搜索那一条用的搜索引擎。下一次投影这次搜索时读取。
    pub fn set_web_search_engine(&self, engine: WebSearchEngine) {
        storage::lock_mutex(&self.inner).web_engine = engine;
    }

    /// 从搜索结果打开一项。总次数和当前这次输入的次数各加 1。
    ///
    /// 当前输入已经清掉时，只加总次数。写盘由 [`Self::flush_usage`] 完成。
    pub fn record_open(&self, key: &UsageKey) -> bool {
        let query = {
            let inner = storage::lock_mutex(&self.inner);
            match &inner.mode {
                Mode::Search(state) => state.text.clone(),
                Mode::Empty | Mode::Capture => String::new(),
            }
        };
        self.record_open_for_query(key, &query)
    }

    /// 用打开当时的输入计次。搜索条收起之后仍用这一份。
    pub fn record_open_for_query(&self, key: &UsageKey, query: &str) -> bool {
        self.usage.record(key, query)
    }

    /// 把内存里的次数写入 `search-usage.json`。没有变化时不写。
    pub fn flush_usage(&self) {
        let keep = storage::lock_mutex(&self.inner).usage_keep();
        self.usage.flush(&keep);
    }

    #[must_use]
    pub fn usage_count(&self, key: &UsageKey) -> u64 {
        self.usage.item_count(key)
    }

    #[must_use]
    pub fn usage_query_count(&self, key: &UsageKey, query: &str) -> u64 {
        self.usage.query_count_text(key, query)
    }

    #[must_use]
    pub fn resident_todo_ids(&self) -> Vec<String> {
        storage::lock_mutex(&self.inner).index.todo_ids()
    }

    #[must_use]
    pub fn resident_note_ids(&self) -> Vec<String> {
        storage::lock_mutex(&self.inner).index.note_ids()
    }

    /// 搜索和收纳共用的缓存。
    #[must_use]
    pub fn icon_cache(&self) -> Arc<Mutex<IconCache>> {
        Arc::clone(&self.icons)
    }

    /// 只取当前可见行的图标。被 20 条名额丢掉的命中不会来取。
    /// 这一批算作用户请求，空闲预热会等到它结束再继续。
    pub fn load_visible_icons(&self) -> Vec<(super::model::IconKey, super::icon::RgbaImage)> {
        let keys = storage::lock_mutex(&self.inner).visible_icon_keys();
        let batch = UserBatch::enter(&self.icons);
        let mut loaded = Vec::new();
        for key in keys {
            if let Some(image) = batch.load(&key) {
                loaded.push((key, image));
            }
        }
        loaded
    }

    #[must_use]
    pub fn latency_records(&self) -> Vec<LatencyRecord> {
        storage::lock_mutex(&self.inner).latency.clone()
    }

    pub fn drain_latency_records(&self) -> Vec<LatencyRecord> {
        std::mem::take(&mut storage::lock_mutex(&self.inner).latency)
    }

    /// 界面在同序号的结果第一次渲染完成时调用。不依赖窗口类型。
    pub fn mark_rendered(&self, sequence: u64, source: &str, end_ns: u64) -> bool {
        let mut inner = storage::lock_mutex(&self.inner);
        let Some(record) = inner
            .latency
            .iter_mut()
            .find(|record| record.seq == sequence && record.source == Some(source))
        else {
            return false;
        };
        record.end_ns = Some(end_ns);
        true
    }

    pub fn mark_warmup(&self, sequence: u64) {
        let mut inner = storage::lock_mutex(&self.inner);
        for record in &mut inner.latency {
            if record.seq == sequence {
                record.warmup = true;
            }
        }
    }

    fn now_ns(&self) -> u64 {
        storage::lock_mutex(&self.clock).now_ns()
    }
}

impl Inner {
    fn rebuild(&mut self) -> Result<(), String> {
        self.index.rebuild_todos(&self.todos)?;
        self.index.rebuild_notes(&self.notes);
        self.index.rebuild_shelves(self.shelves.as_ref());
        while self.changes.try_recv().is_ok() {}
        self.index.rebuild_todos(&self.todos)?;
        self.index.rebuild_notes(&self.notes);
        self.index.rebuild_shelves(self.shelves.as_ref());
        Ok(())
    }

    fn sync(&mut self) -> Result<(), DispatchError> {
        if !self.built {
            return Err(DispatchError::NotReady);
        }
        let mut todo = false;
        let mut note = false;
        let mut shelf = false;
        while let Ok(event) = self.changes.try_recv() {
            match event.kind {
                EntityKind::Todo => todo = true,
                EntityKind::Note => note = true,
                EntityKind::Shelf => shelf = true,
                EntityKind::Config => {}
            }
        }
        if todo {
            self.index
                .rebuild_todos(&self.todos)
                .map_err(DispatchError::Index)?;
        }
        if note {
            self.index.rebuild_notes(&self.notes);
        }
        if shelf {
            self.index.rebuild_shelves(self.shelves.as_ref());
        }
        Ok(())
    }

    fn usage_keep(&self) -> UsageKeep {
        let mut keep = UsageKeep::default();
        if let Ok(lists) = self.todos.lists() {
            keep.todos_known = true;
            for list in lists {
                for item in list.items {
                    keep.todos.insert(item.id);
                }
            }
        }
        keep.notes_known = true;
        for note in self.notes.list() {
            keep.notes.insert(note.id);
        }
        for note in self.notes.deleted() {
            keep.notes.insert(note.id);
        }
        if let Some(shelves) = &self.shelves
            && let Ok(groups) = shelves.list()
        {
            keep.shelves_known = true;
            for group in groups {
                for item in &group.refs {
                    let token = UsageKeep::shelf_token(&group.id, &item.path);
                    if item.folder {
                        keep.shelf_folders.insert(token);
                    } else {
                        keep.shelf_files.insert(token);
                    }
                }
                for web in &group.web_refs {
                    if let Some(normalized) = normalize_web_url(&web.url) {
                        keep.webs.insert(normalized.key);
                    }
                }
            }
        }
        keep
    }

    fn begin(&mut self, surface: Surface, text: &str, now: u64) -> Result<Begun, DispatchError> {
        if !self.built {
            return Err(DispatchError::NotReady);
        }
        match surface {
            Surface::SearchBar => match classify_prefix(text) {
                crate::search::PrefixClass::Empty => {
                    self.invalidate(Mode::Empty);
                    Ok(Begun::Ready(self.view()))
                }
                crate::search::PrefixClass::TodoCapture(_)
                | crate::search::PrefixClass::NoteCapture(_) => {
                    self.invalidate(Mode::Capture);
                    Ok(Begun::Ready(self.view()))
                }
                crate::search::PrefixClass::Search => Ok(self.begin_search(text, now)),
            },
            Surface::Panel => {
                if text.is_empty() {
                    self.invalidate(Mode::Empty);
                    Ok(Begun::Ready(self.view()))
                } else {
                    Ok(self.begin_search(text, now))
                }
            }
        }
    }

    fn begin_search(&mut self, text: &str, now: u64) -> Begun {
        self.generation = self.generation.saturating_add(1);
        self.supersede_current();
        self.sequence = self.sequence.saturating_add(1);
        let sequence = self.sequence;
        let generation = self.generation;
        let blank = is_blank(text);
        self.mode = Mode::Search(SearchState {
            sequence,
            generation,
            text: text.to_owned(),
            start_ns: now,
            local_ready: false,
            local_groups: Vec::new(),
            file_hits: Vec::new(),
            file: if blank {
                FileSlot::Skipped
            } else {
                FileSlot::Waiting {
                    not_before_ns: now.saturating_add(FILE_DELAY_NS),
                }
            },
            unavailable: None,
            path_rows: Vec::new(),
            path_up: None,
            path: if looks_like_local_path(text) {
                PathSlot::Pending
            } else {
                PathSlot::Idle
            },
        });
        self.latency.push(LatencyRecord {
            metric: LATENCY_METRIC_RESULT,
            source: Some(LATENCY_SOURCE_LOCAL),
            seq: sequence,
            start_ns: now,
            end_ns: None,
            warmup: false,
            superseded: false,
        });
        Begun::NeedApps(SearchPlan {
            sequence,
            generation,
            text: text.to_owned(),
        })
    }

    fn invalidate(&mut self, mode: Mode) {
        self.generation = self.generation.saturating_add(1);
        self.supersede_current();
        self.mode = mode;
    }

    fn finish_local(
        &mut self,
        plan: SearchPlan,
        apps: Vec<AppHit>,
        usage: &UsageHandle,
    ) -> QueryView {
        let current = matches!(
            &self.mode,
            Mode::Search(state)
                if state.sequence == plan.sequence && state.generation == plan.generation
        );
        if !current {
            return self.view();
        }
        let groups = usage.with_book(|book| {
            local_groups(
                &self.index,
                book,
                rank_query_hash(&plan.text).as_deref(),
                &plan.text,
                &apps,
            )
        });
        if let Mode::Search(state) = &mut self.mode {
            state.local_groups = groups;
            state.local_ready = true;
        }
        self.view()
    }

    fn take_file_job(&mut self, now: u64) -> Option<FileJob> {
        let Mode::Search(state) = &self.mode else {
            return None;
        };
        if !state.local_ready {
            return None;
        }
        let FileSlot::Waiting { not_before_ns } = state.file else {
            return None;
        };
        if now < not_before_ns {
            return None;
        }
        let job = FileJob {
            sequence: state.sequence,
            generation: state.generation,
            start_ns: state.start_ns,
            text: state.text.clone(),
        };
        if let Mode::Search(state) = &mut self.mode {
            state.file = FileSlot::InFlight;
        }
        Some(job)
    }

    fn finish_file(&mut self, job: &FileJob, result: FileQueryResult) -> bool {
        let current = matches!(
            &self.mode,
            Mode::Search(state)
                if state.sequence == job.sequence && state.generation == job.generation
        );
        if !current {
            self.push_file_sample(job, &result, true);
            return false;
        }
        if result.sequence != job.sequence {
            if let Mode::Search(state) = &mut self.mode {
                state.file = FileSlot::Done;
                state.file_hits.clear();
            }
            self.push_file_sample(job, &result, true);
            return false;
        }
        let unavailable = result.source == FileSource::Unavailable;
        let hits = if unavailable {
            Vec::new()
        } else {
            result.hits.clone()
        };
        if let Mode::Search(state) = &mut self.mode {
            state.file = FileSlot::Done;
            state.file_hits = hits;
            state.unavailable = unavailable.then_some(FILE_INDEX_UNAVAILABLE);
        }
        if matches!(
            result.source,
            FileSource::Everything | FileSource::WindowsSearch
        ) {
            self.push_file_sample(job, &result, false);
        }
        true
    }

    fn push_file_sample(&mut self, job: &FileJob, result: &FileQueryResult, superseded: bool) {
        let Some(source) = latency_source(result.source) else {
            return;
        };
        self.latency.push(LatencyRecord {
            metric: LATENCY_METRIC_RESULT,
            source: Some(source),
            seq: job.sequence,
            start_ns: job.start_ns,
            end_ns: None,
            warmup: false,
            superseded,
        });
    }

    fn supersede_current(&mut self) {
        let Mode::Search(state) = &self.mode else {
            return;
        };
        let sequence = state.sequence;
        // 已经画出来的结果不算被取代：新输入在它之后才到。
        for record in &mut self.latency {
            if record.seq == sequence && record.end_ns.is_none() {
                record.superseded = true;
            }
        }
    }

    fn accepts(&self, sequence: u64) -> bool {
        matches!(&self.mode, Mode::Search(state) if state.sequence == sequence)
    }

    fn apply_path(&mut self, read: PathRead, frequency: impl Fn(&str, bool) -> u64) {
        let presented = present(read, frequency);
        let Mode::Search(state) = &mut self.mode else {
            return;
        };
        state.path = PathSlot::Done;
        state.path_up = presented.up.clone();
        state.path_rows = path_rows(&presented);
    }

    fn view(&self) -> QueryView {
        match &self.mode {
            Mode::Empty => QueryView::idle(ViewPhase::Empty),
            Mode::Capture => QueryView::idle(ViewPhase::Capture),
            Mode::Search(state) => {
                let (rows, file) = project(state, self.web_engine);
                QueryView {
                    phase: ViewPhase::Results,
                    sequence: Some(state.sequence),
                    start_ns: Some(state.start_ns),
                    text: state.text.clone(),
                    rows,
                    file,
                    file_unavailable: state.unavailable,
                    path_up: state.path_up.clone(),
                    path_count: state.path_rows.len(),
                }
            }
        }
    }

    fn visible_icon_keys(&self) -> Vec<super::model::IconKey> {
        self.view()
            .rows
            .into_iter()
            .filter_map(|row| row.icon)
            .collect()
    }
}

fn project(state: &SearchState, engine: WebSearchEngine) -> (Vec<SearchRow>, FileProgress) {
    let mut groups = state.local_groups.clone();
    let progress = match state.file {
        FileSlot::Waiting { .. } => FileProgress::Waiting,
        FileSlot::InFlight => FileProgress::InFlight,
        FileSlot::Skipped => FileProgress::Settled,
        FileSlot::Done => {
            let (files, folders) = file_rows(&state.file_hits);
            if !files.is_empty() {
                groups.push(RankedGroup {
                    group: SearchGroup::File,
                    items: files,
                });
            }
            if !folders.is_empty() {
                groups.push(RankedGroup {
                    group: SearchGroup::Folder,
                    items: folders,
                });
            }
            FileProgress::Settled
        }
    };
    let mut rows: Vec<SearchRow> = allocate_display(&groups)
        .into_iter()
        .map(|(_group, row)| row)
        .collect();
    rows = omit_shown_shelf_files(rows, &state.file_hits);
    if !state.path_rows.is_empty() {
        let mut combined = state.path_rows.clone();
        combined.append(&mut rows);
        rows = combined;
    }
    if let Some(row) = web_search_row(&state.text, engine) {
        rows.push(row);
    }
    (rows, progress)
}

/// 有输入时总是有这一条，放在最后。输入像网址时「打开网址」仍在最前。
fn web_search_row(text: &str, engine: WebSearchEngine) -> Option<SearchRow> {
    if is_blank(text) {
        return None;
    }
    Some(SearchRow {
        group: SearchGroup::WebSearch,
        kind: None,
        usage: UsageKey::WebSearch,
        label: engine.label(text),
        location: String::new(),
        icon: None,
        detail: RowDetail::WebSearch {
            url: engine.search_url(text),
        },
    })
}

fn local_groups(
    index: &MemoryIndex,
    usage: &super::usage::UsageBook,
    query_hash: Option<&str>,
    text: &str,
    apps: &[AppHit],
) -> Vec<RankedGroup<SearchRow>> {
    let mut groups = Vec::new();
    if let Some(row) = browser_row(text) {
        groups.push(RankedGroup {
            group: SearchGroup::Browser,
            items: vec![row],
        });
    }
    let app_rows = rank_app_rows(apps, usage, query_hash);
    if !app_rows.is_empty() {
        groups.push(RankedGroup {
            group: SearchGroup::Application,
            items: app_rows,
        });
    }
    let todos = index.rank_todos(text, usage, query_hash);
    if !todos.is_empty() {
        groups.push(RankedGroup {
            group: SearchGroup::Todo,
            items: todos,
        });
    }
    let notes = index.rank_notes(text, usage, query_hash);
    if !notes.is_empty() {
        groups.push(RankedGroup {
            group: SearchGroup::Note,
            items: notes,
        });
    }
    let shelves = index.rank_shelves(text, usage, query_hash);
    if !shelves.is_empty() {
        groups.push(RankedGroup {
            group: SearchGroup::Shelf,
            items: shelves,
        });
    }
    groups
}

/// 输入像网址时放在最前。名称是「打开网址」，地址由 [`open_website`] 补全。
fn browser_row(text: &str) -> Option<SearchRow> {
    let url = open_website(text)?;
    Some(SearchRow {
        group: SearchGroup::Browser,
        kind: None,
        usage: UsageKey::Browser(url.clone()),
        label: OPEN_WEBSITE_LABEL.to_owned(),
        location: String::new(),
        icon: None,
        detail: RowDetail::Browser { url },
    })
}

fn rank_app_rows(
    hits: &[AppHit],
    usage: &super::usage::UsageBook,
    query_hash: Option<&str>,
) -> Vec<SearchRow> {
    let prepared = hits
        .iter()
        .enumerate()
        .filter_map(|(index, hit)| {
            let id = u64::try_from(index).ok()?;
            Some(Hit {
                id,
                role: FieldRole::Name,
                field_index: 0,
                kind: hit.kind,
                score: hit.score,
            })
        })
        .collect::<Vec<_>>();
    let counts = hits
        .iter()
        .map(|hit| {
            rank_weight(
                usage,
                query_hash,
                &UsageKey::App(launch_key(&hit.entry.target)),
            )
        })
        .collect::<Vec<_>>();
    rank_hits(&prepared, |id| {
        counts.get(id as usize).copied().unwrap_or(0)
    })
    .into_iter()
    .filter_map(|hit| {
        let index = usize::try_from(hit.id).ok()?;
        hits.get(index).map(|app| app_row(app, hit.kind))
    })
    .collect()
}

fn app_row(hit: &AppHit, kind: HitKind) -> SearchRow {
    let key = launch_key(&hit.entry.target);
    SearchRow {
        group: SearchGroup::Application,
        kind: Some(kind),
        usage: UsageKey::App(key),
        label: hit.entry.name.clone(),
        location: app_location(&hit.entry.target),
        icon: app_icon(hit),
        detail: RowDetail::App(hit.clone()),
    }
}

fn app_location(target: &LaunchTarget) -> String {
    match target {
        LaunchTarget::Path { path, .. } => path.display().to_string(),
        LaunchTarget::Aumid { .. } | LaunchTarget::Url { .. } => String::new(),
    }
}

fn app_icon(hit: &AppHit) -> Option<super::model::IconKey> {
    icon_of(&hit.entry)
}

/// 一条应用在图标缓存里的键。没有可取的文件或 AUMID 时为 `None`。
#[must_use]
pub fn icon_of(entry: &crate::apps::AppEntry) -> Option<super::model::IconKey> {
    if let Some(path) = &entry.icon_path {
        let path = path.to_string_lossy().to_string();
        if !path.is_empty() {
            return Some(super::model::IconKey {
                path,
                index: entry.icon_index,
            });
        }
    }
    match &entry.target {
        LaunchTarget::Path { path, .. } => {
            let path = path.to_string_lossy().to_string();
            (!path.is_empty()).then_some(super::model::IconKey { path, index: 0 })
        }
        LaunchTarget::Aumid { aumid } => {
            let aumid = aumid.trim();
            (!aumid.is_empty()).then(|| super::model::IconKey {
                path: format!(r"shell:AppsFolder\{aumid}"),
                index: 0,
            })
        }
        LaunchTarget::Url { .. } => None,
    }
}

/// 已经出现在「收纳」里的文件不再留在「文件」组。去掉之后按来源顺序补后面的文件，仍不超过 20 条。
/// 文件夹组不在这里比较。网页搜索还没接上，不占这 20 条。
fn omit_shown_shelf_files(rows: Vec<SearchRow>, hits: &[FileHit]) -> Vec<SearchRow> {
    let shown = shelf_file_identities(&rows);
    if shown.is_empty() {
        return rows;
    }
    let mut removed = false;
    let mut rows: Vec<SearchRow> = rows
        .into_iter()
        .filter(|row| {
            if row.group != SearchGroup::File {
                return true;
            }
            let RowDetail::File { path } = &row.detail else {
                return true;
            };
            if shown.contains(&file_identity(path)) {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect();
    if !removed {
        return rows;
    }
    let present: HashSet<String> = rows
        .iter()
        .filter_map(|row| match (&row.group, &row.detail) {
            (SearchGroup::File, RowDetail::File { path }) => Some(file_identity(path)),
            _ => None,
        })
        .collect();
    let room = DISPLAY_LIMIT.saturating_sub(rows.len());
    let mut extra = Vec::new();
    for hit in hits {
        if extra.len() >= room {
            break;
        }
        if hit.kind != FileKind::File {
            continue;
        }
        let identity = file_identity(&hit.path);
        if shown.contains(&identity) || present.contains(&identity) {
            continue;
        }
        extra.push(file_row(hit));
    }
    if extra.is_empty() {
        return rows;
    }
    let at = rows
        .iter()
        .rposition(|row| row.group == SearchGroup::File)
        .map(|index| index + 1)
        .or_else(|| rows.iter().position(|row| row.group == SearchGroup::Folder))
        .unwrap_or(rows.len());
    rows.splice(at..at, extra);
    rows
}

fn shelf_file_identities(rows: &[SearchRow]) -> HashSet<String> {
    rows.iter()
        .filter_map(|row| match (&row.group, &row.detail) {
            (SearchGroup::Shelf, RowDetail::File { path }) => Some(file_identity(path)),
            _ => None,
        })
        .collect()
}

fn file_identity(path: &str) -> String {
    normalize_path(path)
        .map(|item| item.key)
        .unwrap_or_else(|_| path.to_owned())
}

fn file_rows(hits: &[FileHit]) -> (Vec<SearchRow>, Vec<SearchRow>) {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    for hit in hits {
        let row = file_row(hit);
        match hit.kind {
            FileKind::File => files.push(row),
            FileKind::Folder => folders.push(row),
        }
    }
    (files, folders)
}

fn path_rows(presented: &crate::search::PathRows) -> Vec<SearchRow> {
    if presented.timeout {
        return vec![SearchRow {
            group: SearchGroup::File,
            kind: None,
            usage: UsageKey::PathTimeout,
            label: READ_TIMEOUT_LABEL.to_owned(),
            location: String::new(),
            icon: None,
            detail: RowDetail::PathTimeout,
        }];
    }
    let mut rows = Vec::new();
    if let Some(path) = &presented.open_file {
        rows.push(open_path_file(path));
    }
    rows.extend(presented.listed.iter().map(listed_row));
    rows
}

fn open_path_file(path: &str) -> SearchRow {
    SearchRow {
        group: SearchGroup::File,
        kind: None,
        usage: UsageKey::File(path.to_owned()),
        label: OPEN_FILE_LABEL.to_owned(),
        location: path.to_owned(),
        icon: Some(super::model::IconKey {
            path: path.to_owned(),
            index: 0,
        }),
        detail: RowDetail::File {
            path: path.to_owned(),
        },
    }
}

fn listed_row(item: &crate::search::Listed) -> SearchRow {
    let icon = Some(super::model::IconKey {
        path: item.path.clone(),
        index: 0,
    });
    if item.folder {
        SearchRow {
            group: SearchGroup::Folder,
            kind: item.kind,
            usage: UsageKey::Folder(item.path.clone()),
            label: item.name.clone(),
            location: item.path.clone(),
            icon,
            detail: RowDetail::Folder {
                path: item.path.clone(),
            },
        }
    } else {
        SearchRow {
            group: SearchGroup::File,
            kind: item.kind,
            usage: UsageKey::File(item.path.clone()),
            label: item.name.clone(),
            location: item.path.clone(),
            icon,
            detail: RowDetail::File {
                path: item.path.clone(),
            },
        }
    }
}

fn file_row(hit: &FileHit) -> SearchRow {
    let icon = if hit.path.is_empty() {
        None
    } else {
        Some(super::model::IconKey {
            path: hit.path.clone(),
            index: 0,
        })
    };
    match hit.kind {
        FileKind::File => SearchRow {
            group: SearchGroup::File,
            kind: None,
            usage: UsageKey::File(hit.path.clone()),
            label: hit.name.clone(),
            location: hit.path.clone(),
            icon,
            detail: RowDetail::File {
                path: hit.path.clone(),
            },
        },
        FileKind::Folder => SearchRow {
            group: SearchGroup::Folder,
            kind: None,
            usage: UsageKey::Folder(hit.path.clone()),
            label: hit.name.clone(),
            location: hit.path.clone(),
            icon,
            detail: RowDetail::Folder {
                path: hit.path.clone(),
            },
        },
    }
}

fn latency_source(source: FileSource) -> Option<&'static str> {
    match source {
        FileSource::Everything => Some(LATENCY_SOURCE_EVERYTHING),
        FileSource::WindowsSearch => Some(LATENCY_SOURCE_WINDOWS_SEARCH),
        FileSource::Unavailable | FileSource::Blank => None,
    }
}

fn is_blank(text: &str) -> bool {
    !text.chars().any(|ch| !ch.is_whitespace())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use crate::apps::AppHit;
    use crate::files::{
        EverythingStatus, FileHit, FileKind, FileQueryResult, FileSource, WindowsSearchStatus,
    };
    use crate::notes::{NoteCommands, NoteInput};
    use crate::storage::{Store, StorePaths};
    use crate::todos::TodoCommands;

    use crate::search::{DirItem, PathRead};

    use super::super::icon::MissingIcons;
    use super::super::model::{FileProgress, RowDetail, Surface, UsageKey};
    use super::FILE_DELAY_NS;
    use super::{AppLookup, Dispatch, FileLookup, Monotonic, PathLookup, Services, Sides};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("lanwork-dispatch-note-{nanos}-{seq}"));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct Clock;

    impl Monotonic for Clock {
        fn now_ns(&self) -> u64 {
            0
        }
    }

    struct NoApps;

    impl AppLookup for NoApps {
        fn query(&mut self, _text: &str) -> Vec<AppHit> {
            Vec::new()
        }
    }

    struct SharedClock(Arc<AtomicU64>);

    impl Monotonic for SharedClock {
        fn now_ns(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    struct OneIndexedFile;

    impl FileLookup for OneIndexedFile {
        fn query(&mut self, sequence: u64, _text: &str) -> FileQueryResult {
            FileQueryResult {
                sequence,
                everything: EverythingStatus::Ready,
                windows_search: WindowsSearchStatus::NotChecked,
                source: FileSource::Everything,
                hits: vec![FileHit {
                    name: "indexed.txt".to_owned(),
                    path: r"C:\Work\indexed.txt".to_owned(),
                    kind: FileKind::File,
                }],
            }
        }
    }

    struct NoFiles;

    impl FileLookup for NoFiles {
        fn query(&mut self, sequence: u64, _text: &str) -> FileQueryResult {
            FileQueryResult {
                sequence,
                everything: EverythingStatus::NotChecked,
                windows_search: WindowsSearchStatus::NotChecked,
                source: FileSource::Blank,
                hits: Vec::new(),
            }
        }
    }

    fn input(title: &str) -> NoteInput {
        NoteInput {
            title: title.to_owned(),
            body: String::new(),
            tags: Vec::new(),
            pinned: false,
        }
    }

    #[test]
    fn concurrent_query_sees_the_note_already_in_memory() {
        let temp = TempDir::new();
        let root = temp.path.clone();
        let store = Store::open(StorePaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
        })
        .unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let notes = NoteCommands::open(store.clone()).unwrap();
        let dispatch = Arc::new(Dispatch::new(
            store.clone(),
            Services {
                todos,
                notes: notes.clone(),
                shelves: None,
            },
            Sides {
                apps: NoApps,
                files: NoFiles,
                clock: Clock,
                icons: MissingIcons,
            },
        ));
        dispatch.build().unwrap();
        let created = notes.create(&input("旧标题")).unwrap();
        let indexed = dispatch.submit(Surface::Panel, "旧标题").unwrap();
        assert_eq!(indexed.rows.len(), 2);
        assert_eq!(indexed.rows[0].label, "旧标题");
        assert_eq!(indexed.rows[1].group, crate::search::SearchGroup::WebSearch);

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let done_rx = Arc::new(Mutex::new(done_rx));
        let wait_rx = Arc::clone(&done_rx);
        let reader = {
            let dispatch = Arc::clone(&dispatch);
            std::thread::spawn(move || {
                started_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("publish");
                let view = dispatch.submit(Surface::Panel, "新标题").unwrap();
                assert_eq!(
                    view.rows
                        .iter()
                        .filter(|row| row.group != crate::search::SearchGroup::WebSearch)
                        .map(|row| row.label.as_str())
                        .collect::<Vec<_>>(),
                    vec!["新标题"]
                );
                done_tx.send(()).unwrap();
            })
        };
        store.set_publish_probe(Some(Arc::new(move || {
            let _ = started_tx.send(());
            let _ = wait_rx
                .lock()
                .expect("done channel")
                .recv_timeout(Duration::from_secs(2));
        })));
        notes
            .save(&created.id, created.revision, &input("新标题"))
            .unwrap();
        store.set_publish_probe(None);
        reader.join().unwrap();
    }

    struct FixedDir;

    impl PathLookup for FixedDir {
        fn load(&mut self, text: &str) -> PathRead {
            let entry = |name: &str, folder: bool| DirItem {
                name: name.to_owned(),
                folder,
                hidden: false,
                system: false,
            };
            if text == r"C:\Work\a" {
                return PathRead::Directory {
                    dir: r"C:\Work".to_owned(),
                    filter: "a".to_owned(),
                    entries: vec![entry("ab.txt", false), entry("aa.txt", false)],
                    up: Some(r"C:\".to_owned()),
                };
            }
            if text.starts_with(r"C:\Work") {
                return PathRead::Directory {
                    dir: r"C:\Work".to_owned(),
                    filter: String::new(),
                    entries: vec![
                        entry("b-file", false),
                        entry("a-dir", true),
                        entry("b-dir", true),
                        entry("a-file", false),
                    ],
                    up: Some(r"C:\".to_owned()),
                };
            }
            if text == r"C:\hang" {
                return PathRead::Timeout {
                    up: Some(r"C:\".to_owned()),
                };
            }
            PathRead::NotPath
        }
    }

    #[test]
    fn path_group_is_first_web_search_is_last_and_usage_reorders() {
        let temp = TempDir::new();
        let root = temp.path.clone();
        let store = Store::open(StorePaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
        })
        .unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let notes = NoteCommands::open(store.clone()).unwrap();
        let dispatch = Dispatch::new(
            store,
            Services {
                todos,
                notes,
                shelves: None,
            },
            Sides {
                apps: NoApps,
                files: NoFiles,
                clock: Clock,
                icons: MissingIcons,
            },
        );
        dispatch.build().unwrap();
        dispatch.set_path_source(Box::new(FixedDir));

        let pending = dispatch.submit(Surface::SearchBar, r"C:\Work\").unwrap();
        assert!(pending.rows.iter().all(|row| row.label != "a-dir"));
        assert_eq!(
            pending.rows.last().map(|row| row.group),
            Some(crate::search::SearchGroup::WebSearch)
        );
        let request = dispatch.take_path_request().expect("path request");
        assert!(dispatch.complete_path(request));
        let view = dispatch.view();
        let labels: Vec<_> = view.rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(&labels[..4], ["a-dir", "b-dir", "a-file", "b-file"]);
        assert_eq!(view.rows[0].group, crate::search::SearchGroup::Folder);
        assert_eq!(
            view.rows.last().unwrap().group,
            crate::search::SearchGroup::WebSearch
        );
        assert_eq!(view.path_up.as_deref(), Some(r"C:\"));
        assert!(matches!(
            view.rows[2].detail,
            RowDetail::File { ref path } if path == r"C:\Work\a-file"
        ));

        dispatch.record_open(&UsageKey::File(r"C:\Work\aa.txt".to_owned()));
        assert_eq!(
            dispatch.usage_count(&UsageKey::File(r"C:\Work\aa.txt".to_owned())),
            1
        );
        dispatch.submit(Surface::SearchBar, r"C:\Work\a").unwrap();
        let request = dispatch.take_path_request().expect("filtered");
        assert!(dispatch.complete_path(request));
        let filtered = dispatch.view();
        let filtered_labels: Vec<_> = filtered
            .rows
            .iter()
            .filter(|row| row.group != crate::search::SearchGroup::WebSearch)
            .map(|row| row.label.as_str())
            .collect();
        assert_eq!(filtered_labels, ["aa.txt", "ab.txt"]);

        dispatch.submit(Surface::SearchBar, r"C:\hang").unwrap();
        let request = dispatch.take_path_request().expect("timeout");
        assert!(dispatch.complete_path(request));
        let timed = dispatch.view();
        assert_eq!(timed.rows[0].label, "读取超时");
        assert!(matches!(timed.rows[0].detail, RowDetail::PathTimeout));
        assert_eq!(timed.rows[0].usage, UsageKey::PathTimeout);
        assert!(!dispatch.record_open(&UsageKey::PathTimeout));
        assert_eq!(dispatch.usage_count(&UsageKey::PathTimeout), 0);
        assert_eq!(
            timed.rows.last().unwrap().group,
            crate::search::SearchGroup::WebSearch
        );

        dispatch.submit(Surface::SearchBar, "a:b").unwrap();
        assert!(dispatch.take_path_request().is_none());
    }

    #[test]
    fn path_completion_keeps_a_file_hit_that_settles_afterward() {
        let now = Arc::new(AtomicU64::new(0));
        let temp = TempDir::new();
        let root = temp.path.clone();
        let store = Store::open(StorePaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
        })
        .unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let notes = NoteCommands::open(store.clone()).unwrap();
        let dispatch = Dispatch::new(
            store,
            Services {
                todos,
                notes,
                shelves: None,
            },
            Sides {
                apps: NoApps,
                files: OneIndexedFile,
                clock: SharedClock(Arc::clone(&now)),
                icons: MissingIcons,
            },
        );
        dispatch.build().unwrap();
        dispatch.set_path_source(Box::new(FixedDir));
        dispatch.submit(Surface::SearchBar, r"C:\Work\").unwrap();
        let request = dispatch.take_path_request().expect("path request");
        assert!(dispatch.complete_path(request));
        let early = dispatch.view();
        assert_eq!(early.file, FileProgress::Waiting);
        assert!(early.rows.iter().any(|row| row.label == "a-dir"));
        assert!(early.rows.iter().all(|row| row.label != "indexed.txt"));
        assert!(early.path_count > 0);

        now.store(FILE_DELAY_NS, Ordering::Relaxed);
        let settled = dispatch.poll().unwrap();
        assert!(settled.sent);
        assert_eq!(settled.view.file, FileProgress::Settled);
        assert!(settled.view.rows.iter().any(|row| row.label == "a-dir"));
        assert!(
            settled
                .view
                .rows
                .iter()
                .any(|row| row.label == "indexed.txt")
        );
        let posted = dispatch.view();
        assert_eq!(posted.file, FileProgress::Settled);
        assert!(posted.rows.iter().any(|row| row.label == "indexed.txt"));
        assert!(posted.rows.iter().any(|row| row.label == "a-dir"));
    }
}
