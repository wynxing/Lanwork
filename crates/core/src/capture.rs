//! 待办收集剩余文本里的日期前缀。
//!
//! 输入是收集前缀 `+` / `＋` 之后的文字，以及调用方给出的「今天」。
//! 本模块不读系统时钟，也不创建待办。
//!
//! 能写入到期日的前缀只有产品规格列出的那些。词表不在这里扩大。
//! 星期、下周、月底里规格还没写明的边界返回 [`TodoDue::Unresolved`]，
//! 不猜测是今天还是七天后、下一周从周一起还是从周日起、月底当天算不算下个月。

/// 公历日期。预览和存储怎么显示由调用方决定，这里不规定界面文字。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    year: i32,
    month: u8,
    day: u8,
}

impl CivilDate {
    /// 年月日不合法时返回 `None`，包括不存在的 2 月 29 日。
    #[must_use]
    pub fn try_from_ymd(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=12).contains(&month) {
            return None;
        }
        if day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    #[must_use]
    pub fn year(self) -> i32 {
        self.year
    }

    #[must_use]
    pub fn month(self) -> u8 {
        self.month
    }

    #[must_use]
    pub fn day(self) -> u8 {
        self.day
    }

    /// 按公历加减天数。跨月、跨年和闰日都算在内。
    #[must_use]
    pub fn checked_add_days(self, days: i32) -> Option<Self> {
        let sum = days_from_civil(self)?.checked_add(i64::from(days))?;
        civil_from_days(sum)
    }

    /// 星期一为 0，星期日为 6。
    fn weekday_monday0(self) -> u8 {
        let days = days_from_civil(self).expect("a constructed date has a day count");
        u8::try_from((days + 3).rem_euclid(7)).expect("weekday is 0..=6")
    }

    fn is_last_day_of_month(self) -> bool {
        self.day == days_in_month(self.year, self.month)
    }

    fn last_day_of_this_month(self) -> Self {
        Self {
            year: self.year,
            month: self.month,
            day: days_in_month(self.year, self.month),
        }
    }
}

/// 日期前缀解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoDuePrefix {
    /// 识别出前缀时，是前缀之后的标题：紧挨着的一段空白会去掉，其余文字原样保留。
    /// 没有识别出前缀时，是输入原文，不裁剪。
    pub title: String,
    pub due: TodoDue,
}

/// 到期日。`Unresolved` 表示前缀已经认出，但到期日落在尚未写进产品规格的边界上。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoDue {
    /// 没有可识别的日期前缀，不写到期日。
    Absent,
    /// 规格已经能确定的到期日。
    Date(CivilDate),
    /// 前缀已识别。到期日等 #9 第 4 项写进产品规格后再计算，这里不选边。
    Unresolved,
}

/// 解析待办收集的剩余文本。
///
/// `text` 是 [`crate::search::classify_prefix`] 判定为待办收集之后的剩余文本
/// （[`crate::search::Capture::remainder`]）。本函数不识别 `+` / `＋`，也不改变前缀分类。
/// `today` 由调用方传入。日期前缀必须是忽略前导空白后的第一个词，并且后面是空白或结束。
/// 认不出时整句作为标题。`下周` 后面不是星期字时，整句作为标题。
///
/// # Examples
///
/// ```
/// use lanwork_core::{parse_todo_due_prefix, CivilDate, TodoDue};
///
/// let today = CivilDate::try_from_ymd(2026, 10, 8).unwrap();
/// let parsed = parse_todo_due_prefix("明天 提交周报", today);
/// assert_eq!(parsed.title, "提交周报");
/// assert_eq!(
///     parsed.due,
///     TodoDue::Date(CivilDate::try_from_ymd(2026, 10, 9).unwrap())
/// );
/// ```
#[must_use]
pub fn parse_todo_due_prefix(text: &str, today: CivilDate) -> TodoDuePrefix {
    let Some(start) = text.find(|c: char| !c.is_whitespace()) else {
        return TodoDuePrefix {
            title: text.to_owned(),
            due: TodoDue::Absent,
        };
    };
    let body = &text[start..];
    if let Some((rest, due)) = match_prefix(body, today) {
        return TodoDuePrefix {
            title: rest.trim_start().to_owned(),
            due,
        };
    }
    TodoDuePrefix {
        title: text.to_owned(),
        due: TodoDue::Absent,
    }
}

