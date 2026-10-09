//! GitHub 命令返回的错误。
//!
//! 文字只包含状态。不包含环境变量、token、`gh` 的原始输出或修复步骤。

use std::fmt;

use crate::storage::Error as StorageError;
use crate::todos::{SourceKind, TodoError};

/// 产品规格没有写明、因此命令不猜测的点。不改已经写好的数据。
///
/// [`fmt::Display`] 只写出类型名，不是界面文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTopic {
    /// 同一 `owner/repo` 再次添加时如何处理，产品规格没有写。
    DuplicateTracked,
    /// 筛选并集含快照里没有的字段。不实现其中的子集。
    SignalFilters,
}

/// `gh` 调用或解析失败。刷新用它留下已有快照，不把它当成来源已关闭。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCallError {
    NotInstalled,
    NotLoggedIn,
    Network,
    RateLimit,
    NotFound,
    Parse,
    /// 仓库名不能作为独立参数传给 `gh`，或不能编码成缓存文件名。
    InvalidRepo,
    /// 进程启动失败，或 `gh` 返回了无法归类的错误。不含原始输出。
    Command,
}

/// 某次刷新没有写该仓库快照的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchFailure {
    NotInstalled,
    NotLoggedIn,
    Network,
    RateLimit,
    NotFound,
    Parse,
    InvalidRepo,
    Command,
    Storage(String),
}

impl FetchFailure {
    pub(crate) fn from_call(err: GhCallError) -> Self {
        match err {
            GhCallError::NotInstalled => Self::NotInstalled,
            GhCallError::NotLoggedIn => Self::NotLoggedIn,
            GhCallError::Network => Self::Network,
            GhCallError::RateLimit => Self::RateLimit,
            GhCallError::NotFound => Self::NotFound,
            GhCallError::Parse => Self::Parse,
            GhCallError::InvalidRepo => Self::InvalidRepo,
            GhCallError::Command => Self::Command,
        }
    }
}

impl fmt::Display for FetchFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "未安装 gh"),
            Self::NotLoggedIn => write!(f, "未登录"),
            Self::Network => write!(f, "网络失败"),
            Self::RateLimit => write!(f, "速率限制"),
            Self::NotFound => write!(f, "仓库不存在"),
            Self::Parse => write!(f, "无法解析"),
            Self::InvalidRepo => write!(f, "仓库名无效"),
            Self::Command => write!(f, "gh 调用失败"),
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

/// GitHub 命令的失败。
#[derive(Debug)]
pub enum GithubError {
    Storage(StorageError),
    Todo(TodoError),
    /// 还没从磁盘载入，不能读列表或刷新。
    NotReady,
    RepoNotTracked {
        repo: String,
    },
    /// 不是 `owner/repo`。不写入追踪列表。
    InvalidRepoFormat,
    ItemNotFound {
        repo: String,
        kind: SourceKind,
        number: u64,
    },
    InvalidItem,
    UnsupportedSchema {
        found: u32,
    },
    PendingSpec(PendingTopic),
}

impl GithubError {
    pub fn pending_topic(&self) -> Option<PendingTopic> {
        match self {
            Self::PendingSpec(topic) => Some(*topic),
            _ => None,
        }
    }
}

impl fmt::Display for GithubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(err) => write!(f, "{err}"),
            Self::Todo(err) => write!(f, "{err}"),
            Self::NotReady => write!(f, "GitHub 尚未加载"),
            Self::RepoNotTracked { repo } => write!(f, "未追踪仓库：{repo}"),
            Self::InvalidRepoFormat => write!(f, "正确格式是 owner/repo"),
            Self::ItemNotFound { repo, number, .. } => write!(f, "找不到条目：{repo}#{number}"),
            Self::InvalidItem => write!(f, "条目无效"),
            Self::UnsupportedSchema { found } => write!(f, "schemaVersion {found} 不受支持"),
            Self::PendingSpec(topic) => write!(f, "{topic:?}"),
        }
    }
}

impl std::error::Error for GithubError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(err) => Some(err),
            Self::Todo(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StorageError> for GithubError {
    fn from(err: StorageError) -> Self {
        Self::Storage(err)
    }
}

impl From<TodoError> for GithubError {
    fn from(err: TodoError) -> Self {
        Self::Todo(err)
    }
}
