//! 查询调度交给界面的结果。
//!
//! 这里没有窗口。搜索条和面板以后读这些类型。

use crate::apps::AppHit;
use crate::search::{HitKind, SearchGroup};

/// 最后一次输入之后，多久才向文件来源发请求。
pub const FILE_QUERY_DELAY_MS: u64 = 60;

pub(crate) const FILE_DELAY_NS: u64 = FILE_QUERY_DELAY_MS * 1_000_000;

/// 结果延迟记录的 metric。热召回不在调度层。
pub const LATENCY_METRIC_RESULT: &str = "result";

pub const LATENCY_SOURCE_LOCAL: &str = "local";
pub const LATENCY_SOURCE_EVERYTHING: &str = "everything";
pub const LATENCY_SOURCE_WINDOWS_SEARCH: &str = "windows_search";

/// 输入从哪来。只有搜索条识别收集前缀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// 搜索条。先经 [`crate::search::classify_prefix`]。
    SearchBar,
    /// 面板搜索框。不识别收集前缀。
    Panel,
}

/// 当前这一轮是空输入、收集，还是搜索。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewPhase {
    Empty,
    Capture,
    Results,
}

/// 文件请求走到哪一步。60ms 到点只表示可以发送，不表示结果已经显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProgress {
    /// 这一轮没有文件请求。
    Idle,
    /// 距最后一次输入还不满 60ms。
    Waiting,
    /// 请求已经发出，文件结果还没合并。
    InFlight,
    /// 文件阶段结束。命中、空列表和不可用都算结束。
    Settled,
}

/// 打开结果时用来给使用次数加 1 的键。调度层只放在内存里。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UsageKey {
    Browser(String),
    App(String),
    Todo(String),
    Note(String),
    File(String),
    Folder(String),
}

/// 壳层图标的键。`index` 来自快捷方式；取图接口没有这一参数时仍用它区分缓存项。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IconKey {
    pub path: String,
    pub index: i32,
}

/// 一条可见结果。`location` 是选中后的路径、清单名或正文行；没有时为空字符串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRow {
    pub group: SearchGroup,
    pub kind: Option<HitKind>,
    pub usage: UsageKey,
    pub label: String,
    pub location: String,
    pub icon: Option<IconKey>,
    pub detail: RowDetail,
}

/// 打开这一条时界面还需要的标识。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowDetail {
    Browser { url: String },
    App(AppHit),
    Todo { list_id: String, item_id: String },
    Note { id: String },
    File { path: String },
    Folder { path: String },
}

/// 界面可以画的当前结果。序号不是当前序号时，不要再画这份旧结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryView {
    pub phase: ViewPhase,
    pub sequence: Option<u64>,
    pub start_ns: Option<u64>,
    pub text: String,
    pub rows: Vec<SearchRow>,
    pub file: FileProgress,
    pub file_unavailable: Option<&'static str>,
}

impl QueryView {
    pub(crate) fn idle(phase: ViewPhase) -> Self {
        Self {
            phase,
            sequence: None,
            start_ns: None,
            text: String::new(),
            rows: Vec::new(),
            file: FileProgress::Idle,
            file_unavailable: None,
        }
    }
}

/// 结果延迟的一条原始记录。终点由界面在首帧填上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyRecord {
    pub metric: &'static str,
    pub source: Option<&'static str>,
    pub seq: u64,
    pub start_ns: u64,
    pub end_ns: Option<u64>,
    pub warmup: bool,
    pub superseded: bool,
}

impl LatencyRecord {
    /// 一行 JSON，字段与 `tools/sample` 的延迟输入一致。
    #[must_use]
    pub fn to_json_line(&self) -> String {
        serde_json::json!({
            "metric": self.metric,
            "source": self.source,
            "seq": self.seq,
            "start_ns": self.start_ns,
            "end_ns": self.end_ns,
            "warmup": self.warmup,
            "superseded": self.superseded,
        })
        .to_string()
    }
}

/// 调度还不能查询，或索引没有从内存建起来。
#[derive(Debug)]
pub enum DispatchError {
    /// `Store::boot` 还没成功。
    NotReady,
    Index(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady => write!(f, "启动未完成"),
            Self::Index(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DispatchError {}