fn match_prefix(body: &str, today: CivilDate) -> Option<(&str, TodoDue)> {
    if let Some(found) = match_relative_day(body, today) {
        return Some(found);
    }
    if let Some(found) = match_weekday(body, today) {
        return Some(found);
    }
    if let Some(found) = match_days_later(body, today) {
        return Some(found);
    }
    match_month_end(body, today)
}

fn match_relative_day(body: &str, today: CivilDate) -> Option<(&str, TodoDue)> {
    const RELATIVE: &[(&str, i32)] = &[
        ("今天", 0),
        ("今日", 0),
        ("明天", 1),
        ("明日", 1),
        ("后天", 2),
    ];
    for &(token, delta) in RELATIVE {
        let Some(rest) = body.strip_prefix(token) else {
            continue;
        };
        if is_token_boundary(rest) {
            let due = today
                .checked_add_days(delta)
                .expect("adding at most 2 days stays in range");
            return Some((rest, TodoDue::Date(due)));
        }
    }
    None
}

fn match_weekday(body: &str, today: CivilDate) -> Option<(&str, TodoDue)> {
    let (next_week, rest) = if let Some(rest) = body.strip_prefix("下周") {
        (true, rest)
    } else if let Some(rest) = body.strip_prefix("星期") {
        (false, rest)
    } else {
        let rest = body.strip_prefix("周")?;
        (false, rest)
    };
    let ch = rest.chars().next()?;
    let target = weekday_index(ch)?;
    let after = &rest[ch.len_utf8()..];
    if !is_token_boundary(after) {
        return None;
    }
    let due = if next_week {
        next_week_due(target, today)
    } else {
        named_weekday_due(target, today)
    };
    Some((after, due))
}

fn weekday_index(ch: char) -> Option<u8> {
    Some(match ch {
        '一' => 0,
        '二' => 1,
        '三' => 2,
        '四' => 3,
        '五' => 4,
        '六' => 5,
        '日' | '天' => 6,
        _ => return None,
    })
}

/// `周X` / `星期X`。
///
/// 该日还在本周剩余的日子里时，「本周这一天」和「下一个这一天」是同一天，直接使用。
/// 今天就是这一天，或者这一天在本周已经过去：两种读法不是同一天。
/// #35 要求这些用例在 #9 第 4 项写进产品规格之前不预填期望。
fn named_weekday_due(target: u8, today: CivilDate) -> TodoDue {
    let today_wd = today.weekday_monday0();
    if target <= today_wd {
        return TodoDue::Unresolved;
    }
    let delta = i32::from(target - today_wd);
    let due = today
        .checked_add_days(delta)
        .expect("a weekday at most 6 days ahead stays in range");
    TodoDue::Date(due)
}

/// `下周X`。以周一为始的下一周和以周日为始的下一周如果落到同一天，就用这一天。
/// 两套周始不一致时，#9 第 4 项还没写明从哪一天算，返回 [`TodoDue::Unresolved`]。
fn next_week_due(target: u8, today: CivilDate) -> TodoDue {
    let from_monday = next_week_from(today, target, WeekStart::Monday);
    let from_sunday = next_week_from(today, target, WeekStart::Sunday);
    if from_monday == from_sunday {
        TodoDue::Date(from_monday)
    } else {
        TodoDue::Unresolved
    }
}

#[derive(Clone, Copy)]
enum WeekStart {
    Monday,
    Sunday,
}

fn next_week_from(today: CivilDate, target: u8, start: WeekStart) -> CivilDate {
    let (to_next_start, offset) = match start {
        WeekStart::Monday => {
            let wd = today.weekday_monday0();
            let to_next_monday = if wd == 0 { 7 } else { i32::from(7 - wd) };
            (to_next_monday, i32::from(target))
        }
        WeekStart::Sunday => {
            // 星期一为 0 时，距本周日的天数：星期日是 0，星期一是 1，星期六是 6。
            let since_sunday = i32::from((today.weekday_monday0() + 1) % 7);
            let to_next_sunday = if since_sunday == 0 {
                7
            } else {
                7 - since_sunday
            };
            let offset = if target == 6 {
                0
            } else {
                i32::from(target) + 1
            };
            (to_next_sunday, offset)
        }
    };
    today
        .checked_add_days(to_next_start + offset)
        .expect("next week is only a few days ahead")
}

