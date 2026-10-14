//! 到期提醒的调度决定。不读时钟，也不写盘。
//!
//! 到点（提醒时刻就是当前这一分钟）的每条各算一条，点击可以定位到该条。
//! 早于当前分钟的多条是错过的，合并成一条补发。安静时段内两条都不发出，
//! 时段结束后再按错过处理。
//!
//! #120 写入安静时段之前，[`configured_quiet_span`] 恒为没有时段。
//! 设置页不在这里。

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::CivilDate;

use super::model::{ClockTime, TodoItem};
use super::schedule::ReminderInstant;

/// 定时器单次等待的上限。更远的提醒到点后重新计算，不是按秒轮询。
pub const MAX_TIMER_MS: u64 = 24 * 60 * 60 * 1000;

/// 本地日上的一段分钟。结束分钟不含在内。
///
/// 开始等于结束时没有时段。开始大于结束时跨过午夜：从开始到当天结束，
/// 以及次日 0 点到结束。起止怎么写进 `config.json` 仍由 #120 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietSpan {
    pub start_minute: u16,
    pub end_minute: u16,
}

impl QuietSpan {
    #[must_use]
    pub fn try_new(start_minute: u16, end_minute: u16) -> Option<Self> {
        if start_minute >= 24 * 60 || end_minute >= 24 * 60 {
            return None;
        }
        Some(Self {
            start_minute,
            end_minute,
        })
    }
}

/// 一条到点或错过的提醒。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueReminder {
    pub id: String,
    pub title: String,
    pub at: ReminderInstant,
}

/// 已经发出过的提醒。到期日或提醒时刻变了就是另一条。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SentKey {
    pub id: String,
    pub due: CivilDate,
    pub minute: u16,
}

impl SentKey {
    #[must_use]
    pub fn from_due(item: &DueReminder) -> Self {
        Self {
            id: item.id.clone(),
            due: item.at.date,
            minute: item.at.time.minute_of_day(),
        }
    }
}

/// 这一轮要发的通知，以及下一次醒来的时刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderPlan {
    /// 提醒时刻就是当前分钟。每条单独一条通知。
    pub due_now: Vec<DueReminder>,
    /// 提醒时刻早于当前分钟。多条合并成一条补发；只有一条时仍可定位。
    pub missed: Vec<DueReminder>,
    /// 下一次要重新计算的时刻。没有更晚的提醒、也不在安静时段时为 `None`。
    pub next: Option<ReminderInstant>,
}

/// #120 把设置里的起止接进来之前，调度看到的是没有安静时段。
///
/// 不读 `config.json`。非空的 `quietHours` 仍由配置层拒绝。
#[must_use]
pub fn configured_quiet_span() -> Option<QuietSpan> {
    None
}

#[must_use]
pub fn in_quiet(minute: u16, span: QuietSpan) -> bool {
    if minute >= 24 * 60 || span.start_minute == span.end_minute {
        return false;
    }
    if span.start_minute < span.end_minute {
        minute >= span.start_minute && minute < span.end_minute
    } else {
        minute >= span.start_minute || minute < span.end_minute
    }
}

/// 当前正在安静时段时，时段结束的那一分钟。结束分钟本身不再安静。
#[must_use]
pub fn quiet_end(now: ReminderInstant, span: QuietSpan) -> Option<ReminderInstant> {
    let minute = now.time.minute_of_day();
    if !in_quiet(minute, span) {
        return None;
    }
    let time = ClockTime::from_minute_of_day(span.end_minute)?;
    if span.start_minute < span.end_minute || minute < span.end_minute {
        return Some(ReminderInstant {
            date: now.date,
            time,
        });
    }
    Some(ReminderInstant {
        date: now.date.checked_add_days(1)?,
        time,
    })
}

#[must_use]
pub fn plan(
    items: &[TodoItem],
    now: ReminderInstant,
    sent: &BTreeSet<SentKey>,
    quiet: Option<QuietSpan>,
) -> ReminderPlan {
    let mut due_now = Vec::new();
    let mut missed = Vec::new();
    let mut next: Option<ReminderInstant> = None;
    for item in items {
        let Some(at) = reminder_of(item) else {
            continue;
        };
        let key = SentKey {
            id: item.id.clone(),
            due: at.date,
            minute: at.time.minute_of_day(),
        };
        match cmp_instant(at, now) {
            Ordering::Greater => {
                if next.is_none_or(|kept| cmp_instant(at, kept) == Ordering::Less) {
                    next = Some(at);
                }
            }
            Ordering::Equal if !sent.contains(&key) => due_now.push(due_from(item, at)),
            Ordering::Less if !sent.contains(&key) => missed.push(due_from(item, at)),
            Ordering::Equal | Ordering::Less => {}
        }
    }
    sort_due(&mut due_now);
    sort_due(&mut missed);
    if quiet.is_some_and(|span| in_quiet(now.time.minute_of_day(), span)) {
        let until = quiet.and_then(|span| quiet_end(now, span));
        return ReminderPlan {
            due_now: Vec::new(),
            missed: Vec::new(),
            next: until.or(next),
        };
    }
    ReminderPlan {
        due_now,
        missed,
        next,
    }
}

