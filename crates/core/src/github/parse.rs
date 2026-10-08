//! 把 `gh pr list --json` / `gh issue list --json` 的数组解析成条目。
//!
//! 也接受 REST 风格的 `html_url`、`updated_at`、`draft`。可选字段缺失或类型不对时，
//! 该字段用默认值，不让整个数组失败。顶层不是数组才算解析失败。
//! 状态字段缺失或无法识别时不推断为 open、closed 或 merged。

use serde_json::Value;

use super::error::GhCallError;

/// `gh` 或 REST 明确给出的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteState {
    Open,
    Closed,
    Merged,
}

/// 一条解析结果。`title`、`url`、`state` 可以缺。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedItem {
    pub number: u64,
    pub title: Option<String>,
    pub url: Option<String>,
    pub state: Option<RemoteState>,
    pub draft: bool,
    pub updated_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedRepo {
    pub pulls: Vec<ParsedItem>,
    pub issues: Vec<ParsedItem>,
}

pub fn parse_pulls(bytes: &[u8]) -> Result<Vec<ParsedItem>, GhCallError> {
    parse_array(bytes, false)
}

pub fn parse_issues(bytes: &[u8]) -> Result<Vec<ParsedItem>, GhCallError> {
    parse_array(bytes, true)
}

fn parse_array(bytes: &[u8], skip_pull_requests: bool) -> Result<Vec<ParsedItem>, GhCallError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| GhCallError::Parse)?;
    let Value::Array(items) = value else {
        return Err(GhCallError::Parse);
    };
    let mut parsed = Vec::new();
    for item in items {
        let Value::Object(map) = item else {
            continue;
        };
        if skip_pull_requests && matches!(map.get("pull_request"), Some(Value::Object(_))) {
            continue;
        }
        let Some(number) = parse_number(&map) else {
            continue;
        };
        parsed.push(ParsedItem {
            number,
            title: parse_string(&map, &["title"]),
            url: parse_string(&map, &["url", "html_url"]),
            state: parse_state(&map),
            draft: parse_draft(&map),
            updated_at_ms: parse_string(&map, &["updatedAt", "updated_at"])
                .as_deref()
                .and_then(parse_time_ms),
        });
    }
    Ok(parsed)
}

fn parse_number(map: &serde_json::Map<String, Value>) -> Option<u64> {
    match map.get("number")? {
        Value::Number(number) => number.as_u64().filter(|number| *number > 0),
        _ => None,
    }
}

fn parse_string(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(text)) = map.get(*key) {
            return Some(text.clone());
        }
    }
    None
}

fn parse_draft(map: &serde_json::Map<String, Value>) -> bool {
    match map.get("isDraft").or_else(|| map.get("draft")) {
        Some(Value::Bool(flag)) => *flag,
        _ => false,
    }
}

fn parse_state(map: &serde_json::Map<String, Value>) -> Option<RemoteState> {
    let text = map.get("state").and_then(Value::as_str)?;
    let state = text.to_ascii_lowercase();
    let merged = match map.get("mergedAt").or_else(|| map.get("merged_at")) {
        Some(Value::String(text)) => !text.is_empty(),
        _ => false,
    };
    match state.as_str() {
        "open" => Some(RemoteState::Open),
        "merged" => Some(RemoteState::Merged),
        "closed" if merged => Some(RemoteState::Merged),
        "closed" => Some(RemoteState::Closed),
        _ => None,
    }
}

/// 解析 `gh` 常用的 UTC 或带数字时区的时间。失败时返回 `None`。
pub fn parse_time_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u8 = date_parts.next()?.parse().ok()?;
    let day: u8 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let (time, offset) = split_offset(rest)?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock_parts = clock.split(':');
    let hour: u8 = clock_parts.next()?.parse().ok()?;
    let minute: u8 = clock_parts.next()?.parse().ok()?;
    let second: u8 = clock_parts.next()?.parse().ok()?;
    if clock_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let millis = fraction_millis(fraction)?;
    let days = days_from_civil(year, month, day)?;
    let mut millis_of_day = i64::from(hour) * 3_600_000
        + i64::from(minute) * 60_000
        + i64::from(second) * 1_000
        + i64::from(millis);
    millis_of_day -= offset;
    days.checked_mul(86_400_000)?.checked_add(millis_of_day)
}

