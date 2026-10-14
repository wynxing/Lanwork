//! 网页引用的网站图标：挑选地址、磁盘缓存、以及可替换的获取和解码。
//!
//! 不访问网络，也不调用 Win32。分组文件不写图标字节。获取失败时调用方仍保留已经保存的引用。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dispatch::RgbaImage;
use crate::storage::{self, atomic_write};

use super::weblink::normalize_web_url;

/// 数据目录下的相对路径。可整份删除后重建，不进入导出和备份。
pub const WEB_ICON_CACHE_DIR: &str = "cache/web-icons";

/// 一次 HTTP 正文的上限。页面和图标共用。
pub const WEB_ICON_BYTES_LIMIT: usize = 1024 * 1024;

/// 只在正文的这一段里找 `<link>`。
pub const WEB_ICON_HTML_SCAN: usize = 512 * 1024;

/// 放进缓存和界面的边长上限。
pub const WEB_ICON_MAX_EDGE: u32 = 256;

/// 超过这个边长的原图不再解码。
pub const WEB_ICON_SOURCE_EDGE: u32 = 4096;

/// 重定向最多跟这么多次。
pub const WEB_ICON_MAX_REDIRECTS: u32 = 5;

/// 页面里声明的图标最多取这么多条，之后再加 `/favicon.ico`。
const MAX_DECLARED: usize = 8;

const MAGIC: &[u8] = b"LWFI";
const CACHE_VERSION: u32 = 1;

/// 一次取回的正文。`final_url` 是重定向之后的地址，用来解析相对链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub final_url: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// 按绝对地址取正文。失败、非 2xx 或超过 `limit` 时返回 `None`。
pub trait IconSource {
    fn get(&mut self, url: &str, limit: usize) -> Option<Fetched>;
}

/// 把图标字节解成 RGBA。解不开时返回 `None`。
pub trait IconDecoder {
    fn decode(&mut self, bytes: &[u8], content_type: Option<&str>) -> Option<RgbaImage>;
}

/// 可以交给 WinHTTP 的 `http` 或 `https` 地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconUrl {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

impl IconUrl {
    #[must_use]
    pub fn scheme(&self) -> &'static str {
        if self.secure { "https" } else { "http" }
    }

    #[must_use]
    pub fn host_port(&self) -> String {
        let default = if self.secure { 443 } else { 80 };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    #[must_use]
    pub fn as_url(&self) -> String {
        format!(
            "{}://{}{}",
            self.scheme(),
            self.host_port(),
            self.path_and_query
        )
    }

    #[must_use]
    pub fn favicon_ico(&self) -> String {
        format!("{}://{}/favicon.ico", self.scheme(), self.host_port())
    }

    fn path_only(&self) -> &str {
        self.path_and_query.split('?').next().unwrap_or("/")
    }
}

/// 数据目录里的网站图标缓存。文件可以删，删了会再取。
#[derive(Debug, Clone)]
pub struct WebIconCache {
    dir: PathBuf,
}

impl WebIconCache {
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(WEB_ICON_CACHE_DIR),
        }
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn load(&self, normalized_key: &str) -> Option<RgbaImage> {
        let path = self.path_for(normalized_key)?;
        let bytes = storage::fs_read(&path).ok()?;
        match decode_cache(&bytes) {
            Some(image) => Some(image),
            None => {
                let _ = storage::fs_remove_file(&path);
                None
            }
        }
    }

    /// 写盘失败时返回 false。调用方仍可以先把图显示出来。
    pub fn store(&self, normalized_key: &str, image: &RgbaImage) -> bool {
        let Some(path) = self.path_for(normalized_key) else {
            return false;
        };
        let Some(bytes) = encode_cache(image) else {
            return false;
        };
        if storage::fs_create_dir_all(&self.dir).is_err() {
            return false;
        }
        atomic_write(&path, &bytes).is_ok()
    }

    fn path_for(&self, normalized_key: &str) -> Option<PathBuf> {
        let name = cache_file_name(normalized_key)?;
        Some(self.dir.join(name))
    }
}

/// 能取图标时返回规范化键。不是 `http`/`https`，或带用户信息时返回 `None`。
#[must_use]
pub fn icon_cache_key(url: &str) -> Option<String> {
    let normalized = normalize_web_url(url)?;
    parse_icon_url(&normalized.stored)?;
    Some(normalized.key)
}

