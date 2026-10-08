//! 便签的模型、服务和薄命令。
//!
//! 一篇便签一个 `notes/<id>.json`。保存时带上加载时的 `revision`，与磁盘不一致就返回冲突并且不覆盖。
//! 去抖由界面负责，时长等 product.md 写入 #9 第 6 项。本模块不依赖 Slint 或 Win32 窗口 API。

mod command;
mod error;
mod model;
mod service;

pub use command::NoteCommands;
pub use error::NoteError;
pub use model::{
    EMPTY_TITLE_DISPLAY, Note, NoteInput, TimestampMillis, display_title, normalize_tags,
};
pub use service::NoteService;
