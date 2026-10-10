//! 待办清单和条目的 JSON 模型。
//!
//! 一个清单一个文件。字段名用架构文档里的 camelCase。
//! 新字段都有默认值，缺了这些字段的旧记录仍能读。
//! 时间戳是 UTC 的 Unix 毫秒。日期是 `YYYY-MM-DD`。提醒时刻是当天的 `HH:MM`。

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::CivilDate;
use crate::storage::SCHEMA_VERSION;

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn is_false(value: &bool) -> bool {
    !*value
}

use super::error::TodoError;

/// 清单种类。收件箱由 `kind` 识别，不由名称识别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ListKind {
    #[default]
    Normal,
    Inbox,
}

/// 周期。`until` 是重复截止日，包含在可以生成的到期日里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceRule {
    Daily,
    Weekly,
    Biweekly,
    Monthly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recurrence {
    pub rule: RecurrenceRule,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_date")]
    pub until: Option<CivilDate>,
    /// 每月重复要回到的日子，1 至 31。小月落到月末后，下一次仍用这个日子。
    ///
    /// 缺字段时由调用方改用当前到期日的日子。不是每月重复时不使用。
    #[serde(default, rename = "monthDay", skip_serializing_if = "Option::is_none")]
    pub month_day: Option<u8>,
}

/// 完成重复待办时生成的下一次。记在被完成的那一条上。
///
/// 只保存当时写入下一次的身份和字段，用来在取消完成时认出它。
/// 服务以前不记这条关系。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedNext {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_date")]
    pub due: Option<CivilDate>,
    #[serde(
        default,
        rename = "remindAt",
        skip_serializing_if = "Option::is_none",
        with = "opt_time"
    )]
    pub remind_at: Option<ClockTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<TodoSource>,
}

impl GeneratedNext {
    pub(crate) fn from_item(item: &TodoItem) -> Self {
        Self {
            id: item.id.clone(),
            title: item.title.clone(),
            due: item.due,
            remind_at: item.remind_at,
            recurrence: item.recurrence.clone(),
            source: item.source.clone(),
        }
    }
}

impl Recurrence {
    /// 每月重复还没有锚点日时，用当前到期日的日子补上。不是 1 至 31 的值视为没有。
    ///
    /// 已经写过的锚点日不改。这样小月里的到期日不会把 31 日覆盖成 28 日或 29 日。
    pub(crate) fn fill_missing_month_day(&mut self, due: Option<CivilDate>) {
        if self.rule != RecurrenceRule::Monthly {
            return;
        }
        if self.month_day.is_some_and(|day| !(1..=31).contains(&day)) {
            self.month_day = None;
        }
        if self.month_day.is_none()
            && let Some(due) = due
        {
            self.month_day = Some(due.day());
        }
    }
}

/// GitHub 来源种类。序列化值为 `github-pr` 与 `github-issue`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    #[serde(rename = "github-pr")]
    GithubPr,
    #[serde(rename = "github-issue")]
    GithubIssue,
}

/// 打开来源时只接受 `http` / `https`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoSource {
    #[serde(rename = "type")]
    pub kind: SourceKind,
    pub url: String,
    pub repo: String,
    pub number: u64,
}

impl TodoSource {
    /// 校验协议、仓库和编号。编号从 1 起。仓库去掉首尾空白后不能为空，也不能含空白。
    pub fn try_new(
        kind: SourceKind,
        url: impl Into<String>,
        repo: impl Into<String>,
        number: u64,
    ) -> Result<Self, TodoError> {
        let url = url.into();
        let repo = repo.into();
        let repo = repo.trim();
        if !is_http_source_url(&url) {
            return Err(TodoError::RejectedSource);
        }
        if repo.is_empty()
            || repo.chars().any(|ch| ch.is_whitespace() || ch.is_control())
            || number == 0
        {
            return Err(TodoError::InvalidSource);
        }
        Ok(Self {
            kind,
            url,
            repo: repo.to_owned(),
            number,
        })
    }
}

/// 协议必须是 `http` 或 `https`（大小写不敏感），并且带 `://` 和非空的后续部分。
#[must_use]
pub fn is_http_source_url(url: &str) -> bool {
    if url.is_empty() || url.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let Some(after) = rest.strip_prefix("//") else {
        return false;
    };
    !after.is_empty()
}

/// 到期日当天的提醒时刻。小时 0–23，分钟 0–59。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTime {
    hour: u8,
    minute: u8,
}

