//! 导出、自动备份、手动备份和导入。
//!
//! 数据文件含 `user-apps.json`。不包含 `github/watchlist.json`、`github/cache` 和 `cache/web-icons`。
//! 旧包里若有这些文件，导入时忽略，不因此失败，也不写入。
//! 导入替换完成之后，已打开的服务先从磁盘重新载入，再发布变更。
//! 不读取环境变量，也不把日志、备份目录和 `import.pending` 放进包。

mod command;
mod error;
mod hold;
mod migrate;
mod package;
mod service;
mod tools;
mod zipstore;

pub use command::BackupCommands;
pub use error::BackupError;
pub use hold::{persist_open_allowed, reopen_after_import, reopen_after_migrate};
pub use migrate::{MigrateError, MigratePhase, MigrateProgress, migrate_data_dir, progress_text};
pub use package::{PackageOverview, PackagePreview, overview_text};
pub use service::{BackupKind, LaunchBackup, ListedBackup};
pub use tools::{ConfirmationLine, RestoreTool};