/// 解析 `http`/`https`。去掉片段和用户信息；协议和主机名小写；默认端口不写进结果。
#[must_use]
pub fn parse_icon_url(raw: &str) -> Option<IconUrl> {
    let raw = raw.trim();
    let raw = raw.split('#').next().unwrap_or(raw);
    if raw.is_empty()
        || raw.contains('@')
        || raw.contains('\\')
        || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return None;
    }
    let (secure, rest) = if let Some(rest) = strip_scheme(raw, "https") {
        (true, rest)
    } else {
        let rest = strip_scheme(raw, "http")?;
        (false, rest)
    };
    if rest.is_empty() {
        return None;
    }
    let (authority, path_and_query) = split_authority(rest)?;
    let default_port = if secure { 443 } else { 80 };
    let (host, port) = split_host_port(authority, default_port)?;
    if port == 0 || !host_ok(&host) {
        return None;
    }
    let path_and_query = if path_and_query.is_empty() {
        "/".to_owned()
    } else if let Some(query) = path_and_query.strip_prefix('?') {
        format!("/?{query}")
    } else if !path_and_query.starts_with('/') {
        return None;
    } else {
        path_and_query.to_owned()
    };
    let (path, query) = split_path_query(&path_and_query);
    let path = normalize_dots(&path)?;
    let mut path_and_query = path;
    if let Some(query) = query {
        path_and_query.push('?');
        path_and_query.push_str(&query);
    }
    Some(IconUrl {
        secure,
        host: host.to_ascii_lowercase(),
        port,
        path_and_query,
    })
}

/// 把页面上的 `href` 收成绝对地址。不是可用的图标地址时返回 `None`。
#[must_use]
pub fn join_url(base: &str, href: &str) -> Option<String> {
    let href = decode_entities(href.trim());
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    if href
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return None;
    }
    if let Some(rest) = href.strip_prefix("data:") {
        let owned = format!("data:{rest}");
        data_image(&owned)?;
        return Some(owned);
    }
    if href.contains("://") {
        return parse_icon_url(&href).map(|url| url.as_url());
    }
    if let Some(rest) = href.strip_prefix("//") {
        let base = parse_icon_url(base)?;
        return parse_icon_url(&format!("{}://{rest}", base.scheme())).map(|url| url.as_url());
    }
    if href.contains(':') && !href.starts_with('/') && !href.starts_with('?') {
        return None;
    }
    let base_url = parse_icon_url(base)?;
    let (path, query) = if let Some(query) = href.strip_prefix('?') {
        let path = base_url.path_only().to_owned();
        let query = query.split('#').next().unwrap_or(query);
        (path, Some(query.to_owned()))
    } else if let Some(href) = href.strip_prefix('/') {
        split_path_query(&format!("/{href}"))
    } else {
        let dir = directory_prefix(base_url.path_only());
        split_path_query(&format!("{dir}{href}"))
    };
    let path = normalize_dots(&path)?;
    let mut built = format!("{}://{}{path}", base_url.scheme(), base_url.host_port());
    if let Some(query) = query {
        built.push('?');
        built.push_str(&query);
    }
    parse_icon_url(&built).map(|url| url.as_url())
}

/// 重定向的 `Location`。绝对或相对都可以，结果必须仍是 `http`/`https`。
#[must_use]
pub fn resolve_icon_redirect(current: &str, location: &str) -> Option<String> {
    join_url(current, location.trim())
}

/// 页面声明的图标在前，站点根上的 `/favicon.ico` 在最后。已经在列表里的不重复。
#[must_use]
pub fn icon_candidates(page_url: &str, html: Option<&str>) -> Vec<String> {
    let Some(page) = parse_icon_url(page_url) else {
        return Vec::new();
    };
    let base = page.as_url();
    let mut found = Vec::new();
    if let Some(html) = html {
        let scan = &html[..html.len().min(WEB_ICON_HTML_SCAN)];
        for inner in link_inners(scan) {
            if found.len() >= MAX_DECLARED {
                break;
            }
            let Some(rel) = attr(inner, "rel") else {
                continue;
            };
            if !rel_is_icon(&rel) {
                continue;
            }
            let Some(href) = attr(inner, "href") else {
                continue;
            };
            let kind = attr(inner, "type").unwrap_or_default();
            if is_svg_ref(&href, &kind) {
                continue;
            }
            let Some(url) = join_url(&base, &href) else {
                continue;
            };
            if !found.contains(&url) {
                found.push(url);
            }
        }
    }
    let fallback = page.favicon_ico();
    if !found.contains(&fallback) {
        found.push(fallback);
    }
    found
}

