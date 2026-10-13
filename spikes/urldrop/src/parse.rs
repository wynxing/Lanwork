//! 与 Win32 无关的拖放解析。产品尚未决定的选择只记在日志里，不在这里定。

/// `DROPEFFECT_NONE`
pub const EFFECT_NONE: u32 = 0;
/// `DROPEFFECT_COPY`
pub const EFFECT_COPY: u32 = 1;
/// `DROPEFFECT_MOVE`
pub const EFFECT_MOVE: u32 = 2;
/// `DROPEFFECT_LINK`
pub const EFFECT_LINK: u32 = 4;

pub const URI_LIST_NAME: &str = "text/uri-list";
pub const HTML_FORMAT_NAME: &str = "HTML Format";
pub const FILE_CONTENTS_NAME: &str = "FileContents";
pub const FILE_GROUP_W: &str = "FileGroupDescriptorW";
pub const FILE_GROUP_A: &str = "FileGroupDescriptor";
pub const URL_W_NAME: &str = "UniformResourceLocatorW";
pub const URL_A_NAME: &str = "UniformResourceLocator";

const FD_W_SIZE: usize = 592;
const FD_A_SIZE: usize = 332;
const NAME_OFFSET: usize = 72;
const NAME_W_BYTES: usize = 520;
const NAME_A_BYTES: usize = 260;

/// 复制优先，其次快捷方式。不返回移动。
pub fn choose_effect(offered: u32) -> u32 {
    if offered & EFFECT_COPY != 0 {
        EFFECT_COPY
    } else if offered & EFFECT_LINK != 0 {
        EFFECT_LINK
    } else {
        EFFECT_NONE
    }
}

pub fn effect_names(mask: u32) -> String {
    let mut parts = Vec::new();
    if mask & EFFECT_COPY != 0 {
        parts.push("copy");
    }
    if mask & EFFECT_MOVE != 0 {
        parts.push("move");
    }
    if mask & EFFECT_LINK != 0 {
        parts.push("link");
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join("|")
    }
}

pub fn format_unix_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as u32, day as u32)
}

