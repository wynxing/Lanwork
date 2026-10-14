//! 安静时段：每天固定的一段，可以跨过午夜。
//!
//! `config.json` 的 `quietHours` 不写或为 `null` 时没有这段。
//! 有值时是 `{"start":"HH:MM","end":"HH:MM"}`。小时可以只写一位，写回时补成两位。
//! 开始与结束相同由 [`super::model::normalize`] 拒绝。
//!
//! [`is_quiet_now`] 判断当前是否在安静时段里。提醒调度尚未并入，这里不调用它。

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::todos::ClockTime;

/// 时刻无法解析时可以显示的错误。与待办提醒的格式错误同一句。
pub const CLOCK_FORMAT_ERROR: &str = "时间格式是 HH:MM";

/// 每天的一段。`start` 与 `end` 是当天的时刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    pub start: ClockTime,
    pub end: ClockTime,
}

/// 打开开关、配置里还没有时段时写入的一段。
///
/// 产品规格把 22:00–08:00 当跨午夜的例子，没有规定第一次打开时的缺省。这里用这一段。
#[must_use]
pub fn default_quiet_hours() -> QuietHours {
    QuietHours {
        start: ClockTime::try_new(22, 0).expect("22:00"),
        end: ClockTime::try_new(8, 0).expect("08:00"),
    }
}

/// `HH:MM`。小时可以一位。冒号可以是全角。空白去掉。
#[must_use]
pub fn parse_clock(text: &str) -> Option<ClockTime> {
    let text = text.trim();
    let (hour, minute) = text.split_once([':', '：'])?;
    if !(1..=2).contains(&hour.len())
        || minute.len() != 2
        || !hour
            .bytes()
            .chain(minute.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hour = hour.parse::<u8>().ok()?;
    let minute = minute.parse::<u8>().ok()?;
    ClockTime::try_new(hour, minute)
}

#[must_use]
pub fn format_clock(time: ClockTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// 当前时刻是否落在安静时段里。
///
/// `hours` 为空时返回假。开始与结束的分钟相同也返回假。
/// 结束晚于开始时，从开始到结束之前算在里面。结束早于开始时跨过午夜：
/// 从开始到 24:00，再加上 0:00 到结束之前。开始时刻算在里面，结束时刻不算。
/// 端点算不算，产品规格没有写；这个半开区间不是产品规则。
#[must_use]
pub fn is_quiet_now(hours: Option<QuietHours>, now: ClockTime) -> bool {
    let Some(hours) = hours else {
        return false;
    };
    let start = hours.start.minute_of_day();
    let end = hours.end.minute_of_day();
    let now = now.minute_of_day();
    if start == end {
        return false;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

impl Serialize for QuietHours {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("QuietHours", 2)?;
        state.serialize_field("start", &format_clock(self.start))?;
        state.serialize_field("end", &format_clock(self.end))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for QuietHours {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            start: String,
            end: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let start =
            parse_clock(&wire.start).ok_or_else(|| serde::de::Error::custom(CLOCK_FORMAT_ERROR))?;
        let end =
            parse_clock(&wire.end).ok_or_else(|| serde::de::Error::custom(CLOCK_FORMAT_ERROR))?;
        Ok(Self { start, end })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(hour: u8, minute: u8) -> ClockTime {
        ClockTime::try_new(hour, minute).unwrap()
    }

    fn hours(start: (u8, u8), end: (u8, u8)) -> QuietHours {
        QuietHours {
            start: clock(start.0, start.1),
            end: clock(end.0, end.1),
        }
    }

    #[test]
    fn overnight_window_includes_the_start_and_excludes_the_end() {
        let window = Some(hours((22, 0), (8, 0)));
        assert!(is_quiet_now(window, clock(22, 0)));
        assert!(is_quiet_now(window, clock(23, 59)));
        assert!(is_quiet_now(window, clock(0, 0)));
        assert!(is_quiet_now(window, clock(7, 59)));
        assert!(!is_quiet_now(window, clock(8, 0)));
        assert!(!is_quiet_now(window, clock(21, 59)));
        assert!(!is_quiet_now(None, clock(23, 0)));
    }

    #[test]
    fn same_day_window_stops_before_the_end() {
        let window = Some(hours((9, 0), (17, 0)));
        assert!(!is_quiet_now(window, clock(8, 59)));
        assert!(is_quiet_now(window, clock(9, 0)));
        assert!(is_quiet_now(window, clock(16, 59)));
        assert!(!is_quiet_now(window, clock(17, 0)));
    }

    #[test]
    fn equal_endpoints_are_never_quiet() {
        let window = Some(hours((10, 0), (10, 0)));
        assert!(!is_quiet_now(window, clock(10, 0)));
        assert!(!is_quiet_now(window, clock(0, 0)));
    }

    #[test]
    fn clock_text_accepts_one_digit_hours_and_writes_two() {
        assert_eq!(parse_clock("9:05"), Some(clock(9, 5)));
        assert_eq!(parse_clock(" 22：00 "), Some(clock(22, 0)));
        assert_eq!(format_clock(clock(9, 5)), "09:05");
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("9:5"), None);
        assert_eq!(parse_clock(""), None);
    }
}