/// 先读缓存。没有则向 `source` 取页面和图标。失败返回 `None`，不写失败标记。
///
/// 缓存写盘失败时仍返回已经解开的图。分组文件不在这里改。
pub fn load_web_icon(
    url: &str,
    cache: &WebIconCache,
    source: &mut impl IconSource,
    decoder: &mut impl IconDecoder,
) -> Option<RgbaImage> {
    let normalized = normalize_web_url(url)?;
    let request = parse_icon_url(&normalized.stored)?;
    if let Some(image) = cache.load(&normalized.key) {
        return Some(image);
    }
    let request_url = request.as_url();
    let page = fetch_page(source, &request_url);
    if let Some(direct) = page.image
        && let Some(image) = accept(decoder, &direct.bytes, direct.content_type.as_deref())
    {
        let _ = cache.store(&normalized.key, &image);
        return Some(image);
    }
    for candidate in icon_candidates(&page.base, page.html.as_deref()) {
        if let Some(image) = fetch_candidate(source, decoder, &candidate) {
            let _ = cache.store(&normalized.key, &image);
            return Some(image);
        }
    }
    None
}

/// 缩到 [`WEB_ICON_MAX_EDGE`] 以内，保持比例。边长为 0 或原图过大时返回 `None`。
#[must_use]
pub fn fit_icon_edge(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || width > WEB_ICON_SOURCE_EDGE || height > WEB_ICON_SOURCE_EDGE {
        return None;
    }
    if width <= WEB_ICON_MAX_EDGE && height <= WEB_ICON_MAX_EDGE {
        return Some((width, height));
    }
    let w = u64::from(width);
    let h = u64::from(height);
    let max = u64::from(WEB_ICON_MAX_EDGE);
    let (nw, nh) = if w >= h {
        (max, (h * max / w).max(1))
    } else {
        ((w * max / h).max(1), max)
    };
    Some((u32::try_from(nw).ok()?, u32::try_from(nh).ok()?))
}

struct DirectImage {
    bytes: Vec<u8>,
    content_type: Option<String>,
}

struct PageBody {
    base: String,
    html: Option<String>,
    image: Option<DirectImage>,
}

fn fetch_page(source: &mut impl IconSource, request_url: &str) -> PageBody {
    let empty = |base: String| PageBody {
        base,
        html: None,
        image: None,
    };
    let Some(fetched) = source.get(request_url, WEB_ICON_BYTES_LIMIT) else {
        return empty(request_url.to_owned());
    };
    if fetched.body.is_empty() || fetched.body.len() > WEB_ICON_BYTES_LIMIT {
        return empty(request_url.to_owned());
    }
    let base = parse_icon_url(&fetched.final_url)
        .map(|url| url.as_url())
        .unwrap_or_else(|| request_url.to_owned());
    let content_type = fetched.content_type.clone();
    if is_image_type(content_type.as_deref()) || sniff_image(&fetched.body) {
        return PageBody {
            base,
            html: None,
            image: Some(DirectImage {
                bytes: fetched.body,
                content_type,
            }),
        };
    }
    if is_html(content_type.as_deref(), &fetched.body) {
        let text = String::from_utf8_lossy(&fetched.body).into_owned();
        return PageBody {
            base,
            html: Some(text),
            image: None,
        };
    }
    empty(base)
}

fn fetch_candidate(
    source: &mut impl IconSource,
    decoder: &mut impl IconDecoder,
    candidate: &str,
) -> Option<RgbaImage> {
    if let Some(bytes) = data_image(candidate) {
        return accept(decoder, &bytes, None);
    }
    let fetched = source.get(candidate, WEB_ICON_BYTES_LIMIT)?;
    if fetched.body.is_empty() || fetched.body.len() > WEB_ICON_BYTES_LIMIT {
        return None;
    }
    accept(decoder, &fetched.body, fetched.content_type.as_deref())
}

fn accept(
    decoder: &mut impl IconDecoder,
    bytes: &[u8],
    content_type: Option<&str>,
) -> Option<RgbaImage> {
    if content_type.is_some_and(|kind| kind.to_ascii_lowercase().contains("svg")) {
        return None;
    }
    let image = decoder.decode(bytes, content_type)?;
    if image.width == 0
        || image.height == 0
        || image.width > WEB_ICON_MAX_EDGE
        || image.height > WEB_ICON_MAX_EDGE
    {
        return None;
    }
    Some(image)
}

