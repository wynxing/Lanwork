//! 本机登录用量。路径、网址、方法和请求头都写死在这里。
//!
//! 清单只能点名 `chatgpt`、`claude` 或 `grok`，不能改地址。
//! 宿主只读登录文件，不写、不刷新、不修改。脚本只拿到规范化后的数字。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::http::{BODY_CAP, Outgoing};

pub const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const GROK_USAGE_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
pub const USAGE_USER_AGENT: &str = "Lanwork";
/// 同一来源两次真正去读文件、发请求的最短间隔。
pub const USAGE_INTERVAL: Duration = Duration::from_secs(60);

const GROK_AUTH_ORIGIN: &str = "https://auth.x.ai";
const GROK_FALLBACK_ORIGIN: &str = "https://accounts.x.ai/sign-in";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UsageSource {
    Chatgpt,
    Claude,
    Grok,
}

impl UsageSource {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "chatgpt" => Some(Self::Chatgpt),
            "claude" => Some(Self::Claude),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::Claude => "claude",
            Self::Grok => "grok",
        }
    }

    #[must_use]
    pub fn confirm_line(self) -> &'static str {
        match self {
            Self::Chatgpt => "使用本机的 ChatGPT 登录",
            Self::Claude => "使用本机的 Claude 登录",
            Self::Grok => "使用本机的 Grok 登录",
        }
    }
}

/// 三个登录目录。没有绝对路径的那一项当作没有登录。
///
/// 产品在 Windows 上先用 `USERPROFILE`。它没设或是空的，才用 `HOME`。
/// 某一项的目录变量优先于这两者。相对路径不补当前工作目录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageRoots {
    pub chatgpt: Option<PathBuf>,
    pub claude: Option<PathBuf>,
    pub grok: Option<PathBuf>,
}

impl UsageRoots {
    #[must_use]
    pub fn from_env() -> Self {
        let userprofile = env_path("USERPROFILE");
        let home = env_path("HOME");
        assemble_roots(
            userprofile.as_deref(),
            home.as_deref(),
            env_path("CODEX_HOME"),
            env_path("CLAUDE_CONFIG_DIR"),
            env_path("GROK_HOME"),
        )
    }

    /// 目录不是绝对路径时返回 `None`，调用方按没有登录处理，不去读文件。
    #[must_use]
    pub fn file(&self, source: UsageSource) -> Option<PathBuf> {
        let dir = match source {
            UsageSource::Chatgpt => self.chatgpt.as_ref(),
            UsageSource::Claude => self.claude.as_ref(),
            UsageSource::Grok => self.grok.as_ref(),
        }?;
        if !dir.is_absolute() {
            return None;
        }
        let name = match source {
            UsageSource::Chatgpt | UsageSource::Grok => "auth.json",
            UsageSource::Claude => ".credentials.json",
        };
        Some(dir.join(name))
    }
}

fn assemble_roots(
    userprofile: Option<&Path>,
    home: Option<&Path>,
    codex_home: Option<PathBuf>,
    claude_config_dir: Option<PathBuf>,
    grok_home: Option<PathBuf>,
) -> UsageRoots {
    resolve_roots(
        userprofile.or(home),
        codex_home,
        claude_config_dir,
        grok_home,
    )
}

#[must_use]
pub fn resolve_roots(
    home: Option<&Path>,
    codex_home: Option<PathBuf>,
    claude_config_dir: Option<PathBuf>,
    grok_home: Option<PathBuf>,
) -> UsageRoots {
    UsageRoots {
        chatgpt: chosen_root(codex_home, home, ".codex"),
        claude: chosen_root(claude_config_dir, home, ".claude"),
        grok: chosen_root(grok_home, home, ".grok"),
    }
}

fn chosen_root(
    override_dir: Option<PathBuf>,
    home: Option<&Path>,
    folder: &str,
) -> Option<PathBuf> {
    let path = match override_dir {
        Some(dir) => dir,
        None => home?.join(folder),
    };
    path.is_absolute().then_some(path)
}

fn env_path(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageWindow {
    pub name: String,
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub limit_seconds: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageSnapshot {
    pub plan: Option<String>,
    pub limit_reached: Option<bool>,
    pub windows: Vec<UsageWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageError {
    LoginExpired,
    Invalid,
    Failed,
    Unauthorized,
}

impl UsageError {
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::LoginExpired => "登录已过期",
            Self::Invalid => "数据无效",
            Self::Failed => "请求失败",
            Self::Unauthorized => "未授权",
        }
    }
}

pub struct UsageCall {
    pub outgoing: Outgoing,
    /// 只留在这次请求的线程上，用来确认规范化结果里没有这些字。不进脚本，也不进日志。
    pub sensitive: Vec<String>,
}

impl std::fmt::Debug for UsageCall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UsageCall")
            .field("outgoing", &self.outgoing)
            .field("sensitive_len", &self.sensitive.len())
            .finish()
    }
}