/// 从当前这一秒等到 `target`。已经到了或过了则为 0。超过 [`MAX_TIMER_MS`] 时截到上限。
#[must_use]
pub fn wait_millis(now: ReminderInstant, target: ReminderInstant, second: u32, millis: u32) -> u64 {
    let minutes = minutes_until(now, target);
    if minutes == 0 {
        return 0;
    }
    let into = u64::from(second.min(59)) * 1_000 + u64::from(millis.min(999));
    let raw = minutes.saturating_mul(60_000).saturating_sub(into);
    if raw == 0 { 1 } else { raw.min(MAX_TIMER_MS) }
}

/// 系统通知的 XML。一条时 `launch` 是待办 id。多条不写 `launch`：规格没有写合并后点哪一条。
///
/// 正文是各条标题。没有操作按钮。
#[must_use]
pub fn toast_xml(items: &[DueReminder]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut xml = String::from("<toast");
    if items.len() == 1 {
        xml.push_str(" launch=\"");
        xml.push_str(&xml_escape(&items[0].id));
        xml.push('"');
    }
    xml.push_str("><visual><binding template=\"ToastGeneric\">");
    for item in items {
        xml.push_str("<text>");
        xml.push_str(&xml_escape(&item.title));
        xml.push_str("</text>");
    }
    xml.push_str("</binding></visual></toast>");
    Some(xml)
}

/// 点击带回的 `launch`。空白表示这条通知没有要定位的待办。
#[must_use]
pub fn activation_id(launch: &str) -> Option<String> {
    let id = launch.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_owned())
    }
}

fn reminder_of(item: &TodoItem) -> Option<ReminderInstant> {
    if item.completed || item.in_trash() {
        return None;
    }
    Some(ReminderInstant {
        date: item.due?,
        time: item.remind_at?,
    })
}

fn due_from(item: &TodoItem, at: ReminderInstant) -> DueReminder {
    DueReminder {
        id: item.id.clone(),
        title: item.title.clone(),
        at,
    }
}

fn sort_due(items: &mut [DueReminder]) {
    items
        .sort_by(|left, right| cmp_instant(left.at, right.at).then_with(|| left.id.cmp(&right.id)));
}

fn cmp_instant(left: ReminderInstant, right: ReminderInstant) -> Ordering {
    left.date
        .cmp(&right.date)
        .then(left.time.minute_of_day().cmp(&right.time.minute_of_day()))
}

fn minutes_until(now: ReminderInstant, target: ReminderInstant) -> u64 {
    match target.date.cmp(&now.date) {
        Ordering::Less => 0,
        Ordering::Equal => {
            let delta =
                i32::from(target.time.minute_of_day()) - i32::from(now.time.minute_of_day());
            if delta > 0 {
                u64::from(delta as u32)
            } else {
                0
            }
        }
        Ordering::Greater => {
            let Some(days) = now.date.days_until(target.date) else {
                return u64::MAX / 60_000;
            };
            let Ok(days) = u64::try_from(days) else {
                return 0;
            };
            days.saturating_mul(24 * 60)
                .saturating_sub(u64::from(now.time.minute_of_day()))
                .saturating_add(u64::from(target.time.minute_of_day()))
        }
    }
}

