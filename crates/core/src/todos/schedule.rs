//! 到期、周期和提醒的纯函数。
//!
//! 不读时钟，也不写盘。今天和当前时刻由调用方传入，供提醒调度使用。
//!
//! 每月重复用 [`Recurrence::month_day`] 作为要回到的日子。缺字段或不是 1 至 31 时，用当前到期日的日子。
//! 这个日子是 31、目标月没有 31 日时，下一次落到该月最后一天；再下一次仍按 31 日。
//! 29 日、30 日在目标月不存在时仍返回 [`NextOccurrence::MonthlyMissingDay`]，不猜测。
//! [`NextOccurrence::OnUntil`] 表示这条的到期日已经是重复截止日，完成时不再生成下一次。

use crate::CivilDate;

use super::model::{ClockTime, Recurrence, RecurrenceRule, TodoItem};

/// 处理模式顺延天数。调用方把设置里的值传进来；设置缺省时用 [`DEFAULT_DEFER_DAYS`]。
pub const MIN_DEFER_DAYS: u32 = 1;
pub const MAX_DEFER_DAYS: u32 = 30;
pub const DEFAULT_DEFER_DAYS: u32 = 3;

/// 下一次周期到期日。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextOccurrence {
    /// 可以生成的下一次。落在截止日当天时也包括在内。
    Date(CivilDate),
    /// 下一次晚于截止日，不再生成。
    PastUntil,
    /// 有周期但没有到期日，不发明一个日期。
    MissingDue,
    /// 每月重复的日不是 31，而目标月没有这一天。不猜测落到哪一天。
    MonthlyMissingDay,
    /// 这条的到期日已经是重复截止日。完成时不再生成下一次。
    OnUntil,
    /// 日期加减超出可表示范围。
    Overflow,
}

/// 回收站保留时间。进入回收站满这段时间后自动清除。
///
/// 一天按 86_400_000 毫秒，不按日历。30 天就是 30 个这样的长度。
pub const TRASH_RETENTION_MS: i64 = 30 * 86_400_000;

/// `deleted_at_ms` 到 `now_ms` 是否已经满 [`TRASH_RETENTION_MS`]。
#[must_use]
pub fn trash_expired(deleted_at_ms: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(deleted_at_ms) >= TRASH_RETENTION_MS
}

/// 从当前到期日和周期规则推下一次。
#[must_use]
pub fn next_occurrence(due: Option<CivilDate>, recurrence: &Recurrence) -> NextOccurrence {
    let Some(due) = due else {
        return NextOccurrence::MissingDue;
    };
    if recurrence.until == Some(due) {
        return NextOccurrence::OnUntil;
    }
    let next = match recurrence.rule {
        RecurrenceRule::Daily => due.checked_add_days(1),
        RecurrenceRule::Weekly => due.checked_add_days(7),
        RecurrenceRule::Biweekly => due.checked_add_days(14),
        RecurrenceRule::Monthly => match add_one_month(due, recurrence.month_day) {
            Ok(next) => Some(next),
            Err(MonthShift::MissingDay) => return NextOccurrence::MonthlyMissingDay,
            Err(MonthShift::Overflow) => return NextOccurrence::Overflow,
        },
    };
    let Some(next) = next else {
        return NextOccurrence::Overflow;
    };
    if let Some(until) = recurrence.until
        && next > until
    {
        return NextOccurrence::PastUntil;
    }
    NextOccurrence::Date(next)
}

