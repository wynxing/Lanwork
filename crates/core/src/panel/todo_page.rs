//! 待办页的数据整理：左侧清单、右侧行、回收站、定位，以及到期日、提醒和周期的输入解析。
//!
//! 输入是 [`TodoCommands::lists`](crate::todos::TodoCommands::lists) 的快照。这里不读写盘，
//! 不读时钟，今天和当前时间由调用方传入。规则仍在待办服务里。
//!
//! 已完成的条目留在清单里。显示时未完成在前、已完成在后，各自保持服务里的 `order`。
//! 这个先后不是产品规则。取消完成不改 `order`，所以该条回到未完成条目之间原来的位置。
//! 回收站里的条目只在回收站视图里出现。

use std::fmt;

use crate::CivilDate;
use crate::todos::{
    ListKind, Recurrence, RecurrenceRule, TRASH_RETENTION_MS, TodoItem, TodoList, is_overdue,
};

const DAY_MS: i64 = 86_400_000;

/// 周期的选择项。第 0 项是没有周期。
pub const RECURRENCE_CHOICES: [&str; 5] = ["无", "每天", "每周", "每两周", "每月"];

/// 当前显示的内容：某个清单，或回收站。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoView {
    List(String),
    Trash,
}

/// 左侧清单一行。`open` 是未完成且不在回收站的条目数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub id: String,
    pub name: String,
    pub open: usize,
    pub inbox: bool,
}

/// 右侧一行。文字字段已经是要显示的样子，空字符串表示没有这个标记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub completed: bool,
    pub current: bool,
    pub due: String,
    pub overdue: bool,
    pub recurrence: &'static str,
    pub source: String,
    /// 回收站行：还剩几天被清除。
    pub trash_note: String,
}

/// 用户在到期日、提醒、重复截止日里输入的文字不合规。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    Date,
    Time,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Date => write!(f, "日期格式是 YYYY-MM-DD"),
            Self::Time => write!(f, "时间格式是 HH:MM"),
        }
    }
}

impl std::error::Error for InputError {}

/// 清单按服务给出的顺序，收件箱不另外提前。
#[must_use]
pub fn list_entries(lists: &[TodoList]) -> Vec<ListEntry> {
    lists
        .iter()
        .map(|list| ListEntry {
            id: list.id.clone(),
            name: list.name.clone(),
            open: list
                .items
                .iter()
                .filter(|item| !item.completed && !item.in_trash())
                .count(),
            inbox: list.kind == ListKind::Inbox,
        })
        .collect()
}

#[must_use]
pub fn trash_count(lists: &[TodoList]) -> usize {
    lists
        .iter()
        .flat_map(|list| &list.items)
        .filter(|item| item.in_trash())
        .count()
}

/// 默认显示收件箱；没有收件箱时显示第一个清单；没有清单时返回 `None`。
#[must_use]
pub fn default_view(lists: &[TodoList]) -> Option<TodoView> {
    lists
        .iter()
        .find(|list| list.kind == ListKind::Inbox)
        .or_else(|| lists.first())
        .map(|list| TodoView::List(list.id.clone()))
}

#[must_use]
pub fn view_exists(lists: &[TodoList], view: &TodoView) -> bool {
    match view {
        TodoView::Trash => true,
        TodoView::List(id) => lists.iter().any(|list| &list.id == id),
    }
}

fn row_of(
    list: &TodoList,
    item: &TodoItem,
    today: Option<CivilDate>,
    now_ms: i64,
    in_trash_view: bool,
) -> Row {
    Row {
        id: item.id.clone(),
        list_id: list.id.clone(),
        title: item.title.clone(),
        completed: item.completed,
        current: item.current,
        due: item.due.map(format_date).unwrap_or_default(),
        overdue: today.is_some_and(|today| is_overdue(item, today)),
        recurrence: item
            .recurrence
            .as_ref()
            .map_or("", |recurrence| rule_label(recurrence.rule)),
        source: item
            .source
            .as_ref()
            .map(|source| format!("{}#{}", source.repo, source.number))
            .unwrap_or_default(),
        trash_note: if in_trash_view {
            item.deleted_at
                .map(|deleted| format!("剩 {} 天", trash_days_left(deleted, now_ms)))
                .unwrap_or_default()
        } else {
            String::new()
        },
    }
}

