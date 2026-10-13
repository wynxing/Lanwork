//! 稳定版本号。只接受三段数字，用来和 GitHub Release 的标签比较。

use std::fmt;

/// `MAJOR.MINOR.PATCH`。比较按数字，不按字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    #[must_use]
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// 接受 `0.2.1` 和 `v0.2.1`。预发布、多出来的一段、前导零都不是稳定版本。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text
            .strip_prefix('v')
            .or_else(|| text.strip_prefix('V'))
            .unwrap_or(text);
        let mut parts = text.split('.');
        let major = parse_component(parts.next()?)?;
        let minor = parse_component(parts.next()?)?;
        let patch = parse_component(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 正在运行的程序版本，来自 Cargo workspace。
#[must_use]
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0))
}

fn parse_component(text: &str) -> Option<u64> {
    if text.is_empty() || text.len() > 9 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stable_versions_and_rejects_the_rest() {
        assert_eq!(Version::parse("0.2.0"), Some(Version::new(0, 2, 0)));
        assert_eq!(Version::parse("v0.10.0"), Some(Version::new(0, 10, 0)));
        assert_eq!(Version::parse("  V1.2.3  "), Some(Version::new(1, 2, 3)));
        assert!(Version::new(0, 2, 0) < Version::new(0, 2, 1));
        assert!(Version::new(0, 9, 0) < Version::new(0, 10, 0));
        assert_eq!(Version::new(0, 2, 0).to_string(), "0.2.0");
        for text in [
            "",
            "0.2",
            "0.2.0.1",
            "0.2.0-rc1",
            "v0.02.0",
            "1.2.3-alpha",
            "vv1.0.0",
        ] {
            assert_eq!(Version::parse(text), None, "{text}");
        }
    }

    #[test]
    fn current_version_matches_the_package() {
        assert_eq!(
            current_version(),
            Version::parse(env!("CARGO_PKG_VERSION")).unwrap()
        );
    }
}