fn match_days_later(body: &str, today: CivilDate) -> Option<(&str, TodoDue)> {
    let digit_bytes = body.bytes().take_while(u8::is_ascii_digit).count();
    if digit_bytes == 0 {
        return None;
    }
    let digits = &body[..digit_bytes];
    let after_digits = &body[digit_bytes..];
    let rest = after_digits.strip_prefix("天后")?;
    if !is_token_boundary(rest) {
        return None;
    }
    let days = parse_ascii_u32(digits)?;
    if !(1..=365).contains(&days) {
        return None;
    }
    let due = today
        .checked_add_days(i32::try_from(days).expect("365 fits in i32"))
        .expect("adding at most 365 days stays in range");
    Some((rest, TodoDue::Date(due)))
}

fn parse_ascii_u32(digits: &str) -> Option<u32> {
    if digits.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for byte in digits.bytes() {
        let digit = u32::from(byte - b'0');
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

fn match_month_end(body: &str, today: CivilDate) -> Option<(&str, TodoDue)> {
    for token in ["月底", "月末"] {
        let Some(rest) = body.strip_prefix(token) else {
            continue;
        };
        if is_token_boundary(rest) {
            let due = if today.is_last_day_of_month() {
                // 月底当天指今天还是下个月底，#9 第 4 项还没写明。
                TodoDue::Unresolved
            } else {
                TodoDue::Date(today.last_day_of_this_month())
            };
            return Some((rest, due));
        }
    }
    None
}

fn is_token_boundary(rest: &str) -> bool {
    rest.is_empty() || rest.starts_with(char::is_whitespace)
}

fn is_leap_year(year: i32) -> bool {
    let year = i64::from(year);
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// 天数算法取 Howard Hinnant 的公历换算，1970-01-01 为第 0 天。
fn days_from_civil(date: CivilDate) -> Option<i64> {
    let mut year = i64::from(date.year);
    let month = i64::from(date.month);
    let day = i64::from(date.day);
    if month <= 2 {
        year -= 1;
    }
    let era = year.div_euclid(400);
    let yoe = u64::try_from(year - era * 400).ok()?;
    let month_shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shifted + 2) / 5 + day - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + u64::try_from(day_of_year).ok()?;
    Some(era * 146097 + i64::try_from(day_of_era).ok()? - 719468)
}

fn civil_from_days(days: i64) -> Option<CivilDate> {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let day_of_era = u64::try_from(z - era * 146097).ok()?;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = i64::try_from(year_of_era).ok()? + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    if month <= 2 {
        year += 1;
    }
    Some(CivilDate {
        year: i32::try_from(year).ok()?,
        month: u8::try_from(month).ok()?,
        day: u8::try_from(day).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::{CivilDate, TodoDue, TodoDuePrefix, parse_todo_due_prefix};

    fn ymd(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::try_from_ymd(year, month, day)
            .unwrap_or_else(|| panic!("invalid date {year}-{month:02}-{day:02}"))
    }

    fn assert_date(input: &str, today: CivilDate, title: &str, due: CivilDate) {
        let parsed = parse_todo_due_prefix(input, today);
        assert_eq!(
            parsed,
            TodoDuePrefix {
                title: title.to_owned(),
                due: TodoDue::Date(due),
            },
            "input {input:?}"
        );
    }

    fn assert_absent(input: &str, today: CivilDate) {
        let parsed = parse_todo_due_prefix(input, today);
        assert_eq!(
            parsed,
            TodoDuePrefix {
                title: input.to_owned(),
                due: TodoDue::Absent,
            },
            "input {input:?}"
        );
    }

    fn assert_unresolved(input: &str, today: CivilDate, title: &str) {
        let parsed = parse_todo_due_prefix(input, today);
        assert_eq!(
            parsed,
            TodoDuePrefix {
                title: title.to_owned(),
                due: TodoDue::Unresolved,
            },
            "input {input:?}"
        );
    }

    #[test]
    fn unix_epoch_and_known_weekdays() {
        let epoch = ymd(1970, 1, 1);
        assert_eq!(super::days_from_civil(epoch), Some(0));
        assert_eq!(epoch.weekday_monday0(), 3, "1970-01-01 is Thursday");
        assert_eq!(ymd(2026, 10, 8).weekday_monday0(), 3, "Thursday");
        assert_eq!(ymd(2026, 8, 12).weekday_monday0(), 2, "Wednesday");
        assert_eq!(ymd(2026, 10, 5).weekday_monday0(), 0, "Monday");
        assert_eq!(ymd(2026, 10, 11).weekday_monday0(), 6, "Sunday");
        assert_eq!(ymd(2000, 1, 1).weekday_monday0(), 5, "Saturday");
        assert_eq!(ymd(1900, 1, 1).weekday_monday0(), 0, "Monday");
    }

    #[test]
    fn civil_date_rejects_impossible_days() {
        assert!(CivilDate::try_from_ymd(2023, 2, 29).is_none());
        assert!(CivilDate::try_from_ymd(1900, 2, 29).is_none());
        assert!(CivilDate::try_from_ymd(2024, 2, 29).is_some());
        assert!(CivilDate::try_from_ymd(2000, 2, 29).is_some());
        assert!(CivilDate::try_from_ymd(2024, 2, 30).is_none());
        assert!(CivilDate::try_from_ymd(2026, 4, 31).is_none());
        assert!(CivilDate::try_from_ymd(2026, 0, 10).is_none());
        assert!(CivilDate::try_from_ymd(2026, 13, 1).is_none());
        assert!(CivilDate::try_from_ymd(2026, 1, 0).is_none());
    }

    #[test]
    fn adding_days_roundtrips_across_months_and_leap_days() {
        let samples = [
            ((2026, 1, 31), 1, (2026, 2, 1)),
            ((2026, 12, 31), 1, (2027, 1, 1)),
            ((2024, 2, 28), 1, (2024, 2, 29)),
            ((2024, 2, 28), 2, (2024, 3, 1)),
            ((2023, 2, 28), 1, (2023, 3, 1)),
            ((2024, 1, 1), 365, (2024, 12, 31)),
            ((2023, 1, 1), 365, (2024, 1, 1)),
            ((2024, 2, 29), 365, (2025, 2, 28)),
            ((2026, 10, 8), 365, (2027, 10, 8)),
            ((1970, 1, 1), -1, (1969, 12, 31)),
        ];
        for ((y, m, d), delta, (ey, em, ed)) in samples {
            let got = ymd(y, m, d).checked_add_days(delta).unwrap();
            assert_eq!(got, ymd(ey, em, ed), "{y}-{m:02}-{d:02} + {delta}");
        }

        let mut date = ymd(2020, 1, 1);
        assert_eq!(date.weekday_monday0(), 2, "2020-01-01 is Wednesday");
        for _ in 0..800 {
            let next = date.checked_add_days(1).unwrap();
            assert_eq!(
                CivilDate::try_from_ymd(next.year(), next.month(), next.day()),
                Some(next)
            );
            assert_eq!(next.weekday_monday0(), (date.weekday_monday0() + 1) % 7);
            date = next;
        }
    }

    #[test]
    fn relative_days_use_the_date_argument() {
        let today = ymd(2026, 10, 8);
        assert_date("今天 整理", today, "整理", ymd(2026, 10, 8));
        assert_date("今日 整理", today, "整理", ymd(2026, 10, 8));
        assert_date("明天 提交周报", today, "提交周报", ymd(2026, 10, 9));
        assert_date("明日 开会", today, "开会", ymd(2026, 10, 9));
        assert_date("后天 取快递", today, "取快递", ymd(2026, 10, 10));

        let other = ymd(2026, 12, 31);
        assert_date("明天 跨年", other, "跨年", ymd(2027, 1, 1));
        assert_date("后天 跨年", other, "跨年", ymd(2027, 1, 2));
        assert_date("今天 仍是今天", other, "仍是今天", other);
    }

    #[test]
    fn bare_relative_prefix_has_an_empty_title() {
        let today = ymd(2026, 10, 8);
        assert_date("明天", today, "", ymd(2026, 10, 9));
        assert_date("明天   ", today, "", ymd(2026, 10, 9));
        assert_date("  明天\t", today, "", ymd(2026, 10, 9));
        assert_date("后天", today, "", ymd(2026, 10, 10));
    }

    #[test]
    fn separator_whitespace_is_dropped_and_the_rest_of_the_title_stays() {
        let today = ymd(2026, 10, 8);
        assert_date("明天  提交 周报 ", today, "提交 周报 ", ymd(2026, 10, 9));
        assert_date("明天\t提交", today, "提交", ymd(2026, 10, 9));
        assert_date("明天\n提交", today, "提交", ymd(2026, 10, 9));
        assert_date("明天\u{3000}提交", today, "提交", ymd(2026, 10, 9));
        assert_date("  \t明天 提交周报", today, "提交周报", ymd(2026, 10, 9));
        assert_date("明天 后天 开会", today, "后天 开会", ymd(2026, 10, 9));
    }

    #[test]
    fn glued_prefixes_and_non_first_words_stay_in_the_title() {
        let today = ymd(2026, 10, 8);
        for input in [
            "明天性计划",
            "后天性",
            "今天的计划",
            "明日程",
            "周五策划案",
            "3天后端联调",
            "月底总结会",
            "下周一开会",
            "请明天 开会",
            "提交 明天 周报",
            "明天，开会",
            "+明天 开会",
            "＋明天 开会",
            "三天后 开会",
            "礼拜五 开会",
            "下星期一 开会",
            "下周 一",
            "星期 五",
            "周 五",
        ] {
            assert_absent(input, today);
        }
    }

    #[test]
    fn next_week_without_a_weekday_character_is_the_whole_title() {
        let today = ymd(2026, 10, 8);
        for input in ["下周 开会", "下周", "下周计划", "下周周会", "  下周 开会"]
        {
            assert_absent(input, today);
        }
    }

    #[test]
    fn days_later_accepts_1_through_365_and_crosses_months_and_years() {
        let today = ymd(2026, 10, 8);
        assert_date("3天后 交房租", today, "交房租", ymd(2026, 10, 11));
        assert_date("365天后 x", today, "x", ymd(2027, 10, 8));
        assert_date("1天后", today, "", ymd(2026, 10, 9));
        assert_date("365天后\u{3000}x", today, "x", ymd(2027, 10, 8));

        for n in [1, 2, 9, 10, 28, 31, 99, 100, 364, 365] {
            let input = format!("{n}天后 事");
            assert_date(&input, today, "事", today.checked_add_days(n).unwrap());
        }

        let jan = ymd(2026, 1, 31);
        assert_date("1天后 跨月", jan, "跨月", ymd(2026, 2, 1));
        assert_date("3天后 跨月", jan, "跨月", ymd(2026, 2, 3));
        let nye = ymd(2026, 12, 31);
        assert_date("1天后 跨年", nye, "跨年", ymd(2027, 1, 1));
        assert_date("365天后 跨年", nye, "跨年", ymd(2027, 12, 31));
        assert_date("1天后 闰日", ymd(2024, 2, 28), "闰日", ymd(2024, 2, 29));
        assert_date("2天后 过闰日", ymd(2024, 2, 28), "过闰日", ymd(2024, 3, 1));
        assert_date("1天后 平年", ymd(2023, 2, 28), "平年", ymd(2023, 3, 1));
        assert_date("365天后 闰年", ymd(2024, 1, 1), "闰年", ymd(2024, 12, 31));
        assert_date("365天后 平年", ymd(2023, 1, 1), "平年", ymd(2024, 1, 1));
        assert_date(
            "365天后 从闰日",
            ymd(2024, 2, 29),
            "从闰日",
            ymd(2025, 2, 28),
        );
    }

    #[test]
    fn days_later_rejects_out_of_range_and_non_ascii_spellings() {
        let today = ymd(2026, 10, 8);
        for input in [
            "0天后 x",
            "00天后 x",
            "000天后 x",
            "366天后 x",
            "999天后 x",
            "1000天后 x",
            "-1天后 x",
            "－1天后 x",
            "3 天后 x",
            "天后 x",
            "3天後 x",
            "３天后 x",
            "３６５天后 x",
            "١天后 x",
        ] {
            assert_absent(input, today);
        }
        let huge = format!("{}天后 x", "9".repeat(80));
        assert_absent(&huge, today);
        // 很长的前导零不增加数值，也不该 panic。数值仍是 1，所以识别。
        let padded = format!("{}1天后 x", "0".repeat(40));
        assert_date(&padded, today, "x", ymd(2026, 10, 9));

        // 数值在 1 至 365 里时，前导零仍是这个前缀。规格约束的是 N，不是写法里能不能补零。
        assert_date("01天后 x", today, "x", ymd(2026, 10, 9));
        assert_date("007天后 x", today, "x", ymd(2026, 10, 15));
        assert_date("0365天后 x", today, "x", ymd(2027, 10, 8));

        assert_absent("  366天后 x", today);
        assert_date("  365天后 x", today, "x", ymd(2027, 10, 8));
    }

    #[test]
    fn named_weekdays_that_are_still_ahead_this_week() {
        // 2026-10-08 是周四。周五、周六、周日还没到，本周和下一个是同一天。
        let thursday = ymd(2026, 10, 8);
        assert_date("周五 清理", thursday, "清理", ymd(2026, 10, 9));
        assert_date("星期六 清理", thursday, "清理", ymd(2026, 10, 10));
        assert_date("周日 休息", thursday, "休息", ymd(2026, 10, 11));
        assert_date("周天 休息", thursday, "休息", ymd(2026, 10, 11));
        assert_date("星期天 休息", thursday, "休息", ymd(2026, 10, 11));
        assert_date("星期日", thursday, "", ymd(2026, 10, 11));

        // 2026-10-05 是周一。除了当天，本周其余日子都还在前面。
        let monday = ymd(2026, 10, 5);
        let ahead = [
            ("二", 6u8),
            ("三", 7),
            ("四", 8),
            ("五", 9),
            ("六", 10),
            ("日", 11),
            ("天", 11),
        ];
        for (name, day) in ahead {
            let due = ymd(2026, 10, day);
            assert_date(&format!("周{name} 事"), monday, "事", due);
            assert_date(&format!("星期{name} 事"), monday, "事", due);
        }
    }

    #[test]
    fn same_weekday_and_already_passed_weekdays_do_not_guess() {
        let thursday = ymd(2026, 10, 8);
        assert_unresolved("周四 例会", thursday, "例会");
        assert_unresolved("星期四 例会", thursday, "例会");
        assert_unresolved("周四", thursday, "");
        for name in ["一", "二", "三"] {
            assert_unresolved(&format!("周{name} 计划"), thursday, "计划");
            assert_unresolved(&format!("星期{name} 计划"), thursday, "计划");
        }

        let sunday = ymd(2026, 10, 11);
        assert_unresolved("周日 休息", sunday, "休息");
        assert_unresolved("周天 休息", sunday, "休息");
        assert_unresolved("星期天 休息", sunday, "休息");
        for name in ["一", "二", "三", "四", "五", "六"] {
            assert_unresolved(&format!("周{name} 事"), sunday, "事");
        }

        let wednesday = ymd(2026, 8, 12);
        assert_unresolved("周三 周会", wednesday, "周会");
        assert_unresolved("星期一 计划", wednesday, "计划");
        assert_date("周五 清理", wednesday, "清理", ymd(2026, 8, 14));
        assert_date("星期天 休息", wednesday, "休息", ymd(2026, 8, 16));
    }

    #[test]
    fn next_weekday_when_both_week_starts_agree() {
        // 2026-10-07 是周三。下周一到下周六在两种周始下是同一天，下周日不是。
        let wednesday = ymd(2026, 10, 7);
        let agreed = [
            ("一", 12u8),
            ("二", 13),
            ("三", 14),
            ("四", 15),
            ("五", 16),
            ("六", 17),
        ];
        for (name, day) in agreed {
            assert_date(
                &format!("下周{name} 开会"),
                wednesday,
                "开会",
                ymd(2026, 10, day),
            );
        }
        assert_unresolved("下周日 休息", wednesday, "休息");
        assert_unresolved("下周天 休息", wednesday, "休息");

        // 2026-10-11 是周日。只有下周日两种周始重合。
        let sunday = ymd(2026, 10, 11);
        assert_date("下周日 休息", sunday, "休息", ymd(2026, 10, 18));
        assert_date("下周天", sunday, "", ymd(2026, 10, 18));
        for name in ["一", "二", "三", "四", "五", "六"] {
            assert_unresolved(&format!("下周{name} 事"), sunday, "事");
        }

        // 2026-10-05 是周一。下周一到下周六重合，下周日不重合。
        let monday = ymd(2026, 10, 5);
        assert_date("下周一 计划", monday, "计划", ymd(2026, 10, 12));
        assert_date("下周六 计划", monday, "计划", ymd(2026, 10, 17));
        assert_unresolved("下周日 休息", monday, "休息");

        // 2026-10-08 是周四。当天的「周四」仍未定，但「下周四」两种周始都是 10 月 15 日。
        let thursday = ymd(2026, 10, 8);
        let agreed_on_thursday = [
            ("一", 12u8),
            ("二", 13),
            ("三", 14),
            ("四", 15),
            ("五", 16),
            ("六", 17),
        ];
        for (name, day) in agreed_on_thursday {
            assert_date(
                &format!("下周{name} 开会"),
                thursday,
                "开会",
                ymd(2026, 10, day),
            );
        }
        assert_unresolved("下周日 休息", thursday, "休息");
        assert_unresolved("下周天 休息", thursday, "休息");
    }

    #[test]
    fn month_end_is_the_last_day_until_that_day_itself() {
        assert_date("月底 发布", ymd(2026, 10, 8), "发布", ymd(2026, 10, 31));
        assert_date("月末 对账", ymd(2026, 10, 8), "对账", ymd(2026, 10, 31));
        assert_date("月底", ymd(2026, 2, 10), "", ymd(2026, 2, 28));
        assert_date("月末 闰月", ymd(2024, 2, 10), "闰月", ymd(2024, 2, 29));
        assert_date(
            "月底 仍在二月",
            ymd(2024, 2, 28),
            "仍在二月",
            ymd(2024, 2, 29),
        );
        assert_date("月末 四月", ymd(2026, 4, 29), "四月", ymd(2026, 4, 30));
        assert_date("月底 一月", ymd(2026, 1, 30), "一月", ymd(2026, 1, 31));
        assert_date(
            "月底 十二月",
            ymd(2026, 12, 30),
            "十二月",
            ymd(2026, 12, 31),
        );
        assert_date(
            "  月末 十一月",
            ymd(2026, 11, 15),
            "十一月",
            ymd(2026, 11, 30),
        );

        for today in [
            ymd(2026, 10, 31),
            ymd(2026, 2, 28),
            ymd(2024, 2, 29),
            ymd(2026, 4, 30),
            ymd(2026, 1, 31),
            ymd(2026, 12, 31),
            ymd(2023, 2, 28),
        ] {
            assert_unresolved("月底 收工", today, "收工");
            assert_unresolved("月末", today, "");
        }
    }

    #[test]
    fn todo_capture_remainder_is_parsed_without_changing_prefix_classification() {
        use crate::search::{PrefixClass, classify_prefix};

        let today = ymd(2026, 10, 8);

        let PrefixClass::TodoCapture(capture) = classify_prefix("+ 明天 提交周报") else {
            panic!("expected todo capture");
        };
        assert!(capture.submittable());
        assert_eq!(capture.remainder(), "明天 提交周报");
        assert_date(capture.remainder(), today, "提交周报", ymd(2026, 10, 9));

        let PrefixClass::TodoCapture(capture) = classify_prefix("＋ 3天后 交房租") else {
            panic!("expected todo capture");
        };
        assert_eq!(capture.remainder(), "3天后 交房租");
        assert_date(capture.remainder(), today, "交房租", ymd(2026, 10, 11));

        let PrefixClass::TodoCapture(capture) = classify_prefix("+ 明天性计划") else {
            panic!("expected todo capture");
        };
        assert_eq!(capture.remainder(), "明天性计划");
        assert_absent(capture.remainder(), today);

        let PrefixClass::TodoCapture(capture) = classify_prefix("+ 下周 开会") else {
            panic!("expected todo capture");
        };
        assert_eq!(capture.remainder(), "下周 开会");
        assert_absent(capture.remainder(), today);

        let PrefixClass::TodoCapture(capture) = classify_prefix("+ 明天  提交 周报 ") else {
            panic!("expected todo capture");
        };
        assert_eq!(capture.remainder(), "明天  提交 周报 ");
        assert_date(capture.remainder(), today, "提交 周报 ", ymd(2026, 10, 9));

        // 只有日期词时，前缀分类仍可提交；标题为空由调用方决定能不能创建。
        let PrefixClass::TodoCapture(capture) = classify_prefix("+明天") else {
            panic!("expected todo capture");
        };
        assert!(capture.submittable());
        assert_eq!(capture.remainder(), "明天");
        assert_date(capture.remainder(), today, "", ymd(2026, 10, 9));

        let PrefixClass::TodoCapture(capture) = classify_prefix("+") else {
            panic!("expected todo capture");
        };
        assert!(!capture.submittable());
        assert_eq!(capture.remainder(), "");
        assert_absent(capture.remainder(), today);

        let PrefixClass::TodoCapture(capture) = classify_prefix("+ 周四 例会") else {
            panic!("expected todo capture");
        };
        assert_unresolved(capture.remainder(), today, "例会");

        assert!(matches!(
            classify_prefix("/note 明天 开会"),
            PrefixClass::NoteCapture(_)
        ));
        assert!(matches!(classify_prefix("微信"), PrefixClass::Search));
        assert!(matches!(classify_prefix(""), PrefixClass::Empty));
    }

    #[test]
    fn empty_and_whitespace_only_inputs_keep_the_original_text() {
        let today = ymd(2026, 10, 8);
        assert_absent("", today);
        assert_absent("   ", today);
        assert_absent("\t\n", today);
        assert_absent("　", today);
    }

    /// 今天是 2026-10-08（周四）。`周四` 可能指今天，也可能指 2026-10-15。
    /// 不预填期望日期。产品规格写明后再补断言，并去掉 ignore。
    #[test]
    #[ignore = "待 #9 第 4 项写入 product.md：今天是该星期时，「周X / 星期X」指今天还是七天后"]
    fn issue_9_same_weekday_due_date() {
        let today = ymd(2026, 10, 8);
        let parsed = parse_todo_due_prefix("周四 例会", today);
        assert_eq!(parsed.title, "例会");
        let _today_or_next = [ymd(2026, 10, 8), ymd(2026, 10, 15)];
        todo!("#9 第 4 项尚未写入 product.md：不预填周四的到期日");
    }

    /// 2026-10-08 是周四。`周一` 若指本周则是 2026-10-05，若指下一个则是 2026-10-12。
    #[test]
    #[ignore = "待 #9 第 4 项写入 product.md：周X 指本周已经过去的那一天，还是下一个该星期"]
    fn issue_9_weekday_already_passed() {
        let today = ymd(2026, 10, 8);
        let parsed = parse_todo_due_prefix("周一 计划", today);
        assert_eq!(parsed.title, "计划");
        let _this_week_or_next = [ymd(2026, 10, 5), ymd(2026, 10, 12)];
        todo!("#9 第 4 项尚未写入 product.md：不预填已过去星期的到期日");
    }

    /// 2026-10-08 是周四。下周日：以周一为始是 2026-10-18，以周日为始是 2026-10-11。
    /// 2026-10-11 是周日。下周一：以周一为始是 2026-10-12，以周日为始是 2026-10-19。
    #[test]
    #[ignore = "待 #9 第 4 项写入 product.md：下周从周一起算还是从周日起算"]
    fn issue_9_next_week_start_day() {
        let thursday = ymd(2026, 10, 8);
        let sunday_token = parse_todo_due_prefix("下周日 休息", thursday);
        assert_eq!(sunday_token.title, "休息");
        let _sunday_candidates = [ymd(2026, 10, 18), ymd(2026, 10, 11)];

        let sunday = ymd(2026, 10, 11);
        let monday_token = parse_todo_due_prefix("下周一 计划", sunday);
        assert_eq!(monday_token.title, "计划");
        let _monday_candidates = [ymd(2026, 10, 12), ymd(2026, 10, 19)];
        todo!("#9 第 4 项尚未写入 product.md：不预填下周的周始");
    }

    /// 月底当天可能指今天，也可能指下个月的最后一天。不预填期望。
    #[test]
    #[ignore = "待 #9 第 4 项写入 product.md：月底当天输入「月底 / 月末」指今天还是下个月底"]
    fn issue_9_month_end_on_the_last_day() {
        let cases = [
            ymd(2026, 10, 31),
            ymd(2026, 2, 28),
            ymd(2024, 2, 29),
            ymd(2026, 4, 30),
            ymd(2026, 12, 31),
        ];
        for today in cases {
            assert_eq!(parse_todo_due_prefix("月底 收工", today).title, "收工");
            assert_eq!(parse_todo_due_prefix("月末", today).title, "");
        }
        todo!("#9 第 4 项尚未写入 product.md：不预填月底当天的到期日");
    }
}
