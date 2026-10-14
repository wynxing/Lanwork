//! 允许访问的地址。下载实现在 `crates/app`，这里只判断地址能不能用。

use std::fmt;
use std::time::Duration;

use super::version::Version;

/// 最新正式版上的 `latest.json`。不经过 `api.github.com`。
pub const LATEST_MANIFEST_URL: &str =
    "https://github.com/wynxing/Lanwork/releases/latest/download/latest.json";

const DOWNLOAD_PREFIX: &str = "/wynxing/Lanwork/releases/download/v";

const CDN_HOSTS: [&str; 2] = [
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];

/// `latest.json` 正文的上限。
pub const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;

/// 安装包上限。超过就不继续下载。
pub const MAX_INSTALLER_BYTES: u64 = 200 * 1024 * 1024;

/// 发行说明保留的 Unicode 标量值个数。
pub const NOTES_CHARS: usize = 8_000;

/// 重定向最多跟这么多次。
pub const MAX_REDIRECTS: u32 = 5;

/// 启动后第一次检查前等这么久，让事件循环先跑起来。
pub const STARTUP_DELAY: Duration = Duration::from_secs(1);

/// 进程一直开着时，两次自动检查的间隔。
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 日志里 HTTP 原因最多保留这么多个标量值。
const REASON_CHARS: usize = 160;

/// 读取失败。文字里没有地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Network,
    TooLarge,
    Disk,
    /// 用 [`TransportError::http`] 生成。`reason` 里已经去掉网址。
    Http {
        status: u16,
        reason: String,
    },
}

impl TransportError {
    /// HTTP 失败。记下状态码，并从正文抽出一句原因。网址不进入原因。
    #[must_use]
    pub fn http(status: u16, body: &[u8]) -> Self {
        Self::Http {
            status,
            reason: brief_http_reason(status, body),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network => write!(f, "网络"),
            Self::TooLarge => write!(f, "过大"),
            Self::Disk => write!(f, "写入临时文件失败"),
            Self::Http { status, reason } => write!(f, "http {status}，{reason}"),
        }
    }
}

/// 每个请求都带上的头。`User-Agent` 不能空：GitHub 的 REST API 对空头回 403。
///
/// `accept` 里有控制字符时返回 `None`，调用方不要发这个请求。
#[must_use]
pub fn request_headers(accept: &str) -> Option<String> {
    let agent = format!("Lanwork/{}", super::version::current_version());
    if !header_value_ok(accept) || !header_value_ok(&agent) {
        return None;
    }
    Some(format!("User-Agent: {agent}\r\nAccept: {accept}\r\n"))
}