pub fn session_header(log_path: &str) -> String {
    format!(
        "\
这是 Lanwork 的网址拖放验证，不是收纳窗口。

请用 Chrome 和 Microsoft Edge 各做两步。做的时候不要关掉原来的网页。

1. 打开一个普通网页。按住地址栏左边的锁图标，拖进这个窗口，松开。没有锁、只有网站小图标或调音图标时，拖的就是那一个，它和锁在同一位置。
2. 在同一个网页里按住一个链接，拖进这个窗口，松开。

每松开一次，看浏览器里原来的标签还在不在，有没有被关掉，有没有换到别的位置。

这个程序只把拖放里的网址和标题记下来。浏览器附带的虚拟 .url 文件不会保存，也不会复制到磁盘。程序不会去点浏览器。

下面每一段 kind=drop 都是一次拖放。测完后，把这个窗口里的字全选复制，或者打开日志文件，整段贴回去。

日志文件：
{log_path}
"
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlLink {
    pub href: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlExtract {
    pub source_url: Option<String>,
    pub document_title: Option<String>,
    pub links: Vec<HtmlLink>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedFormat {
    pub id: u16,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UriListFound {
    pub format_name: String,
    pub format_id: u16,
    pub urls: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    pub offered_effects: u32,
    pub chosen_effect: u32,
    pub enum_error: Option<String>,
    pub formats: Vec<ListedFormat>,
    pub html: Option<HtmlExtract>,
    pub html_error: Option<String>,
    pub html_truncated: bool,
    pub uri_lists: Vec<UriListFound>,
    pub uri_list_error: Option<String>,
    pub unicode_text: Option<String>,
    pub ansi_text: Option<String>,
    pub url_w: Option<String>,
    pub url_a: Option<String>,
    pub virtual_names: Vec<String>,
    pub descriptor_errors: Vec<String>,
    pub read_errors: Vec<String>,
    pub file_contents_offered: bool,
    pub hdrop_offered: bool,
}

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
    let document_title = element_text(&String::from_utf8_lossy(html), "title");
    HtmlExtract {
        source_url: header.source_url,
        document_title,
        links,
    }
}

pub fn parse_uri_list(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(strip_trailing_nuls(bytes));
    let mut urls = Vec::new();
    for line in text.split(['\n', '\r']) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        urls.push(line.to_string());
    }
    urls
}

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

pub fn encode_utf16_text(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for unit in text.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend(unit.to_le_bytes());
    }
    bytes
}

pub fn file_names_w(bytes: &[u8]) -> Result<Vec<String>, String> {
    file_names(bytes, FD_W_SIZE, NAME_W_BYTES, true)
}

pub fn file_names_a(bytes: &[u8]) -> Result<Vec<String>, String> {
    file_names(bytes, FD_A_SIZE, NAME_A_BYTES, false)
}

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

pub fn descriptor_a(names: &[&str]) -> Vec<u8> {
    let mut bytes = vec![0u8; 4 + names.len() * FD_A_SIZE];
    bytes[..4].copy_from_slice(&(names.len() as u32).to_le_bytes());
    for (index, name) in names.iter().enumerate() {
        let start = 4 + index * FD_A_SIZE + NAME_OFFSET;
        let raw = name.as_bytes();
        let take = raw.len().min(NAME_A_BYTES.saturating_sub(1));
        bytes[start..start + take].copy_from_slice(&raw[..take]);
    }
    bytes
}

pub fn format_report(kind: &str, when: &str, extracted: &Extracted) -> String {
    let mut out = String::new();
    push(&mut out, "======== 网址拖放 ========");
    push(&mut out, &format!("kind={kind}"));
    push(&mut out, &format!("when={when}"));
    push(
        &mut out,
        &format!("chosen-effect={}", effect_names(extracted.chosen_effect)),
    );
    push(
        &mut out,
        &format!(
            "offered-effects={}",
            effect_names(extracted.offered_effects)
        ),
    );
    push(&mut out, "browser-control-called=false");
    push(&mut out, "file-contents-read=false");
    push(&mut out, "file-contents-written=false");
    push(&mut out, "hdrop-read=false");
    push(&mut out, "virtual-url-stored=false");
    push(&mut out, "virtual-url-copied=false");
    push(&mut out, "format-priority=not-decided");
    push(&mut out, "plain-text-rule=not-decided");
    push(&mut out, "scheme-rule=not-decided");
    push(&mut out, "extra-format-rule=not-decided");
    if let Some(error) = &extracted.enum_error {
        push(&mut out, &format!("format-enum-error={}", one_line(error)));
    }
    push(
        &mut out,
        &format!("format-count={}", extracted.formats.len()),
    );
    for (index, format) in extracted.formats.iter().enumerate() {
        push(
            &mut out,
            &format!(
                "format-{}={} id={}",
                index + 1,
                one_line(&format.name),
                format.id
            ),
        );
    }
    match &extracted.html {
        Some(html) => {
            push(&mut out, "cf-html-present=true");
            push(
                &mut out,
                &format!(
                    "cf-html-source-url={}",
                    one_line(html.source_url.as_deref().unwrap_or(""))
                ),
            );
            push(
                &mut out,
                &format!(
                    "cf-html-document-title={}",
                    one_line(html.document_title.as_deref().unwrap_or(""))
                ),
            );
            push(
                &mut out,
                &format!("cf-html-link-count={}", html.links.len()),
            );
            for (index, link) in html.links.iter().enumerate() {
                push(
                    &mut out,
                    &format!("cf-html-link-{}-href={}", index + 1, one_line(&link.href)),
                );
                push(
                    &mut out,
                    &format!("cf-html-link-{}-text={}", index + 1, one_line(&link.text)),
                );
            }
        }
        None => push(&mut out, "cf-html-present=false"),
    }
    if extracted.html_truncated {
        push(&mut out, "cf-html-truncated=true");
    }
    if let Some(error) = &extracted.html_error {
        push(&mut out, &format!("cf-html-error={}", one_line(error)));
    }
    if extracted.uri_lists.is_empty() {
        push(&mut out, "text-uri-list-present=false");
        push(
            &mut out,
            &format!("text-uri-list-probed-name={URI_LIST_NAME}"),
        );
    } else {
        push(&mut out, "text-uri-list-present=true");
        for (index, found) in extracted.uri_lists.iter().enumerate() {
            push(
                &mut out,
                &format!(
                    "text-uri-list-{}-name={} id={}",
                    index + 1,
                    one_line(&found.format_name),
                    found.format_id
                ),
            );
            push(
                &mut out,
                &format!("text-uri-list-{}-count={}", index + 1, found.urls.len()),
            );
            for (url_index, url) in found.urls.iter().enumerate() {
                push(
                    &mut out,
                    &format!(
                        "text-uri-list-{}-url-{}={}",
                        index + 1,
                        url_index + 1,
                        one_line(url)
                    ),
                );
            }
        }
    }
    if let Some(error) = &extracted.uri_list_error {
        push(
            &mut out,
            &format!("text-uri-list-error={}", one_line(error)),
        );
    }
    match &extracted.unicode_text {
        Some(text) => push(&mut out, &format!("cf-unicodetext={}", one_line(text))),
        None => push(&mut out, "cf-unicodetext-present=false"),
    }
    match &extracted.ansi_text {
        Some(text) => {
            push(&mut out, &format!("cf-text={}", one_line(text)));
            push(
                &mut out,
                "cf-text-note=产品规格尚未决定是否接受 CF_TEXT。这里只记录。",
            );
        }
        None => push(&mut out, "cf-text-present=false"),
    }
    match &extracted.url_w {
        Some(text) => {
            push(
                &mut out,
                &format!("UniformResourceLocatorW={}", one_line(text)),
            );
            push(
                &mut out,
                "UniformResourceLocatorW-note=产品规格尚未决定是否读取。这里只记录。",
            );
        }
        None => push(&mut out, "UniformResourceLocatorW-present=false"),
    }
    match &extracted.url_a {
        Some(text) => {
            push(
                &mut out,
                &format!("UniformResourceLocator={}", one_line(text)),
            );
            push(
                &mut out,
                "UniformResourceLocator-note=产品规格尚未决定是否读取。这里只记录。",
            );
        }
        None => push(&mut out, "UniformResourceLocator-present=false"),
    }
    push(
        &mut out,
        &format!("file-contents-offered={}", extracted.file_contents_offered),
    );
    push(
        &mut out,
        &format!("cf-hdrop-offered={}", extracted.hdrop_offered),
    );
    push(
        &mut out,
        &format!("virtual-url-count={}", extracted.virtual_names.len()),
    );
    for (index, name) in extracted.virtual_names.iter().enumerate() {
        push(
            &mut out,
            &format!("virtual-url-name-{}={}", index + 1, one_line(name)),
        );
    }
    for (index, error) in extracted.descriptor_errors.iter().enumerate() {
        push(
            &mut out,
            &format!("descriptor-error-{}={}", index + 1, one_line(error)),
        );
    }
    for (index, error) in extracted.read_errors.iter().enumerate() {
        push(
            &mut out,
            &format!("read-error-{}={}", index + 1, one_line(error)),
        );
    }
    push(&mut out, "======== 记录结束 ========");
    out
}

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
    assert_eq!(header.len(), header_len, "cf_html header width changed");
    let mut out = header.into_bytes();
    out.extend(html.as_bytes());
    out
}

fn file_names(
    bytes: &[u8],
    stride: usize,
    name_bytes: usize,
    wide: bool,
) -> Result<Vec<String>, String> {
    if bytes.len() < 4 {
        return Err("descriptor shorter than count".to_string());
    }
    let count = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes")) as usize;
    let body = count
        .checked_mul(stride)
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
        let start = 4 + index * stride + NAME_OFFSET;
        let raw = &bytes[start..start + name_bytes];
        let name = if wide {
            decode_utf16_text(raw)
        } else {
            let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        };
        names.push(name);
    }
    Ok(names)
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
                b"SourceURL" => {
                    source_url = Some(String::from_utf8_lossy(value).into_owned());
                }
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
    if number < 0 { None } else { Some(number) }
}

