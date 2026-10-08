//! 先加载缓存，再在后台重建。目录变化只重建开始菜单来源。
//!
//! `refresh` 是进程内的整表重建。规格缺口 #9 第 2 项还没有写进 product.md，
//! 手动刷新放在界面的哪里没有产品规则，这里不提供入口。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::search::{FieldInput, FieldRole, HitKind, MatchIndex};

use super::cache::{self, CacheLoad, CacheStatus};
use super::model::{
    AppEntry, AppSource, SourceAttempt, SourceError, SourceSnapshots, apply_source_results,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppHit {
    pub entry: AppEntry,
    pub kind: HitKind,
    pub score: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    pub entry_count: usize,
    pub errors: Vec<SourceError>,
    pub cache_saved: bool,
    pub cache_error: Option<String>,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub enum IndexError {
    WorkerStopped,
    Timeout,
    MissingEnv,
    UnsupportedPlatform,
    Spawn(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkerStopped => write!(f, "应用索引线程已停止"),
            Self::Timeout => write!(f, "应用索引刷新超时"),
            Self::MissingEnv => write!(f, "缺少 LOCALAPPDATA"),
            Self::UnsupportedPlatform => write!(f, "应用索引的系统枚举只在 Windows 上可用"),
            Self::Spawn(message) => write!(f, "应用索引线程没有启动: {message}"),
        }
    }
}

impl std::error::Error for IndexError {}

pub(crate) trait SourceEnumerator: Send {
    fn enumerate(&mut self, source: AppSource) -> Result<Vec<AppEntry>, String>;
    fn watch_directories(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RebuildKind {
    All,
    StartMenu,
}

struct Msg {
    ticket: u64,
    kind: RebuildKind,
    shutdown: bool,
}

struct Published {
    entries: Vec<AppEntry>,
    match_index: MatchIndex,
}

struct Gate {
    done_ticket: u64,
    stopped: bool,
    report: RefreshReport,
}

struct Shared {
    published: RwLock<Published>,
    gate: Mutex<Gate>,
    cv: Condvar,
    cache_path: PathBuf,
}

pub struct AppIndex {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    submitted: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    cache_status: CacheStatus,
}

pub(crate) struct OpenOptions {
    pub cache_path: PathBuf,
    pub extra_shortcut_dir: Option<PathBuf>,
    pub watch: bool,
    pub background_rebuild: bool,
    pub debounce: Duration,
    pub enumerator: Option<Box<dyn SourceEnumerator>>,
}

impl AppIndex {
    /// 从进程环境打开索引。缓存固定在 `%LOCALAPPDATA%\Lanwork\cache\apps.json`。
    /// 测量用快捷方式目录只读 `LANWORK_EXTRA_SHORTCUT_DIR`。
    pub fn open_from_process() -> Result<Self, IndexError> {
        let local = std::env::var("LOCALAPPDATA").map_err(|_| IndexError::MissingEnv)?;
        let local = local.trim();
        if local.is_empty() {
            return Err(IndexError::MissingEnv);
        }
        let cache_path = crate::storage::cache_dir(Path::new(local)).join(cache::CACHE_FILE_NAME);
        let extra = std::env::var(super::EXTRA_SHORTCUT_DIR_ENV)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        open_with(OpenOptions {
            cache_path,
            extra_shortcut_dir: extra,
            watch: true,
            background_rebuild: true,
            debounce: super::DEFAULT_DEBOUNCE,
            enumerator: None,
        })
    }

    #[must_use]
    pub fn cache_status(&self) -> &CacheStatus {
        &self.cache_status
    }

    #[must_use]
    pub fn entries(&self) -> Vec<AppEntry> {
        read_lock(&self.shared.published).entries.clone()
    }

    /// 用匹配引擎查应用名。别名等规格缺口 #9 第 1 项，这里不加别名字段。
    #[must_use]
    pub fn query(&self, text: &str) -> Vec<AppHit> {
        let published = read_lock(&self.shared.published);
        let hits = published.match_index.query(text);
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for hit in hits {
            if !seen.insert(hit.id) {
                continue;
            }
            let Some(entry) = published.entries.get(hit.id as usize) else {
                continue;
            };
            out.push(AppHit {
                entry: entry.clone(),
                kind: hit.kind,
                score: hit.score,
            });
        }
        out
    }

    /// 重新枚举全部来源并替换缓存。这是内部接口，不是设置里的按钮。
    pub fn refresh(&self) -> Result<RefreshReport, IndexError> {
        self.refresh_timeout(Duration::from_secs(180))
    }

    pub(crate) fn refresh_timeout(&self, timeout: Duration) -> Result<RefreshReport, IndexError> {
        let ticket = self.submit(RebuildKind::All)?;
        self.wait(ticket, timeout)
    }

    pub fn wait_idle(&self) -> Result<RefreshReport, IndexError> {
        let ticket = self.submitted.load(Ordering::SeqCst);
        if ticket == 0 {
            return Ok(lock_mutex(&self.shared.gate).report.clone());
        }
        self.wait(ticket, Duration::from_secs(180))
    }

    fn submit(&self, kind: RebuildKind) -> Result<u64, IndexError> {
        let ticket = self.submitted.fetch_add(1, Ordering::SeqCst) + 1;
        self.tx
            .send(Msg {
                ticket,
                kind,
                shutdown: false,
            })
            .map_err(|_| IndexError::WorkerStopped)?;
        Ok(ticket)
    }

    fn wait(&self, ticket: u64, timeout: Duration) -> Result<RefreshReport, IndexError> {
        let deadline = Instant::now() + timeout;
        let mut guard = lock_mutex(&self.shared.gate);
        while guard.done_ticket < ticket {
            if guard.stopped {
                return Err(IndexError::WorkerStopped);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(IndexError::Timeout);
            }
            let (next, wait) = self
                .shared
                .cv
                .wait_timeout(guard, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard = next;
            if wait.timed_out() && guard.done_ticket < ticket {
                return Err(IndexError::Timeout);
            }
        }
        Ok(guard.report.clone())
    }
}

impl Drop for AppIndex {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg {
            ticket: 0,
            kind: RebuildKind::All,
            shutdown: true,
        });
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn open_with(options: OpenOptions) -> Result<AppIndex, IndexError> {
    let loaded = cache::load_cache(&options.cache_path);
    let cache_status = CacheStatus::from(&loaded);
    let initial_entries = match &loaded {
        CacheLoad::Loaded(entries) => entries.clone(),
        CacheLoad::Missing | CacheLoad::Discarded { .. } => Vec::new(),
    };
    let snapshots = SourceSnapshots::from_entries(&initial_entries);
    let published = build_published(snapshots.merged());
    let report = RefreshReport {
        entry_count: published.entries.len(),
        errors: Vec::new(),
        cache_saved: false,
        cache_error: None,
        elapsed: Duration::ZERO,
    };
    let shared = Arc::new(Shared {
        published: RwLock::new(published),
        gate: Mutex::new(Gate {
            done_ticket: 0,
            stopped: false,
            report,
        }),
        cv: Condvar::new(),
        cache_path: options.cache_path,
    });
    let (tx, rx) = mpsc::channel();
    let submitted = Arc::new(AtomicU64::new(0));
    if options.background_rebuild {
        let ticket = submitted.fetch_add(1, Ordering::SeqCst) + 1;
        tx.send(Msg {
            ticket,
            kind: RebuildKind::All,
            shutdown: false,
        })
        .map_err(|_| IndexError::WorkerStopped)?;
    }
    let enumerator = match options.enumerator {
        Some(enumerator) => enumerator,
        None => default_enumerator(options.extra_shortcut_dir)?,
    };
    let worker_shared = Arc::clone(&shared);
    let worker_tx = tx.clone();
    let watch = options.watch;
    let debounce = options.debounce;
    let worker = thread::Builder::new()
        .name("lanwork-apps".to_owned())
        .spawn(move || {
            worker_main(
                rx,
                worker_tx,
                worker_shared,
                enumerator,
                snapshots,
                watch,
                debounce,
            );
        })
        .map_err(|err| IndexError::Spawn(err.to_string()))?;
    Ok(AppIndex {
        shared,
        tx,
        submitted,
        worker: Some(worker),
        cache_status,
    })
}

fn default_enumerator(
    extra_shortcut_dir: Option<PathBuf>,
) -> Result<Box<dyn SourceEnumerator>, IndexError> {
    #[cfg(windows)]
    {
        Ok(Box::new(super::windows::WindowsSources::new(
            extra_shortcut_dir,
        )))
    }
    #[cfg(not(windows))]
    {
        let _ = extra_shortcut_dir;
        Err(IndexError::UnsupportedPlatform)
    }
}

fn worker_main(
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    mut enumerator: Box<dyn SourceEnumerator>,
    mut snapshots: SourceSnapshots,
    watch: bool,
    debounce: Duration,
) {
    let _notify = WorkerExit(Arc::clone(&shared));
    let watcher = if watch {
        let directories = enumerator.watch_directories();
        spawn_watcher(directories, tx, debounce)
    } else {
        None
    };
    while let Some(batch) = recv_batch(&rx) {
        let shutdown = batch.iter().any(|msg| msg.shutdown);
        let rebuilds: Vec<&Msg> = batch.iter().filter(|msg| !msg.shutdown).collect();
        if !rebuilds.is_empty() {
            let ticket = rebuilds.iter().map(|msg| msg.ticket).max().unwrap_or(0);
            let kind = if rebuilds.iter().any(|msg| msg.kind == RebuildKind::All) {
                RebuildKind::All
            } else {
                RebuildKind::StartMenu
            };
            let outcome = rebuild(&mut *enumerator, &mut snapshots, kind, &shared.cache_path);
            publish(&shared, outcome, ticket);
        }
        if shutdown {
            drop(watcher);
            break;
        }
    }
}

struct WorkerExit(Arc<Shared>);

impl Drop for WorkerExit {
    fn drop(&mut self) {
        let mut gate = lock_mutex(&self.0.gate);
        gate.stopped = true;
        drop(gate);
        self.0.cv.notify_all();
    }
}

fn recv_batch(rx: &Receiver<Msg>) -> Option<Vec<Msg>> {
    let first = rx.recv().ok()?;
    let mut batch = vec![first];
    while let Ok(extra) = rx.try_recv() {
        batch.push(extra);
    }
    Some(batch)
}

struct RebuildOutcome {
    entries: Vec<AppEntry>,
    report: RefreshReport,
}

fn rebuild(
    enumerator: &mut dyn SourceEnumerator,
    snapshots: &mut SourceSnapshots,
    kind: RebuildKind,
    cache_path: &Path,
) -> RebuildOutcome {
    let started = Instant::now();
    let sources: &[AppSource] = match kind {
        RebuildKind::All => &AppSource::ALL,
        RebuildKind::StartMenu => &[AppSource::StartMenu],
    };
    let mut attempts = Vec::with_capacity(sources.len());
    for source in sources {
        let result = catch_unwind(AssertUnwindSafe(|| enumerator.enumerate(*source)));
        let result = match result {
            Ok(result) => result,
            Err(_) => Err("枚举中断".to_owned()),
        };
        attempts.push(SourceAttempt {
            source: *source,
            result,
        });
    }
    let applied = apply_source_results(snapshots, &attempts);
    let (cache_saved, cache_error) = match cache::save_cache(cache_path, &applied.entries) {
        Ok(()) => (true, None),
        Err(message) => (false, Some(message)),
    };
    RebuildOutcome {
        entries: applied.entries,
        report: RefreshReport {
            entry_count: 0,
            errors: applied.errors,
            cache_saved,
            cache_error,
            elapsed: started.elapsed(),
        },
    }
}

fn publish(shared: &Shared, outcome: RebuildOutcome, ticket: u64) {
    let mut report = outcome.report;
    let published = build_published(outcome.entries);
    report.entry_count = published.entries.len();
    {
        let mut slot = write_lock(&shared.published);
        *slot = published;
    }
    {
        let mut gate = lock_mutex(&shared.gate);
        if ticket > gate.done_ticket {
            gate.done_ticket = ticket;
        }
        gate.report = report;
    }
    shared.cv.notify_all();
}

fn build_published(entries: Vec<AppEntry>) -> Published {
    let mut match_index = MatchIndex::new();
    for (index, entry) in entries.iter().enumerate() {
        // 别名等 #9 第 1 项。现在只准备显示名。
        match_index.insert(
            index as u64,
            &[FieldInput {
                role: FieldRole::Name,
                text: &entry.name,
            }],
        );
    }
    Published {
        entries,
        match_index,
    }
}

fn spawn_watcher(
    directories: Vec<PathBuf>,
    tx: Sender<Msg>,
    debounce: Duration,
) -> Option<Watcher> {
    if directories.is_empty() {
        return None;
    }
    #[cfg(windows)]
    {
        super::windows::watch::spawn(directories, debounce, move || {
            let _ = tx.send(Msg {
                ticket: 0,
                kind: RebuildKind::StartMenu,
                shutdown: false,
            });
        })
        .ok()
        .map(Watcher::Windows)
    }
    #[cfg(not(windows))]
    {
        let _ = (directories, tx, debounce);
        None
    }
}

enum Watcher {
    /// 句柄只靠 `Drop` 停掉监视线程，业务代码不读取它。
    #[cfg(windows)]
    #[allow(dead_code)]
    Windows(super::windows::watch::WatchHandle),
}

fn lock_mutex<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
struct Delay {
    release: Mutex<bool>,
    cv: Condvar,
}

#[cfg(test)]
impl Delay {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            release: Mutex::new(false),
            cv: Condvar::new(),
        })
    }

    fn wait(&self) {
        let mut released = lock_mutex(&self.release);
        let start = Instant::now();
        while !*released && start.elapsed() < Duration::from_secs(5) {
            let (next, _) = self
                .cv
                .wait_timeout(released, Duration::from_millis(100))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            released = next;
        }
    }

    fn release(&self) {
        *lock_mutex(&self.release) = true;
        self.cv.notify_all();
    }
}

#[cfg(test)]
struct FakeSources {
    results: std::collections::HashMap<AppSource, Result<Vec<AppEntry>, String>>,
    delay: Option<Arc<Delay>>,
    delayed: bool,
}

#[cfg(test)]
impl SourceEnumerator for FakeSources {
    fn enumerate(&mut self, source: AppSource) -> Result<Vec<AppEntry>, String> {
        if !self.delayed {
            if let Some(delay) = &self.delay {
                delay.wait();
            }
            self.delayed = true;
        }
        self.results
            .get(&source)
            .cloned()
            .unwrap_or_else(|| Ok(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::LaunchTarget;
    use crate::storage::test_temp::TempDir;

    fn entry(name: &str, source: AppSource, path: &str) -> AppEntry {
        AppEntry {
            name: name.to_owned(),
            source,
            target: LaunchTarget::Path {
                path: PathBuf::from(path),
                args: String::new(),
                working_directory: None,
            },
            icon_path: None,
            icon_index: 0,
        }
    }

    #[test]
    fn cache_is_searchable_before_background_rebuild_replaces_it() {
        let temp = TempDir::new();
        let cache_path = temp.path().join("apps.json");
        let cached = entry("来自缓存", AppSource::Path, r"C:\Cache\old.exe");
        cache::save_cache(&cache_path, &[cached]).unwrap();
        let delay = Delay::new();
        let fresh = entry("重建后", AppSource::Path, r"C:\Fresh\new.exe");
        let mut results = std::collections::HashMap::new();
        results.insert(AppSource::Path, Ok(vec![fresh]));
        let index = open_with(OpenOptions {
            cache_path,
            extra_shortcut_dir: None,
            watch: false,
            background_rebuild: true,
            debounce: Duration::from_millis(50),
            enumerator: Some(Box::new(FakeSources {
                results,
                delay: Some(Arc::clone(&delay)),
                delayed: false,
            })),
        })
        .unwrap();
        assert!(matches!(index.cache_status(), CacheStatus::Loaded(1)));
        assert_eq!(index.entries()[0].name, "来自缓存");
        assert_eq!(index.query("来自缓存").len(), 1);
        delay.release();
        let report = index.wait_idle().unwrap();
        assert!(report.cache_saved);
        assert!(report.errors.is_empty());
        assert_eq!(index.entries().len(), 1);
        assert_eq!(index.entries()[0].name, "重建后");
        assert!(index.query("来自缓存").is_empty());
        assert_eq!(index.query("重建后").len(), 1);
    }

    #[test]
    fn corrupt_cache_is_discarded_and_refresh_writes_a_new_file() {
        let temp = TempDir::new();
        let cache_path = temp.path().join("apps.json");
        std::fs::write(&cache_path, b"not-json").unwrap();
        let fresh = entry("新的", AppSource::Store, r"C:\Apps\new.exe");
        let mut results = std::collections::HashMap::new();
        results.insert(AppSource::Store, Ok(vec![fresh]));
        results.insert(AppSource::StartMenu, Err("开始菜单不可用".into()));
        let index = open_with(OpenOptions {
            cache_path: cache_path.clone(),
            extra_shortcut_dir: None,
            watch: false,
            background_rebuild: false,
            debounce: Duration::from_millis(50),
            enumerator: Some(Box::new(FakeSources {
                results,
                delay: None,
                delayed: false,
            })),
        })
        .unwrap();
        assert_eq!(*index.cache_status(), CacheStatus::Discarded);
        assert!(index.entries().is_empty());
        assert!(!cache_path.exists());
        let report = index.refresh_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].source, AppSource::StartMenu);
        assert_eq!(index.entries().len(), 1);
        assert_eq!(index.entries()[0].name, "新的");
        assert!(cache_path.is_file());
    }
}
