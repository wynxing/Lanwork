//! `github/watchlist.json` 和 `github/cache/<id>.json`。
//!
//! 字段名用 camelCase。新字段有默认值。缺少 `schemaVersion` 的旧文件按 1 读取。

use serde::{Deserialize, Serialize};

use crate::storage::{SCHEMA_VERSION, validate_id};
use crate::todos::SourceKind;

use super::error::GithubError;

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

/// 产品规格里的默认长期未更新天数。0 表示不标。
pub const DEFAULT_STALE_DAYS: u32 = 14;

/// 离线时列表上的标记。快照文件本身不写这四个字。
pub const OFFLINE_CACHE_LABEL: &str = "离线缓存";

/// 一天的毫秒数。长期未更新按这个长度折算，不按日历月。
pub const DAY_MS: i64 = 86_400_000;

/// 调用方从配置传入的值。本服务不读 `config.json`。
///
/// `source_sync` 与 `auto_complete_on_close` 如何组合，产品规格没有写。
/// 自动完成只看 `auto_complete_on_close`。`source_sync` 原样出现在刷新结果里。
/// `refresh_interval_ms` 也不在这里解释，包括 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GithubSettings {
    pub stale_days: u32,
    pub auto_complete_on_close: bool,
    pub source_sync: bool,
    pub refresh_interval_ms: u64,
}

/// 被记住的忽略或钉住。仓库字符串按原文比较，不做大小写折叠。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMark {
    pub repo: String,
    #[serde(rename = "kind")]
    pub kind: SourceKind,
    pub number: u64,
}

/// 追踪仓库、忽略和钉住。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watchlist {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub ignored: Vec<ItemMark>,
    #[serde(default)]
    pub pinned: Vec<ItemMark>,
}

impl Watchlist {
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            repos: Vec::new(),
            ignored: Vec::new(),
            pinned: Vec::new(),
        }
    }
}

impl Default for Watchlist {
    fn default() -> Self {
        Self::empty()
    }
}

/// 快照里的一条 open 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotItem {
    pub kind: SourceKind,
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(default, rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub draft: bool,
}

/// 一个仓库的快照。只含刷新时状态明确为 open 的条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSnapshot {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
    pub repo: String,
    #[serde(rename = "fetchedAt")]
    pub fetched_at: i64,
    #[serde(default)]
    pub items: Vec<SnapshotItem>,
}

/// 仓库名编码成单个缓存文件名。每个 `/` 换成 `%2F`。
///
/// 已经含有 `%2F` 的名字会和带斜线的名字撞车。产品规格没有写这种输入。
pub fn cache_file_id(repo: &str) -> Result<String, GithubError> {
    let id = repo.replace('/', "%2F");
    validate_id(&id).map_err(|_| GithubError::InvalidItem)?;
    Ok(id)
}

/// 列表上的一条。隐藏规则已经应用。没有筛选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedItem {
    pub repo: String,
    pub kind: SourceKind,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub stale: bool,
    pub ignored: bool,
    pub pinned: bool,
}

/// 一个已追踪仓库在列表里的样子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoList {
    pub repo: String,
    pub fetched_at: Option<i64>,
    pub offline_cache: bool,
    /// `offline_cache` 为真时是 [`OFFLINE_CACHE_LABEL`]。
    pub cache_label: Option<&'static str>,
    pub failure: Option<super::error::FetchFailure>,
    pub items: Vec<ListedItem>,
}

/// 筛选请求。任一字段为真都等 #9 第 18 项，不表示交集或并集。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GithubFilter {
    pub mine: bool,
    pub mentioned: bool,
    pub assigned: bool,
    pub participating: bool,
    pub needs_action: bool,
    pub needs_review: bool,
    pub ci_failed: bool,
    pub stale: bool,
    pub draft: bool,
}

impl GithubFilter {
    #[must_use]
    pub fn is_unfiltered(&self) -> bool {
        !self.mine
            && !self.mentioned
            && !self.assigned
            && !self.participating
            && !self.needs_action
            && !self.needs_review
            && !self.ci_failed
            && !self.stale
            && !self.draft
    }
}

/// 转为待办的标题。含仓库、编号和原标题。
#[must_use]
pub fn todo_title(repo: &str, number: u64, title: &str) -> String {
    format!("{repo}#{number} {title}")
}

pub(crate) fn unique_repos(repos: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for repo in repos {
        if !out.iter().any(|existing| existing == repo) {
            out.push(repo.clone());
        }
    }
    out
}

pub(crate) fn mark_is_set(marks: &[ItemMark], repo: &str, kind: SourceKind, number: u64) -> bool {
    marks
        .iter()
        .any(|mark| mark.repo == repo && mark.kind == kind && mark.number == number)
}

/// 有变化时返回 true。
pub(crate) fn set_mark(marks: &mut Vec<ItemMark>, mark: ItemMark, on: bool) -> bool {
    let pos = marks.iter().position(|item| {
        item.repo == mark.repo && item.kind == mark.kind && item.number == mark.number
    });
    match (pos, on) {
        (Some(index), false) => {
            marks.remove(index);
            true
        }
        (None, true) => {
            marks.push(mark);
            true
        }
        _ => false,
    }
}

pub(crate) fn check_mark(repo: &str, number: u64) -> Result<(), GithubError> {
    if repo.is_empty()
        || repo.trim().is_empty()
        || repo.chars().any(char::is_control)
        || number == 0
    {
        return Err(GithubError::InvalidItem);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_watchlist_missing_marks_reads() {
        let list: Watchlist = serde_json::from_str(r#"{"repos":["example/widget"]}"#).unwrap();
        assert_eq!(list.schema_version, SCHEMA_VERSION);
        assert_eq!(list.repos, vec!["example/widget".to_owned()]);
        assert!(list.ignored.is_empty());
        assert!(list.pinned.is_empty());
    }

    #[test]
    fn cache_id_encodes_slashes() {
        assert_eq!(cache_file_id("example/widget").unwrap(), "example%2Fwidget");
        assert!(cache_file_id("").is_err());
        assert!(cache_file_id("CON").is_err());
    }

    #[test]
    fn todo_title_contains_repo_number_and_text() {
        let title = todo_title("example/widget", 12, "Fix the latch");
        assert_eq!(title, "example/widget#12 Fix the latch");
    }
}
