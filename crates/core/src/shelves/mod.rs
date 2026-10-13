//! 收纳的模型、服务和薄命令。
//!
//! 一个分组一个 `shelves/<id>.json`。分组保存路径引用和网页引用，不移动、不复制、不删除用户文件，
//! 也不关闭或移动浏览器标签。本模块不依赖 Slint 或 Win32 窗口 API。

mod command;
mod error;
mod model;
mod path;
mod service;
mod weblink;

pub use crate::notes::{FloatDock, FloatGeometry, FloatStackSlot};
pub use command::ShelfCommands;
pub use error::ShelfError;
pub use model::{
    AddRefsOutcome, DEFAULT_GROUP_NAME, EXISTS_TIMEOUT, IncomingRef, IncomingWeb, Shelf, ShelfRef,
    WebRef,
};
pub use path::{NormalizedPath, normalize_path};
pub use service::ShelfService;
pub use weblink::{
    HtmlExtract, HtmlLink, NormalizedWebUrl, PickedWebRef, WebDropInput, chrome_like_cf_html,
    decode_utf16_text, descriptor_w, encode_utf16_text, file_names_w, normalize_web_url,
    parse_cf_html, pick_web_drop, title_from_descriptor_names, title_from_url_filename,
    web_display_name,
};
