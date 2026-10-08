//! 收纳引用的路径规范化和去重键。
//!
//! 比较的是路径段，不是字符串前缀。`C:\A` 不会被当成 `C:\AB` 的前缀，
//! 段名里的 `foo..` 也不会被当成上级目录。

use super::error::ShelfError;

/// 规范化之后的绝对路径。
///
/// `stored` 保留第一次见到的大小写，分隔符已经统一。`key` 用于同一分组内的去重。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPath {
    pub stored: String,
    pub key: String,
    pub name: String,
}

/// 把拖入的路径整理成可保存、可比较的绝对路径。
///
/// Windows 绝对路径（盘符或 UNC）按架构文档「收纳」的规则处理。
/// 非 Windows 的测试构建还接受以单个 `/` 开头的绝对路径，供临时目录断言原文件仍在。
pub fn normalize_path(raw: &str) -> Result<NormalizedPath, ShelfError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(invalid(raw));
    }
    if is_windows_candidate(raw) {
        return normalize_windows(raw);
    }
    #[cfg(not(windows))]
    if let Some(path) = normalize_posix(raw)? {
        return Ok(path);
    }
    Err(invalid(raw))
}

fn invalid(path: &str) -> ShelfError {
    ShelfError::InvalidPath {
        path: path.to_owned(),
    }
}

fn is_windows_candidate(raw: &str) -> bool {
    raw.starts_with(r"\\") || raw.starts_with("//") || looks_like_drive(raw)
}