pub fn prepare_usage(
    source: UsageSource,
    roots: &UsageRoots,
    now_ms: i64,
) -> Result<UsageCall, UsageError> {
    let Some(path) = roots.file(source) else {
        return Err(UsageError::LoginExpired);
    };
    let bytes = read_auth(&path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| UsageError::LoginExpired)?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| UsageError::LoginExpired)?;
    match source {
        UsageSource::Chatgpt => prepare_chatgpt(&value, now_ms),
        UsageSource::Claude => prepare_claude(&value, now_ms),
        UsageSource::Grok => prepare_grok(&value, now_ms),
    }
}

pub fn interpret_usage(
    source: UsageSource,
    status: u16,
    body: &str,
    sensitive: &[String],
) -> Result<UsageSnapshot, UsageError> {
    if status == 401 || status == 403 {
        return Err(UsageError::LoginExpired);
    }
    if !(200..300).contains(&status) {
        return Err(UsageError::Failed);
    }
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| UsageError::Invalid)?;
    let snapshot = match source {
        UsageSource::Chatgpt => parse_chatgpt(&value)?,
        UsageSource::Claude => parse_claude(&value)?,
        UsageSource::Grok => parse_grok(&value)?,
    };
    if snapshot
        .plan
        .as_deref()
        .is_some_and(|plan| leaks(plan, sensitive))
    {
        return Err(UsageError::Invalid);
    }
    Ok(snapshot)
}

fn read_auth(path: &Path) -> Result<Vec<u8>, UsageError> {
    let file = std::fs::File::open(path).map_err(|_| UsageError::LoginExpired)?;
    let mut reader = file.take(u64::try_from(BODY_CAP).unwrap_or(u64::MAX) + 1);
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|_| UsageError::LoginExpired)?;
    if buf.len() > BODY_CAP {
        return Err(UsageError::LoginExpired);
    }
    Ok(buf)
}

fn prepare_chatgpt(value: &serde_json::Value, now_ms: i64) -> Result<UsageCall, UsageError> {
    let tokens = value
        .get("tokens")
        .and_then(serde_json::Value::as_object)
        .ok_or(UsageError::LoginExpired)?;
    let token = tokens
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .ok_or(UsageError::LoginExpired)?;
    let account = tokens
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(UsageError::LoginExpired)?;
    if !header_safe(token) || !header_safe(account) {
        return Err(UsageError::LoginExpired);
    }
    let exp_ms = jwt_expiry_ms(token).ok_or(UsageError::LoginExpired)?;
    if now_ms >= exp_ms {
        return Err(UsageError::LoginExpired);
    }
    Ok(UsageCall {
        outgoing: get_request(
            CHATGPT_USAGE_URL,
            token,
            vec![("ChatGPT-Account-Id".to_string(), account.to_string())],
        ),
        sensitive: vec![token.to_string(), account.to_string()],
    })
}

fn prepare_claude(value: &serde_json::Value, now_ms: i64) -> Result<UsageCall, UsageError> {
    let oauth = value
        .get("claudeAiOauth")
        .and_then(serde_json::Value::as_object)
        .ok_or(UsageError::LoginExpired)?;
    let token = oauth
        .get("accessToken")
        .and_then(serde_json::Value::as_str)
        .ok_or(UsageError::LoginExpired)?;
    let expires = json_i64(oauth.get("expiresAt")).ok_or(UsageError::LoginExpired)?;
    if !header_safe(token) || now_ms >= expires {
        return Err(UsageError::LoginExpired);
    }
    Ok(UsageCall {
        outgoing: get_request(
            CLAUDE_USAGE_URL,
            token,
            vec![("anthropic-beta".to_string(), "oauth-2025-04-20".to_string())],
        ),
        sensitive: vec![token.to_string()],
    })
}

fn prepare_grok(value: &serde_json::Value, now_ms: i64) -> Result<UsageCall, UsageError> {
    let map = value.as_object().ok_or(UsageError::LoginExpired)?;
    let (token, _expires) = grok_credential(map, now_ms)?;
    Ok(UsageCall {
        outgoing: get_request(
            GROK_USAGE_URL,
            token,
            vec![("X-XAI-Token-Auth".to_string(), "xai-grok-cli".to_string())],
        ),
        sensitive: vec![token.to_string()],
    })
}

fn grok_credential(
    map: &serde_json::Map<String, serde_json::Value>,
    now_ms: i64,
) -> Result<(&str, i64), UsageError> {
    if let Some(pair) = first_usable(map, GROK_AUTH_ORIGIN, now_ms) {
        return Ok(pair);
    }
    first_usable(map, GROK_FALLBACK_ORIGIN, now_ms).ok_or(UsageError::LoginExpired)
}

/// 键等于这个来源，或者紧接着的字符是 `/` 或 `:`。
/// `https://auth.x.ai.evil.example` 不算同一来源。
fn origin_match(key: &str, origin: &str) -> bool {
    let Some(rest) = key.strip_prefix(origin) else {
        return false;
    };
    matches!(rest.chars().next(), None | Some('/' | ':'))
}

fn first_usable<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    origin: &str,
    now_ms: i64,
) -> Option<(&'a str, i64)> {
    let mut keys: Vec<&String> = map.keys().filter(|key| origin_match(key, origin)).collect();
    keys.sort();
    for key in keys {
        if let Some(pair) = usable_entry(map.get(key), now_ms) {
            return Some(pair);
        }
    }
    None
}

