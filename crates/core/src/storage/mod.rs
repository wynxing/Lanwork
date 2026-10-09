//! 数据目录、原子写入、损坏隔离、日志和变更消息。
//!
//! 进程内一把写锁。单文件替换是原子的；跨文件操作用 [`Store::begin_batch`]，
//! 提交成功后才发布 [`EntityChanged`]。写盘失败不发布消息，并保留原文件。
//! 订阅回调在写锁释放之后执行。批次持有写锁期间，不要从同一线程再调用写入。
//!
//! 这里不投递 Win32 窗口消息，也不依赖 Slint。应用外壳订阅变更消息。

mod atomic;
mod boot;
mod change;
mod error;
mod fsutil;
mod log;
mod paths;
mod schema;
mod store;

#[cfg(test)]
pub(crate) mod test_temp;

use std::sync::{Mutex, MutexGuard};

pub use boot::{
    BootHooks, BootReport, DataLoad, FnRepair, ImportPending, ImportRecover, LoadRepair,
};
pub use change::{ChangeMeta, EntityChanged, EntityKind};
pub use error::{Error, IoAction, StartupError};
pub use log::{LOG_MAX_BYTES, LOG_MAX_FILES, Log, LogSettings};
pub(crate) use paths::validate_id;
pub use paths::{
    BOOTSTRAP_FILE, DataDirSource, ENV_DATA_DIR, ResolveInput, ResolvedPaths, absolute_lexical,
    cache_dir, maydolist_dir, resolve, resolve_from_process, write_bootstrap,
};
pub use schema::{ExampleDocument, SCHEMA_VERSION, is_supported_schema};
pub use store::{
    Batch, CollectionKind, DocumentId, LoadCollection, LoadedFile, QuarantineInfo, Store,
    StorePaths, WriteReceipt,
};

pub(crate) use atomic::atomic_write;
pub(crate) use fsutil::{
    DirEntry as FsDirEntry, create_dir_all as fs_create_dir_all, exists as fs_exists,
    read as fs_read, read_dir as fs_read_dir, remove_file as fs_remove_file,
};

pub(crate) fn lock_mutex<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