/// 读取一个 HTTPS 正文。
///
/// 实现要拒绝允许名单之外的地址和重定向，并且不超过 `limit`。错误里不要放地址。
pub trait GetBytes {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, TransportError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsUrl {
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

/// 只解析 `https`。带用户信息、反斜杠或非 443 以外的写法留给调用方再筛。
#[must_use]
pub fn parse_https_url(raw: &str) -> Option<HttpsUrl> {
    let rest = raw.strip_prefix("https://")?;
    if rest.is_empty() || rest.contains('@') || rest.contains('\\') || rest.contains(' ') {
        return None;
    }
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || path.is_empty() {
        return None;
    }
    let (host, port) = if let Some((host, port_text)) = authority.rsplit_once(':') {
        if host.is_empty() || port_text.is_empty() || host.contains(':') {
            return None;
        }
        (host, port_text.parse::<u16>().ok()?)
    } else {
        (authority, 443)
    };
    if !host_ok(host) {
        return None;
    }
    Some(HttpsUrl {
        host: host.to_ascii_lowercase(),
        port,
        path_and_query: path.to_owned(),
    })
}

/// 安装包第一跳。标签路径是 `v<版本>`，文件名是 `Lanwork-Setup-<版本>.exe`。
#[must_use]
pub fn pinned_installer_url(version: Version) -> String {
    format!(
        "https://github.com/wynxing/Lanwork/releases/download/v{version}/{}",
        installer_file_name(version)
    )
}

/// `latest.json` 第一跳。和安装包用同一个标签路径。
#[must_use]
pub fn pinned_latest_json_url(version: Version) -> String {
    format!("https://github.com/wynxing/Lanwork/releases/download/v{version}/latest.json")
}

/// 第一跳可以请求的地址。只有最新正式版的 `latest.json`，以及两个钉死的版本路径。
///
/// 不接受查询串、片段、`..` 和任何别的路径。CDN 主机不能当第一跳。不请求 `api.github.com`。
#[must_use]
pub fn url_allowed(url: &str) -> bool {
    url == LATEST_MANIFEST_URL || pinned_asset_url(url)
}

/// 重定向之后可以继续请求的地址。
///
/// `releases/latest/download/latest.json` 的下一跳是钉死的 `latest.json` 版本路径，再下一跳才是
/// CDN。安装包第一跳的下一跳直接是 CDN。路径里的 `..`、反斜杠和百分号编码后再变成 `..` 的都拒绝。
#[must_use]
pub fn redirect_allowed(url: &str) -> bool {
    cdn_redirect(url) || pinned_asset_url(url)
}

fn pinned_asset_url(url: &str) -> bool {
    let Some(version) = pinned_download_version(url) else {
        return false;
    };
    url == pinned_installer_url(version) || url == pinned_latest_json_url(version)
}

fn cdn_redirect(url: &str) -> bool {
    let Some(parsed) = parse_https_url(url) else {
        return false;
    };
    if parsed.port != 443 || !CDN_HOSTS.contains(&parsed.host.as_str()) {
        return false;
    }
    if contains_dotdot(&parsed.path_and_query) {
        return false;
    }
    let path = parsed.path_and_query.split(['?', '#']).next().unwrap_or("");
    path.starts_with('/') && path != "/"
}

/// 可以交给系统打开的发布页。
#[must_use]
pub fn release_page_allowed(url: &str) -> bool {
    let Some(parsed) = parse_https_url(url) else {
        return false;
    };
    parsed.port == 443
        && parsed.host == "github.com"
        && !contains_dotdot(&parsed.path_and_query)
        && !parsed.path_and_query.contains('?')
        && !parsed.path_and_query.contains('#')
        && parsed
            .path_and_query
            .starts_with("/wynxing/Lanwork/releases/")
}

/// 把重定向的 `Location` 收成绝对 `https` 地址。相对路径只接受以 `/` 开头的。
#[must_use]
pub fn resolve_redirect(current: &str, location: &str) -> Option<String> {
    let location = location.trim();
    if location.is_empty()
        || location
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return None;
    }
    if location.starts_with("https://") {
        parse_https_url(location)?;
        return Some(location.to_owned());
    }
    if location.starts_with("http://") || !location.starts_with('/') {
        return None;
    }
    let base = parse_https_url(current)?;
    if base.port != 443 {
        return None;
    }
    Some(format!("https://{}{location}", base.host))
}

#[must_use]
pub fn installer_file_name(version: Version) -> String {
    format!("Lanwork-Setup-{version}.exe")
}

fn pinned_download_version(url: &str) -> Option<Version> {
    let parsed = parse_https_url(url)?;
    if parsed.port != 443 || parsed.host != "github.com" {
        return None;
    }
    if contains_dotdot(&parsed.path_and_query) {
        return None;
    }
    if parsed.path_and_query.contains('?') || parsed.path_and_query.contains('#') {
        return None;
    }
    let rest = parsed.path_and_query.strip_prefix(DOWNLOAD_PREFIX)?;
    let (version_text, file) = rest.split_once('/')?;
    if file.is_empty() || file.contains('/') {
        return None;
    }
    let version = Version::parse(version_text)?;
    if version_text != version.to_string() {
        return None;
    }
    if file == installer_file_name(version) || file == "latest.json" {
        Some(version)
    } else {
        None
    }
}

/// `..` 本身，以及百分号编码一次或两次之后会变成 `..` 的写法。
fn contains_dotdot(text: &str) -> bool {
    let mut current = text.to_owned();
    for _ in 0..3 {
        if current.contains("..") || current.contains('\\') {
            return true;
        }
        let decoded = percent_decode(&current);
        if decoded == current {
            return false;
        }
        current = decoded;
    }
    current.contains("..") || current.contains('\\')
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (from_hex(bytes[index + 1]), from_hex(bytes[index + 2]))
        {
            out.push((hi << 4) | lo);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn header_value_ok(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(|ch| ch.is_control())
}

fn brief_http_reason(status: u16, body: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(body).chars().take(4096).collect();
    let extracted = json_message(&text).unwrap_or(text);
    let cleaned = tidy_reason(&strip_urls(&strip_markup(&extracted)));
    if cleaned.chars().any(|ch| ch.is_alphanumeric()) {
        truncate_reason(&cleaned)
    } else {
        fallback_reason(status).to_owned()
    }
}

fn json_message(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let message = value.get("message")?.as_str()?.trim();
    (!message.is_empty()).then(|| message.to_owned())
}

fn strip_markup(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn strip_urls(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    loop {
        let http = rest.find("http://");
        let https = rest.find("https://");
        let start = match (http, https) {
            (Some(left), Some(right)) => left.min(right),
            (Some(left), None) => left,
            (None, Some(right)) => right,
            (None, None) => {
                out.push_str(rest);
                break;
            }
        };
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after
            .char_indices()
            .find(|(_, ch)| {
                ch.is_whitespace() || matches!(*ch, '<' | '>' | '"' | '\'' | ')' | ']' | '}')
            })
            .map(|(index, _)| index)
            .unwrap_or(after.len());
        if end == 0 {
            out.push_str(&after[..after.chars().next().map(char::len_utf8).unwrap_or(1)]);
            rest = &after[after.chars().next().map(char::len_utf8).unwrap_or(1)..];
            continue;
        }
        rest = &after[end..];
    }
    out
}

fn tidy_reason(text: &str) -> String {
    let text = collapse_ws(&text.replace("()", ""));
    text.replace(" .", ".").replace(" ,", ",").trim().to_owned()
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch.is_control() {
            pending = !out.is_empty();
            continue;
        }
        if pending {
            out.push(' ');
            pending = false;
        }
        out.push(ch);
    }
    out
}

fn truncate_reason(text: &str) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= REASON_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn fallback_reason(status: u16) -> &'static str {
    match status {
        401 | 403 => "拒绝访问",
        404 => "没有这个文件",
        408 => "超时",
        429 => "请求过多",
        500..=599 => "服务器错误",
        _ => "请求失败",
    }
}

fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.starts_with('-')
        && !host.contains("..")
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_pinned_download_urls_are_a_first_hop() {
        let version = Version::new(0, 3, 0);
        let installer = pinned_installer_url(version);
        let latest = pinned_latest_json_url(version);
        assert_eq!(
            installer,
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe"
        );
        assert_eq!(
            latest,
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/latest.json"
        );
        assert_eq!(
            LATEST_MANIFEST_URL,
            "https://github.com/wynxing/Lanwork/releases/latest/download/latest.json"
        );
        assert!(url_allowed(LATEST_MANIFEST_URL));
        assert!(url_allowed(&installer));
        assert!(url_allowed(&latest));
        assert!(redirect_allowed(&latest));
        assert!(redirect_allowed(&installer));
        assert!(!redirect_allowed(LATEST_MANIFEST_URL));
        assert!(!redirect_allowed(
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/latest.json?x=1"
        ));
        assert!(!redirect_allowed(
            "https://api.github.com/repos/wynxing/Lanwork/releases/latest"
        ));
        assert!(!url_allowed(
            "https://api.github.com/repos/wynxing/Lanwork/releases/latest"
        ));
        assert!(release_page_allowed(
            "https://github.com/wynxing/Lanwork/releases/tag/v0.3.0"
        ));
        let cdn =
            "https://release-assets.githubusercontent.com/github-production-release-asset/abcd";
        let objects =
            "https://objects.githubusercontent.com/github-production-release-asset/abcd?name=1";
        assert!(!url_allowed(cdn));
        assert!(!url_allowed(objects));
        assert!(redirect_allowed(cdn));
        assert!(redirect_allowed(objects));
        for url in [
            "http://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe",
            "https://github.com/other/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe",
            "https://github.com.evil/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe",
            "https://evil.github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe",
            "https://user:pass@github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe",
            "https://github.com:444/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe",
            "https://example.com/Lanwork-Setup-0.3.0.exe",
            "https://api.github.com/repos/wynxing/Lanwork/releases/latest",
            "https://api.github.com/repos/wynxing/Lanwork/releases/latest/extra",
            "https://api.github.com/repos/other/Lanwork/releases/latest",
            "https://github.com/wynxing/Lanwork/releases/latest/download/latest.json/extra",
            "https://github.com/wynxing/Lanwork/releases/latest/download/other.json",
            "https://github.com/wynxing/Lanwork/releases/latest/download/latest.json?x=1",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/other.exe",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe/extra",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/../Lanwork-Setup-0.3.0.exe",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/%2e%2e/Lanwork-Setup-0.3.0.exe",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/%252e%252e/latest.json",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe?x=1",
            "https://github.com/wynxing/Lanwork/Lanwork-Setup-0.3.0.exe",
            "https://objects.githubusercontent.com/a/../../secret",
            "https://release-assets.githubusercontent.com/a/%2e%2e/secret",
            "https://evil.example/setup.exe",
        ] {
            assert!(!url_allowed(url), "{url}");
        }
        assert!(!redirect_allowed(
            "https://objects.githubusercontent.com/a/../../secret"
        ));
        assert!(!redirect_allowed(
            "https://release-assets.githubusercontent.com/a/%2e%2e/secret"
        ));
        assert!(!redirect_allowed("https://evil.example/setup.exe"));
        assert!(!release_page_allowed(
            "https://github.com/wynxing/Lanwork/issues/31"
        ));
        assert!(!release_page_allowed(
            "https://github.com/wynxing/Lanwork/releases/../releases/tag/v0.3.0"
        ));
    }

    #[test]
    fn redirects_stay_on_https_and_absolute_paths() {
        let current = "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/setup.exe";
        assert_eq!(
            resolve_redirect(current, "https://objects.githubusercontent.com/a"),
            Some("https://objects.githubusercontent.com/a".to_owned())
        );
        assert_eq!(
            resolve_redirect(
                current,
                "/wynxing/Lanwork/releases/download/v0.3.0/setup.exe"
            ),
            Some(
                "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/setup.exe".to_owned()
            )
        );
        assert_eq!(resolve_redirect(current, "http://github.com/a"), None);
        assert_eq!(resolve_redirect(current, "setup.exe"), None);
        assert_eq!(resolve_redirect(current, "https://evil.com/a b"), None);
    }

    #[test]
    fn requests_name_the_app_and_keep_a_user_agent() {
        let headers = request_headers("*/*").expect("headers");
        let version = super::super::version::current_version();
        assert_eq!(
            headers,
            format!("User-Agent: Lanwork/{version}\r\nAccept: */*\r\n")
        );
        assert!(request_headers("application/json").is_some());
        assert!(request_headers("").is_none());
        assert!(request_headers("bad\r\nX-Injected: 1").is_none());
    }

    #[test]
    fn http_failure_records_status_and_a_short_reason_without_a_url() {
        let missing_agent = b"Request forbidden by administrative rules. Please make sure your request has a User-Agent header (https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#user-agent-required). Check https://developer.github.com for other possible causes.";
        let missing = TransportError::http(403, missing_agent);
        let text = missing.to_string();
        assert!(text.starts_with("http 403，"), "{text}");
        assert!(text.contains("User-Agent"), "{text}");
        assert!(!text.contains("http://"), "{text}");
        assert!(!text.contains("https://"), "{text}");

        let rate = br#"{"message":"API rate limit exceeded for 203.0.113.4. (But here's the good news: Authenticated requests get a higher rate limit. Check out the documentation for more details.)","documentation_url":"https://docs.github.com/rest/overview/resources-in-the-rest-api#rate-limiting"}"#;
        let limited = TransportError::http(403, rate).to_string();
        assert!(
            limited.contains("http 403，API rate limit exceeded"),
            "{limited}"
        );
        assert!(!limited.contains("https://"), "{limited}");
        assert!(!limited.contains("documentation_url"), "{limited}");

        assert_eq!(
            TransportError::http(403, b"").to_string(),
            "http 403，拒绝访问"
        );
        assert_eq!(
            TransportError::http(404, b"   ").to_string(),
            "http 404，没有这个文件"
        );
        assert_eq!(
            TransportError::http(429, b"<html></html>").to_string(),
            "http 429，请求过多"
        );
        assert_eq!(
            TransportError::http(503, b"<html><body>unavailable</body></html>").to_string(),
            "http 503，unavailable"
        );
        let only_url = TransportError::http(403, b"https://evil.example/secret");
        assert_eq!(only_url.to_string(), "http 403，拒绝访问");
        let long = format!("字{}", "测".repeat(REASON_CHARS));
        let truncated = TransportError::http(400, long.as_bytes()).to_string();
        assert!(truncated.ends_with('…'), "{truncated}");
        assert!(!truncated.contains("https://"));
    }

    #[test]
    fn schedule_checks_after_startup_and_then_once_a_day() {
        assert_eq!(STARTUP_DELAY, Duration::from_secs(1));
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(24 * 60 * 60));
        assert!(STARTUP_DELAY < CHECK_INTERVAL);
    }
}
