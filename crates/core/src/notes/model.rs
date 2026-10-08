//! 便签模型和纯规则：显示标题、标签规范化、列表顺序。

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::storage::{SCHEMA_VERSION, is_supported_schema};

use super::error::NoteError;

/// 标题没有非空白字符时的显示名。
///
/// 便签列表和快速收集预览用同一规则。空白使用 Unicode `White_Space`（[`str::trim`]）。
pub const EMPTY_TITLE_DISPLAY: &str = "无标题";

/// Unix 纪元起的 UTC 毫秒。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TimestampMillis(i64);

impl TimestampMillis {
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }

    pub fn from_system(time: SystemTime) -> Self {
        match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => Self(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)),
            Err(_) => Self(0),
        }
    }
}

/// 一篇便签。文件名是 [`Self::id`]，路径为 `notes/<id>.json`。
///
/// 标题按调用方给的原文保存。空标题不会改写成「无标题」，显示时用 [`Self::display_title`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub created_at: TimestampMillis,
    pub updated_at: TimestampMillis,
    pub deleted_at: Option<TimestampMillis>,
    pub revision: u64,
}

impl Note {
    /// 列表和快速收集预览用的标题。
    pub fn display_title(&self) -> &str {
        display_title(&self.title)
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// 创建或保存时调用方提交的正文。服务借用它，失败时调用方仍持有这份数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteInput {
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub pinned: bool,
}

/// 标题没有非空白字符时返回「无标题」，否则返回原文（不裁剪首尾空白）。
///
/// # 例
///
/// ```
/// use lanwork_core::notes::display_title;
///
/// assert_eq!(display_title(""), "无标题");
/// assert_eq!(display_title("   "), "无标题");
/// assert_eq!(display_title("会议"), "会议");
/// ```
#[must_use]
pub fn display_title(title: &str) -> &str {
    if title.trim().is_empty() {
        EMPTY_TITLE_DISPLAY
    } else {
        title
    }
}

/// 去掉每个标签的首尾空白，丢掉空标签，并按原文去重（保留第一次出现的顺序）。
///
/// 比较区分大小写，也不做兼容分解。标签内部的空格保留。
///
/// # 例
///
/// ```
/// use lanwork_core::notes::normalize_tags;
///
/// assert_eq!(
///     normalize_tags([" 工作 ", "工作", "会议 记录", " "]),
///     vec!["工作".to_owned(), "会议 记录".to_owned()],
/// );
/// ```
#[must_use]
pub fn normalize_tags<I, S>(tags: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut normalized = Vec::new();
    for tag in tags {
        let trimmed = tag.as_ref().trim();
        if trimmed.is_empty() {
            continue;
        }
        if normalized.iter().any(|existing| existing == trimmed) {
            continue;
        }
        normalized.push(trimmed.to_owned());
    }
    normalized
}

/// 筛选用的标签。只有空白时没有可匹配的标签。
pub(crate) fn normalize_query_tag(tag: &str) -> Option<&str> {
    let trimmed = tag.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// 置顶在前，然后 `updated_at` 从新到旧，再按 id 升序。
pub(crate) fn sort_for_list(notes: &mut [Note]) {
    notes.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then(right.updated_at.cmp(&left.updated_at))
            .then(left.id.cmp(&right.id))
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkipReason {
    UnsupportedSchema { found: u32 },
}

impl SkipReason {
    pub(crate) fn error(&self, id: &str) -> NoteError {
        match self {
            Self::UnsupportedSchema { found } => NoteError::UnsupportedSchema {
                id: id.to_owned(),
                found: *found,
            },
        }
    }
}

fn default_note_schema() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NoteFile {
    #[serde(rename = "schemaVersion", default = "default_note_schema")]
    schema_version: u32,
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(rename = "createdAt", default)]
    created_at: TimestampMillis,
    #[serde(rename = "updatedAt", default)]
    updated_at: TimestampMillis,
    #[serde(rename = "deletedAt", default, skip_serializing_if = "Option::is_none")]
    deleted_at: Option<TimestampMillis>,
    #[serde(default)]
    revision: u64,
}

impl NoteFile {
    pub(crate) fn from_note(note: &Note) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: note.id.clone(),
            title: note.title.clone(),
            body: note.body.clone(),
            tags: note.tags.clone(),
            pinned: note.pinned,
            created_at: note.created_at,
            updated_at: note.updated_at,
            deleted_at: note.deleted_at,
            revision: note.revision,
        }
    }

    /// 文件名是 id。JSON 里的 `id` 缺了或与文件名不同时，以文件名为准，不在加载时写回。
    pub(crate) fn into_note(self, file_id: String) -> Result<Note, SkipReason> {
        if !is_supported_schema(self.schema_version) {
            return Err(SkipReason::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        Ok(Note {
            id: file_id,
            title: self.title,
            body: self.body,
            tags: normalize_tags(&self.tags),
            pinned: self.pinned,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
            revision: self.revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EMPTY_TITLE_DISPLAY, Note, TimestampMillis, display_title, normalize_tags, sort_for_list,
    };

    fn note(id: &str, pinned: bool, updated_at: i64) -> Note {
        Note {
            id: id.to_owned(),
            title: id.to_owned(),
            body: String::new(),
            tags: Vec::new(),
            pinned,
            created_at: TimestampMillis::from_millis(0),
            updated_at: TimestampMillis::from_millis(updated_at),
            deleted_at: None,
            revision: 1,
        }
    }

    #[test]
    fn empty_title_uses_the_shared_placeholder() {
        assert_eq!(display_title(""), EMPTY_TITLE_DISPLAY);
        assert_eq!(display_title(" "), EMPTY_TITLE_DISPLAY);
        assert_eq!(display_title(" \n\t "), EMPTY_TITLE_DISPLAY);
        assert_eq!(display_title("　"), EMPTY_TITLE_DISPLAY);
        assert_eq!(display_title("  会议"), "  会议");
        assert_eq!(display_title("会议 "), "会议 ");
        assert_eq!(display_title("无标题"), "无标题");
    }

    #[test]
    fn tags_trim_dedup_and_keep_internal_spaces() {
        let tags = normalize_tags([
            " 工作 ",
            "会议 记录",
            "工作",
            " ",
            "\t",
            "会议 记录",
            "　项目　",
            "Note",
            "note",
            "会议　记录",
        ]);
        assert_eq!(
            tags,
            vec![
                "工作".to_owned(),
                "会议 记录".to_owned(),
                "项目".to_owned(),
                "Note".to_owned(),
                "note".to_owned(),
                "会议　记录".to_owned(),
            ]
        );
    }

    #[test]
    fn list_order_is_pinned_then_newer_then_id() {
        let mut notes = vec![
            note("a", false, 5),
            note("b", true, 1),
            note("c", true, 3),
            note("d", false, 9),
            note("e", true, 3),
        ];
        sort_for_list(&mut notes);
        let ids: Vec<_> = notes.into_iter().map(|note| note.id).collect();
        assert_eq!(ids, vec!["c", "e", "b", "d", "a"]);
    }
}
