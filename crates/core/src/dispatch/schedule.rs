//! 一个待处理请求、递增序号，以及本地结果和文件结果的合并。
//!
//! 组合中的预编辑不要调用 [`Dispatch::submit`]。起点时间是已提交文本变化的时刻。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::apps::{AppHit, AppIndex, LaunchTarget, launch_key};
use crate::files::{
    FILE_INDEX_UNAVAILABLE, FileCommands, FileHit, FileKind, FileQueryResult, FileSource,
};
use crate::notes::NoteCommands;
use crate::search::{
    FieldRole, Hit, HitKind, RankedGroup, SearchGroup, WebSearchEngine, allocate_display,
    classify_prefix, rank_hits,
};
use crate::storage::{self, EntityKind, Error as StorageError, StartupError, Store};
use crate::todos::{TodoCommands, is_http_source_url};

use super::icon::{IconCache, IconLoader};
use super::index::MemoryIndex;
use super::model::{
    DispatchError, FILE_DELAY_NS, FileProgress, LATENCY_METRIC_RESULT, LATENCY_SOURCE_EVERYTHING,
    LATENCY_SOURCE_LOCAL, LATENCY_SOURCE_WINDOWS_SEARCH, LatencyRecord, QueryView, RowDetail,
    SearchRow, Surface, UsageKey, ViewPhase,
};

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
}

