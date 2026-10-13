//! 允许访问的地址。下载实现在 `crates/app`，这里只判断地址能不能用。

use std::fmt;
use std::time::Duration;

use super::version::Version;

/// GitHub 上 Lanwork 的最新 Release。
pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/wynxing/Lanwork/releases/latest";

/// `latest.json` 和 Release API 正文的上限。
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

/// 读取失败。文字里没有地址。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    Network,
    TooLarge,
    Disk,
    Http(u16),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network => write!(f, "网络"),
            Self::TooLarge => write!(f, "过大"),
            Self::Disk => write!(f, "写入临时文件失败"),
            Self::Http(code) => write!(f, "http {code}"),
        }
    }
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

/// 安装包、`latest.json` 和 Release API 可以下载的地址。
#[must_use]
pub fn url_allowed(url: &str) -> bool {
    let Some(parsed) = parse_https_url(url) else {
        return false;
    };
    if parsed.port != 443 {
        return false;
    }
    match parsed.host.as_str() {
        "api.github.com" => parsed.path_and_query.starts_with("/repos/wynxing/Lanwork/"),
        "github.com" => parsed.path_and_query.starts_with("/wynxing/Lanwork/"),
        "release-assets.githubusercontent.com" | "objects.githubusercontent.com" => true,
        _ => false,
    }
}

/// 可以交给系统打开的发布页。
#[must_use]
pub fn release_page_allowed(url: &str) -> bool {
    let Some(parsed) = parse_https_url(url) else {
        return false;
    };
    parsed.port == 443
        && parsed.host == "github.com"
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
    fn only_the_lanwork_release_hosts_are_allowed() {
        assert!(url_allowed(LATEST_RELEASE_URL));
        assert!(url_allowed(
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe"
        ));
        assert!(url_allowed(
            "https://release-assets.githubusercontent.com/github-production-release-asset/abcd"
        ));
        assert!(url_allowed(
            "https://objects.githubusercontent.com/github-production-release-asset/abcd"
        ));
        assert!(release_page_allowed(
            "https://github.com/wynxing/Lanwork/releases/tag/v0.3.0"
        ));
        for url in [
            "http://github.com/wynxing/Lanwork/releases/latest",
            "https://github.com/other/Lanwork/releases/latest",
            "https://github.com.evil/wynxing/Lanwork/releases/latest",
            "https://evil.github.com/wynxing/Lanwork/releases/latest",
            "https://user:pass@github.com/wynxing/Lanwork/releases/latest",
            "https://github.com:444/wynxing/Lanwork/releases/latest",
            "https://example.com/Lanwork-Setup-0.3.0.exe",
            "https://api.github.com/repos/other/Lanwork/releases/latest",
        ] {
            assert!(!url_allowed(url), "{url}");
        }
        assert!(!release_page_allowed(
            "https://github.com/wynxing/Lanwork/issues/31"
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
    fn schedule_checks_after_startup_and_then_once_a_day() {
        assert_eq!(STARTUP_DELAY, Duration::from_secs(1));
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(24 * 60 * 60));
        assert!(STARTUP_DELAY < CHECK_INTERVAL);
    }
}
