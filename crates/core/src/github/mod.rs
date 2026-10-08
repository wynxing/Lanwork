//! GitHub 服务。
//!
//! 只通过本机 `gh` 读取。token 不进入配置、日志或这里写出的文件。
//! 追踪仓库的添加、移除，以及除长期未更新和 Draft 以外的信号与筛选，等 #9 第 17、18 项。
//! 界面调用 [`command::GithubCommands`]。本模块不依赖 Slint，也不打开浏览器。

mod command;
mod error;
mod gh;
mod model;
mod parse;
mod service;
mod signals;

pub use command::GithubCommands;
pub use error::{FetchFailure, GhCallError, GithubError, PendingTopic};
pub use gh::{
    CREATE_NO_WINDOW, CommandCapture, CommandRunner, GH_LIST_LIMIT, GhClient, GhProbe, ProcessGh,
    SpawnFailure, version_from_stdout, version_log_line,
};
pub use model::{
    DAY_MS, DEFAULT_STALE_DAYS, GithubFilter, GithubSettings, ItemMark, ListedItem,
    OFFLINE_CACHE_LABEL, RepoList, RepoSnapshot, SnapshotItem, Watchlist, cache_file_id,
    todo_title,
};
pub use parse::{FetchedRepo, ParsedItem, RemoteState, parse_issues, parse_pulls};
pub use service::{RefreshReport, RepoOutcome, RepoResult, SyncFailure, SyncReport};
pub use signals::{SignalExtension, is_stale};