fn cache_file_name(normalized_key: &str) -> Option<String> {
    if normalized_key.is_empty() || normalized_key.len() > 16_384 {
        return None;
    }
    let digest = Sha256::digest(normalized_key.as_bytes());
    let mut name = String::with_capacity(68);
    for byte in digest {
        name.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
        name.push(char::from(b"0123456789abcdef"[(byte & 0x0f) as usize]));
    }
    name.push_str(".bin");
    Some(name)
}

fn encode_cache(image: &RgbaImage) -> Option<Vec<u8>> {
    if image.width > WEB_ICON_MAX_EDGE || image.height > WEB_ICON_MAX_EDGE {
        return None;
    }
    let mut out = Vec::with_capacity(16 + image.pixels.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&image.width.to_le_bytes());
    out.extend_from_slice(&image.height.to_le_bytes());
    out.extend_from_slice(&image.pixels);
    Some(out)
}

fn decode_cache(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() < 16 || !bytes.starts_with(MAGIC) {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if version != CACHE_VERSION {
        return None;
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    if width == 0 || height == 0 || width > WEB_ICON_MAX_EDGE || height > WEB_ICON_MAX_EDGE {
        return None;
    }
    let pixels = bytes[16..].to_vec();
    RgbaImage::new(width, height, pixels)
}

fn strip_scheme<'a>(raw: &'a str, scheme: &str) -> Option<&'a str> {
    let prefix = scheme.len() + 3;
    if raw.len() < prefix {
        return None;
    }
    let (head, rest) = raw.split_at(prefix);
    if head.get(..scheme.len())?.eq_ignore_ascii_case(scheme)
        && head.get(scheme.len()..)?.eq_ignore_ascii_case("://")
    {
        Some(rest)
    } else {
        None
    }
}

fn split_authority(rest: &str) -> Option<(&str, &str)> {
    if rest.starts_with('[') {
        let end = rest.find(']')?;
        let after = rest.get(end + 1..)?;
        if let Some(path_at) = after.find(['/', '?']) {
            let split = end + 1 + path_at;
            return Some((rest.get(..split)?, rest.get(split..)?));
        }
        return Some((rest, ""));
    }
    match rest.find(['/', '?']) {
        Some(index) => Some((&rest[..index], &rest[index..])),
        None => Some((rest, "")),
    }
}

fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(end) = authority.find(']') {
        if !authority.starts_with('[') {
            return None;
        }
        let host = authority.get(..=end)?.to_owned();
        let after = authority.get(end + 1..)?;
        let port = if after.is_empty() {
            default_port
        } else {
            let port = after.strip_prefix(':')?;
            if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            port.parse::<u16>().ok()?
        };
        return Some((host, port));
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        return Some((host.to_owned(), port.parse::<u16>().ok()?));
    }
    if authority.is_empty() {
        return None;
    }
    Some((authority.to_owned(), default_port))
}

fn host_ok(host: &str) -> bool {
    let name = if let Some(inner) = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        if inner.is_empty() {
            return false;
        }
        inner
    } else if host.starts_with('[') || host.contains(':') {
        return false;
    } else {
        host
    };
    !name.is_empty()
        && name.chars().count() <= 253
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '/' | '\\' | '@' | '#'))
}

fn split_path_query(text: &str) -> (String, Option<String>) {
    match text.split_once('?') {
        Some((path, query)) => {
            let query = query.split('#').next().unwrap_or(query);
            (path.to_owned(), Some(query.to_owned()))
        }
        None => {
            let path = text.split('#').next().unwrap_or(text);
            (path.to_owned(), None)
        }
    }
}

fn directory_prefix(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[..=index],
        None => "/",
    }
}

fn normalize_dots(path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\\') {
        return None;
    }
    let trailing = path.ends_with('/') && path != "/";
    let mut stack = Vec::new();
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            stack.pop();
            continue;
        }
        stack.push(segment);
    }
    let mut out = String::from("/");
    out.push_str(&stack.join("/"));
    if trailing && out != "/" {
        out.push('/');
    }
    Some(out)
}

fn link_inners(html: &str) -> Vec<&str> {
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = lower[from..].find("<link") {
        let start = from + offset;
        let after = start + 5;
        let boundary = html.as_bytes().get(after).copied().unwrap_or(b' ');
        if boundary.is_ascii_alphanumeric() {
            from = after;
            continue;
        }
        let rest = &html[after..];
        let Some(end) = find_tag_end(rest) else {
            break;
        };
        found.push(&rest[..end]);
        from = after + end + 1;
    }
    found
}

