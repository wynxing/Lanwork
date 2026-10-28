//! 查询调度。
//!
//! 搜索条和面板共用这一层。待办、便签和收纳引用的索引常驻内存，文件请求在最后一次输入
//! 60ms 之后才发出。搜索条已接入，面板还没有。

mod icon;
mod index;
mod model;
mod schedule;
mod usage;

pub use icon::{
    ICON_CACHE_BYTES, ICON_CACHE_ITEMS, IconCache, IconLoader, MissingIcons, RgbaImage, ShellIcons,
};
pub use model::{
    FILE_QUERY_DELAY_MS, FileProgress, IconKey, LATENCY_METRIC_RESULT, LATENCY_SOURCE_EVERYTHING,
    LATENCY_SOURCE_LOCAL, LATENCY_SOURCE_WINDOWS_SEARCH, LatencyRecord, QueryView, RowDetail,
    SearchRow, Surface, UsageKey, ViewPhase,
};
pub use schedule::{
    AppLookup, Dispatch, FileLookup, Monotonic, PathRequest, PollReport, Services, Sides,
    SystemClock, icon_of,
};
pub use usage::{USAGE_SAVE_DELAY, query_hash};

pub use crate::search::SearchGroup;
pub use model::DispatchError;
