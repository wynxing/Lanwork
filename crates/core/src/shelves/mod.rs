//! 收纳的模型、服务和薄命令。
//!
//! 一个分组一个 `shelves/<id>.json`。分组只保存路径引用，不移动、不复制、不删除用户文件。
//! 本模块不依赖 Slint 或 Win32 窗口 API。

mod command;
mod error;
mod model;
mod path;
mod service;

pub use command::ShelfCommands;
pub use error::ShelfError;
pub use model::{AddRefsOutcome, DEFAULT_GROUP_NAME, EXISTS_TIMEOUT, IncomingRef, Shelf, ShelfRef};
pub use path::{NormalizedPath, normalize_path};
pub use service::ShelfService;