/// 顺延 `days` 天。没有到期日时从 `today` 起算。
pub fn deferred_due(
    due: Option<CivilDate>,
    today: CivilDate,
    days: u32,
) -> Result<CivilDate, DeferError> {
    if !(MIN_DEFER_DAYS..=MAX_DEFER_DAYS).contains(&days) {
        return Err(DeferError::InvalidDays);
    }
    let base = due.unwrap_or(today);
    let days = i32::try_from(days).map_err(|_| DeferError::Overflow)?;
    base.checked_add_days(days).ok_or(DeferError::Overflow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferError {
    InvalidDays,
    Overflow,
}

/// 未完成、不在回收站、到期日早于今天。到期日就是今天时不算逾期。
#[must_use]
pub fn is_overdue(item: &TodoItem, today: CivilDate) -> bool {
    if item.completed || item.in_trash() {
        return false;
    }
    matches!(item.due, Some(due) if due < today)
}

#[must_use]
pub fn overdue_count<'a, I>(items: I, today: CivilDate) -> u32
where
    I: IntoIterator<Item = &'a TodoItem>,
{
    items
        .into_iter()
        .filter(|item| is_overdue(item, today))
        .count() as u32
}

/// 下一次提醒。没有更晚的提醒时返回 `None`，不补发已经过去的时刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReminderInstant {
    pub date: CivilDate,
    pub time: ClockTime,
}

/// `minute_of_day` 是调用方所在本地日的分钟数，0 到 1439。
/// 提醒时刻等于这一分钟时仍返回，供到点发送。
#[must_use]
pub fn next_reminder(
    item: &TodoItem,
    today: CivilDate,
    minute_of_day: u16,
) -> Option<ReminderInstant> {
    if item.completed || item.in_trash() || minute_of_day > 24 * 60 - 1 {
        return None;
    }
    let due = item.due?;
    let time = item.remind_at?;
    let upcoming = due > today || (due == today && time.minute_of_day() >= minute_of_day);
    if upcoming {
        Some(ReminderInstant { date: due, time })
    } else {
        None
    }
}

enum MonthShift {
    MissingDay,
    Overflow,
}

fn monthly_anchor(due: CivilDate, month_day: Option<u8>) -> u8 {
    match month_day {
        Some(day) if (1..=31).contains(&day) => day,
        _ => due.day(),
    }
}

fn add_one_month(date: CivilDate, month_day: Option<u8>) -> Result<CivilDate, MonthShift> {
    let (year, month) = if date.month() == 12 {
        let Some(year) = date.year().checked_add(1) else {
            return Err(MonthShift::Overflow);
        };
        (year, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    let day = monthly_anchor(date, month_day);
    if let Some(next) = CivilDate::try_from_ymd(year, month, day) {
        return Ok(next);
    }
    if day == 31 {
        let last = crate::capture::days_in_month(year, month);
        return CivilDate::try_from_ymd(year, month, last).ok_or(MonthShift::Overflow);
    }
    Err(MonthShift::MissingDay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::model::{Recurrence, RecurrenceRule, TodoItem};

    fn date(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::try_from_ymd(year, month, day).unwrap()
    }

    fn item_due(due: CivilDate) -> TodoItem {
        let mut item = TodoItem::new("a".into(), "t".into(), 0);
        item.due = Some(due);
        item
    }

    #[test]
    fn recurrence_crosses_month_and_year() {
        let daily = Recurrence {
            rule: RecurrenceRule::Daily,
            until: None,
            month_day: None,
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 31)), &daily),
            NextOccurrence::Date(date(2026, 2, 1))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 12, 31)), &daily),
            NextOccurrence::Date(date(2027, 1, 1))
        );
        let weekly = Recurrence {
            rule: RecurrenceRule::Weekly,
            until: None,
            month_day: None,
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 28)), &weekly),
            NextOccurrence::Date(date(2026, 2, 4))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 12, 30)), &weekly),
            NextOccurrence::Date(date(2027, 1, 6))
        );
        let biweekly = Recurrence {
            rule: RecurrenceRule::Biweekly,
            until: None,
            month_day: None,
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 25)), &biweekly),
            NextOccurrence::Date(date(2026, 2, 8))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 12, 20)), &biweekly),
            NextOccurrence::Date(date(2027, 1, 3))
        );
        let monthly = Recurrence {
            rule: RecurrenceRule::Monthly,
            until: None,
            month_day: None,
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 15)), &monthly),
            NextOccurrence::Date(date(2026, 2, 15))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 12, 31)), &monthly),
            NextOccurrence::Date(date(2027, 1, 31))
        );
        assert_eq!(
            next_occurrence(Some(date(2024, 1, 29)), &monthly),
            NextOccurrence::Date(date(2024, 2, 29))
        );
    }

    #[test]
    fn until_includes_the_boundary_day_but_not_the_day_after() {
        let daily = Recurrence {
            rule: RecurrenceRule::Daily,
            until: Some(date(2026, 1, 31)),
            month_day: None,
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 30)), &daily),
            NextOccurrence::Date(date(2026, 1, 31))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 31)), &daily),
            NextOccurrence::OnUntil
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 2, 1)), &daily),
            NextOccurrence::PastUntil
        );
    }

    #[test]
    fn monthly_31_lands_on_the_last_day_of_a_short_month() {
        let monthly = Recurrence {
            rule: RecurrenceRule::Monthly,
            until: None,
            month_day: None,
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 31)), &monthly),
            NextOccurrence::Date(date(2026, 2, 28))
        );
        assert_eq!(
            next_occurrence(Some(date(2024, 1, 31)), &monthly),
            NextOccurrence::Date(date(2024, 2, 29))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 3, 31)), &monthly),
            NextOccurrence::Date(date(2026, 4, 30))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 2, 28)), &monthly),
            NextOccurrence::Date(date(2026, 3, 28))
        );
        let anchored = Recurrence {
            rule: RecurrenceRule::Monthly,
            until: None,
            month_day: Some(31),
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 31)), &anchored),
            NextOccurrence::Date(date(2026, 2, 28))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 2, 28)), &anchored),
            NextOccurrence::Date(date(2026, 3, 31))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 3, 31)), &anchored),
            NextOccurrence::Date(date(2026, 4, 30))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 4, 30)), &anchored),
            NextOccurrence::Date(date(2026, 5, 31))
        );
        assert_eq!(
            next_occurrence(Some(date(2024, 2, 29)), &anchored),
            NextOccurrence::Date(date(2024, 3, 31))
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 30)), &monthly),
            NextOccurrence::MonthlyMissingDay
        );
        let thirtieth = Recurrence {
            rule: RecurrenceRule::Monthly,
            until: None,
            month_day: Some(30),
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 30)), &thirtieth),
            NextOccurrence::MonthlyMissingDay
        );
        assert_eq!(
            next_occurrence(Some(date(2025, 1, 29)), &monthly),
            NextOccurrence::MonthlyMissingDay
        );
    }

    #[test]
    fn trash_expires_when_thirty_days_have_elapsed() {
        let deleted_at = 1_000;
        assert!(!trash_expired(
            deleted_at,
            deleted_at + TRASH_RETENTION_MS - 1
        ));
        assert!(trash_expired(deleted_at, deleted_at + TRASH_RETENTION_MS));
        assert_eq!(TRASH_RETENTION_MS, 30 * 86_400_000);
    }

    #[test]
    fn defer_accepts_1_to_30_and_uses_today_when_due_is_absent() {
        let today = date(2026, 10, 8);
        assert_eq!(
            deferred_due(Some(today), today, 1).unwrap(),
            date(2026, 10, 9)
        );
        assert_eq!(
            deferred_due(Some(today), today, 30).unwrap(),
            date(2026, 11, 7)
        );
        assert_eq!(deferred_due(None, today, 3).unwrap(), date(2026, 10, 11));
        assert_eq!(
            deferred_due(Some(today), today, 0),
            Err(DeferError::InvalidDays)
        );
        assert_eq!(
            deferred_due(Some(today), today, 31),
            Err(DeferError::InvalidDays)
        );
        assert_eq!(DEFAULT_DEFER_DAYS, 3);
    }

    #[test]
    fn overdue_is_strictly_before_today_and_skips_done_or_trash() {
        let today = date(2026, 10, 8);
        let mut yesterday = item_due(date(2026, 10, 7));
        assert!(is_overdue(&yesterday, today));
        let today_item = item_due(today);
        assert!(!is_overdue(&today_item, today));
        yesterday.completed = true;
        assert!(!is_overdue(&yesterday, today));
        yesterday.completed = false;
        yesterday.deleted_at = Some(1);
        assert!(!is_overdue(&yesterday, today));
        let open = item_due(date(2026, 10, 1));
        assert_eq!(overdue_count([&yesterday, &today_item, &open], today), 1);
    }

    #[test]
    fn next_reminder_keeps_the_current_minute_and_drops_the_past() {
        let today = date(2026, 10, 8);
        let mut item = item_due(today);
        item.remind_at = ClockTime::try_new(9, 0);
        let at_nine = next_reminder(&item, today, 9 * 60).unwrap();
        assert_eq!(at_nine.time, ClockTime::try_new(9, 0).unwrap());
        assert!(next_reminder(&item, today, 9 * 60 + 1).is_none());
        item.due = Some(date(2026, 10, 9));
        assert!(next_reminder(&item, today, 23 * 60).is_some());
        item.completed = true;
        assert!(next_reminder(&item, today, 0).is_none());
    }
}