fn usable_entry(value: Option<&serde_json::Value>, now_ms: i64) -> Option<(&str, i64)> {
    let obj = value?.as_object()?;
    let key = obj.get("key")?.as_str()?;
    if !header_safe(key) {
        return None;
    }
    let expires = obj.get("expires_at").and_then(parse_time_to_ms)?;
    if now_ms >= expires {
        return None;
    }
    Some((key, expires))
}

fn get_request(url: &str, token: &str, extra: Vec<(String, String)>) -> Outgoing {
    let mut headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
    headers.extend(extra);
    headers.push(("Accept".to_string(), "application/json".to_string()));
    headers.push(("User-Agent".to_string(), USAGE_USER_AGENT.to_string()));
    Outgoing {
        method: "GET".to_string(),
        url: url.to_string(),
        headers,
        body: Vec::new(),
    }
}

fn header_safe(value: &str) -> bool {
    !value.is_empty() && !value.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0))
}

fn parse_chatgpt(value: &serde_json::Value) -> Result<UsageSnapshot, UsageError> {
    let root = value.as_object().ok_or(UsageError::Invalid)?;
    let rate = root
        .get("rate_limit")
        .and_then(serde_json::Value::as_object)
        .ok_or(UsageError::Invalid)?;
    let mut windows = Vec::new();
    if let Some(window) = chatgpt_window(rate.get("primary_window")) {
        windows.push(window);
    }
    if let Some(window) = chatgpt_window(rate.get("secondary_window")) {
        windows.push(window);
    }
    if windows.is_empty() {
        return Err(UsageError::Invalid);
    }
    let plan = optional_plan(root.get("plan_type"))?;
    let limit_reached = match rate.get("limit_reached") {
        Some(serde_json::Value::Bool(flag)) => Some(*flag),
        Some(_) => return Err(UsageError::Invalid),
        None => None,
    };
    Ok(UsageSnapshot {
        plan,
        limit_reached,
        windows,
    })
}

fn chatgpt_window(value: Option<&serde_json::Value>) -> Option<UsageWindow> {
    let obj = value?.as_object()?;
    let used = json_f64(obj.get("used_percent"))?;
    if !used.is_finite() {
        return None;
    }
    let limit = json_i64(obj.get("limit_window_seconds")).filter(|seconds| *seconds > 0);
    let name = match limit {
        Some(seconds) => duration_name(seconds),
        None => "窗口".to_string(),
    };
    Some(UsageWindow {
        name,
        used_percent: used,
        resets_at_ms: obj.get("reset_at").and_then(parse_time_to_ms),
        limit_seconds: limit,
    })
}

fn duration_name(seconds: i64) -> String {
    match seconds {
        18_000 => "5小时".to_string(),
        604_800 => "每周".to_string(),
        n if n > 0 && n % 86_400 == 0 => format!("{}天", n / 86_400),
        n if n > 0 && n % 3_600 == 0 => format!("{}小时", n / 3_600),
        _ => "窗口".to_string(),
    }
}

fn parse_claude(value: &serde_json::Value) -> Result<UsageSnapshot, UsageError> {
    let root = value.as_object().ok_or(UsageError::Invalid)?;
    let mut windows = Vec::new();
    for (key, name) in [
        ("five_hour", "5小时"),
        ("seven_day", "7天"),
        ("seven_day_opus", "7天 Opus"),
        ("seven_day_sonnet", "7天 Sonnet"),
    ] {
        if let Some(window) = claude_window(name, root.get(key)) {
            windows.push(window);
        }
    }
    if windows.is_empty() {
        return Err(UsageError::Invalid);
    }
    Ok(UsageSnapshot {
        plan: None,
        limit_reached: None,
        windows,
    })
}

fn claude_window(name: &str, value: Option<&serde_json::Value>) -> Option<UsageWindow> {
    if value.is_none_or(serde_json::Value::is_null) {
        return None;
    }
    let obj = value?.as_object()?;
    let used = json_f64(obj.get("utilization"))?;
    if !used.is_finite() {
        return None;
    }
    let resets_at_ms = match obj.get("resets_at") {
        Some(serde_json::Value::String(text)) => parse_rfc3339_ms(text),
        Some(_) => None,
        None => None,
    };
    Some(UsageWindow {
        name: name.to_string(),
        used_percent: used,
        resets_at_ms,
        limit_seconds: None,
    })
}

fn parse_grok(value: &serde_json::Value) -> Result<UsageSnapshot, UsageError> {
    let root = value.as_object().ok_or(UsageError::Invalid)?;
    let used = grok_percent(root).ok_or(UsageError::Invalid)?;
    Ok(UsageSnapshot {
        plan: None,
        limit_reached: None,
        windows: vec![UsageWindow {
            name: "额度".to_string(),
            used_percent: used,
            resets_at_ms: grok_reset(root),
            limit_seconds: None,
        }],
    })
}

fn grok_percent(root: &serde_json::Map<String, serde_json::Value>) -> Option<f64> {
    if let Some(config) = root.get("config").and_then(serde_json::Value::as_object)
        && let Some(percent) = json_f64(config.get("creditUsagePercent"))
        && percent.is_finite()
    {
        return Some(percent);
    }
    let used = demand_val(root, "onDemandUsed")?;
    let cap = demand_val(root, "onDemandCap")?;
    if !used.is_finite() || !cap.is_finite() || cap == 0.0 {
        return None;
    }
    let percent = used / cap * 100.0;
    percent.is_finite().then_some(percent)
}