impl ClockTime {
    #[must_use]
    pub fn try_new(hour: u8, minute: u8) -> Option<Self> {
        if hour > 23 || minute > 59 {
            return None;
        }
        Some(Self { hour, minute })
    }

    #[must_use]
    pub fn hour(self) -> u8 {
        self.hour
    }

    #[must_use]
    pub fn minute(self) -> u8 {
        self.minute
    }

    #[must_use]
    pub fn minute_of_day(self) -> u16 {
        u16::from(self.hour) * 60 + u16::from(self.minute)
    }
}

/// 一条待办。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub completed: bool,
    /// 曾经被标为完成。取消完成不清这个标记，用来认出「下一次已经完成过」。
    #[serde(default, rename = "everCompleted", skip_serializing_if = "is_false")]
    pub ever_completed: bool,
    /// 这条是完成重复待办时生成的下一次，并且之后没有被改过。
    #[serde(
        default,
        rename = "generatedUntouched",
        skip_serializing_if = "is_false"
    )]
    pub generated_untouched: bool,
    /// 完成这条重复待办时生成的下一次。没有生成、或取消完成之后为空。
    #[serde(
        default,
        rename = "generatedNext",
        skip_serializing_if = "Option::is_none"
    )]
    pub generated_next: Option<GeneratedNext>,
    #[serde(default)]
    pub order: i64,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_date")]
    pub due: Option<CivilDate>,
    #[serde(
        default,
        rename = "remindAt",
        skip_serializing_if = "Option::is_none",
        with = "opt_time"
    )]
    pub remind_at: Option<ClockTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<TodoSource>,
    #[serde(default)]
    pub current: bool,
    #[serde(
        default,
        rename = "currentSince",
        skip_serializing_if = "Option::is_none"
    )]
    pub current_since: Option<i64>,
    #[serde(default, rename = "deletedAt", skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
    #[serde(
        default,
        rename = "originListId",
        skip_serializing_if = "Option::is_none"
    )]
    pub origin_list_id: Option<String>,
    #[serde(default, rename = "movedAt", skip_serializing_if = "Option::is_none")]
    pub moved_at: Option<i64>,
}

impl TodoItem {
    pub(crate) fn new(id: String, title: String, order: i64) -> Self {
        Self {
            id,
            title,
            completed: false,
            ever_completed: false,
            generated_untouched: false,
            generated_next: None,
            order,
            due: None,
            remind_at: None,
            recurrence: None,
            source: None,
            current: false,
            current_since: None,
            deleted_at: None,
            origin_list_id: None,
            moved_at: None,
        }
    }

    #[must_use]
    pub fn in_trash(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// 一个清单文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoList {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: ListKind,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub items: Vec<TodoItem>,
}

impl TodoList {
    pub(crate) fn new(id: String, name: String, kind: ListKind, order: i64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id,
            name,
            kind,
            order,
            items: Vec::new(),
        }
    }
}

/// 新建条目时命令接受的字段。标题由命令校验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTodo {
    pub title: String,
    pub due: Option<CivilDate>,
    pub remind_at: Option<ClockTime>,
    pub recurrence: Option<Recurrence>,
    pub source: Option<TodoSource>,
}