fn find_tag_end(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in text.char_indices() {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(open), ch) if ch == open => quote = None,
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while !rest.is_empty() {
        rest = rest.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '/');
        if rest.is_empty() {
            break;
        }
        let name_end = rest
            .find(|ch: char| ch.is_whitespace() || ch == '=' || ch == '/')
            .unwrap_or(rest.len());
        let attr_name = &rest[..name_end];
        rest = &rest[name_end..];
        rest = rest.trim_start();
        let value = if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let (value, next) = read_value(after)?;
            rest = next;
            value
        } else {
            String::new()
        };
        if attr_name.eq_ignore_ascii_case(name) {
            return Some(decode_entities(&value));
        }
    }
    None
}

fn read_value(text: &str) -> Option<(String, &str)> {
    let first = text.chars().next()?;
    if first == '"' || first == '\'' {
        let end = text[1..].find(first)?;
        let value = text[1..1 + end].to_owned();
        return Some((value, &text[1 + end + 1..]));
    }
    let end = text
        .find(|ch: char| ch.is_whitespace() || ch == '>')
        .unwrap_or(text.len());
    Some((text[..end].to_owned(), &text[end..]))
}

fn rel_is_icon(rel: &str) -> bool {
    rel.split_whitespace().any(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "icon" | "apple-touch-icon" | "apple-touch-icon-precomposed"
        )
    })
}

fn is_svg_ref(href: &str, type_attr: &str) -> bool {
    if type_attr.to_ascii_lowercase().contains("svg") {
        return true;
    }
    let path = href.split(['?', '#']).next().unwrap_or(href);
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("svg"))
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some(end) = rest.find(';')
            && let Some(ch) = entity_char(&rest[1..end])
        {
            out.push(ch);
            rest = &rest[end + 1..];
            continue;
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn entity_char(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "lt" => Some('<'),
        "gt" => Some('>'),
        _ => {
            let code = if let Some(hex) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                let dec = entity.strip_prefix('#')?;
                dec.parse().ok()?
            };
            char::from_u32(code).filter(|ch| !ch.is_control())
        }
    }
}

fn data_image(url: &str) -> Option<Vec<u8>> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let meta = meta.to_ascii_lowercase();
    if meta.contains("svg") {
        return None;
    }
    let mime = meta.split(';').next().unwrap_or("").trim();
    const ALLOWED: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/bmp",
        "image/x-icon",
        "image/vnd.microsoft.icon",
        "image/ico",
    ];
    if !ALLOWED.contains(&mime) {
        return None;
    }
    if !meta.split(';').any(|part| part.trim() == "base64") {
        return None;
    }
    let bytes = base64_decode(payload.trim())?;
    if bytes.is_empty() || bytes.len() > WEB_ICON_BYTES_LIMIT {
        return None;
    }
    Some(bytes)
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if bytes.is_empty() || bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let left = bytes.len() - index;
        if left == 1 {
            return None;
        }
        let b0 = value(bytes[index])?;
        let b1 = value(bytes[index + 1])?;
        let pad2 = left == 2 || (left >= 3 && bytes[index + 2] == b'=');
        let pad3 = left < 4 || bytes.get(index + 3).copied() == Some(b'=');
        if pad2 {
            out.push((b0 << 2) | (b1 >> 4));
            break;
        }
        let b2 = value(bytes[index + 2])?;
        out.push((b0 << 2) | (b1 >> 4));
        out.push((b1 << 4) | (b2 >> 2));
        if pad3 {
            break;
        }
        let b3 = value(*bytes.get(index + 3)?)?;
        out.push((b2 << 6) | b3);
        index += 4;
    }
    Some(out)
}

fn is_image_type(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|kind| {
        let kind = kind.to_ascii_lowercase();
        kind.starts_with("image/") && !kind.contains("svg")
    })
}

fn sniff_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG")
        || bytes.starts_with(b"GIF8")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(&[0x00, 0x00, 0x01, 0x00])
        || bytes.starts_with(b"RIFF")
}