/// 视图里的行。读不到今天的日期时（`today` 为空）不标逾期。清单视图不含回收站条目；回收站视图含所有清单里已软删除的条目，最近删除的在前。
#[must_use]
pub fn rows(
    lists: &[TodoList],
    view: &TodoView,
    today: Option<CivilDate>,
    now_ms: i64,
) -> Vec<Row> {
    match view {
        TodoView::List(id) => {
            let Some(list) = lists.iter().find(|list| &list.id == id) else {
                return Vec::new();
            };
            let live = || list.items.iter().filter(|item| !item.in_trash());
            live()
                .filter(|item| !item.completed)
                .chain(live().filter(|item| item.completed))
                .map(|item| row_of(list, item, today, now_ms, false))
                .collect()
        }
        TodoView::Trash => {
            let mut trashed: Vec<(&TodoList, &TodoItem)> = lists
                .iter()
                .flat_map(|list| list.items.iter().map(move |item| (list, item)))
                .filter(|(_, item)| item.in_trash())
                .collect();
            trashed.sort_by(|(_, left), (_, right)| {
                right
                    .deleted_at
                    .cmp(&left.deleted_at)
                    .then_with(|| left.id.cmp(&right.id))
            });
            trashed
                .into_iter()
                .map(|(list, item)| row_of(list, item, today, now_ms, true))
                .collect()
        }
    }
}

/// 条目所在的视图和它在 [`rows`] 里的下标。条目在回收站时定位到回收站视图。
#[must_use]
pub fn locate(
    lists: &[TodoList],
    item_id: &str,
    today: Option<CivilDate>,
    now_ms: i64,
) -> Option<(TodoView, usize)> {
    let (list, item) = lists.iter().find_map(|list| {
        list.items
            .iter()
            .find(|item| item.id == item_id)
            .map(|item| (list, item))
    })?;
    let view = if item.in_trash() {
        TodoView::Trash
    } else {
        TodoView::List(list.id.clone())
    };
    let index = rows(lists, &view, today, now_ms)
        .iter()
        .position(|row| row.id == item_id)?;
    Some((view, index))
}

/// 把未完成的 `item_id` 在清单里上移或下移一位后的完整 id 顺序，交给 `reorder_items`。
///
/// 只和相邻的未完成、未删除条目互换。已完成和回收站里的条目留在原位置，
/// 因为服务要求传入清单里的全部条目。已经在顶端或底端时返回 `None`。
#[must_use]
pub fn reordered_ids(
    lists: &[TodoList],
    list_id: &str,
    item_id: &str,
    up: bool,
) -> Option<Vec<String>> {
    let list = lists.iter().find(|list| list.id == list_id)?;
    let movable: Vec<usize> = list
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.completed && !item.in_trash())
        .map(|(index, _)| index)
        .collect();
    let at = movable
        .iter()
        .position(|index| list.items[*index].id == item_id)?;
    let other = if up { at.checked_sub(1)? } else { at + 1 };
    let other = *movable.get(other)?;
    let mut ids: Vec<String> = list.items.iter().map(|item| item.id.clone()).collect();
    ids.swap(movable[at], other);
    Some(ids)
}

/// 进入回收站的条目还有几天被清除，向上取整。已经满 30 天时是 0。
#[must_use]
pub fn trash_days_left(deleted_at_ms: i64, now_ms: i64) -> i64 {
    let remaining = deleted_at_ms
        .saturating_add(TRASH_RETENTION_MS)
        .saturating_sub(now_ms);
    if remaining <= 0 {
        0
    } else {
        (remaining + DAY_MS - 1) / DAY_MS
    }
}

#[must_use]
pub fn format_date(date: CivilDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn rule_label(rule: RecurrenceRule) -> &'static str {
    RECURRENCE_CHOICES[match rule {
        RecurrenceRule::Daily => 1,
        RecurrenceRule::Weekly => 2,
        RecurrenceRule::Biweekly => 3,
        RecurrenceRule::Monthly => 4,
    }]
}

/// 空白是清除。分隔符接受 `-`、`/`、`.`，月和日可以只写一位。
pub fn parse_due(text: &str) -> Result<Option<CivilDate>, InputError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let mut parts = text.split(['-', '/', '.']);
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(InputError::Date);
    };
    if year.len() != 4 || !(1..=2).contains(&month.len()) || !(1..=2).contains(&day.len()) {
        return Err(InputError::Date);
    }
    if ![year, month, day]
        .iter()
        .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return Err(InputError::Date);
    }
    let year = year.parse::<i32>().map_err(|_| InputError::Date)?;
    let month = month.parse::<u8>().map_err(|_| InputError::Date)?;
    let day = day.parse::<u8>().map_err(|_| InputError::Date)?;
    CivilDate::try_from_ymd(year, month, day)
        .map(Some)
        .ok_or(InputError::Date)
}