fn fraction_millis(fraction: &str) -> Option<u16> {
    if fraction.is_empty() {
        return Some(0);
    }
    if !fraction.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let mut digits = fraction.to_owned();
    if digits.len() > 3 {
        digits.truncate(3);
    }
    while digits.len() < 3 {
        digits.push('0');
    }
    digits.parse().ok()
}

fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(time) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
        return Some((time, 0));
    }
    let split = rest
        .match_indices(['+', '-'])
        .next()
        .map(|(index, _)| index)?;
    if split == 0 {
        return None;
    }
    let time = &rest[..split];
    let offset = &rest[split..];
    let sign: i64 = if offset.starts_with('+') { 1 } else { -1 };
    let body = &offset[1..];
    let (hour, minute) = body.split_once(':')?;
    let hour: i64 = hour.parse().ok()?;
    let minute: i64 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((time, sign * (hour * 3_600_000 + minute * 60_000)))
}

fn days_from_civil(year: i32, month: u8, day: u8) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 {
        return None;
    }
    let year = i64::from(year);
    let month = i64::from(month);
    let day = i64::from(day);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    Some(era * 146_097 + doe as i64 - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gh_and_rest_fields_and_skips_broken_elements() {
        let pulls = parse_pulls(
            br#"[
                {"number":12,"title":"Fix the latch","url":"https://github.com/example/widget/pull/12","state":"OPEN","isDraft":false,"updatedAt":"2026-01-02T03:04:05Z"},
                {"number":13,"title":"Draft notes","html_url":"https://github.com/example/widget/pull/13","state":"open","draft":true},
                {"number":3,"title":"Landed","url":"https://github.com/example/widget/pull/3","state":"CLOSED","mergedAt":"2026-01-01T00:00:00Z"},
                {"number":4,"state":"CLOSED","merged_at":null},
                {"title":"missing number","state":"CLOSED"},
                "not-an-object",
                {"number":0,"state":"OPEN","title":"zero"}
            ]"#,
        )
        .unwrap();
        assert_eq!(pulls.len(), 4);
        assert_eq!(pulls[0].updated_at_ms, Some(1_767_323_045_000));
        assert!(!pulls[0].draft);
        assert_eq!(pulls[1].state, Some(RemoteState::Open));
        assert!(pulls[1].draft);
        assert!(pulls[1].updated_at_ms.is_none());
        assert_eq!(pulls[2].state, Some(RemoteState::Merged));
        assert_eq!(pulls[3].state, Some(RemoteState::Closed));
    }

    #[test]
    fn issues_skip_embedded_pulls_and_reject_non_arrays() {
        let issues = parse_issues(
            br#"[
                {"number":8,"title":"Document the panel","url":"https://github.com/example/widget/issues/8","state":"OPEN"},
                {"number":9,"title":"Actually a pull","state":"OPEN","pull_request":{"url":"https://example.invalid"}}
            ]"#,
        )
        .unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 8);
        assert!(parse_pulls(br#"{"message":"nope"}"#).is_err());
    }

    #[test]
    fn unknown_state_and_bad_time_stay_empty() {
        let pulls = parse_pulls(
            br#"[{"number":1,"title":"A","url":"https://example.com/1","state":"PENDING","updatedAt":"yesterday"}]"#,
        )
        .unwrap();
        assert!(pulls[0].state.is_none());
        assert!(pulls[0].updated_at_ms.is_none());
        assert_eq!(
            parse_time_ms("2026-01-02T03:04:05.123Z"),
            Some(1_767_323_045_123)
        );
        assert_eq!(parse_time_ms("1970-01-01T00:00:00Z"), Some(0));
    }
}