impl AppLookup for AppIndex {
    fn query(&mut self, text: &str) -> Vec<AppHit> {
        AppIndex::query(self, text)
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

/// 待办和便签服务。索引在启动完成之后读它们的内存。
pub struct Services {
    pub todos: TodoCommands,
    pub notes: NoteCommands,
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
/// 然后 [`Dispatch::build`]。这两份服务必须就是正在写入的那一份；克隆仍共享内存。
/// 查询时不读盘。变更消息发出时，待办和便签的内存已经是新值。
pub struct Dispatch {
    store: Store,
    inner: Mutex<Inner>,
    apps: Mutex<Box<dyn AppLookup>>,
    files: Mutex<Box<dyn FileLookup>>,
    clock: Mutex<Box<dyn Monotonic>>,
    icons: Arc<Mutex<IconCache>>,
}

struct Inner {
    todos: TodoCommands,
    notes: NoteCommands,
    index: MemoryIndex,
    changes: std::sync::mpsc::Receiver<crate::storage::EntityChanged>,
    built: bool,
    generation: u64,
    sequence: u64,
    mode: Mode,
    freq: HashMap<UsageKey, u64>,
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
}

enum FileSlot {
    Waiting { not_before_ns: u64 },
    InFlight,
    Done,
    Skipped,
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
        Self {
            store,
            inner: Mutex::new(Inner {
                todos: services.todos,
                notes: services.notes,
                index: MemoryIndex::empty(),
                changes,
                built: false,
                generation: 0,
                sequence: 0,
                mode: Mode::Empty,
                freq: HashMap::new(),
                latency: Vec::new(),
                web_engine: WebSearchEngine::default(),
            }),
            apps: Mutex::new(Box::new(sides.apps)),
            files: Mutex::new(Box::new(sides.files)),
            clock: Mutex::new(Box::new(sides.clock)),
            icons: Arc::new(Mutex::new(IconCache::new(sides.icons))),
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
        match built {
            Ok(()) => Ok(()),
            Err(StorageError::NotReady) => Err(DispatchError::NotReady),
            Err(err) => Err(DispatchError::Index(err.to_string())),
        }
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
                Ok(inner.finish_local(plan, apps))
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

    #[must_use]
    pub fn view(&self) -> QueryView {
        storage::lock_mutex(&self.inner).view()
    }

    /// 界面只接受当前序号。收集、空输入和更新的输入之后，旧序号为假。
    #[must_use]
    pub fn accepts(&self, sequence: u64) -> bool {
        storage::lock_mutex(&self.inner).accepts(sequence)
    }

    /// 网页搜索那一条用的搜索引擎。从下一次输入起生效。
    pub fn set_web_search_engine(&self, engine: WebSearchEngine) {
        storage::lock_mutex(&self.inner).web_engine = engine;
    }

    /// 从搜索结果打开一项。次数加 1，只影响之后的组内排序。
    pub fn record_open(&self, key: &UsageKey) {
        let mut inner = storage::lock_mutex(&self.inner);
        let count = inner.freq.entry(key.clone()).or_default();
        *count = count.saturating_add(1);
    }

    #[must_use]
    pub fn usage_count(&self, key: &UsageKey) -> u64 {
        storage::lock_mutex(&self.inner)
            .freq
            .get(key)
            .copied()
            .unwrap_or(0)
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
    pub fn load_visible_icons(&self) -> Vec<(super::model::IconKey, super::icon::RgbaImage)> {
        let keys = storage::lock_mutex(&self.inner).visible_icon_keys();
        let mut cache = storage::lock_mutex(&self.icons);
        let mut loaded = Vec::new();
        for key in keys {
            if let Some(image) = cache.get(&key) {
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
        while self.changes.try_recv().is_ok() {}
        self.index.rebuild_todos(&self.todos)?;
        self.index.rebuild_notes(&self.notes);
        Ok(())
    }

    fn sync(&mut self) -> Result<(), DispatchError> {
        if !self.built {
            return Err(DispatchError::NotReady);
        }
        let mut todo = false;
        let mut note = false;
        while let Ok(event) = self.changes.try_recv() {
            match event.kind {
                EntityKind::Todo => todo = true,
                EntityKind::Note => note = true,
                _ => {}
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
        Ok(())
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

    fn finish_local(&mut self, plan: SearchPlan, apps: Vec<AppHit>) -> QueryView {
        let current = matches!(
            &self.mode,
            Mode::Search(state)
                if state.sequence == plan.sequence && state.generation == plan.generation
        );
        if !current {
            return self.view();
        }
        let groups = local_groups(&self.index, &self.freq, &plan.text, &apps);
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
    if let Some(row) = web_search_row(&state.text, engine) {
        rows.push(row);
    }
    (rows, progress)
}

/// 有输入时总是有这一条，放在最后。输入是网址时浏览器那一条仍在最前。
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
    freq: &HashMap<UsageKey, u64>,
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
    let app_rows = rank_app_rows(apps, freq);
    if !app_rows.is_empty() {
        groups.push(RankedGroup {
            group: SearchGroup::Application,
            items: app_rows,
        });
    }
    let todos = index.rank_todos(text, freq);
    if !todos.is_empty() {
        groups.push(RankedGroup {
            group: SearchGroup::Todo,
            items: todos,
        });
    }
    let notes = index.rank_notes(text, freq);
    if !notes.is_empty() {
        groups.push(RankedGroup {
            group: SearchGroup::Note,
            items: notes,
        });
    }
    groups
}

fn browser_row(text: &str) -> Option<SearchRow> {
    if !is_http_source_url(text) {
        return None;
    }
    Some(SearchRow {
        group: SearchGroup::Browser,
        kind: None,
        usage: UsageKey::Browser(text.to_owned()),
        label: text.to_owned(),
        location: String::new(),
        icon: None,
        detail: RowDetail::Browser {
            url: text.to_owned(),
        },
    })
}

fn rank_app_rows(hits: &[AppHit], freq: &HashMap<UsageKey, u64>) -> Vec<SearchRow> {
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
            freq.get(&UsageKey::App(launch_key(&hit.entry.target)))
                .copied()
                .unwrap_or(0)
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
    if let Some(path) = &hit.entry.icon_path {
        let path = path.to_string_lossy().to_string();
        if !path.is_empty() {
            return Some(super::model::IconKey {
                path,
                index: hit.entry.icon_index,
            });
        }
    }
    match &hit.entry.target {
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
    use crate::files::{EverythingStatus, FileQueryResult, FileSource, WindowsSearchStatus};
    use crate::notes::{NoteCommands, NoteInput};
    use crate::storage::{Store, StorePaths};
    use crate::todos::TodoCommands;

    use super::super::icon::MissingIcons;
    use super::super::model::Surface;
    use super::{AppLookup, Dispatch, FileLookup, Monotonic, Services, Sides};

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
}
