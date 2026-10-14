//! 网页引用的网址规范化和拖放挑选。
//!
//! 不读文件，不调用 Win32。剪贴板字节的解析沿用已通过拖放技术验证的做法。
//! 几种已接受格式同时出现时先用哪一个，产品规格仍待定；这里的顺序是实现选择，写在架构文档里。

/// 比较用的键，以及去掉首尾空白后的原文。原文不改大小写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedWebUrl {
    pub key: String,
    pub stored: String,
}

/// 一次拖放里已经读出的内容。没有的格式是 `None`。
///
/// 不包含 `FileContents` 和 `CF_HDROP`。调用方不得为了填这个结构去读它们。
#[derive(Debug, Clone, Copy, Default)]
pub struct WebDropInput<'a> {
    pub html: Option<&'a [u8]>,
    pub url_w: Option<&'a str>,
    pub moz_url: Option<&'a str>,
    pub unicode_text: Option<&'a str>,
    pub descriptor_names: &'a [String],
}

/// 准备写入收纳的一条网页引用。`url` 是挑中的原文，`title` 可能为空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedWebRef {
    pub url: String,
    pub title: String,
}

/// 比较前的三步：去掉首尾空白，协议和主机名小写，去掉路径末尾的一个 `/`。
///
/// 问号或井号前面的那个斜杠也算。路径其余大小写、`www`、端口、用户名、查询和片段不改。
/// 没有方案、没有主机、或中间有空白时不接受。这是实现选择，产品规格里「怎样才算网址」仍待定。
#[must_use]
pub fn normalize_web_url(raw: &str) -> Option<NormalizedWebUrl> {
    let stored = raw.trim();
    if stored.is_empty()
        || stored.chars().count() > 16_384
        || stored.chars().any(char::is_whitespace)
    {
        return None;
    }
    let scheme_end = stored.find("://")?;
    if scheme_end == 0 {
        return None;
    }
    let scheme = &stored[..scheme_end];
    if !scheme
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return None;
    }
    let after = &stored[scheme_end + 3..];
    if after.is_empty() {
        return None;
    }
    let host_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..host_end];
    if authority.is_empty() {
        return None;
    }
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(at) => (&authority[..=at], &authority[at + 1..]),
        None => ("", authority),
    };
    let (hostname, port) = split_host_port(hostport)?;
    if hostname.is_empty() {
        return None;
    }
    let mut key = String::with_capacity(stored.len());
    key.push_str(&scheme.to_ascii_lowercase());
    key.push_str("://");
    key.push_str(userinfo);
    key.push_str(&hostname.to_ascii_lowercase());
    key.push_str(port);
    key.push_str(&strip_one_slash(&after[host_end..]));
    Some(NormalizedWebUrl {
        key,
        stored: stored.to_owned(),
    })
}

/// 没有以 `.url` 结尾时没有标题。大小写不敏感。后缀前面没有字符时标题是空字符串。
#[must_use]
pub fn title_from_url_filename(name: &str) -> Option<String> {
    let trimmed = name.trim();
    let cut = trimmed.len().saturating_sub(4);
    let stem = trimmed.get(..cut)?;
    let suffix = trimmed.get(cut..)?;
    if suffix.eq_ignore_ascii_case(".url") {
        Some(stem.to_string())
    } else {
        None
    }
}

/// 第一个以 `.url` 结尾的 `FileGroupDescriptorW` 文件名，去掉这个后缀。没有时为空。
#[must_use]
pub fn title_from_descriptor_names(names: &[String]) -> String {
    names
        .iter()
        .find_map(|name| title_from_url_filename(name))
        .unwrap_or_default()
}

/// 列表和搜索里的名称。标题为空时用保存的网址。标题缺失时显示什么，产品规格仍待定，这是实现选择。
#[must_use]
pub fn web_display_name(title: &str, url: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        url.to_owned()
    } else {
        title.to_owned()
    }
}