fn looks_like_drive(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn normalize_windows(raw: &str) -> Result<NormalizedPath, ShelfError> {
    let unified = raw.replace('/', r"\");
    let expanded = expand_verbatim(&unified);
    if expanded.starts_with(r"\\") {
        return normalize_unc(raw, &expanded);
    }
    if looks_like_drive(&expanded) {
        return normalize_drive(raw, &expanded);
    }
    Err(invalid(raw))
}

/// `\\?\UNC\...` 展开成 `\\server\...`，其他 `\\?\` 去掉前缀。
fn expand_verbatim(path: &str) -> String {
    let Some(rest) = strip_ascii_prefix(path, r"\\?\") else {
        return path.to_owned();
    };
    if let Some(body) = strip_ascii_prefix(rest, r"UNC\") {
        return format!("\\\\{body}");
    }
    rest.to_owned()
}

fn strip_ascii_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let prefix = prefix.as_bytes();
    if bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

fn normalize_drive(raw: &str, path: &str) -> Result<NormalizedPath, ShelfError> {
    let bytes = path.as_bytes();
    if bytes.len() < 3 || bytes[2] != b'\\' {
        return Err(invalid(raw));
    }
    let drive = &path[..2];
    let mut end = path.len();
    while end > 3 && path.as_bytes()[end - 1] == b'\\' {
        end -= 1;
    }
    let segments = resolve_segments(raw, &path[3..end], '\\')?;
    let stored = if segments.is_empty() {
        format!("{drive}\\")
    } else {
        format!("{drive}\\{}", segments.join("\\"))
    };
    let name = segments.last().cloned().unwrap_or_else(|| stored.clone());
    Ok(finish(stored, name))
}

fn normalize_unc(raw: &str, path: &str) -> Result<NormalizedPath, ShelfError> {
    let parts: Vec<&str> = path.split('\\').filter(|part| !part.is_empty()).collect();
    if parts.len() < 2 {
        return Err(invalid(raw));
    }
    let server = parts[0];
    let share = parts[1];
    if !is_unc_root(server) || !is_unc_root(share) {
        return Err(invalid(raw));
    }
    let mut segments = Vec::new();
    for part in parts.into_iter().skip(2) {
        push_segment(raw, &mut segments, part)?;
    }
    let stored = if segments.is_empty() {
        format!("\\\\{server}\\{share}")
    } else {
        format!("\\\\{server}\\{share}\\{}", segments.join("\\"))
    };
    let name = segments.last().cloned().unwrap_or_else(|| share.to_owned());
    Ok(finish(stored, name))
}

fn is_unc_root(part: &str) -> bool {
    valid_segment(part) && part != "." && part != ".." && part != "?"
}

fn resolve_segments(raw: &str, body: &str, sep: char) -> Result<Vec<String>, ShelfError> {
    let mut segments = Vec::new();
    if body.is_empty() {
        return Ok(segments);
    }
    for part in body.split(sep) {
        push_segment(raw, &mut segments, part)?;
    }
    Ok(segments)
}

fn push_segment(raw: &str, segments: &mut Vec<String>, part: &str) -> Result<(), ShelfError> {
    if part.is_empty() || part == "." {
        return Ok(());
    }
    if part == ".." {
        segments.pop();
        return Ok(());
    }
    if !valid_segment(part) {
        return Err(invalid(raw));
    }
    segments.push(part.to_owned());
    Ok(())
}

fn valid_segment(part: &str) -> bool {
    !part.is_empty()
        && !part.chars().any(|ch| {
            matches!(ch, '<' | '>' | '"' | '|' | '?' | '*' | ':' | '\\' | '/') || ch.is_control()
        })
}

fn finish(stored: String, name: String) -> NormalizedPath {
    let key = fold_key(&stored);
    NormalizedPath { stored, key, name }
}

/// 近似 NTFS 的简单大写。
///
/// 每个字符取 [`char::to_uppercase`]。结果恰好是一个字符时才替换，否则保留原字符。
/// 不按区域设置折叠，也不做随位置变化的 Σ 小写。
pub(crate) fn fold_key(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for ch in text.chars() {
        let mut upper = ch.to_uppercase();
        let Some(first) = upper.next() else {
            folded.push(ch);
            continue;
        };
        if upper.next().is_none() {
            folded.push(first);
        } else {
            folded.push(ch);
        }
    }
    folded
}

#[cfg(not(windows))]
fn normalize_posix(raw: &str) -> Result<Option<NormalizedPath>, ShelfError> {
    if !raw.starts_with('/') || raw.starts_with("//") {
        return Ok(None);
    }
    let unified = raw.replace('\\', "/");
    let mut end = unified.len();
    while end > 1 && unified.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    let segments = resolve_segments(raw, &unified[1..end], '/')?;
    let stored = if segments.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", segments.join("/"))
    };
    let name = segments.last().cloned().unwrap_or_else(|| stored.clone());
    Ok(Some(finish(stored, name)))
}

#[cfg(test)]
mod tests {
    use super::normalize_path;

    fn key(raw: &str) -> String {
        normalize_path(raw).unwrap().key
    }

    fn stored(raw: &str) -> String {
        normalize_path(raw).unwrap().stored
    }

    #[test]
    fn drive_paths_ignore_case_slash_and_trailing_separator() {
        let first = normalize_path(r"C:\A\b.txt").unwrap();
        let second = normalize_path(r"c:/a/B.TXT\").unwrap();
        assert_eq!(first.key, second.key);
        assert_eq!(first.stored, r"C:\A\b.txt");
        assert_eq!(second.stored, r"c:\a\B.TXT");
        assert_eq!(first.name, "b.txt");
        assert_eq!(second.name, "B.TXT");
    }

    #[test]
    fn parent_segments_are_resolved_without_string_prefixes() {
        assert_eq!(key(r"C:\A\B\..\b.txt"), key(r"C:\A\b.txt"));
        assert_eq!(key(r"C:\A\..\AB\x"), key(r"C:\AB\x"));
        assert_ne!(key(r"C:\A\x"), key(r"C:\AB\x"));
        assert_eq!(stored(r"C:\foo..\bar"), r"C:\foo..\bar");
        assert_eq!(stored(r"C:\foo\..\..\..\x"), r"C:\x");
        assert_eq!(stored(r"C:\.."), r"C:\");
        assert_eq!(stored(r"C:\foo\.\bar\\"), r"C:\foo\bar");
    }

    #[test]
    fn verbatim_prefixes_expand_before_comparison() {
        assert_eq!(key(r"\\?\C:\A\b.txt"), key(r"C:\A\b.txt"));
        assert_eq!(stored(r"//?/C:/A/b.txt"), r"C:\A\b.txt");
        assert_eq!(
            key(r"\\?\UNC\server\share\a\..\b.txt"),
            key(r"\\server\share\b.txt")
        );
        assert_eq!(
            stored(r"\\?\unc\Server\Share\File.TXT"),
            r"\\Server\Share\File.TXT"
        );
        assert_eq!(
            normalize_path(r"\\server\share\a\..\..\b").unwrap().stored,
            r"\\server\share\b"
        );
        assert_eq!(normalize_path(r"\\server\share\").unwrap().name, "share");
        assert_eq!(normalize_path(r"C:\").unwrap().name, r"C:\");
    }

    #[test]
    fn relative_device_and_volume_paths_are_rejected() {
        assert!(normalize_path(r"foo\bar").is_err());
        assert!(normalize_path(r"C:foo").is_err());
        assert!(normalize_path(r"\\server").is_err());
        assert!(normalize_path(r"\\.\C:\foo").is_err());
        assert!(normalize_path(r"\\?\Volume{guid}\foo").is_err());
        assert!(normalize_path("").is_err());
        assert!(normalize_path(r"C:\A\b<.txt").is_err());
    }

    #[test]
    fn dedup_key_is_simple_uppercase_not_full_lowercase() {
        assert_ne!(key("C:\\İ"), key("C:\\i"));
        assert_ne!(key("C:\\ẞ"), key("C:\\ß"));
        assert_eq!(key("C:\\σ"), key("C:\\ς"));
        assert_eq!(key("C:\\σ"), key("C:\\Σ"));
        assert_eq!(key("C:\\ı"), key("C:\\i"));
        assert_eq!(key("C:\\ΣA"), key("C:\\σA"));
        assert_eq!(key("C:\\AΣ"), key("C:\\Aς"));
        assert_eq!(key("C:\\AΣ"), key("C:\\Aσ"));
    }
}