fn is_html(content_type: Option<&str>, body: &[u8]) -> bool {
    if let Some(kind) = content_type {
        let kind = kind.to_ascii_lowercase();
        if kind.contains("html") || kind.starts_with("text/") {
            return true;
        }
        if kind.starts_with("image/") || kind.contains("octet-stream") {
            return false;
        }
    }
    let text = String::from_utf8_lossy(&body[..body.len().min(64)]);
    text.trim_start().starts_with('<')
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::shelves::{IncomingWeb, ShelfCommands};
    use crate::storage::test_temp::TempDir;
    use crate::storage::{Store, StorePaths};

    struct MapSource {
        pages: HashMap<String, Fetched>,
        calls: Vec<String>,
    }

    impl IconSource for MapSource {
        fn get(&mut self, url: &str, limit: usize) -> Option<Fetched> {
            self.calls.push(url.to_owned());
            let fetched = self.pages.get(url)?.clone();
            if fetched.body.is_empty() || fetched.body.len() > limit {
                return None;
            }
            Some(fetched)
        }
    }

    struct TagDecode;

    impl IconDecoder for TagDecode {
        fn decode(&mut self, bytes: &[u8], content_type: Option<&str>) -> Option<RgbaImage> {
            if content_type.is_some_and(|kind| kind.contains("svg")) {
                return None;
            }
            if bytes.starts_with(b"PNGOK") {
                return RgbaImage::new(1, 1, vec![9, 8, 7, 255]);
            }
            if bytes.starts_with(b"HUGE") {
                return RgbaImage::new(300, 1, vec![1; 1200]);
            }
            None
        }
    }

    fn page(url: &str, body: &str, content_type: &str) -> Fetched {
        Fetched {
            final_url: url.to_owned(),
            content_type: Some(content_type.to_owned()),
            body: body.as_bytes().to_vec(),
        }
    }

    fn bytes_at(url: &str, body: &[u8], content_type: &str) -> Fetched {
        Fetched {
            final_url: url.to_owned(),
            content_type: Some(content_type.to_owned()),
            body: body.to_vec(),
        }
    }

    fn store() -> (TempDir, Store) {
        let temp = TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("local-cache"),
            user_profile: temp.path().join("profile"),
            local_app_data: temp.path().join("local"),
        })
        .unwrap();
        (temp, store)
    }

    #[test]
    fn page_icon_comes_before_root_favicon() {
        let html = r#"<html><head>
            <link rel="shortcut icon" href="/a.png">
            <link href="touch.png" rel="apple-touch-icon">
        </head></html>"#;
        let found = icon_candidates("https://example.com/docs/index.html", Some(html));
        assert_eq!(
            found,
            vec![
                "https://example.com/a.png".to_owned(),
                "https://example.com/docs/touch.png".to_owned(),
                "https://example.com/favicon.ico".to_owned(),
            ]
        );
    }

    #[test]
    fn missing_link_falls_back_to_favicon_ico_only_once() {
        let found = icon_candidates("https://example.com/a/?q=1", None);
        assert_eq!(found, vec!["https://example.com/favicon.ico".to_owned()]);
        let html = r#"<link rel="icon" href="/favicon.ico">"#;
        let found = icon_candidates("http://example.com:8080/a", Some(html));
        assert_eq!(
            found,
            vec!["http://example.com:8080/favicon.ico".to_owned()]
        );
    }

    #[test]
    fn svg_is_skipped_and_relative_forms_resolve() {
        let html = r#"
            <link rel="icon" type="image/svg+xml" href="/icon.svg">
            <link rel="mask-icon" href="/mask.svg">
            <link rel="icon" href="//cdn.example/f.png">
            <link rel="icon" href="https://cdn.example/ok.png?v=1">
        "#;
        let found = icon_candidates("https://example.com/a/b", Some(html));
        assert_eq!(
            found,
            vec![
                "https://cdn.example/f.png".to_owned(),
                "https://cdn.example/ok.png?v=1".to_owned(),
                "https://example.com/favicon.ico".to_owned(),
            ]
        );
    }

    #[test]
    fn javascript_and_non_http_pages_have_no_icon_request() {
        let html =
            r#"<link rel="icon" href="javascript:alert(1)"><link rel="icon" href="note.png">"#;
        let found = icon_candidates("https://example.com/dir/page", Some(html));
        assert_eq!(
            found,
            vec![
                "https://example.com/dir/note.png".to_owned(),
                "https://example.com/favicon.ico".to_owned(),
            ]
        );
        assert!(icon_candidates("ftp://example.com/a", None).is_empty());
        assert!(icon_cache_key("ftp://example.com/a").is_none());
        assert!(icon_cache_key("https://user:pw@example.com/a").is_none());
    }

    #[test]
    fn html_entities_and_data_urls_are_candidates() {
        let html = r#"<link rel="icon" href="/icons/a.png?x=1&amp;y=2">
            <link rel="icon" href="data:image/png;base64,UE5HT0s=">"#;
        let found = icon_candidates("https://example.com/a", Some(html));
        assert_eq!(found[0], "https://example.com/icons/a.png?x=1&y=2");
        assert!(found[1].starts_with("data:image/png;base64,"));
        assert_eq!(data_image(&found[1]).unwrap(), b"PNGOK");
    }

    #[test]
    fn ipv6_origin_favicon_keeps_brackets_and_port() {
        let found = icon_candidates("https://[::1]:8443/a", None);
        assert_eq!(found, vec!["https://[::1]:8443/favicon.ico".to_owned()]);
    }

    #[test]
    fn declared_icon_is_cached_and_the_shelf_file_has_no_icon_bytes() {
        let (_temp, store) = store();
        let commands = ShelfCommands::open_at(store.clone(), || 10).unwrap();
        let outcome = commands
            .add_web_refs(
                None,
                &[IncomingWeb {
                    url: "https://example.com/docs/page".to_owned(),
                    title: "Example".to_owned(),
                }],
            )
            .unwrap();
        assert_eq!(outcome.added, 1);
        let id = outcome.group.unwrap().id;
        let shelf_path = store.data_dir().join("shelves").join(format!("{id}.json"));
        let before = std::fs::read(&shelf_path).unwrap();
        let text = String::from_utf8(before.clone()).unwrap();
        assert!(text.contains("https://example.com/docs/page"));
        assert!(!text.contains("PNGOK"));
        assert!(!text.to_ascii_lowercase().contains("icon"));

        let page_url = "https://example.com/docs/page";
        let icon_url = "https://example.com/docs/a.png";
        let mut source = MapSource {
            pages: HashMap::from([
                (
                    page_url.to_owned(),
                    page(
                        page_url,
                        r#"<link rel="icon" href="a.png">"#,
                        "text/html; charset=utf-8",
                    ),
                ),
                (
                    icon_url.to_owned(),
                    bytes_at(icon_url, b"PNGOK", "image/png"),
                ),
            ]),
            calls: Vec::new(),
        };
        let cache = WebIconCache::new(store.data_dir());
        let image = load_web_icon(page_url, &cache, &mut source, &mut TagDecode).unwrap();
        assert_eq!(image.pixels, vec![9, 8, 7, 255]);
        assert_eq!(source.calls, vec![page_url.to_owned(), icon_url.to_owned()]);
        assert_eq!(std::fs::read(&shelf_path).unwrap(), before);
        assert!(
            cache
                .dir()
                .join(cache_file_name(icon_cache_key(page_url).unwrap().as_str()).unwrap())
                .is_file()
                || cache
                    .load(icon_cache_key(page_url).unwrap().as_str())
                    .is_some()
        );

        source.calls.clear();
        let again = load_web_icon(page_url, &cache, &mut source, &mut TagDecode).unwrap();
        assert_eq!(again, image);
        assert!(source.calls.is_empty());
    }

    #[test]
    fn failed_fetches_leave_the_saved_ref_and_write_no_cache() {
        let (_temp, store) = store();
        let commands = ShelfCommands::open_at(store.clone(), || 10).unwrap();
        let outcome = commands
            .add_web_refs(
                None,
                &[IncomingWeb {
                    url: "https://example.com/missing".to_owned(),
                    title: "Missing".to_owned(),
                }],
            )
            .unwrap();
        let id = outcome.group.unwrap().id;
        let shelf_path = store.data_dir().join("shelves").join(format!("{id}.json"));
        let before = std::fs::read(&shelf_path).unwrap();
        let mut source = MapSource {
            pages: HashMap::new(),
            calls: Vec::new(),
        };
        let cache = WebIconCache::new(store.data_dir());
        let image = load_web_icon(
            "https://example.com/missing",
            &cache,
            &mut source,
            &mut TagDecode,
        );
        assert!(image.is_none());
        assert_eq!(
            source.calls,
            vec![
                "https://example.com/missing".to_owned(),
                "https://example.com/favicon.ico".to_owned(),
            ]
        );
        assert_eq!(std::fs::read(&shelf_path).unwrap(), before);
        assert!(!cache.dir().exists());
    }

    #[test]
    fn broken_declared_icon_uses_favicon_ico() {
        let page_url = "https://example.com/a";
        let mut source = MapSource {
            pages: HashMap::from([(
                page_url.to_owned(),
                page(
                    page_url,
                    r#"<link rel="icon" href="/gone.png">"#,
                    "text/html",
                ),
            )]),
            calls: Vec::new(),
        };
        source.pages.insert(
            "https://example.com/favicon.ico".to_owned(),
            bytes_at("https://example.com/favicon.ico", b"PNGOK", "image/x-icon"),
        );
        let (_temp, store) = store();
        let cache = WebIconCache::new(store.data_dir());
        let image = load_web_icon(page_url, &cache, &mut source, &mut TagDecode).unwrap();
        assert_eq!(image.width, 1);
        assert!(
            source
                .calls
                .contains(&"https://example.com/gone.png".to_owned())
        );
        assert!(
            source
                .calls
                .contains(&"https://example.com/favicon.ico".to_owned())
        );
    }

    #[test]
    fn undecodable_bytes_and_oversized_images_try_the_next_candidate() {
        let page_url = "https://example.com/a";
        let mut source = MapSource {
            pages: HashMap::from([
                (
                    page_url.to_owned(),
                    page(page_url, "<html></html>", "text/html"),
                ),
                (
                    "https://example.com/favicon.ico".to_owned(),
                    bytes_at("https://example.com/favicon.ico", b"HUGE!!!!", "image/png"),
                ),
            ]),
            calls: Vec::new(),
        };
        let (_temp, store) = store();
        let cache = WebIconCache::new(store.data_dir());
        assert!(load_web_icon(page_url, &cache, &mut source, &mut TagDecode).is_none());
        assert!(!cache.dir().exists());
    }

    #[test]
    fn corrupt_cache_is_discarded_and_fetched_again() {
        let page_url = "https://example.com/a";
        let (_temp, store) = store();
        let cache = WebIconCache::new(store.data_dir());
        let key = icon_cache_key(page_url).unwrap();
        std::fs::create_dir_all(cache.dir()).unwrap();
        let path = cache.path_for(&key).unwrap();
        std::fs::write(&path, b"not-a-cache").unwrap();
        let mut source = MapSource {
            pages: HashMap::from([(
                "https://example.com/favicon.ico".to_owned(),
                bytes_at("https://example.com/favicon.ico", b"PNGOK", "image/x-icon"),
            )]),
            calls: Vec::new(),
        };
        let image = load_web_icon(page_url, &cache, &mut source, &mut TagDecode).unwrap();
        assert_eq!(image.pixels, vec![9, 8, 7, 255]);
        assert!(cache.load(&key).is_some());
    }

    #[test]
    fn redirect_location_stays_on_http() {
        assert_eq!(
            resolve_icon_redirect("https://example.com/a", "/favicon.ico"),
            Some("https://example.com/favicon.ico".to_owned())
        );
        assert_eq!(
            resolve_icon_redirect("https://example.com/a", "https://cdn.example/f.png"),
            Some("https://cdn.example/f.png".to_owned())
        );
        assert!(resolve_icon_redirect("https://example.com/a", "file:///tmp/x").is_none());
        assert!(resolve_icon_redirect("https://example.com/a", "javascript:alert(1)").is_none());
    }

    #[test]
    fn fit_icon_edge_shrinks_the_long_side() {
        assert_eq!(fit_icon_edge(16, 16), Some((16, 16)));
        assert_eq!(fit_icon_edge(512, 256), Some((256, 128)));
        assert_eq!(fit_icon_edge(100, 400), Some((64, 256)));
        assert!(fit_icon_edge(0, 10).is_none());
        assert!(fit_icon_edge(WEB_ICON_SOURCE_EDGE + 1, 10).is_none());
    }

    #[test]
    fn uppercase_page_is_requested_in_canonical_form() {
        let mut source = MapSource {
            pages: HashMap::new(),
            calls: Vec::new(),
        };
        let (_temp, store) = store();
        let cache = WebIconCache::new(store.data_dir());
        assert!(
            load_web_icon("HTTP://Example.COM/a/", &cache, &mut source, &mut TagDecode).is_none()
        );
        assert_eq!(
            source.calls,
            vec![
                "http://example.com/a/".to_owned(),
                "http://example.com/favicon.ico".to_owned(),
            ]
        );
    }
}
