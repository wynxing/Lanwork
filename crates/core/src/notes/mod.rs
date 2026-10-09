//! 便签的模型、服务和薄命令。
//!
//! 一篇便签一个 `notes/<id>.json`。保存时带上加载时的 `revision`，与磁盘不一致就返回冲突并且不覆盖。
//! 停止编辑 1 秒后的去抖由界面负责。删除写入 `deletedAt`。永久删除只作用于回收站中的便签。
//! 进入回收站满 30 天后，打开服务时清除。本模块不依赖 Slint 或 Win32 窗口 API。

mod command;
mod error;
mod model;
mod service;

pub use command::NoteCommands;
pub use error::NoteError;
pub use model::{
    EMPTY_TITLE_DISPLAY, Note, NoteInput, TRASH_RETENTION_MS, TimestampMillis, display_title,
    normalize_tags,
};
pub use service::NoteService;
