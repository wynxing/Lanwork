//! 到期、周期和提醒的纯函数。
//!
//! 不读时钟，也不写盘。今天和当前时刻由调用方传入，供提醒调度使用。
//!
//! 两处边界等产品规格写明，这里返回 [`NextOccurrence::MonthlyMissingDay`] 和
//! [`NextOccurrence::OnUntil`]，不把 31 日夹到月末，也不决定截止日当天是否再生成。

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
    /// 目标月没有这一天。#9 第 5 项。
    MonthlyMissingDay,
    /// 这条的到期日已经是截止日。#9 第 5 项。
    OnUntil,
    /// 日期加减超出可表示范围。
    Overflow,
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
        RecurrenceRule::Monthly => add_one_month(due),
    };
    let Some(next) = next else {
        return match recurrence.rule {
            RecurrenceRule::Monthly => NextOccurrence::MonthlyMissingDay,
            _ => NextOccurrence::Overflow,
        };
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

fn add_one_month(date: CivilDate) -> Option<CivilDate> {
    let (year, month) = if date.month() == 12 {
        (date.year().checked_add(1)?, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    CivilDate::try_from_ymd(year, month, date.day())
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
    fn monthly_missing_day_is_unspecified() {
        let monthly = Recurrence {
            rule: RecurrenceRule::Monthly,
            until: None,
        };
        assert_eq!(
            next_occurrence(Some(date(2026, 1, 31)), &monthly),
            NextOccurrence::MonthlyMissingDay
        );
        assert_eq!(
            next_occurrence(Some(date(2025, 1, 29)), &monthly),
            NextOccurrence::MonthlyMissingDay
        );
        assert_eq!(
            next_occurrence(Some(date(2026, 3, 31)), &monthly),
            NextOccurrence::MonthlyMissingDay
        );
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