fn slice_range(bytes: &[u8], start: Option<i64>, end: Option<i64>) -> Option<&[u8]> {
    let start = usize::try_from(start?).ok()?;
    let end = usize::try_from(end?).ok()?;
    if start < end && end <= bytes.len() {
        Some(&bytes[start..end])
    } else {
        None
    }
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

fn element_text(html: &str, name: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let open = format!("<{name}");
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(open.as_bytes()) {
            let after = index + open.len();
            let boundary = bytes
                .get(after)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>');
            if boundary {
                let tag_end = find_tag_end(html, index)?;
                let content_start = tag_end + 1;
                let close_at = find_close_tag(&lower[content_start..], name)?;
                let text = visible_text(&html[content_start..content_start + close_at]);
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        index += 1;
    }
    None
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

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' => ' ',
            _ => ch,
        })
        .collect()
}

fn push(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

pub fn check_parsers() -> Result<(), String> {
    ensure(format_unix_utc(0) == "1970-01-01T00:00:00Z", "unix 0")?;
    ensure(
        format_unix_utc(1_600_000_000) == "2020-09-13T12:26:40Z",
        "unix 1600000000",
    )?;
    ensure(
        format_unix_utc(1_791_892_800) == "2026-10-13T12:00:00Z",
        "unix 2026-10-13",
    )?;
    for mask in 0..8 {
        let chosen = choose_effect(mask);
        if chosen & EFFECT_MOVE != 0 {
            return Err(format!("effect {mask} returned move"));
        }
    }
    ensure(
        choose_effect(EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK) == EFFECT_COPY,
        "copy wins",
    )?;
    ensure(
        choose_effect(EFFECT_MOVE | EFFECT_LINK) == EFFECT_LINK,
        "link second",
    )?;
    ensure(choose_effect(EFFECT_MOVE) == EFFECT_NONE, "move refused")?;
    ensure(choose_effect(0) == EFFECT_NONE, "empty refused")?;

    let href = "https://example.com/a/?q=1";
    let title = "示例标题";
    let html = chrome_like_cf_html(href, href, title);
    let parsed = parse_cf_html(&html);
    ensure(parsed.source_url.as_deref() == Some(href), "source url")?;
    ensure(parsed.links.len() == 1, "one link")?;
    ensure(parsed.links[0].href == href, "href")?;
    ensure(parsed.links[0].text == title, "title")?;

    let entity = chrome_like_cf_html("https://example.com/", "https://example.com/x", "A &amp; B");
    let entity_parsed = parse_cf_html(&entity);
    ensure(entity_parsed.links[0].text == "A & B", "entity")?;

    let quoted = b"Version:0.9\r\nStartHTML:-1\r\nEndHTML:-1\r\n<html><a href='https://example.com/q'>quoted</a></html>";
    let quoted_parsed = parse_cf_html(quoted);
    ensure(
        quoted_parsed
            .links
            .first()
            .is_some_and(|link| link.href == "https://example.com/q"),
        "quoted href",
    )?;

    let titled = "<html><head><title>页 标题</title></head><body><a href=\"https://example.com/t\">link</a></body></html>";
    let titled_parsed = parse_cf_html(titled.as_bytes());
    ensure(
        titled_parsed.document_title.as_deref() == Some("页 标题"),
        "document title",
    )?;

    let two = b"<a href=\"https://example.com/1\">one</a><a href=\"https://example.com/2\">two</a>";
    let two_parsed = parse_cf_html(two);
    ensure(two_parsed.links.len() == 2, "two links")?;
    ensure(two_parsed.links[1].text == "two", "second text")?;

    let list = b"# comment\r\nhttps://example.com/a/?q=1\r\n\r\nhttps://example.com/b\r\n";
    let urls = parse_uri_list(list);
    ensure(urls.len() == 2, "uri-list count")?;
    ensure(urls[0] == href, "uri-list first")?;
    ensure(urls[1] == "https://example.com/b", "uri-list second")?;
    ensure(
        parse_uri_list(b"http://example.com/a#frag\n").len() == 1,
        "hash in url is not a comment",
    )?;

    let text = decode_utf16_text(&encode_utf16_text(href));
    ensure(text == href, "utf16 roundtrip")?;

    let names = file_names_w(&descriptor_w(&["页面.url", "notes.txt"]))?;
    ensure(
        names == vec!["页面.url".to_string(), "notes.txt".to_string()],
        "descriptor names",
    )?;
    ensure(file_names_w(&[1, 0, 0, 0]).is_err(), "short descriptor")?;
    let ansi_names = file_names_a(&descriptor_a(&["page.url"]))?;
    ensure(
        ansi_names == vec!["page.url".to_string()],
        "ansi descriptor",
    )?;
    ensure(FILE_GROUP_A == "FileGroupDescriptor", "group a name")?;
    ensure(URL_W_NAME == "UniformResourceLocatorW", "url w name")?;
    ensure(URL_A_NAME == "UniformResourceLocator", "url a name")?;

    let header = session_header(r"C:\Temp\lanwork-spike-urldrop.log");
    for phrase in ["锁图标", "链接", "标签", "虚拟 .url", "贴回去"] {
        if !header.contains(phrase) {
            return Err(format!("header missing {phrase}"));
        }
    }

    let extracted = Extracted {
        offered_effects: EFFECT_COPY | EFFECT_LINK | EFFECT_MOVE,
        chosen_effect: EFFECT_COPY,
        formats: vec![
            ListedFormat {
                id: 100,
                name: HTML_FORMAT_NAME.to_string(),
            },
            ListedFormat {
                id: 101,
                name: URI_LIST_NAME.to_string(),
            },
            ListedFormat {
                id: 13,
                name: "CF_UNICODETEXT".to_string(),
            },
            ListedFormat {
                id: 102,
                name: FILE_GROUP_W.to_string(),
            },
            ListedFormat {
                id: 103,
                name: FILE_CONTENTS_NAME.to_string(),
            },
        ],
        html: Some(parsed),
        uri_lists: vec![UriListFound {
            format_name: URI_LIST_NAME.to_string(),
            format_id: 101,
            urls: urls.clone(),
        }],
        unicode_text: Some(href.to_string()),
        url_w: Some(href.to_string()),
        virtual_names: names,
        file_contents_offered: true,
        hdrop_offered: true,
        ..Extracted::default()
    };
    let report = format_report("drop", "2026-10-13T12:00:00Z", &extracted);
    for phrase in [
        "kind=drop",
        "chosen-effect=copy",
        "offered-effects=copy|move|link",
        "browser-control-called=false",
        "file-contents-read=false",
        "file-contents-written=false",
        "hdrop-read=false",
        "virtual-url-stored=false",
        "virtual-url-copied=false",
        "format-priority=not-decided",
        "plain-text-rule=not-decided",
        "cf-html-source-url=https://example.com/a/?q=1",
        "cf-html-link-1-text=示例标题",
        "text-uri-list-1-url-1=https://example.com/a/?q=1",
        "text-uri-list-1-url-2=https://example.com/b",
        "cf-unicodetext=https://example.com/a/?q=1",
        "UniformResourceLocatorW=https://example.com/a/?q=1",
        "virtual-url-name-1=页面.url",
        "virtual-url-name-2=notes.txt",
        "file-contents-offered=true",
        "cf-hdrop-offered=true",
        "extra-format-rule=not-decided",
    ] {
        if !report.contains(phrase) {
            return Err(format!("report missing {phrase}\n{report}"));
        }
    }
    if report.contains("secret.example") {
        return Err("report included a url that was not extracted".to_string());
    }
    Ok(())
}

fn ensure(cond: bool, message: &str) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_cover_the_three_formats_and_ignore_rules() {
        check_parsers().expect("parsers");
    }
}