fn demand_val(root: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    nested_val(root.get(key)).or_else(|| {
        root.get("config")
            .and_then(serde_json::Value::as_object)
            .and_then(|config| nested_val(config.get(key)))
    })
}

fn nested_val(value: Option<&serde_json::Value>) -> Option<f64> {
    let obj = value?.as_object()?;
    json_f64(obj.get("val")).or_else(|| {
        obj.get("val")
            .and_then(serde_json::Value::as_str)
            .and_then(|text| text.trim().parse().ok())
    })
}

fn grok_reset(root: &serde_json::Map<String, serde_json::Value>) -> Option<i64> {
    let config = root.get("config")?.as_object()?;
    if let Some(end) = config
        .get("currentPeriod")
        .and_then(serde_json::Value::as_object)
        .and_then(|period| period.get("end"))
        && let Some(ms) = parse_time_to_ms(end)
    {
        return Some(ms);
    }
    config.get("billingPeriodEnd").and_then(parse_time_to_ms)
}

fn optional_plan(value: Option<&serde_json::Value>) -> Result<Option<String>, UsageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(text) = value.as_str() else {
        return Err(UsageError::Invalid);
    };
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 64 || text.chars().any(char::is_control) {
        return Ok(None);
    }
    Ok(Some(text.to_string()))
}

fn leaks(text: &str, sensitive: &[String]) -> bool {
    sensitive
        .iter()
        .any(|item| !item.is_empty() && text.contains(item))
}

fn json_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

fn json_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(number) => number.as_i64().or_else(|| {
            number.as_f64().and_then(|value| {
                if value.is_finite() {
                    Some(value as i64)
                } else {
                    None
                }
            })
        }),
        _ => None,
    }
}

fn parse_time_to_ms(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .and_then(scale_unix),
        serde_json::Value::String(text) => {
            let text = text.trim();
            if let Ok(number) = text.parse::<i64>() {
                return scale_unix(number);
            }
            if let Ok(number) = text.parse::<f64>()
                && number.is_finite()
            {
                return scale_unix(number as i64);
            }
            parse_rfc3339_ms(text)
        }
        _ => None,
    }
}

fn scale_unix(value: i64) -> Option<i64> {
    if value <= 0 {
        return None;
    }
    if value >= 1_000_000_000_000 {
        Some(value)
    } else if value >= 1_000_000_000 {
        value.checked_mul(1000)
    } else {
        None
    }
}

fn jwt_expiry_ms(token: &str) -> Option<i64> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let bytes = b64url_decode(payload)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let secs = json_i64(value.get("exp"))?;
    if secs < 0 {
        return Some(0);
    }
    Some(secs.checked_mul(1000).unwrap_or(i64::MAX))
}

fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut alphabet = [255u8; 256];
    for (index, byte) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        alphabet[usize::from(*byte)] = u8::try_from(index).unwrap_or(255);
    }
    alphabet[usize::from(b'-')] = 62;
    alphabet[usize::from(b'_')] = 63;
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for byte in input.as_bytes() {
        if *byte == b'=' {
            break;
        }
        let value = alphabet[usize::from(*byte)];
        if value == 255 {
            return None;
        }
        buf = (buf << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buf >> bits) & 0xff).unwrap_or(0));
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year = parse_digits(&bytes[0..4])?;
    if bytes[4] != b'-' || bytes[7] != b'-' || (bytes[10] != b'T' && bytes[10] != b' ') {
        return None;
    }
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let hour = parse_digits(&bytes[11..13])?;
    let minute = parse_digits(&bytes[14..16])?;
    let second = parse_digits(&bytes[17..19])?;
    let mut index = 19;
    let mut frac_ms = 0i64;
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if start == index {
            return None;
        }
        let mut scale = 100i64;
        for digit in bytes[start..index].iter().take(3) {
            frac_ms += i64::from(digit - b'0') * scale;
            scale /= 10;
        }
    }
    let offset_minutes = if index < bytes.len() && bytes[index] == b'Z' {
        if index + 1 != bytes.len() {
            return None;
        }
        0
    } else if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
        let sign = if bytes[index] == b'+' { 1 } else { -1 };
        index += 1;
        let (off_hour, off_minute, next) = if bytes.len() >= index + 5 && bytes[index + 2] == b':' {
            (
                parse_digits(&bytes[index..index + 2])?,
                parse_digits(&bytes[index + 3..index + 5])?,
                index + 5,
            )
        } else if bytes.len() >= index + 4 {
            (
                parse_digits(&bytes[index..index + 2])?,
                parse_digits(&bytes[index + 2..index + 4])?,
                index + 4,
            )
        } else {
            return None;
        };
        if next != bytes.len() || off_minute > 59 {
            return None;
        }
        sign * (off_hour * 60 + off_minute)
    } else {
        return None;
    };
    civil_to_unix_ms(CivilClock {
        year,
        month,
        day,
        hour,
        minute,
        second,
        frac_ms,
        offset_minutes,
    })
}

