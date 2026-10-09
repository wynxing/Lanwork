use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::atomic::atomic_write;
use super::boot::{BootHooks, BootReport, ImportPending};
use super::change::{ChangeBus, ChangeMeta, EntityChanged, EntityKind};
use super::error::{Error, IoAction, StartupError};
use super::fsutil;
use super::lock_mutex;
use super::log::{Log, LogSettings};
use super::paths::{absolute_lexical, reject_maydolist, validate_id, write_bootstrap};
use super::schema::require_written_schema;

static QUARANTINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 打开存储时使用的目录。来自 [`super::resolve`] 或测试里的临时目录。
#[derive(Debug, Clone)]
pub struct StorePaths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub user_profile: PathBuf,
    pub local_app_data: PathBuf,
}

impl From<super::ResolvedPaths> for StorePaths {
    fn from(paths: super::ResolvedPaths) -> Self {
        Self {
            data_dir: paths.data_dir,
            cache_dir: paths.cache_dir,
            user_profile: paths.user_profile,
            local_app_data: paths.local_app_data,
        }
    }
}

/// 一次成功写入的序号。并发写入里，序号最大的那次是最后一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteReceipt {
    pub generation: u64,
}

/// 数据文件。集合里的 id 是单个路径分量，文件名是 `<id>.json`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocumentId {
    Config,
    Todo(String),
    Note(String),
    Shelf(String),
    GithubWatchlist,
    GithubCache(String),
}

impl DocumentId {
    pub fn kind(&self) -> EntityKind {
        match self {
            Self::Config => EntityKind::Config,
            Self::Todo(_) => EntityKind::Todo,
            Self::Note(_) => EntityKind::Note,
            Self::Shelf(_) => EntityKind::Shelf,
            Self::GithubWatchlist => EntityKind::GithubWatchlist,
            Self::GithubCache(_) => EntityKind::GithubCache,
        }
    }