/// 从已接受的格式里挑一条网址，标题另从描述符文件名取。
///
/// 顺序是实现选择：`CF_HTML` 的第一个能规范化的链接地址，然后 `UniformResourceLocatorW`，
/// 然后 `text/x-moz-url` 的第一行，最后整段 `CF_UNICODETEXT`。不用 `SourceURL`，不用文档标题，
/// 也不用 `text/x-moz-url` 的第二行当标题。一次只返回一条。
#[must_use]
pub fn pick_web_drop(input: &WebDropInput<'_>) -> Option<PickedWebRef> {
    let url = first_html_href(input.html)
        .or_else(|| stored_url(input.url_w))
        .or_else(|| moz_first_line(input.moz_url))
        .or_else(|| stored_url(input.unicode_text))?;
    Some(PickedWebRef {
        url,
        title: title_from_descriptor_names(input.descriptor_names),
    })
}

fn stored_url(text: Option<&str>) -> Option<String> {
    normalize_web_url(text?).map(|url| url.stored)
}

fn moz_first_line(text: Option<&str>) -> Option<String> {
    let text = text?;
    let line = text
        .split(['\n', '\r'])
        .find(|line| !line.trim().is_empty())?;
    stored_url(Some(line))
}

fn first_html_href(bytes: Option<&[u8]>) -> Option<String> {
    let parsed = parse_cf_html(bytes?);
    parsed
        .links
        .into_iter()
        .find_map(|link| stored_url(Some(&link.href)))
}

fn split_host_port(hostport: &str) -> Option<(&str, &str)> {
    if hostport.is_empty() {
        return None;
    }
    if let Some(rest) = hostport.strip_prefix('[') {
        let end = rest.find(']')?;
        let hostname = &hostport[..end + 2];
        let port = &hostport[end + 2..];
        if port.is_empty() || port.starts_with(':') {
            return Some((hostname, port));
        }
        return None;
    }
    if let Some(colon) = hostport.rfind(':') {
        Some((&hostport[..colon], &hostport[colon..]))
    } else {
        Some((hostport, ""))
    }
}

fn strip_one_slash(tail: &str) -> String {
    if let Some(split) = tail.find(['?', '#']) {
        let (path, rest) = tail.split_at(split);
        if let Some(path) = path.strip_suffix('/') {
            let mut out = String::with_capacity(tail.len());
            out.push_str(path);
            out.push_str(rest);
            return out;
        }
        return tail.to_owned();
    }
    tail.strip_suffix('/').unwrap_or(tail).to_owned()
}

/// `CF_HTML` 里的链接。文档标题不参与收纳。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlLink {
    pub href: String,
    pub text: String,
}

/// `CF_HTML` 的解析结果。`source_url` 只供核对，挑选网址时不用它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlExtract {
    pub source_url: Option<String>,
    pub links: Vec<HtmlLink>,
}

#[must_use]
pub fn parse_cf_html(bytes: &[u8]) -> HtmlExtract {
    let bytes = strip_trailing_nuls(bytes);
    let header = parse_header(bytes);
    let html = slice_range(bytes, header.start_html, header.end_html)
        .unwrap_or(&bytes[header.header_end.min(bytes.len())..]);
    let fragment = slice_range(bytes, header.start_frag, header.end_frag).unwrap_or(html);
    let mut links = extract_links(&String::from_utf8_lossy(fragment));
    if links.is_empty() && (fragment.as_ptr() != html.as_ptr() || fragment.len() != html.len()) {
        links = extract_links(&String::from_utf8_lossy(html));
    }
    HtmlExtract {
        source_url: header.source_url,
        links,
    }
}