struct CivilClock {
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    frac_ms: i64,
    offset_minutes: i32,
}

fn parse_digits(bytes: &[u8]) -> Option<i32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn civil_to_unix_ms(clock: CivilClock) -> Option<i64> {
    if !(1..=12).contains(&clock.month)
        || !(1..=31).contains(&clock.day)
        || !(0..=23).contains(&clock.hour)
        || !(0..=59).contains(&clock.minute)
        || !(0..=59).contains(&clock.second)
    {
        return None;
    }
    let days = days_from_civil(clock.year, clock.month, clock.day)?;
    let secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(clock.hour) * 3_600)?
        .checked_add(i64::from(clock.minute) * 60)?
        .checked_add(i64::from(clock.second))?
        .checked_sub(i64::from(clock.offset_minutes) * 60)?;
    secs.checked_mul(1000)?.checked_add(clock.frac_ms)
}

/// Howard Hinnant 的公历转 Unix 日。1970-01-01 是 0。
fn days_from_civil(mut year: i32, month: i32, day: i32) -> Option<i64> {
    if month <= 2 {
        year -= 1;
    }
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = u32::try_from(year - era * 400).ok()?;
    let month = u32::try_from(month).ok()?;
    let day = u32::try_from(day).ok()?;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(i64::from(era) * 146_097 + i64::from(doe) - 719_468)
}

struct CacheSlot {
    at: Instant,
    outcome: Result<UsageSnapshot, UsageError>,
}

thread_local! {
    static USAGE_CACHE: std::cell::RefCell<HashMap<UsageSource, CacheSlot>> =
        std::cell::RefCell::new(HashMap::new());
}

#[must_use]
pub fn cached_usage(source: UsageSource) -> Option<Result<UsageSnapshot, UsageError>> {
    USAGE_CACHE.with(|slot| {
        let cache = slot.borrow();
        let entry = cache.get(&source)?;
        if entry.at.elapsed() >= USAGE_INTERVAL {
            None
        } else {
            Some(entry.outcome.clone())
        }
    })
}

pub fn store_usage(source: UsageSource, outcome: Result<UsageSnapshot, UsageError>) {
    USAGE_CACHE.with(|slot| {
        slot.borrow_mut().insert(
            source,
            CacheSlot {
                at: Instant::now(),
                outcome,
            },
        );
    });
}

