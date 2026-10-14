//! 网站图标的磁盘缓存。
//!
//! 不在 [`crate::shelves`] 里。收纳服务不删除、不移动、不复制用户文件。
//! 这里只读写数据目录下的 `cache/web-icons`，坏文件也只在这个目录里丢掉。

mod cache;

pub use cache::{WEB_ICON_CACHE_DIR, WEB_ICON_MAX_EDGE, WebIconCache};
