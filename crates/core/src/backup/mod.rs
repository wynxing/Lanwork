//! 导出、自动备份、手动备份和导入。
//!
//! 数据文件含 `user-apps.json`。GitHub 缓存在备份里总是包含，导出时由调用方选择。
//! 不读取环境变量，也不把日志、备份目录和 `import.pending` 放进包。

mod command;
mod error;
mod package;
mod service;
mod zipstore;

pub use command::BackupCommands;
pub use error::BackupError;
pub use package::PackageOverview;
pub use service::LaunchBackup;