#[cfg(test)]
pub(crate) fn reset_usage_cache_for_test() {
    USAGE_CACHE.with(|slot| slot.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn age_usage_cache_for_test(by: Duration) {
    USAGE_CACHE.with(|slot| {
        for entry in slot.borrow_mut().values_mut() {
            entry.at = entry.at.checked_sub(by).unwrap_or(entry.at);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(exp: i64) -> String {
        let header = b64url(br#"{"alg":"none"}"#);
        let payload = b64url(format!(r#"{{"exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    fn b64url(bytes: &[u8]) -> String {
        const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        let mut index = 0;
        while index + 3 <= bytes.len() {
            let n = (u32::from(bytes[index]) << 16)
                | (u32::from(bytes[index + 1]) << 8)
                | u32::from(bytes[index + 2]);
            out.push(ALPHA[((n >> 18) & 63) as usize] as char);
            out.push(ALPHA[((n >> 12) & 63) as usize] as char);
            out.push(ALPHA[((n >> 6) & 63) as usize] as char);
            out.push(ALPHA[(n & 63) as usize] as char);
            index += 3;
        }
        if index < bytes.len() {
            let mut n = u32::from(bytes[index]) << 16;
            out.push(ALPHA[((n >> 18) & 63) as usize] as char);
            if index + 1 < bytes.len() {
                n |= u32::from(bytes[index + 1]) << 8;
                out.push(ALPHA[((n >> 12) & 63) as usize] as char);
                out.push(ALPHA[((n >> 6) & 63) as usize] as char);
            } else {
                out.push(ALPHA[((n >> 12) & 63) as usize] as char);
            }
        }
        out
    }

    fn roots(dir: &Path) -> UsageRoots {
        UsageRoots {
            chatgpt: Some(dir.join("codex")),
            claude: Some(dir.join("claude")),
            grok: Some(dir.join("grok")),
        }
    }

    fn write_auth(dir: &Path, source: UsageSource, body: &str) {
        let path = roots(dir).file(source).expect("绝对路径");
        std::fs::create_dir_all(path.parent().expect("目录")).expect("目录");
        std::fs::write(path, body).expect("写入");
    }

    #[test]
    fn default_dirs_follow_home_and_overrides() {
        let home = std::env::temp_dir().join("lanwork-usage-home");
        assert!(home.is_absolute());
        let roots = resolve_roots(Some(&home), None, None, None);
        let codex = home.join(".codex");
        let claude = home.join(".claude");
        let grok = home.join(".grok");
        assert_eq!(roots.chatgpt.as_deref(), Some(codex.as_path()));
        assert_eq!(roots.claude.as_deref(), Some(claude.as_path()));
        assert_eq!(roots.grok.as_deref(), Some(grok.as_path()));
        assert_eq!(
            roots.file(UsageSource::Chatgpt),
            Some(codex.join("auth.json"))
        );
        assert_eq!(
            roots.file(UsageSource::Claude),
            Some(claude.join(".credentials.json"))
        );
        let codex_override = home.join("codex-override");
        let claude_override = home.join("claude-override");
        let grok_override = home.join("grok-override");
        let overridden = resolve_roots(
            Some(&home),
            Some(codex_override),
            Some(claude_override),
            Some(grok_override.clone()),
        );
        assert_eq!(
            overridden.file(UsageSource::Grok),
            Some(grok_override.join("auth.json"))
        );
        let home_only = assemble_roots(None, Some(&home), None, None, None);
        assert_eq!(home_only.chatgpt.as_deref(), Some(codex.as_path()));
        let profile = home.join("profile");
        let profile_codex = profile.join(".codex");
        let profile_first = assemble_roots(Some(&profile), Some(&home), None, None, None);
        assert_eq!(
            profile_first.chatgpt.as_deref(),
            Some(profile_codex.as_path())
        );
    }

    #[test]
    fn missing_or_relative_home_is_no_login() {
        let missing = resolve_roots(None, None, None, None);
        assert!(missing.chatgpt.is_none());
        assert!(missing.claude.is_none());
        assert!(missing.grok.is_none());
        assert_eq!(
            prepare_usage(UsageSource::Chatgpt, &missing, 0).unwrap_err(),
            UsageError::LoginExpired
        );
        let relative = resolve_roots(
            Some(Path::new("relative-home")),
            Some(PathBuf::from("codex")),
            Some(PathBuf::from("claude")),
            Some(PathBuf::from(".")),
        );
        assert!(relative.file(UsageSource::Chatgpt).is_none());
        assert!(relative.file(UsageSource::Claude).is_none());
        assert!(relative.file(UsageSource::Grok).is_none());
        assert_eq!(
            prepare_usage(UsageSource::Grok, &relative, 0).unwrap_err(),
            UsageError::LoginExpired
        );
        let dotted = UsageRoots {
            chatgpt: Some(PathBuf::from(".")),
            claude: None,
            grok: None,
        };
        assert!(dotted.file(UsageSource::Chatgpt).is_none());
        let home = std::env::temp_dir();
        let only_codex = home.join("only-codex");
        let partial = assemble_roots(None, None, Some(only_codex.clone()), None, None);
        assert_eq!(partial.chatgpt.as_deref(), Some(only_codex.as_path()));
        assert!(partial.claude.is_none());
        assert!(partial.grok.is_none());
        let blocked = assemble_roots(
            Some(Path::new("rel-profile")),
            Some(&home),
            None,
            None,
            None,
        );
        assert!(blocked.chatgpt.is_none());
        assert!(blocked.claude.is_none());
        assert!(blocked.grok.is_none());
    }

    #[test]
    fn chatgpt_parses_windows_and_rejects_expiry_missing_file_and_bad_json() {
        let dir =
            std::env::temp_dir().join(format!("lanwork-usage-chatgpt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let token = jwt(4_102_444_800);
        let account = "acct-usage-test";
        write_auth(
            &dir,
            UsageSource::Chatgpt,
            &format!(r#"{{"tokens":{{"access_token":"{token}","account_id":"{account}"}}}}"#),
        );
        let call = prepare_usage(UsageSource::Chatgpt, &roots(&dir), 1_700_000_000_000).unwrap();
        assert_eq!(call.outgoing.url, CHATGPT_USAGE_URL);
        assert_eq!(call.outgoing.method, "GET");
        assert!(call.outgoing.body.is_empty());
        assert!(call.outgoing.headers.iter().any(|(name, value)| {
            name == "Authorization" && value == &format!("Bearer {token}")
        }));
        assert!(
            call.outgoing
                .headers
                .iter()
                .any(|(name, value)| name == "ChatGPT-Account-Id" && value == account)
        );
        assert!(
            call.outgoing
                .headers
                .iter()
                .any(|(name, value)| name == "Accept" && value == "application/json")
        );
        assert!(
            call.outgoing
                .headers
                .iter()
                .any(|(name, value)| name == "User-Agent" && value == USAGE_USER_AGENT)
        );
        let body = r#"{"plan_type":"plus","rate_limit":{"limit_reached":false,"primary_window":{"used_percent":12.5,"limit_window_seconds":18000,"reset_at":1577836800},"secondary_window":{"used_percent":3,"limit_window_seconds":604800,"reset_at":1578441600}}}"#;
        let snapshot = interpret_usage(UsageSource::Chatgpt, 200, body, &call.sensitive).unwrap();
        assert_eq!(snapshot.plan.as_deref(), Some("plus"));
        assert_eq!(snapshot.limit_reached, Some(false));
        assert_eq!(snapshot.windows[0].name, "5小时");
        assert!((snapshot.windows[0].used_percent - 12.5).abs() < 1e-9);
        assert_eq!(snapshot.windows[0].limit_seconds, Some(18_000));
        assert_eq!(snapshot.windows[0].resets_at_ms, Some(1_577_836_800_000));
        assert_eq!(snapshot.windows[1].name, "每周");
        assert_eq!(snapshot.windows[1].resets_at_ms, Some(1_578_441_600_000));
        let shown = format!("{snapshot:?} {}", UsageError::LoginExpired.text());
        assert!(!shown.contains(&token));
        assert!(!shown.contains(account));
        let leaked = format!(
            r#"{{"plan_type":"{token}","rate_limit":{{"primary_window":{{"used_percent":1,"limit_window_seconds":18000}}}}}}"#
        );
        assert_eq!(
            interpret_usage(UsageSource::Chatgpt, 200, &leaked, &call.sensitive).unwrap_err(),
            UsageError::Invalid
        );
        assert_eq!(
            interpret_usage(UsageSource::Chatgpt, 401, &token, &call.sensitive).unwrap_err(),
            UsageError::LoginExpired
        );
        assert_eq!(
            interpret_usage(UsageSource::Chatgpt, 403, "nope", &call.sensitive).unwrap_err(),
            UsageError::LoginExpired
        );
        assert_eq!(
            interpret_usage(UsageSource::Chatgpt, 500, "down", &[]).unwrap_err(),
            UsageError::Failed
        );
        assert_eq!(
            interpret_usage(UsageSource::Chatgpt, 200, "{", &[]).unwrap_err(),
            UsageError::Invalid
        );
        assert_eq!(
            interpret_usage(UsageSource::Chatgpt, 302, "https://evil.example", &[]).unwrap_err(),
            UsageError::Failed
        );

        let expired = jwt(1_000);
        write_auth(
            &dir,
            UsageSource::Chatgpt,
            &format!(r#"{{"tokens":{{"access_token":"{expired}","account_id":"{account}"}}}}"#),
        );
        let err = prepare_usage(UsageSource::Chatgpt, &roots(&dir), 1_700_000_000_000).unwrap_err();
        assert_eq!(err, UsageError::LoginExpired);
        assert!(!err.text().contains(&expired));
        let auth = roots(&dir).file(UsageSource::Chatgpt).expect("绝对路径");
        std::fs::remove_file(&auth).unwrap();
        assert_eq!(
            prepare_usage(UsageSource::Chatgpt, &roots(&dir), 0).unwrap_err(),
            UsageError::LoginExpired
        );
        assert!(!auth.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_reads_optional_windows_and_expiry_in_milliseconds() {
        let dir = std::env::temp_dir().join(format!("lanwork-usage-claude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let token = "claude-token-secret";
        write_auth(
            &dir,
            UsageSource::Claude,
            &format!(
                r#"{{"claudeAiOauth":{{"accessToken":"{token}","expiresAt":1800000000000}}}}"#
            ),
        );
        let call = prepare_usage(UsageSource::Claude, &roots(&dir), 1_700_000_000_000).unwrap();
        assert_eq!(call.outgoing.url, CLAUDE_USAGE_URL);
        assert!(
            call.outgoing
                .headers
                .iter()
                .any(|(name, value)| { name == "anthropic-beta" && value == "oauth-2025-04-20" })
        );
        assert!(call.outgoing.headers.iter().any(|(name, value)| {
            name == "Authorization" && value == &format!("Bearer {token}")
        }));
        let body = r#"{"five_hour":{"utilization":25,"resets_at":"2020-01-01T00:00:00Z"},"seven_day":{"utilization":4.5,"resets_at":"2020-01-08T00:00:00+00:00"},"seven_day_opus":null,"seven_day_sonnet":{"utilization":1,"resets_at":"2020-01-08T08:00:00+08:00"}}"#;
        let snapshot = interpret_usage(UsageSource::Claude, 200, body, &call.sensitive).unwrap();
        assert!(snapshot.plan.is_none());
        assert_eq!(
            snapshot
                .windows
                .iter()
                .map(|window| window.name.as_str())
                .collect::<Vec<_>>(),
            ["5小时", "7天", "7天 Sonnet"]
        );
        assert_eq!(snapshot.windows[0].resets_at_ms, Some(1_577_836_800_000));
        assert_eq!(snapshot.windows[2].resets_at_ms, Some(1_578_441_600_000));
        let shown = format!("{snapshot:?}");
        assert!(!shown.contains(token));
        write_auth(
            &dir,
            UsageSource::Claude,
            &format!(
                r#"{{"claudeAiOauth":{{"accessToken":"{token}","expiresAt":1600000000000}}}}"#
            ),
        );
        assert_eq!(
            prepare_usage(UsageSource::Claude, &roots(&dir), 1_700_000_000_000).unwrap_err(),
            UsageError::LoginExpired
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grok_prefers_auth_xai_then_falls_back_and_parses_credits() {
        let dir = std::env::temp_dir().join(format!("lanwork-usage-grok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let preferred = "grok-preferred-token";
        let fallback = "grok-fallback-token";
        write_auth(
            &dir,
            UsageSource::Grok,
            &format!(
                r#"{{"https://accounts.x.ai/sign-in":{{"key":"{fallback}","expires_at":"2099-01-01T00:00:00Z"}},"https://auth.x.ai/oauth/device":{{"key":"{preferred}","expires_at":4102444800000}}}}"#
            ),
        );
        let call = prepare_usage(UsageSource::Grok, &roots(&dir), 1_700_000_000_000).unwrap();
        assert_eq!(call.outgoing.url, GROK_USAGE_URL);
        assert!(call.outgoing.headers.iter().any(|(name, value)| {
            name == "Authorization" && value == &format!("Bearer {preferred}")
        }));
        assert!(
            call.outgoing
                .headers
                .iter()
                .any(|(name, value)| { name == "X-XAI-Token-Auth" && value == "xai-grok-cli" })
        );
        let debug = format!("{:?}", call.outgoing);
        assert!(!debug.contains(preferred), "{debug}");
        assert!(debug.contains("<redacted>"));
        let body = r#"{"config":{"creditUsagePercent":42.5,"currentPeriod":{"end":"not-a-time"},"billingPeriodEnd":1577836800},"onDemandUsed":{"val":9},"onDemandCap":{"val":10}}"#;
        let snapshot = interpret_usage(UsageSource::Grok, 200, body, &call.sensitive).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].name, "额度");
        assert!((snapshot.windows[0].used_percent - 42.5).abs() < 1e-9);
        assert_eq!(snapshot.windows[0].resets_at_ms, Some(1_577_836_800_000));
        let ratio = r#"{"onDemandUsed":{"val":"1"},"onDemandCap":{"val":4}}"#;
        let ratio = interpret_usage(UsageSource::Grok, 200, ratio, &[]).unwrap();
        assert!((ratio.windows[0].used_percent - 25.0).abs() < 1e-9);
        assert!(ratio.windows[0].resets_at_ms.is_none());

        write_auth(
            &dir,
            UsageSource::Grok,
            &format!(
                r#"{{"https://auth.x.ai/old":{{"key":"{preferred}","expires_at":1000}},"https://accounts.x.ai/sign-in":{{"key":"{fallback}","expires_at":"2099-01-01T00:00:00Z"}}}}"#
            ),
        );
        let call = prepare_usage(UsageSource::Grok, &roots(&dir), 1_700_000_000_000).unwrap();
        assert!(call.outgoing.headers.iter().any(|(name, value)| {
            name == "Authorization" && value == &format!("Bearer {fallback}")
        }));
        write_auth(
            &dir,
            UsageSource::Grok,
            r#"{"https://accounts.x.ai/other":{"key":"nope","expires_at":4102444800000}}"#,
        );
        assert_eq!(
            prepare_usage(UsageSource::Grok, &roots(&dir), 1_700_000_000_000).unwrap_err(),
            UsageError::LoginExpired
        );
        let evil = "grok-evil-token";
        write_auth(
            &dir,
            UsageSource::Grok,
            &format!(
                r#"{{"https://auth.x.ai.evil.example":{{"key":"{evil}","expires_at":4102444800000}},"https://accounts.x.ai/sign-in.evil":{{"key":"{evil}","expires_at":4102444800000}},"https://accounts.x.ai/sign-in":{{"key":"{fallback}","expires_at":"2099-01-01T00:00:00Z"}}}}"#
            ),
        );
        let call = prepare_usage(UsageSource::Grok, &roots(&dir), 1_700_000_000_000).unwrap();
        assert!(call.outgoing.headers.iter().any(|(name, value)| {
            name == "Authorization" && value == &format!("Bearer {fallback}")
        }));
        assert!(
            call.outgoing
                .headers
                .iter()
                .all(|(_, value)| !value.contains(evil))
        );
        write_auth(
            &dir,
            UsageSource::Grok,
            &format!(
                r#"{{"https://auth.x.ai.evil.example":{{"key":"{evil}","expires_at":4102444800000}}}}"#
            ),
        );
        assert_eq!(
            prepare_usage(UsageSource::Grok, &roots(&dir), 1_700_000_000_000).unwrap_err(),
            UsageError::LoginExpired
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_returns_the_normalized_result_until_the_interval_passes() {
        reset_usage_cache_for_test();
        let snapshot = UsageSnapshot {
            plan: Some("plus".to_string()),
            limit_reached: None,
            windows: Vec::new(),
        };
        assert!(cached_usage(UsageSource::Chatgpt).is_none());
        store_usage(UsageSource::Chatgpt, Ok(snapshot.clone()));
        assert_eq!(
            cached_usage(UsageSource::Chatgpt).unwrap().unwrap().plan,
            snapshot.plan
        );
        assert!(cached_usage(UsageSource::Claude).is_none());
        age_usage_cache_for_test(Duration::from_secs(61));
        assert!(cached_usage(UsageSource::Chatgpt).is_none());
        store_usage(UsageSource::Grok, Err(UsageError::LoginExpired));
        assert_eq!(
            cached_usage(UsageSource::Grok).unwrap().unwrap_err(),
            UsageError::LoginExpired
        );
        reset_usage_cache_for_test();
    }

    #[test]
    fn confirm_lines_are_fixed() {
        assert_eq!(UsageSource::parse("chatgpt"), Some(UsageSource::Chatgpt));
        assert!(UsageSource::parse("ChatGPT").is_none());
        assert_eq!(UsageSource::Claude.confirm_line(), "使用本机的 Claude 登录");
        assert_eq!(
            UsageSource::Chatgpt.confirm_line(),
            "使用本机的 ChatGPT 登录"
        );
        assert_eq!(UsageSource::Grok.confirm_line(), "使用本机的 Grok 登录");
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2020-01-01T00:00:00Z"),
            Some(1_577_836_800_000)
        );
    }
}
