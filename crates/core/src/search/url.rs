//! 判断输入是否像网址，并给出默认浏览器要打开的地址。
//!
//! 规则是实现选择，见 architecture.md「打开网址」。不是产品规则。

/// 「打开网址」结果的名称。
pub const OPEN_WEBSITE_LABEL: &str = "打开网址";

/// 输入像网址时返回要打开的地址。没写协议时补成 `https://`。
///
/// 去掉首尾空白后再判断。中间有空白、其它协议，或主机不像域名、`localhost`、
/// IPv4、方括号 IPv6 时返回 `None`。
#[must_use]
pub fn open_website(input: &str) -> Option<String> {
    let text = input.trim();
    if text.is_empty() || has_forbidden(text) {
        return None;
    }
    let (scheme, rest) = split_scheme(text)?;
    if !remainder_ok(rest) {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

fn has_forbidden(text: &str) -> bool {
    text.chars().any(|ch| {
        ch.is_whitespace() || ch.is_control() || matches!(ch, '@' | '\\' | '<' | '>' | '"')
    })
}

/// 有 `://` 时只接受 `http` / `https`。没有时按未写协议，调用方补 `https`。
fn split_scheme(text: &str) -> Option<(&'static str, &str)> {
    let Some(index) = text.find("://") else {
        return Some(("https", text));
    };
    let scheme = &text[..index];
    let rest = &text[index + 3..];
    if scheme.eq_ignore_ascii_case("https") {
        Some(("https", rest))
    } else if scheme.eq_ignore_ascii_case("http") {
        Some(("http", rest))
    } else {
        None
    }
}

fn remainder_ok(rest: &str) -> bool {
    let Some((authority, tail)) = split_authority(rest) else {
        return false;
    };
    let Some((host, port)) = split_host_port(authority) else {
        return false;
    };
    if !host_ok(host) {
        return false;
    }
    if port.is_some_and(|port| !port_ok(port)) {
        return false;
    }
    tail.is_empty() || tail.starts_with(['/', '?', '#'])
}

fn split_authority(rest: &str) -> Option<(&str, &str)> {
    if rest.is_empty() {
        return None;
    }
    let cut = if rest.starts_with('[') {
        let end = rest.find(']')?;
        let after = end + 1;
        if rest.as_bytes().get(after) == Some(&b':') {
            let port_start = after + 1;
            rest[port_start..]
                .find(['/', '?', '#'])
                .map_or(rest.len(), |index| port_start + index)
        } else {
            after
        }
    } else {
        rest.find(['/', '?', '#']).unwrap_or(rest.len())
    };
    if cut == 0 {
        return None;
    }
    Some((&rest[..cut], &rest[cut..]))
}

fn split_host_port(authority: &str) -> Option<(&str, Option<&str>)> {
    if let Some(host) = authority.strip_prefix('[') {
        let end = host.find(']')?;
        let after = &host[end + 1..];
        if after.is_empty() {
            return Some((&authority[..=end + 1], None));
        }
        let port = after.strip_prefix(':')?;
        if port.is_empty() {
            return None;
        }
        return Some((&authority[..=end + 1], Some(port)));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() || port.is_empty() || host.contains(':') {
                None
            } else {
                Some((host, Some(port)))
            }
        }
        None => Some((authority, None)),
    }
}

fn host_ok(host: &str) -> bool {
    if let Some(inside) = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        return valid_ipv6(inside);
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    ipv4_ok(host) || domain_ok(host)
}

fn ipv4_ok(host: &str) -> bool {
    let mut count = 0;
    for part in host.split('.') {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Ok(value) = part.parse::<u16>() else {
            return false;
        };
        if value > 255 {
            return false;
        }
        count += 1;
    }
    count == 4
}

fn domain_ok(host: &str) -> bool {
    if host.len() > 253 || !host.contains('.') {
        return false;
    }
    let mut count = 0;
    let mut last = "";
    for label in host.split('.') {
        if !label_ok(label) {
            return false;
        }
        last = label;
        count += 1;
    }
    count >= 2 && last.len() >= 2 && last.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn label_ok(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

fn port_ok(port: &str) -> bool {
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    matches!(port.parse::<u16>(), Ok(value) if (1..=65535).contains(&value))
}

fn valid_ipv6(text: &str) -> bool {
    if text.is_empty()
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b':')
    {
        return false;
    }
    match text.matches("::").count() {
        0 => ipv6_group_count(text) == Some(8),
        1 => {
            let Some((left, right)) = text.split_once("::") else {
                return false;
            };
            let left_count = if left.is_empty() {
                Some(0)
            } else {
                ipv6_group_count(left)
            };
            let right_count = if right.is_empty() {
                Some(0)
            } else {
                ipv6_group_count(right)
            };
            matches!(
                (left_count, right_count),
                (Some(left), Some(right)) if left + right < 8
            )
        }
        _ => false,
    }
}

fn ipv6_group_count(text: &str) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    let mut count = 0;
    for group in text.split(':') {
        if group.is_empty()
            || group.len() > 4
            || !group.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        count += 1;
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::{OPEN_WEBSITE_LABEL, open_website};

    #[test]
    fn label_is_open_website() {
        assert_eq!(OPEN_WEBSITE_LABEL, "打开网址");
    }

    #[test]
    fn accepts_websites_and_keeps_a_written_scheme() {
        let cases = [
            ("cursor.com", "https://cursor.com"),
            ("  cursor.com  ", "https://cursor.com"),
            ("cursor.com/", "https://cursor.com/"),
            ("cursor.com/dashboard", "https://cursor.com/dashboard"),
            (
                "https://cursor.com/dashboard",
                "https://cursor.com/dashboard",
            ),
            (
                "  HTTPS://Cursor.COM/Dashboard  ",
                "https://Cursor.COM/Dashboard",
            ),
            ("http://cursor.com", "http://cursor.com"),
            ("HTTP://Cursor.COM/a", "http://Cursor.COM/a"),
            ("cursor.com:8080/a?q=1#x", "https://cursor.com:8080/a?q=1#x"),
            ("cursor.com?q=1", "https://cursor.com?q=1"),
            ("localhost", "https://localhost"),
            ("LocalHost:3000", "https://LocalHost:3000"),
            ("127.0.0.1", "https://127.0.0.1"),
            ("http://127.0.0.1:8080/a", "http://127.0.0.1:8080/a"),
            ("[::1]", "https://[::1]"),
            ("https://[2001:db8::1]:443/a", "https://[2001:db8::1]:443/a"),
            ("https://cursor.com/文档", "https://cursor.com/文档"),
            ("t.co", "https://t.co"),
            ("my-site.com", "https://my-site.com"),
            ("0.0.0.0", "https://0.0.0.0"),
            ("cursor.com:65535", "https://cursor.com:65535"),
            ("https://[::]", "https://[::]"),
            ("https://[1:2:3:4:5:6:7:8]", "https://[1:2:3:4:5:6:7:8]"),
        ];
        for (input, url) in cases {
            assert_eq!(open_website(input).as_deref(), Some(url), "{input}");
        }
    }

    #[test]
    fn single_dot_filename_currently_opens_because_that_case_is_undecided() {
        assert_eq!(
            open_website("readme.md").as_deref(),
            Some("https://readme.md")
        );
    }

    #[test]
    fn rejects_non_websites() {
        for input in [
            "",
            "   ",
            "cursor .com",
            "https://cursor.com/a b",
            "not-a-url",
            "com",
            "a.b",
            "http://",
            "https://",
            "http://x",
            "htp://x",
            "file:///C:/Windows",
            "ftp://example.com",
            "javascript:alert(1)",
            "steam://run/570",
            "name@cursor.com",
            "https://user:pass@cursor.com",
            "cursor.com:0",
            "cursor.com:65536",
            "cursor.com:",
            "256.1.1.1",
            "1.2.3",
            ".com",
            "cursor.com.",
            "-bad.com",
            "bad-.com",
            "https://例子.中国",
            "例子.中国",
            "//cursor.com",
            "http:cursor.com",
            "::1",
            "https://[::1::2]",
            "https://[gggg::1]",
            "https://[fe80::1%1]",
            r"cursor.com\path",
            "https://cursor.com/<script>",
            "a..com",
        ] {
            assert_eq!(open_website(input), None, "{input}");
        }
    }
}