    pub fn change_id(&self) -> &str {
        match self {
            Self::Config => "config",
            Self::GithubWatchlist => "watchlist",
            Self::Todo(id) | Self::Note(id) | Self::Shelf(id) | Self::GithubCache(id) => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionKind {
    Todos,
    Notes,
    Shelves,
    GithubCaches,
}

impl CollectionKind {
    pub fn entity_kind(self) -> EntityKind {
        match self {
            Self::Todos => EntityKind::Todo,
            Self::Notes => EntityKind::Note,
            Self::Shelves => EntityKind::Shelf,
            Self::GithubCaches => EntityKind::GithubCache,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFile<T> {
    pub id: String,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineInfo {
    pub path: PathBuf,
    pub quarantine: Option<PathBuf>,
}

#[derive(Debug)]
pub struct LoadCollection<T> {
    pub files: Vec<LoadedFile<T>>,
    pub quarantined: Vec<QuarantineInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Init,
    Recovering,
    Loading,
    Repairing,
    Ready,
    Failed,
}

struct Inner {
    data_dir: PathBuf,
    cache_dir: PathBuf,
    user_profile: PathBuf,
    local_app_data: PathBuf,
    write_lock: Mutex<()>,
    boot_lock: Mutex<()>,
    generation: AtomicU64,
    phase: Mutex<Phase>,
    changes: ChangeBus,
    log: Log,
}

/// 进程内的单写者存储。
///
/// 克隆只增加引用计数，写锁和变更订阅是同一份。
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("data_dir", &self.inner.data_dir)
            .field("cache_dir", &self.inner.cache_dir)
            .finish()
    }
}

impl Store {
    pub fn open(paths: StorePaths) -> Result<Self, Error> {
        Self::open_with(paths, LogSettings::default())
    }

    pub fn open_with(paths: StorePaths, log_settings: LogSettings) -> Result<Self, Error> {
        let data_dir = absolute_lexical(&paths.data_dir)?;
        reject_maydolist(&data_dir, &paths.user_profile)?;
        for relative in [
            "",
            "todos",
            "notes",
            "shelves",
            "github",
            "github/cache",
            "backups",
            "logs",
        ] {
            let dir = join_relative(&data_dir, relative);
            fsutil::create_dir_all(&dir)
                .map_err(|source| Error::io(IoAction::CreateDir, dir, source))?;
        }
        fsutil::create_dir_all(&paths.cache_dir)
            .map_err(|source| Error::io(IoAction::CreateDir, &paths.cache_dir, source))?;
        let log = Log::open(data_dir.join("logs").join("app.log"), log_settings)
            .map_err(|source| Error::io(IoAction::CreateDir, data_dir.join("logs"), source))?;
        Ok(Self {
            inner: Arc::new(Inner {
                data_dir,
                cache_dir: paths.cache_dir,
                user_profile: paths.user_profile,
                local_app_data: paths.local_app_data,
                write_lock: Mutex::new(()),
                boot_lock: Mutex::new(()),
                generation: AtomicU64::new(0),
                phase: Mutex::new(Phase::Init),
                changes: ChangeBus::new(),
                log,
            }),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }

    pub fn cache_dir(&self) -> &Path {
        &self.inner.cache_dir
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.inner.data_dir.join("backups")
    }

    pub fn log_path(&self) -> &Path {
        self.inner.log.path()
    }

    /// 记一条信息日志。落盘前仍按存储层规则去掉密钥形态。
    ///
    /// 调用方不要把环境变量、token 或子进程的原始输出放进 `message`。
    pub fn log_info(&self, message: &str) {
        self.inner.log.info(message);
    }

    /// 记一条警告。规则与 [`Self::log_info`] 相同。
    pub fn log_warn(&self, message: &str) {
        self.inner.log.warn(message);
    }

    /// 记一条错误。规则与 [`Self::log_info`] 相同。
    pub fn log_error(&self, message: &str) {
        self.inner.log.error(message);
    }

    pub fn import_pending_path(&self) -> PathBuf {
        self.inner.data_dir.join("import.pending")
    }

    pub fn collection_dir(&self, kind: CollectionKind) -> PathBuf {
        collection_dir(&self.inner.data_dir, kind)
    }

    pub fn document_path(&self, doc: &DocumentId) -> Result<PathBuf, Error> {
        document_path(&self.inner.data_dir, doc)
    }

    pub fn is_ready(&self) -> bool {
        *lock_mutex(&self.inner.phase) == Phase::Ready
    }

    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<EntityChanged> {
        self.inner.changes.subscribe()
    }

    /// 记住迁移后的数据目录。不切换当前已打开的目录，也不发布变更消息。
    pub fn remember_data_dir(&self, data_dir: &Path) -> Result<(), Error> {
        let data_dir = absolute_lexical(data_dir)?;
        reject_maydolist(&data_dir, &self.inner.user_profile)?;
        self.inner.ensure_writable()?;
        let _guard = lock_mutex(&self.inner.write_lock);
        write_bootstrap(&self.inner.local_app_data, &data_dir)
    }

    pub fn write_json<T: Serialize>(
        &self,
        doc: &DocumentId,
        value: &T,
    ) -> Result<WriteReceipt, Error> {
        self.write_with(doc, value, ChangeMeta::default())
    }

    /// 先写盘，再执行 `before_publish`，最后才发布变更。
    ///
    /// 调用方在 `before_publish` 里更新内存。订阅者读到事件时，内存已经是新值。
    pub(crate) fn write_json_with_before_publish<T, F>(
        &self,
        doc: &DocumentId,
        value: &T,
        before_publish: F,
    ) -> Result<WriteReceipt, Error>
    where
        T: Serialize,
        F: FnOnce(),
    {
        self.write_with_before_publish(doc, value, ChangeMeta::default(), before_publish)
    }

    pub fn write_with<T: Serialize>(
        &self,
        doc: &DocumentId,
        value: &T,
        meta: ChangeMeta,
    ) -> Result<WriteReceipt, Error> {
        self.write_with_before_publish(doc, value, meta, || {})
    }

    /// 先写盘，再执行 `before_publish`，最后才发布变更。
    ///
    /// 调用方在 `before_publish` 里更新内存。订阅者读到事件时，内存已经是新值。
    pub(crate) fn write_with_before_publish<T, F>(
        &self,
        doc: &DocumentId,
        value: &T,
        meta: ChangeMeta,
        before_publish: F,
    ) -> Result<WriteReceipt, Error>
    where
        T: Serialize,
        F: FnOnce(),
    {
        let (receipt, event) = {
            let _guard = lock_mutex(&self.inner.write_lock);
            self.inner.write_json(doc, value, meta)?
        };
        before_publish();
        self.inner.changes.publish(event);
        Ok(receipt)
    }

    /// 测试用。在变更已经入队、`publish` 返回之前调用。
    #[cfg(test)]
    pub(crate) fn set_publish_probe(&self, probe: Option<std::sync::Arc<dyn Fn() + Send + Sync>>) {
        self.inner.changes.set_probe(probe);
    }

    /// 文件不存在时返回 `Ok(false)`，不发布消息。
    pub fn remove(&self, doc: &DocumentId) -> Result<bool, Error> {
        let event = {
            let _guard = lock_mutex(&self.inner.write_lock);
            self.inner.remove_json(doc)?
        };
        if let Some(event) = event {
            self.inner.changes.publish(event);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn read_json<T: DeserializeOwned>(&self, doc: &DocumentId) -> Result<Option<T>, Error> {
        let _guard = lock_mutex(&self.inner.write_lock);
        self.inner.read_json(doc)
    }

    /// 解析失败的文件被隔离，其余文件仍返回。
    pub fn read_collection<T: DeserializeOwned>(
        &self,
        kind: CollectionKind,
    ) -> Result<LoadCollection<T>, Error> {
        let _guard = lock_mutex(&self.inner.write_lock);
        self.inner.read_collection(kind)
    }

    pub fn write_import_pending(&self, backup: &Path) -> Result<(), Error> {
        if !backup.is_absolute() {
            return Err(Error::ImportPending {
                message: "备份路径不是绝对路径",
            });
        }
        let text = backup.to_str().ok_or(Error::ImportPending {
            message: "备份路径不是 UTF-8",
        })?;
        if text.is_empty() || text.contains('\n') || text.contains('\r') {
            return Err(Error::ImportPending {
                message: "备份路径无效",
            });
        }
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\n');
        let path = self.import_pending_path();
        let _guard = lock_mutex(&self.inner.write_lock);
        self.inner.ensure_writable()?;
        atomic_write(&path, &bytes).map_err(|source| Error::io(IoAction::Replace, path, source))
    }

    pub fn clear_import_pending(&self) -> Result<(), Error> {
        let path = self.import_pending_path();
        let _guard = lock_mutex(&self.inner.write_lock);
        self.inner.ensure_writable()?;
        fsutil::remove_file(&path).map_err(|source| Error::io(IoAction::Remove, path, source))
    }

    pub fn read_import_pending(&self) -> Result<Option<ImportPending>, Error> {
        let path = self.import_pending_path();
        if !fsutil::exists(&path) {
            return Ok(None);
        }
        let bytes =
            fsutil::read(&path).map_err(|source| Error::io(IoAction::Read, &path, source))?;
        let text =
            String::from_utf8(bytes).map_err(|_| Error::Startup(StartupError::PendingInvalid))?;
        let backup = text.trim();
        if backup.is_empty() || backup.contains('\n') || backup.contains('\r') {
            return Err(Error::Startup(StartupError::PendingInvalid));
        }
        Ok(Some(ImportPending {
            backup_path: PathBuf::from(backup),
        }))
    }

    /// 导入恢复、加载、加载修复都成功后才进入可建索引状态。
    ///
    /// 任一钩子失败则返回错误，不写默认文档，[`Self::build_index`] 也不会执行调用方的闭包。
    /// 钩子里不要再调用 `boot`。
    pub fn boot(&self, mut hooks: BootHooks) -> Result<BootReport, Error> {
        let _boot = lock_mutex(&self.inner.boot_lock);
        match *lock_mutex(&self.inner.phase) {
            Phase::Ready => {
                return Ok(BootReport {
                    import_recovered: false,
                });
            }
            Phase::Init => {}
            _ => return Err(Error::NotReady),
        }
        match self.run_boot(&mut hooks) {
            Ok(report) => {
                self.inner.set_phase(Phase::Ready);
                Ok(report)
            }
            Err(err) => {
                self.inner.log.error(&err.to_string());
                self.inner.set_phase(Phase::Failed);
                Err(err)
            }
        }
    }

    pub fn build_index<T>(
        &self,
        build: impl FnOnce(&Store) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.inner.ensure_ready()?;
        build(self)
    }

    pub fn handle_notification_click<T>(
        &self,
        handle: impl FnOnce(&Store) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.inner.ensure_ready()?;
        handle(self)
    }

    /// 跨文件写入。提交成功后按写入顺序发布变更；丢弃或中途失败则一条都不发布。
    ///
    /// 已经写入的文件会留在磁盘上。调用方按架构文档的顺序写入，加载修复负责收尾。
    #[must_use = "提交后才发布变更消息"]
    pub fn begin_batch(&self) -> Result<Batch<'_>, Error> {
        self.inner.ensure_writable()?;
        let inner = self.inner.as_ref();
        let guard = lock_mutex(&inner.write_lock);
        Ok(Batch {
            inner,
            guard: Some(guard),
            pending: Vec::new(),
            failure: None,
        })
    }

    fn run_boot(&self, hooks: &mut BootHooks) -> Result<BootReport, Error> {
        self.inner.set_phase(Phase::Recovering);
        let import_recovered = self.recover_import(hooks)?;
        self.inner.set_phase(Phase::Loading);
        if let Some(load) = hooks.load.take() {
            load(self).map_err(|message| Error::Startup(StartupError::Load { message }))?;
        }
        self.inner.set_phase(Phase::Repairing);
        for repair in &hooks.repairs {
            repair.repair(self).map_err(|message| {
                Error::Startup(StartupError::Repair {
                    name: repair.name().to_owned(),
                    message,
                })
            })?;
        }
        Ok(BootReport { import_recovered })
    }

    fn recover_import(&self, hooks: &mut BootHooks) -> Result<bool, Error> {
        let Some(pending) = self.read_import_pending()? else {
            return Ok(false);
        };
        let Some(recover) = hooks.recover_import.take() else {
            return Err(Error::Startup(StartupError::ImportRecoveryMissing {
                backup: pending.backup_path,
            }));
        };
        recover(self, &pending)
            .map_err(|message| Error::Startup(StartupError::ImportRecovery { message }))?;
        if fsutil::exists(&self.import_pending_path()) {
            return Err(Error::Startup(StartupError::ImportStillPending));
        }
        Ok(true)
    }
}

impl Inner {
    fn set_phase(&self, phase: Phase) {
        *lock_mutex(&self.phase) = phase;
    }

    fn ensure_writable(&self) -> Result<(), Error> {
        if *lock_mutex(&self.phase) == Phase::Failed {
            Err(Error::NotReady)
        } else {
            Ok(())
        }
    }

    fn ensure_ready(&self) -> Result<(), Error> {
        if *lock_mutex(&self.phase) == Phase::Ready {
            Ok(())
        } else {
            Err(Error::NotReady)
        }
    }

    fn write_json<T: Serialize>(
        &self,
        doc: &DocumentId,
        value: &T,
        meta: ChangeMeta,
    ) -> Result<(WriteReceipt, EntityChanged), Error> {
        self.ensure_writable()?;
        let path = self.prepare_write(doc)?;
        let mut bytes = serde_json::to_vec(value).map_err(|_| Error::Encode)?;
        require_written_schema(&bytes)?;
        bytes.push(b'\n');
        if let Err(source) = atomic_write(&path, &bytes) {
            let err = Error::io(IoAction::Replace, &path, source);
            self.log.warn(&err.to_string());
            return Err(err);
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        Ok((
            WriteReceipt { generation },
            EntityChanged {
                kind: doc.kind(),
                id: doc.change_id().to_owned(),
                revision: meta.revision,
            },
        ))
    }

    fn remove_json(&self, doc: &DocumentId) -> Result<Option<EntityChanged>, Error> {
        self.ensure_writable()?;
        let path = self.prepare_write(doc)?;
        if !fsutil::exists(&path) {
            return Ok(None);
        }
        fsutil::remove_file(&path).map_err(|source| Error::io(IoAction::Remove, &path, source))?;
        Ok(Some(EntityChanged {
            kind: doc.kind(),
            id: doc.change_id().to_owned(),
            revision: None,
        }))
    }

    fn read_json<T: DeserializeOwned>(&self, doc: &DocumentId) -> Result<Option<T>, Error> {
        let path = document_path(&self.data_dir, doc)?;
        if !fsutil::exists(&path) {
            return Ok(None);
        }
        let bytes =
            fsutil::read(&path).map_err(|source| Error::io(IoAction::Read, &path, source))?;
        match serde_json::from_slice(&bytes) {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                let quarantine = self.quarantine(&path, &err);
                Err(Error::Quarantined { path, quarantine })
            }
        }
    }

    fn read_collection<T: DeserializeOwned>(
        &self,
        kind: CollectionKind,
    ) -> Result<LoadCollection<T>, Error> {
        let dir = collection_dir(&self.data_dir, kind);
        if !fsutil::exists(&dir) {
            return Ok(LoadCollection {
                files: Vec::new(),
                quarantined: Vec::new(),
            });
        }
        let entries =
            fsutil::read_dir(&dir).map_err(|source| Error::io(IoAction::Read, &dir, source))?;
        let mut files = Vec::new();
        let mut quarantined = Vec::new();
        for entry in entries {
            if !entry.is_file {
                continue;
            }
            let Some(file_name) = entry.path.file_name().and_then(|name| name.to_str()) else {
                self.log.warn("skipped a file name that is not utf-8");
                continue;
            };
            let Some(id) = file_name.strip_suffix(".json") else {
                continue;
            };
            if validate_id(id).is_err() {
                self.log
                    .warn(&format!("skipped invalid id path={}", entry.path.display()));
                continue;
            }
            let bytes = fsutil::read(&entry.path)
                .map_err(|source| Error::io(IoAction::Read, &entry.path, source))?;
            match serde_json::from_slice::<T>(&bytes) {
                Ok(value) => files.push(LoadedFile {
                    id: id.to_owned(),
                    value,
                }),
                Err(err) => {
                    let quarantine = self.quarantine(&entry.path, &err);
                    quarantined.push(QuarantineInfo {
                        path: entry.path,
                        quarantine,
                    });
                }
            }
        }
        files.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(LoadCollection { files, quarantined })
    }

    fn prepare_write(&self, doc: &DocumentId) -> Result<PathBuf, Error> {
        let path = document_path(&self.data_dir, doc)?;
        reject_maydolist(&path, &self.user_profile)?;
        if let Some(parent) = path.parent() {
            fsutil::create_dir_all(parent)
                .map_err(|source| Error::io(IoAction::CreateDir, parent, source))?;
        }
        Ok(path)
    }

    fn quarantine(&self, path: &Path, err: &serde_json::Error) -> Option<PathBuf> {
        let file_name = path.file_name()?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let seq = QUARANTINE_SEQ.fetch_add(1, Ordering::Relaxed);
        let mut name = std::ffi::OsString::from(file_name);
        name.push(format!(".corrupt-{millis}-{seq}"));
        let target = path.with_file_name(name);
        let category = json_category(err);
        match fsutil::rename(path, &target) {
            Ok(()) => {
                self.log.warn(&format!(
                    "json quarantined path={} quarantine={} category={category} line={} column={}",
                    path.display(),
                    target.display(),
                    err.line(),
                    err.column(),
                ));
                Some(target)
            }
            Err(rename_err) => {
                self.log.warn(&format!(
                    "json quarantine rename failed path={} category={category} line={} column={} os={:?}",
                    path.display(),
                    err.line(),
                    err.column(),
                    rename_err.raw_os_error(),
                ));
                None
            }
        }
    }
}

/// 持有写锁的跨文件批次。`Drop` 不发布变更消息。
pub struct Batch<'a> {
    inner: &'a Inner,
    guard: Option<MutexGuard<'a, ()>>,
    pending: Vec<EntityChanged>,
    failure: Option<String>,
}

impl Batch<'_> {
    pub fn write_json<T: Serialize>(
        &mut self,
        doc: &DocumentId,
        value: &T,
    ) -> Result<WriteReceipt, Error> {
        self.write_with(doc, value, ChangeMeta::default())
    }

    pub fn write_with<T: Serialize>(
        &mut self,
        doc: &DocumentId,
        value: &T,
        meta: ChangeMeta,
    ) -> Result<WriteReceipt, Error> {
        if let Some(message) = &self.failure {
            return Err(Error::Batch {
                message: message.clone(),
            });
        }
        match self.inner.write_json(doc, value, meta) {
            Ok((receipt, event)) => {
                self.pending.push(event);
                Ok(receipt)
            }
            Err(err) => {
                self.failure = Some(err.to_string());
                Err(err)
            }
        }
    }

    pub fn remove(&mut self, doc: &DocumentId) -> Result<bool, Error> {
        if let Some(message) = &self.failure {
            return Err(Error::Batch {
                message: message.clone(),
            });
        }
        match self.inner.remove_json(doc) {
            Ok(Some(event)) => {
                self.pending.push(event);
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(err) => {
                self.failure = Some(err.to_string());
                Err(err)
            }
        }
    }

    pub fn commit(mut self) -> Result<(), Error> {
        if let Some(message) = self.failure.take() {
            return Err(Error::Batch { message });
        }
        let events = std::mem::take(&mut self.pending);
        self.guard.take();
        for event in events {
            self.inner.changes.publish(event);
        }
        Ok(())
    }
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        self.guard.take();
    }
}

fn document_path(data_dir: &Path, doc: &DocumentId) -> Result<PathBuf, Error> {
    match doc {
        DocumentId::Config => Ok(data_dir.join("config.json")),
        DocumentId::Todo(id) => file_in(data_dir, "todos", id),
        DocumentId::Note(id) => file_in(data_dir, "notes", id),
        DocumentId::Shelf(id) => file_in(data_dir, "shelves", id),
        DocumentId::GithubWatchlist => Ok(data_dir.join("github").join("watchlist.json")),
        DocumentId::GithubCache(id) => file_in(data_dir, "github/cache", id),
    }
}

fn file_in(data_dir: &Path, sub: &str, id: &str) -> Result<PathBuf, Error> {
    validate_id(id)?;
    let path = join_relative(data_dir, sub).join(format!("{id}.json"));
    if !super::paths::is_same_or_under(&path, data_dir) {
        return Err(Error::InvalidId { id: id.to_owned() });
    }
    Ok(path)
}

fn collection_dir(data_dir: &Path, kind: CollectionKind) -> PathBuf {
    match kind {
        CollectionKind::Todos => data_dir.join("todos"),
        CollectionKind::Notes => data_dir.join("notes"),
        CollectionKind::Shelves => data_dir.join("shelves"),
        CollectionKind::GithubCaches => data_dir.join("github").join("cache"),
    }
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    if !relative.is_empty() {
        for part in relative.split('/') {
            path.push(part);
        }
    }
    path
}

fn json_category(err: &serde_json::Error) -> &'static str {
    if err.is_syntax() {
        "syntax"
    } else if err.is_data() {
        "data"
    } else if err.is_eof() {
        "eof"
    } else {
        "io"
    }
}