pub(crate) fn normalize_required(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn format_date(date: CivilDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn parse_date(text: &str) -> Option<CivilDate> {
    if text.len() != 10 {
        return None;
    }
    let year: i32 = text.get(0..4)?.parse().ok()?;
    let month: u8 = text.get(5..7)?.parse().ok()?;
    let day: u8 = text.get(8..10)?.parse().ok()?;
    if text.as_bytes().get(4) != Some(&b'-') || text.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    CivilDate::try_from_ymd(year, month, day)
}

fn format_time(time: ClockTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

fn parse_time(text: &str) -> Option<ClockTime> {
    if text.len() != 5 || text.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let hour: u8 = text.get(0..2)?.parse().ok()?;
    let minute: u8 = text.get(3..5)?.parse().ok()?;
    ClockTime::try_new(hour, minute)
}

mod opt_date {
    use super::{Deserialize, Deserializer, Serializer, format_date, parse_date};
    use crate::CivilDate;

    pub fn serialize<S>(value: &Option<CivilDate>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(date) => serializer.serialize_some(&format_date(*date)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<CivilDate>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = Option::<String>::deserialize(deserializer)?;
        match text {
            None => Ok(None),
            Some(text) => parse_date(&text)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom("日期无效")),
        }
    }
}

mod opt_time {
    use super::{ClockTime, Deserialize, Deserializer, Serializer, format_time, parse_time};

    pub fn serialize<S>(value: &Option<ClockTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(time) => serializer.serialize_some(&format_time(*time)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<ClockTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = Option::<String>::deserialize(deserializer)?;
        match text {
            None => Ok(None),
            Some(text) => parse_time(&text)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom("提醒时间无效")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_item_missing_optional_fields_reads() {
        let list: TodoList = serde_json::from_str(
            r#"{"id":"l1","name":"工作","items":[{"id":"a","title":"旧待办"}]}"#,
        )
        .unwrap();
        assert_eq!(list.schema_version, SCHEMA_VERSION);
        assert_eq!(list.kind, ListKind::Normal);
        assert_eq!(list.order, 0);
        let item = &list.items[0];
        assert!(!item.completed);
        assert!(!item.ever_completed);
        assert!(!item.generated_untouched);
        assert!(item.generated_next.is_none());
        assert!(item.due.is_none());
        assert!(item.remind_at.is_none());
        assert!(item.recurrence.is_none());
        assert!(item.source.is_none());
        assert!(!item.current);
        assert!(item.current_since.is_none());
        assert!(item.deleted_at.is_none());
        assert!(item.origin_list_id.is_none());
        assert!(item.moved_at.is_none());
    }

    #[test]
    fn present_fields_use_camel_case() {
        let mut item = TodoItem::new("a".into(), "标题".into(), 1);
        item.due = CivilDate::try_from_ymd(2026, 1, 31);
        item.remind_at = ClockTime::try_new(9, 5);
        item.ever_completed = true;
        item.generated_untouched = true;
        item.generated_next = Some(GeneratedNext {
            id: "n".into(),
            title: "下一次".into(),
            due: CivilDate::try_from_ymd(2026, 2, 28),
            remind_at: None,
            recurrence: None,
            source: None,
        });
        item.current = true;
        item.current_since = Some(10);
        item.deleted_at = Some(11);
        item.origin_list_id = Some("inbox".into());
        item.moved_at = Some(12);
        item.recurrence = Some(Recurrence {
            rule: RecurrenceRule::Monthly,
            until: CivilDate::try_from_ymd(2026, 12, 31),
            month_day: Some(31),
        });
        item.source = Some(
            TodoSource::try_new(
                SourceKind::GithubPr,
                "https://github.com/wynxing/Lanwork/pull/1",
                "wynxing/Lanwork",
                1,
            )
            .unwrap(),
        );
        let list = TodoList {
            schema_version: 1,
            id: "l1".into(),
            name: "工作".into(),
            kind: ListKind::Inbox,
            order: 0,
            items: vec![item],
        };
        let text = serde_json::to_string(&list).unwrap();
        assert!(text.contains("\"schemaVersion\":1"), "{text}");
        assert!(text.contains("\"movedAt\":12"), "{text}");
        assert!(text.contains("\"currentSince\":10"), "{text}");
        assert!(text.contains("\"deletedAt\":11"), "{text}");
        assert!(text.contains("\"originListId\":\"inbox\""), "{text}");
        assert!(text.contains("\"remindAt\":\"09:05\""), "{text}");
        assert!(text.contains("\"due\":\"2026-01-31\""), "{text}");
        assert!(text.contains("\"type\":\"github-pr\""), "{text}");
        assert!(text.contains("\"rule\":\"monthly\""), "{text}");
        assert!(text.contains("\"monthDay\":31"), "{text}");
        assert!(text.contains("\"everCompleted\":true"), "{text}");
        assert!(text.contains("\"generatedUntouched\":true"), "{text}");
        assert!(text.contains("\"generatedNext\""), "{text}");
        assert!(text.contains("\"id\":\"n\""), "{text}");
        assert!(!text.contains("moved_at"), "{text}");
        let parsed: TodoList = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, list);
    }

    #[test]
    fn source_url_schemes() {
        assert!(is_http_source_url("https://github.com/wynxing/Lanwork"));
        assert!(is_http_source_url("HTTP://example.com/a"));
        assert!(!is_http_source_url("javascript:alert(1)"));
        assert!(!is_http_source_url("file:///C:/Windows"));
        assert!(!is_http_source_url("ftp://example.com"));
        assert!(!is_http_source_url("http://"));
        assert!(!is_http_source_url(" https://example.com"));
        assert!(
            TodoSource::try_new(SourceKind::GithubIssue, "javascript:alert(1)", "a/b", 1).is_err()
        );
        assert!(
            TodoSource::try_new(SourceKind::GithubIssue, "https://example.com", "a/b", 0).is_err()
        );
    }
}
