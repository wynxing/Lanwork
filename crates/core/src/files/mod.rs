//! 文件来源：Everything SDK 与 Windows Search 回退。
//!
//! 不复制全盘索引，也不为查询启动进程。界面以后调用 [`FileCommands`]。
//! 查询队列、60ms 等待和结果合并不在这里。

mod backend;
mod command;
mod model;
mod package;
mod service;
mod sql;

#[cfg(not(windows))]
mod stub;
#[cfg(windows)]
mod windows;

pub use command::FileCommands;
pub use model::{
    EverythingStatus, FILE_INDEX_UNAVAILABLE, FileHit, FileKind, FileQueryResult, FileSource,
    MAX_FILE_RESULTS, WindowsSearchStatus,
};
pub use package::{PROGRAM_DIRECTORY_FILES, ProgramDirectoryFile};