/// 空白是清除。`HH:MM`，小时可以只写一位。
pub fn parse_remind(text: &str) -> Result<Option<(u8, u8)>, InputError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let Some((hour, minute)) = text.split_once([':', '：']) else {
        return Err(InputError::Time);
    };
    if !(1..=2).contains(&hour.len())
        || minute.len() != 2
        || !hour
            .chars()
            .chain(minute.chars())
            .all(|ch| ch.is_ascii_digit())
    {
        return Err(InputError::Time);
    }
    let hour = hour.parse::<u8>().map_err(|_| InputError::Time)?;
    let minute = minute.parse::<u8>().map_err(|_| InputError::Time)?;
    if hour > 23 || minute > 59 {
        return Err(InputError::Time);
    }
    Ok(Some((hour, minute)))
}

/// 周期选择项的下标，对应 [`RECURRENCE_CHOICES`]。
#[must_use]
pub fn recurrence_index(recurrence: Option<&Recurrence>) -> usize {
    match recurrence.map(|recurrence| recurrence.rule) {
        None => 0,
        Some(RecurrenceRule::Daily) => 1,
        Some(RecurrenceRule::Weekly) => 2,
        Some(RecurrenceRule::Biweekly) => 3,
        Some(RecurrenceRule::Monthly) => 4,
    }
}