fn xml_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other if other.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::model::TodoItem;

    fn date(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::try_from_ymd(year, month, day).unwrap()
    }

    fn at(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> ReminderInstant {
        ReminderInstant {
            date: date(year, month, day),
            time: ClockTime::try_new(hour, minute).unwrap(),
        }
    }

    fn remind(id: &str, title: &str, when: ReminderInstant) -> TodoItem {
        let mut item = TodoItem::new(id.into(), title.into(), 0);
        item.due = Some(when.date);
        item.remind_at = Some(when.time);
        item
    }

    fn span(start_h: u8, start_m: u8, end_h: u8, end_m: u8) -> QuietSpan {
        QuietSpan::try_new(
            ClockTime::try_new(start_h, start_m)
                .unwrap()
                .minute_of_day(),
            ClockTime::try_new(end_h, end_m).unwrap().minute_of_day(),
        )
        .unwrap()
    }

    #[test]
    fn quiet_hours_cross_midnight_and_stop_at_the_end_minute() {
        let overnight = span(22, 0, 7, 0);
        assert!(in_quiet(23 * 60, overnight));
        assert!(in_quiet(0, overnight));
        assert!(in_quiet(6 * 60 + 59, overnight));
        assert!(!in_quiet(7 * 60, overnight));
        assert!(!in_quiet(21 * 60 + 59, overnight));
        let evening = at(2026, 10, 14, 23, 30);
        assert_eq!(quiet_end(evening, overnight), Some(at(2026, 10, 15, 7, 0)));
        let morning = at(2026, 10, 15, 6, 0);
        assert_eq!(quiet_end(morning, overnight), Some(at(2026, 10, 15, 7, 0)));

        let daytime = span(1, 0, 6, 0);
        assert!(in_quiet(3 * 60, daytime));
        assert!(!in_quiet(22 * 60, daytime));
        assert_eq!(
            quiet_end(at(2026, 10, 14, 3, 0), daytime),
            Some(at(2026, 10, 14, 6, 0))
        );
        assert!(!in_quiet(9 * 60, span(9, 0, 9, 0)));
        assert!(configured_quiet_span().is_none());
    }

    #[test]
    fn on_time_items_stay_separate_and_missed_ones_collect() {
        let now = at(2026, 10, 14, 9, 0);
        let items = vec![
            remind("a", "现在甲", now),
            remind("b", "现在乙", now),
            remind("c", "早一点", at(2026, 10, 14, 8, 0)),
            remind("d", "更早", at(2026, 10, 13, 23, 0)),
            remind("e", "以后", at(2026, 10, 14, 10, 30)),
        ];
        let planned = plan(&items, now, &BTreeSet::new(), None);
        assert_eq!(
            planned
                .due_now
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(
            planned
                .missed
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["d", "c"]
        );
        assert_eq!(planned.next, Some(at(2026, 10, 14, 10, 30)));
    }

    #[test]
    fn quiet_hours_hold_due_items_until_the_span_ends() {
        let now = at(2026, 10, 14, 23, 0);
        let items = vec![
            remind("a", "夜里", now),
            remind("b", "刚才", at(2026, 10, 14, 22, 30)),
        ];
        let planned = plan(&items, now, &BTreeSet::new(), Some(span(22, 0, 7, 0)));
        assert!(planned.due_now.is_empty());
        assert!(planned.missed.is_empty());
        assert_eq!(planned.next, Some(at(2026, 10, 15, 7, 0)));

        let later = plan(
            &items,
            at(2026, 10, 15, 7, 0),
            &BTreeSet::new(),
            Some(span(22, 0, 7, 0)),
        );
        assert!(later.due_now.is_empty());
        assert_eq!(later.missed.len(), 2);
    }

    #[test]
    fn sent_completed_and_trashed_items_do_not_fire_again() {
        let now = at(2026, 10, 14, 9, 0);
        let mut done = remind("done", "完成", at(2026, 10, 14, 8, 0));
        done.completed = true;
        let mut trashed = remind("trash", "回收站", at(2026, 10, 14, 8, 0));
        trashed.deleted_at = Some(1);
        let mut plain = remind("plain", "没有提醒", now);
        plain.remind_at = None;
        let due = remind("due", "已发过", at(2026, 10, 14, 8, 0));
        let key = SentKey {
            id: due.id.clone(),
            due: due.due.unwrap(),
            minute: due.remind_at.unwrap().minute_of_day(),
        };
        let mut sent = BTreeSet::new();
        sent.insert(key);
        let planned = plan(&[done, trashed, plain, due], now, &sent, None);
        assert!(planned.due_now.is_empty());
        assert!(planned.missed.is_empty());
    }

    #[test]
    fn a_changed_reminder_is_not_the_sent_one() {
        let now = at(2026, 10, 14, 11, 0);
        let item = remind("a", "改过", at(2026, 10, 14, 11, 0));
        let mut sent = BTreeSet::new();
        sent.insert(SentKey {
            id: "a".into(),
            due: date(2026, 10, 14),
            minute: 9 * 60,
        });
        let planned = plan(std::slice::from_ref(&item), now, &sent, None);
        assert_eq!(planned.due_now.len(), 1);
    }

    #[test]
    fn wait_is_zero_when_due_and_caps_a_far_reminder() {
        let now = at(2026, 10, 14, 9, 0);
        assert_eq!(wait_millis(now, now, 30, 0), 0);
        assert_eq!(wait_millis(now, at(2026, 10, 14, 9, 1), 30, 0), 30_000);
        assert_eq!(wait_millis(now, at(2028, 1, 1, 0, 0), 0, 0), MAX_TIMER_MS);
    }

    #[test]
    fn toast_lists_titles_and_only_a_single_item_launches() {
        let one = DueReminder {
            id: "i1".into(),
            title: "买菜 & 水果".into(),
            at: at(2026, 10, 14, 9, 0),
        };
        let xml = toast_xml(std::slice::from_ref(&one)).unwrap();
        assert!(xml.contains("launch=\"i1\""));
        assert!(xml.contains("买菜 &amp; 水果"));
        assert!(!xml.contains("<action"));
        let two = [
            one.clone(),
            DueReminder {
                id: "i2".into(),
                title: "写信".into(),
                at: at(2026, 10, 14, 8, 0),
            },
        ];
        let merged = toast_xml(&two).unwrap();
        assert!(!merged.contains("launch="));
        assert!(merged.contains("<text>买菜 &amp; 水果</text>"));
        assert!(merged.contains("<text>写信</text>"));
        assert!(!merged.contains("<action"));
        assert_eq!(activation_id(" i1 "), Some("i1".into()));
        assert_eq!(activation_id("  "), None);
        assert!(toast_xml(&[]).is_none());
    }
}