#[must_use]
pub fn decode_utf16_text(bytes: &[u8]) -> String {
    let mut units = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        let unit = u16::from_le_bytes([bytes[index], bytes[index + 1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
        index += 2;
    }
    String::from_utf16_lossy(&units)
}

#[must_use]
pub fn encode_utf16_text(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for unit in text.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend(unit.to_le_bytes());
    }
    bytes
}

const FD_W_SIZE: usize = 592;
const NAME_OFFSET: usize = 72;
const NAME_W_BYTES: usize = 520;

/// 读 `FileGroupDescriptorW` 的文件名。不读 `FileContents`。
pub fn file_names_w(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 4 {
        return Err("descriptor shorter than count".to_string());
    }
    let count = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes")) as usize;
    let body = count
        .checked_mul(FD_W_SIZE)
        .ok_or_else(|| format!("descriptor count {count} overflows"))?;
    let need = body
        .checked_add(4)
        .ok_or_else(|| format!("descriptor count {count} overflows"))?;
    if bytes.len() < need {
        return Err(format!(
            "descriptor size {} is shorter than {need}",
            bytes.len()
        ));
    }
    let mut names = Vec::with_capacity(count);
    for index in 0..count {
        let start = 4 + index * FD_W_SIZE + NAME_OFFSET;
        let raw = &bytes[start..start + NAME_W_BYTES];
        names.push(decode_utf16_text(raw));
    }
    Ok(names)
}

#[must_use]
pub fn descriptor_w(names: &[&str]) -> Vec<u8> {
    let mut bytes = vec![0u8; 4 + names.len() * FD_W_SIZE];
    bytes[..4].copy_from_slice(&(names.len() as u32).to_le_bytes());
    for (index, name) in names.iter().enumerate() {
        let start = 4 + index * FD_W_SIZE + NAME_OFFSET;
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        for (unit_index, unit) in wide.iter().take(260).enumerate() {
            let offset = start + unit_index * 2;
            bytes[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    bytes
}

/// 造一段和 Chrome 相近的 `CF_HTML`，给测试用。
#[must_use]
pub fn chrome_like_cf_html(source_url: &str, href: &str, title: &str) -> Vec<u8> {
    let fragment = format!("<a href=\"{href}\">{title}</a>");
    let html = format!(
        "<html>\r\n<body>\r\n<!--StartFragment-->{fragment}<!--EndFragment-->\r\n</body>\r\n</html>"
    );
    let header_len = format!(
        "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\nSourceURL:{source_url}\r\n",
        0, 0, 0, 0
    )
    .len();
    let marker = "<!--StartFragment-->";
    let end_marker = "<!--EndFragment-->";
    let start_html = header_len;
    let start_fragment = start_html + html.find(marker).expect("start marker") + marker.len();
    let end_fragment = start_html + html.find(end_marker).expect("end marker");
    let end_html = start_html + html.len();
    let header = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\nSourceURL:{source_url}\r\n"
    );
    let mut out = header.into_bytes();
    out.extend(html.as_bytes());
    out
}

struct HtmlHeader {
    source_url: Option<String>,
    start_html: Option<i64>,
    end_html: Option<i64>,
    start_frag: Option<i64>,
    end_frag: Option<i64>,
    header_end: usize,
}

fn parse_header(bytes: &[u8]) -> HtmlHeader {
    let mut offset = 0;
    let mut source_url = None;
    let mut start_html = None;
    let mut end_html = None;
    let mut start_frag = None;
    let mut end_frag = None;
    while offset < bytes.len() {
        if bytes[offset] == b'<' {
            break;
        }
        let end = bytes[offset..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| offset + index + 1)
            .unwrap_or(bytes.len());
        let mut line = &bytes[offset..end];
        if line.last() == Some(&b'\n') {
            line = &line[..line.len() - 1];
        }
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if let Some(colon) = line.iter().position(|byte| *byte == b':') {
            let key = &line[..colon];
            let value = trim_ascii(&line[colon + 1..]);
            match key {
                b"SourceURL" => source_url = Some(String::from_utf8_lossy(value).into_owned()),
                b"StartHTML" => start_html = parse_offset(value),
                b"EndHTML" => end_html = parse_offset(value),
                b"StartFragment" => start_frag = parse_offset(value),
                b"EndFragment" => end_frag = parse_offset(value),
                _ => {}
            }
        } else if !trim_ascii(line).is_empty() {
            break;
        }
        offset = end;
    }
    HtmlHeader {
        source_url,
        start_html,
        end_html,
        start_frag,
        end_frag,
        header_end: offset,
    }
}

fn parse_offset(value: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(value).ok()?.trim();
    let number: i64 = text.parse().ok()?;
    (number >= 0).then_some(number)
}

fn slice_range(bytes: &[u8], start: Option<i64>, end: Option<i64>) -> Option<&[u8]> {
    let start = usize::try_from(start?).ok()?;
    let end = usize::try_from(end?).ok()?;
    (start < end && end <= bytes.len()).then_some(&bytes[start..end])
}

fn strip_trailing_nuls(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| *byte != 0)
        .map(|index| index + 1)
        .unwrap_or(0);
    &bytes[..end]
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn extract_links(html: &str) -> Vec<HtmlLink> {
    let lower = html.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut index = 0;
    let mut links = Vec::new();
    while index + 2 < bytes.len() {
        let tag_name = bytes[index] == b'<'
            && bytes[index + 1] == b'a'
            && bytes
                .get(index + 2)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>' || *byte == b'/');
        if tag_name && let Some(tag_end) = find_tag_end(html, index) {
            let href = attr_value(&html[index..tag_end], "href");
            let content_start = tag_end + 1;
            let close_at = find_close_tag(&lower[content_start..], "a");
            let inner = close_at
                .map(|len| &html[content_start..content_start + len])
                .unwrap_or("");
            if let Some(href) = href {
                links.push(HtmlLink {
                    href,
                    text: visible_text(inner),
                });
            }
            index = content_start + close_at.unwrap_or(0);
            continue;
        }
        index += 1;
    }
    links
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut index = start;
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(mark) = quote {
            if byte == mark {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn find_close_tag(lower_tail: &str, name: &str) -> Option<usize> {
    let bytes = lower_tail.as_bytes();
    let marker = format!("</{name}");
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(marker.as_bytes()) {
            let after = index + marker.len();
            if bytes
                .get(after)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>')
            {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let needle = name.as_bytes();
    let mut index = 0;
    while index + needle.len() < bytes.len() {
        if bytes[index..].starts_with(needle) {
            let before_ok = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
            let mut cursor = index + needle.len();
            if before_ok {
                while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                if bytes.get(cursor) == Some(&b'=') {
                    cursor += 1;
                    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                        cursor += 1;
                    }
                    let raw = read_attr_raw(&tag[cursor..]);
                    let decoded = decode_entities(raw.trim());
                    if !decoded.is_empty() {
                        return Some(decoded);
                    }
                    return None;
                }
            }
        }
        index += 1;
    }
    None
}

fn read_attr_raw(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.first() == Some(&b'"') || bytes.first() == Some(&b'\'') {
        let quote = bytes[0];
        let end = bytes[1..]
            .iter()
            .position(|byte| *byte == quote)
            .map(|index| index + 1)
            .unwrap_or(bytes.len());
        &value[1..end]
    } else {
        let end = bytes
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || *byte == b'>')
            .unwrap_or(bytes.len());
        &value[..end]
    }
}

fn visible_text(html: &str) -> String {
    let mut raw = String::new();
    let bytes = html.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'<'
            && let Some(end) = bytes[index..].iter().position(|byte| *byte == b'>')
        {
            index += end + 1;
            raw.push(' ');
            continue;
        }
        let ch = html[index..].chars().next().unwrap_or('\u{FFFD}');
        raw.push(ch);
        index += ch.len_utf8();
    }
    decode_entities(&raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        if let Some(semi) = rest.find(';')
            && let Some(ch) = entity_char(&rest[1..semi])
        {
            out.push(ch);
            rest = &rest[semi + 1..];
            continue;
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn entity_char(body: &str) -> Option<char> {
    match body {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00A0}'),
        _ if body.starts_with('#') => {
            let code =
                if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()?
                } else {
                    body[1..].parse().ok()?
                };
            char::from_u32(code)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(raw: &str) -> String {
        normalize_web_url(raw).expect(raw).key
    }

    #[test]
    fn normalization_follows_the_three_steps() {
        assert_eq!(key("  HTTP://Example.COM/a/  "), "http://example.com/a");
        assert_eq!(key("http://example.com/"), key("http://example.com"));
        assert_eq!(
            key("http://example.com/a/?q=1"),
            key("http://example.com/a?q=1")
        );
        assert_ne!(key("http://example.com"), key("https://example.com"));
        assert_ne!(key("http://example.com/A"), key("http://example.com/a"));
        assert_ne!(
            key("http://example.com/a"),
            key("http://example.com/a#frag")
        );
        assert_eq!(
            key("http://User:Secret@Example.COM:8080/A/"),
            "http://User:Secret@example.com:8080/A"
        );
        assert_eq!(key("http://example.com/a//"), "http://example.com/a/");
        assert_eq!(key("http://[::1]/a/"), "http://[::1]/a");
        assert!(normalize_web_url("www.example.com").is_none());
        assert!(normalize_web_url("C:\\notes\\a.url").is_none());
        assert!(normalize_web_url("http://example.com/a b").is_none());
        assert!(normalize_web_url("not a url").is_none());
    }

    #[test]
    fn title_drops_a_trailing_url_suffix() {
        assert_eq!(
            title_from_url_filename("Cursor Status.url").as_deref(),
            Some("Cursor Status")
        );
        assert_eq!(title_from_url_filename("页面.URL").as_deref(), Some("页面"));
        assert_eq!(title_from_url_filename(".url").as_deref(), Some(""));
        assert_eq!(title_from_url_filename("notes.txt"), None);
        assert_eq!(
            title_from_descriptor_names(&["notes.txt".to_owned(), "页面.url".to_owned()]),
            "页面"
        );
    }

    #[test]
    fn pick_prefers_the_html_link_and_ignores_the_document_source() {
        let html = chrome_like_cf_html(
            "https://example.com/page",
            "https://github.com/1024XEngineer/neo-code",
            "neo-code",
        );
        let names = vec!["neo-code.url".to_owned()];
        let picked = pick_web_drop(&WebDropInput {
            html: Some(&html),
            url_w: Some("https://example.com/page"),
            unicode_text: Some("https://example.com/page"),
            descriptor_names: &names,
            ..WebDropInput::default()
        })
        .expect("picked");
        assert_eq!(picked.url, "https://github.com/1024XEngineer/neo-code");
        assert_eq!(picked.title, "neo-code");
        let parsed = parse_cf_html(&html);
        assert_eq!(
            parsed.source_url.as_deref(),
            Some("https://example.com/page")
        );
        assert_eq!(parsed.links[0].text, "neo-code");
    }

    #[test]
    fn lock_icon_without_html_uses_uniform_resource_locator_w() {
        let names = vec!["Cursor Status.url".to_owned()];
        let picked = pick_web_drop(&WebDropInput {
            url_w: Some("https://status.cursor.com/"),
            unicode_text: Some("https://other.example/"),
            descriptor_names: &names,
            ..WebDropInput::default()
        })
        .expect("picked");
        assert_eq!(picked.url, "https://status.cursor.com/");
        assert_eq!(picked.title, "Cursor Status");
    }

    #[test]
    fn moz_url_uses_the_first_line_and_not_its_title_line() {
        let picked = pick_web_drop(&WebDropInput {
            moz_url: Some("https://example.com/moz\n不要这个标题\n"),
            unicode_text: Some("https://example.com/text"),
            descriptor_names: &[],
            ..WebDropInput::default()
        })
        .expect("picked");
        assert_eq!(picked.url, "https://example.com/moz");
        assert!(picked.title.is_empty());
        assert_eq!(
            web_display_name(&picked.title, &picked.url),
            "https://example.com/moz"
        );
    }

    #[test]
    fn plain_text_is_ignored_unless_the_whole_value_is_one_url() {
        assert!(
            pick_web_drop(&WebDropInput {
                unicode_text: Some("看看 https://example.com"),
                descriptor_names: &[],
                ..WebDropInput::default()
            })
            .is_none()
        );
        let picked = pick_web_drop(&WebDropInput {
            unicode_text: Some("  https://example.com/plain  "),
            descriptor_names: &[],
            ..WebDropInput::default()
        })
        .expect("picked");
        assert_eq!(picked.url, "https://example.com/plain");
    }

    #[test]
    fn descriptor_round_trip_does_not_need_file_contents() {
        let bytes = descriptor_w(&["页面.url", "Cursor Status.URL"]);
        let names = file_names_w(&bytes).expect("names");
        assert_eq!(title_from_descriptor_names(&names), "页面");
    }
}