/// 由选择项和重复截止日得到要写入的周期。选「无」返回 `None`。
///
/// 每月重复沿用已有的锚点日 `monthDay`，没有就留空，由服务按到期日补。
#[must_use]
pub fn recurrence_from_choice(
    choice: usize,
    until: Option<CivilDate>,
    existing: Option<&Recurrence>,
) -> Option<Recurrence> {
    let rule = match choice {
        1 => RecurrenceRule::Daily,
        2 => RecurrenceRule::Weekly,
        3 => RecurrenceRule::Biweekly,
        4 => RecurrenceRule::Monthly,
        _ => return None,
    };
    let month_day = existing
        .filter(|_| rule == RecurrenceRule::Monthly)
        .and_then(|recurrence| recurrence.month_day);
    Some(Recurrence {
        rule,
        until,
        month_day,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::{SourceKind, TodoSource};

    fn date(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::try_from_ymd(year, month, day).unwrap()
    }

    fn item(id: &str, order: i64) -> TodoItem {
        TodoItem::new(id.to_owned(), format!("标题 {id}"), order)
    }

    fn list(id: &str, kind: ListKind, items: Vec<TodoItem>) -> TodoList {
        TodoList {
            schema_version: 1,
            id: id.to_owned(),
            name: if kind == ListKind::Inbox {
                "收件箱".to_owned()
            } else {
                format!("清单 {id}")
            },
            kind,
            order: 0,
            items,
        }
    }

    fn ids(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.id.as_str()).collect()
    }

    #[test]
    fn default_view_is_the_inbox_even_when_it_is_not_first() {
        let lists = vec![
            list("work", ListKind::Normal, vec![]),
            list("inbox", ListKind::Inbox, vec![]),
        ];
        assert_eq!(
            default_view(&lists),
            Some(TodoView::List("inbox".to_owned()))
        );
        assert_eq!(
            default_view(&lists[..1]),
            Some(TodoView::List("work".to_owned()))
        );
        assert_eq!(default_view(&[]), None);
    }

    #[test]
    fn entries_count_only_open_items_and_trash_counts_deleted_ones() {
        let mut done = item("b", 1);
        done.completed = true;
        let mut gone = item("c", 2);
        gone.deleted_at = Some(5);
        let lists = vec![list(
            "inbox",
            ListKind::Inbox,
            vec![item("a", 0), done, gone],
        )];
        let entries = list_entries(&lists);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].open, 1);
        assert!(entries[0].inbox);
        assert_eq!(trash_count(&lists), 1);
    }

    #[test]
    fn list_rows_put_open_items_first_and_hide_the_trash() {
        let mut done = item("a", 0);
        done.completed = true;
        let mut gone = item("c", 2);
        gone.deleted_at = Some(5);
        let lists = vec![list(
            "inbox",
            ListKind::Inbox,
            vec![done, item("b", 1), gone],
        )];
        let view = TodoView::List("inbox".to_owned());
        let shown = rows(&lists, &view, Some(date(2026, 10, 10)), 0);
        assert_eq!(ids(&shown), ["b", "a"]);
        assert!(shown[1].completed);
        assert!(
            rows(
                &lists,
                &TodoView::List("nope".to_owned()),
                Some(date(2026, 10, 10)),
                0
            )
            .is_empty()
        );
    }

    #[test]
    fn marks_show_due_overdue_recurrence_source_and_current() {
        let mut marked = item("a", 0);
        marked.due = Some(date(2026, 10, 9));
        marked.recurrence = Some(Recurrence {
            rule: RecurrenceRule::Biweekly,
            until: None,
            month_day: None,
        });
        marked.source = Some(
            TodoSource::try_new(
                SourceKind::GithubPr,
                "https://github.com/o/r/pull/7",
                "o/r",
                7,
            )
            .unwrap(),
        );
        marked.current = true;
        let lists = vec![list("inbox", ListKind::Inbox, vec![marked, item("b", 1)])];
        let view = TodoView::List("inbox".to_owned());
        let shown = rows(&lists, &view, Some(date(2026, 10, 10)), 0);
        assert_eq!(shown[0].due, "2026-10-09");
        assert!(shown[0].overdue);
        assert_eq!(shown[0].recurrence, "每两周");
        assert_eq!(shown[0].source, "o/r#7");
        assert!(shown[0].current);
        assert_eq!(shown[1].due, "");
        assert!(!shown[1].overdue);
        assert_eq!(shown[1].recurrence, "");
        assert_eq!(shown[1].source, "");
        let today = rows(&lists, &view, Some(date(2026, 10, 9)), 0);
        assert!(!today[0].overdue, "到期日是今天不算逾期");
        let unknown = rows(&lists, &view, None, 0);
        assert!(!unknown[0].overdue, "读不到今天的日期时不标逾期");
    }

    #[test]
    fn trash_rows_come_from_every_list_newest_first_with_days_left() {
        let mut older = item("a", 0);
        older.deleted_at = Some(1_000);
        let mut newer = item("b", 0);
        newer.deleted_at = Some(2_000);
        let lists = vec![
            list("inbox", ListKind::Inbox, vec![older]),
            list("work", ListKind::Normal, vec![newer, item("live", 1)]),
        ];
        let shown = rows(&lists, &TodoView::Trash, Some(date(2026, 10, 10)), 2_000);
        assert_eq!(ids(&shown), ["b", "a"]);
        assert_eq!(shown[0].trash_note, "剩 30 天");
    }

    #[test]
    fn days_left_rounds_up_and_reaches_zero_at_thirty_days() {
        let deleted = 1_000;
        assert_eq!(trash_days_left(deleted, deleted), 30);
        assert_eq!(trash_days_left(deleted, deleted + DAY_MS), 29);
        assert_eq!(trash_days_left(deleted, deleted + DAY_MS + 1), 29);
        assert_eq!(trash_days_left(deleted, deleted + 29 * DAY_MS + 1), 1);
        assert_eq!(trash_days_left(deleted, deleted + 30 * DAY_MS), 0);
        assert_eq!(trash_days_left(deleted, deleted + 31 * DAY_MS), 0);
    }

    #[test]
    fn clearing_completed_puts_the_item_back_among_open_items_by_order() {
        let mut done = item("b", 1);
        done.completed = true;
        let completed = vec![list(
            "inbox",
            ListKind::Inbox,
            vec![item("a", 0), done, item("c", 2)],
        )];
        let today = Some(date(2026, 10, 10));
        assert_eq!(
            ids(&rows(&completed, &TodoView::List("inbox".into()), today, 0)),
            ["a", "c", "b"]
        );
        let reopened = vec![list(
            "inbox",
            ListKind::Inbox,
            vec![item("a", 0), item("b", 1), item("c", 2)],
        )];
        assert_eq!(
            ids(&rows(&reopened, &TodoView::List("inbox".into()), today, 0)),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn locate_finds_the_list_row_or_the_trash_row() {
        let mut done = item("a", 0);
        done.completed = true;
        let mut gone = item("g", 3);
        gone.deleted_at = Some(5);
        let lists = vec![
            list("inbox", ListKind::Inbox, vec![done, item("b", 1)]),
            list(
                "work",
                ListKind::Normal,
                vec![item("w1", 0), item("w2", 1), gone],
            ),
        ];
        let today = Some(date(2026, 10, 10));
        assert_eq!(
            locate(&lists, "w2", today, 0),
            Some((TodoView::List("work".to_owned()), 1))
        );
        assert_eq!(
            locate(&lists, "a", today, 0),
            Some((TodoView::List("inbox".to_owned()), 1)),
            "已完成的排在未完成之后"
        );
        assert_eq!(locate(&lists, "g", today, 10), Some((TodoView::Trash, 0)));
        assert_eq!(locate(&lists, "missing", today, 0), None);
    }

    #[test]
    fn reorder_swaps_with_the_adjacent_open_item_and_keeps_the_rest_in_place() {
        let mut done = item("d", 1);
        done.completed = true;
        let mut gone = item("g", 3);
        gone.deleted_at = Some(5);
        let lists = vec![list(
            "inbox",
            ListKind::Inbox,
            vec![item("a", 0), done, item("b", 2), gone, item("c", 4)],
        )];
        assert_eq!(
            reordered_ids(&lists, "inbox", "b", true).unwrap(),
            ["b", "d", "a", "g", "c"]
        );
        assert_eq!(
            reordered_ids(&lists, "inbox", "b", false).unwrap(),
            ["a", "d", "c", "g", "b"]
        );
        assert_eq!(reordered_ids(&lists, "inbox", "a", true), None);
        assert_eq!(reordered_ids(&lists, "inbox", "c", false), None);
        assert_eq!(reordered_ids(&lists, "inbox", "d", true), None);
        assert_eq!(reordered_ids(&lists, "inbox", "zzz", true), None);
        assert_eq!(reordered_ids(&lists, "none", "a", true), None);
    }

    #[test]
    fn due_input_accepts_iso_forms_and_rejects_the_rest() {
        assert_eq!(parse_due(""), Ok(None));
        assert_eq!(parse_due("  "), Ok(None));
        assert_eq!(parse_due("2026-10-12"), Ok(Some(date(2026, 10, 12))));
        assert_eq!(parse_due(" 2026/1/5 "), Ok(Some(date(2026, 1, 5))));
        assert_eq!(parse_due("2028.02.29"), Ok(Some(date(2028, 2, 29))));
        for bad in [
            "2027-02-29",
            "2026-13-01",
            "2026-00-10",
            "26-10-10",
            "2026-10",
            "2026-10-10-1",
            "abc",
            "2026-1a-10",
            "２０２６-10-10",
        ] {
            assert_eq!(parse_due(bad), Err(InputError::Date), "{bad}");
        }
        assert_eq!(InputError::Date.to_string(), "日期格式是 YYYY-MM-DD");
    }

    #[test]
    fn remind_input_is_hour_and_minute() {
        assert_eq!(parse_remind(""), Ok(None));
        assert_eq!(parse_remind("9:05"), Ok(Some((9, 5))));
        assert_eq!(parse_remind("09:05"), Ok(Some((9, 5))));
        assert_eq!(parse_remind("23：59"), Ok(Some((23, 59))));
        for bad in [
            "24:00", "12:60", "12:5", "1205", "12:", ":30", "ab:cd", "12:30:00",
        ] {
            assert_eq!(parse_remind(bad), Err(InputError::Time), "{bad}");
        }
    }

    #[test]
    fn recurrence_choice_round_trips_and_keeps_the_month_anchor() {
        assert_eq!(recurrence_index(None), 0);
        assert_eq!(
            recurrence_from_choice(0, Some(date(2027, 1, 1)), None),
            None
        );
        let weekly = recurrence_from_choice(2, Some(date(2027, 1, 1)), None).unwrap();
        assert_eq!(weekly.rule, RecurrenceRule::Weekly);
        assert_eq!(weekly.until, Some(date(2027, 1, 1)));
        assert_eq!(recurrence_index(Some(&weekly)), 2);
        let monthly = Recurrence {
            rule: RecurrenceRule::Monthly,
            until: None,
            month_day: Some(31),
        };
        let kept = recurrence_from_choice(4, None, Some(&monthly)).unwrap();
        assert_eq!(kept.month_day, Some(31));
        let dropped = recurrence_from_choice(1, None, Some(&monthly)).unwrap();
        assert_eq!(dropped.month_day, None);
        assert_eq!(RECURRENCE_CHOICES.len(), 5);
        for (index, label) in RECURRENCE_CHOICES.iter().enumerate().skip(1) {
            let recurrence = recurrence_from_choice(index, None, None).unwrap();
            assert_eq!(rule_label(recurrence.rule), *label);
        }
    }
}
