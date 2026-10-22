//! 搜索条里的本机路径：判断、展开、列一层目录。
//!
//! 读目录在调用方的后台线程里进行。这里的 [`load_system`] 自己再开一个线程，
//! 到点就返回，避免断开的 UNC 把调用方挂住。

use std::cmp::Ordering;
use std::path::Path;
use std::time::{Duration, Instant};

use super::prepare::{FieldInput, FieldRole, MatchIndex};
use super::query::HitKind;
use super::rank_hits;

/// 一次最多放进结果的可见条目。
pub const PATH_LIST_LIMIT: usize = 200;

/// 读目录的时限。到点返回 [`PathRead::Timeout`]。
pub const PATH_LIST_TIMEOUT: Duration = Duration::from_millis(1500);

/// 路径指向已经存在的文件时，第一条的名称。
pub const OPEN_FILE_LABEL: &str = "打开这个文件";

/// 时限内没有读完时，这一组只有这一行。
pub const READ_TIMEOUT_LABEL: &str = "读取超时";

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const ENV_PASSES: usize = 8;

/// 环境变量展开后的一次读取结果。调用方再套上使用次数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRead {
    /// 输入不是本机路径。
    NotPath,
    /// 要列出的目录不存在，或打不开。没有路径行。
    Missing { up: Option<String> },
    /// 1.5 秒内没有读完。
    Timeout { up: Option<String> },
    /// 展开后的路径是文件。
    File { path: String, up: Option<String> },
    /// 展开后要列出的那一层。`entries` 已去掉隐藏项和系统项，条数可以超过 [`PATH_LIST_LIMIT`]。
    /// 过滤和排序之后再截到 200 条。
    Directory {
        dir: String,
        filter: String,
        entries: Vec<DirItem>,
        up: Option<String>,
    },
}

/// 目录里的一条。`hidden` 和 `system` 为真时不进入结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirItem {
    pub name: String,
    pub folder: bool,
    pub hidden: bool,
    pub system: bool,
}

/// 排好序、可以画出来的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub path: String,
    pub folder: bool,
    pub kind: Option<HitKind>,
}

/// 路径这一组交给界面之前的形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRows {
    pub open_file: Option<String>,
    pub listed: Vec<Listed>,
    pub timeout: bool,
    pub up: Option<String>,
}

/// 输入以盘符加冒号再加斜杠、`\\`、`~\`、`~/` 或 `%变量名%` 开头。
#[must_use]
pub fn looks_like_local_path(input: &str) -> bool {
    let bytes = input.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    if input.starts_with(r"\\") || input.starts_with("~\\") || input.starts_with("~/") {
        return true;
    }
    starts_with_env(input)
}

/// 展开开头的 `~` 和全部 `%变量名%`，并把 `/` 收成 `\`。
///
/// `lookup` 按变量名返回值；没有时保留 `%变量名%`。比较名字由调用方决定大小写。
#[must_use]
pub fn expand_path(input: &str, lookup: impl Fn(&str) -> Option<String>, home: &str) -> String {
    let mut text = input.to_owned();
    if let Some(rest) = strip_home_prefix(&text)
        && !home.is_empty()
    {
        text = join_home(home, rest);
    }
    text = expand_env(&text, lookup);
    collapse_slashes(&text)
}

/// 选中文件夹之后写回输入框的文本，末尾带 `\`。
#[must_use]
pub fn folder_completion(path: &str) -> String {
    let collapsed = collapse_slashes(&path.replace('/', "\\"));
    if collapsed.ends_with('\\') {
        collapsed
    } else {
        format!("{collapsed}\\")
    }
}

/// 当前列出的目录的上一层。盘符根和 UNC 共享根没有上一层。
#[must_use]
pub fn parent_directory(path: &str) -> Option<String> {
    let collapsed = collapse_slashes(path);
    let trimmed = collapsed.trim_end_matches('\\');
    if is_drive(trimmed) || is_unc_share_root(trimmed) {
        return None;
    }
    let idx = trimmed.rfind('\\')?;
    let parent = &trimmed[..idx];
    if parent.is_empty() || parent == "\\" {
        return None;
    }
    Some(ensure_trailing_dir(parent))
}

/// 按使用次数把目录项排成文件夹在前、文件在后，再保留前 [`PATH_LIST_LIMIT`] 条。
/// 空过滤按名称排序。隐藏项和系统项先去掉，不占这 200 条。
#[must_use]
pub fn arrange(
    dir: &str,
    entries: &[DirItem],
    filter: &str,
    frequency: impl Fn(&str, bool) -> u64,
) -> Vec<Listed> {
    let visible = take_visible(entries);
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for item in visible {
        let path = child_path(dir, &item.name);
        if item.folder {
            folders.push((path, item.name));
        } else {
            files.push((path, item.name));
        }
    }
    let mut listed = order_names(&folders, filter, true, &frequency);
    listed.extend(order_names(&files, filter, false, &frequency));
    listed.truncate(PATH_LIST_LIMIT);
    listed
}

/// Tab 只在当前输入是本机路径、且选中的是文件夹时补全。否则返回 `None`。
#[must_use]
pub fn folder_tab_target(input: &str, folder_path: Option<&str>) -> Option<String> {
    if !looks_like_local_path(input) {
        return None;
    }
    Some(folder_completion(folder_path?))
}

/// 把一次读取收成路径这一组。`frequency` 的参数是完整路径和是否文件夹。
#[must_use]
pub fn present(read: PathRead, frequency: impl Fn(&str, bool) -> u64) -> PathRows {
    match read {
        PathRead::NotPath => PathRows {
            open_file: None,
            listed: Vec::new(),
            timeout: false,
            up: None,
        },
        PathRead::Missing { up } => PathRows {
            open_file: None,
            listed: Vec::new(),
            timeout: false,
            up,
        },
        PathRead::Timeout { up } => PathRows {
            open_file: None,
            listed: Vec::new(),
            timeout: true,
            up,
        },
        PathRead::File { path, up } => PathRows {
            open_file: Some(path),
            listed: Vec::new(),
            timeout: false,
            up,
        },
        PathRead::Directory {
            dir,
            filter,
            entries,
            up,
        } => PathRows {
            open_file: None,
            listed: arrange(&dir, &entries, &filter, frequency),
            timeout: false,
            up,
        },
    }
}

/// 用进程环境变量和用户文件夹读取。不是路径时不访问磁盘。
#[must_use]
pub fn load_system(input: &str) -> PathRead {
    if !looks_like_local_path(input) {
        return PathRead::NotPath;
    }
    let expanded = expand_path(input, system_env, &user_profile());
    let shaped = stat_path(&expanded);
    let up = timeout_up(&shaped);
    match run_bounded(PATH_LIST_TIMEOUT, move || {
        load_within(&shaped, Instant::now() + PATH_LIST_TIMEOUT)
    }) {
        Ok(read) => read,
        Err(()) => PathRead::Timeout { up },
    }
}

/// 跳过隐藏项和系统项。顺序保持输入顺序，不截断。
#[must_use]
fn take_visible(entries: &[DirItem]) -> Vec<DirItem> {
    entries
        .iter()
        .filter(|item| item.is_visible())
        .cloned()
        .collect()
}

impl DirItem {
    fn is_visible(&self) -> bool {
        !self.hidden
            && !self.system
            && !self.name.is_empty()
            && self.name != "."
            && self.name != ".."
    }
}

fn order_names(
    items: &[(String, String)],
    filter: &str,
    folder: bool,
    frequency: &impl Fn(&str, bool) -> u64,
) -> Vec<Listed> {
    if filter_is_empty(filter) {
        let mut ordered = items.to_vec();
        ordered.sort_by(|left, right| name_ord(&left.1, &right.1));
        return ordered
            .into_iter()
            .map(|(path, name)| Listed {
                name,
                path,
                folder,
                kind: None,
            })
            .collect();
    }
    let mut index = MatchIndex::new();
    for (id, (_, name)) in items.iter().enumerate() {
        let Ok(id) = u64::try_from(id) else {
            continue;
        };
        index.insert(
            id,
            &[FieldInput {
                role: FieldRole::Name,
                text: name,
            }],
        );
    }
    let hits = index.query(filter);
    rank_hits(&hits, |id| {
        let Some(item) = usize::try_from(id).ok().and_then(|index| items.get(index)) else {
            return 0;
        };
        frequency(&item.0, folder)
    })
    .into_iter()
    .filter_map(|hit| {
        let index = usize::try_from(hit.id).ok()?;
        let (path, name) = items.get(index)?;
        Some(Listed {
            name: name.clone(),
            path: path.clone(),
            folder,
            kind: Some(hit.kind),
        })
    })
    .collect()
}

fn filter_is_empty(filter: &str) -> bool {
    !filter.chars().any(|ch| !ch.is_whitespace())
}

fn name_ord(left: &str, right: &str) -> Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

fn load_within(shaped: &str, deadline: Instant) -> PathRead {
    if Instant::now() >= deadline {
        return PathRead::Timeout {
            up: timeout_up(shaped),
        };
    }
    match metadata_kind(Path::new(shaped)) {
        Meta::Dir => {
            return read_listing(shaped, "", deadline);
        }
        Meta::File => {
            return PathRead::File {
                path: shaped.to_owned(),
                up: parent_directory(shaped),
            };
        }
        Meta::Missing => {}
    }
    let (dir, filter) = split_dir_filter(shaped);
    if Instant::now() >= deadline {
        return PathRead::Timeout {
            up: parent_directory(&dir),
        };
    }
    match metadata_kind(Path::new(&dir)) {
        Meta::Dir => read_listing(&dir, &filter, deadline),
        Meta::File | Meta::Missing => PathRead::Missing {
            up: parent_directory(&dir),
        },
    }
}

fn read_listing(dir: &str, filter: &str, deadline: Instant) -> PathRead {
    let up = parent_directory(dir);
    match read_one_level(Path::new(dir), deadline) {
        Ok(entries) => PathRead::Directory {
            dir: dir.to_owned(),
            filter: filter.to_owned(),
            entries,
            up,
        },
        Err(ReadFail::TimedOut) => PathRead::Timeout { up },
        Err(ReadFail::Unavailable) => PathRead::Missing { up },
    }
}

enum Meta {
    File,
    Dir,
    Missing,
}

fn metadata_kind(path: &Path) -> Meta {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Meta::Dir,
        Ok(_) => Meta::File,
        Err(_) => Meta::Missing,
    }
}

#[derive(Debug)]
enum ReadFail {
    TimedOut,
    Unavailable,
}

fn read_one_level(dir: &Path, deadline: Instant) -> Result<Vec<DirItem>, ReadFail> {
    let iter = std::fs::read_dir(dir).map_err(|_| ReadFail::Unavailable)?;
    let mut out = Vec::new();
    for entry in iter {
        if Instant::now() >= deadline {
            return Err(ReadFail::TimedOut);
        }
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let item = DirItem {
            name,
            folder: meta.is_dir(),
            hidden: attribute_hidden(&meta),
            system: attribute_system(&meta),
        };
        if !item.is_visible() {
            continue;
        }
        out.push(DirItem {
            hidden: false,
            system: false,
            ..item
        });
    }
    Ok(out)
}

fn attribute_hidden(meta: &std::fs::Metadata) -> bool {
    file_attributes(meta) & FILE_ATTRIBUTE_HIDDEN != 0
}

fn attribute_system(meta: &std::fs::Metadata) -> bool {
    file_attributes(meta) & FILE_ATTRIBUTE_SYSTEM != 0
}

#[cfg(windows)]
fn file_attributes(meta: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt as _;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn file_attributes(_meta: &std::fs::Metadata) -> u32 {
    0
}

fn run_bounded<T: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ()> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("lanwork-path-list".to_owned())
        .spawn(move || {
            let _ = sender.send(work());
        });
    if spawned.is_err() {
        return Err(());
    }
    receiver.recv_timeout(timeout).map_err(|_| ())
}

fn timeout_up(shaped: &str) -> Option<String> {
    let (dir, filter) = split_dir_filter(shaped);
    if filter.is_empty() {
        parent_directory(shaped)
    } else {
        parent_directory(&dir)
    }
}

fn system_env(name: &str) -> Option<String> {
    std::env::vars().find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
}

fn user_profile() -> String {
    std::env::var("USERPROFILE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOME").ok().filter(|value| !value.is_empty()))
        .unwrap_or_default()
}

fn strip_home_prefix(input: &str) -> Option<&str> {
    input
        .strip_prefix("~\\")
        .or_else(|| input.strip_prefix("~/"))
}

fn join_home(home: &str, rest: &str) -> String {
    let home = home.trim_end_matches(['\\', '/']);
    if rest.is_empty() {
        format!("{home}\\")
    } else {
        format!("{home}\\{rest}")
    }
}

fn expand_env(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut current = text.to_owned();
    for _ in 0..ENV_PASSES {
        let next = replace_env_once(&current, &lookup);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn replace_env_once(text: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + '%'.len_utf8()..];
        if let Some(end) = after.find('%') {
            let name = &after[..end];
            if valid_env_name(name)
                && let Some(value) = lookup(name)
            {
                out.push_str(&value);
                rest = &after[end + '%'.len_utf8()..];
                continue;
            }
        }
        out.push('%');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn starts_with_env(input: &str) -> bool {
    let Some(after) = input.strip_prefix('%') else {
        return false;
    };
    let Some(end) = after.find('%') else {
        return false;
    };
    valid_env_name(&after[..end])
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\\', '/', '%'])
}

fn collapse_slashes(path: &str) -> String {
    let slashed: String = path
        .chars()
        .map(|ch| if ch == '/' { '\\' } else { ch })
        .collect();
    let chars: Vec<char> = slashed.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut index = 0;
    if chars.len() >= 2 && chars[0] == '\\' && chars[1] == '\\' {
        out.push('\\');
        out.push('\\');
        index = 2;
    }
    let mut previous_slash = false;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\\' {
            if !previous_slash {
                out.push('\\');
            }
            previous_slash = true;
        } else {
            out.push(ch);
            previous_slash = false;
        }
        index += 1;
    }
    out
}

fn stat_path(path: &str) -> String {
    let collapsed = collapse_slashes(path);
    let trimmed = collapsed.trim_end_matches('\\');
    if is_drive(trimmed) {
        return format!("{trimmed}\\");
    }
    if trimmed.is_empty() {
        return collapsed;
    }
    trimmed.to_owned()
}

fn split_dir_filter(path: &str) -> (String, String) {
    let Some(idx) = path.rfind('\\') else {
        return (path.to_owned(), String::new());
    };
    let dir = ensure_trailing_dir_prefix(&path[..idx]);
    let filter = path[idx + '\\'.len_utf8()..].to_owned();
    (dir, filter)
}

fn ensure_trailing_dir_prefix(dir: &str) -> String {
    if is_drive(dir) {
        format!("{dir}\\")
    } else if dir.is_empty() {
        "\\".to_owned()
    } else {
        dir.to_owned()
    }
}

fn ensure_trailing_dir(path: &str) -> String {
    if is_drive(path) {
        format!("{path}\\")
    } else if path.ends_with('\\') {
        path.to_owned()
    } else {
        format!("{path}\\")
    }
}

fn is_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_unc_share_root(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(r"\\") else {
        return false;
    };
    let rest = rest.trim_end_matches('\\');
    let mut parts = rest.split('\\');
    let server = parts.next().unwrap_or("");
    let share = parts.next();
    let more = parts.next();
    !server.is_empty() && share.is_some_and(|share| !share.is_empty()) && more.is_none()
}

fn child_path(dir: &str, name: &str) -> String {
    if dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributes_are_hidden_or_system(attributes: u32) -> bool {
        attributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
    }

    fn name_has_reading(name: &str) -> bool {
        name.chars()
            .any(|ch| !crate::search::readings(ch).is_empty())
    }

    fn env(name: &str) -> Option<String> {
        match name.to_ascii_uppercase().as_str() {
            "APPDATA" => Some(r"C:\Users\me\AppData\Roaming".to_owned()),
            "USERNAME" => Some("me".to_owned()),
            "EMPTY" => Some(String::new()),
            "NESTED" => Some("%USERNAME%".to_owned()),
            _ => None,
        }
    }

    fn item(name: &str, folder: bool) -> DirItem {
        DirItem {
            name: name.to_owned(),
            folder,
            hidden: false,
            system: false,
        }
    }

    fn names(listed: &[Listed]) -> Vec<&str> {
        listed.iter().map(|item| item.name.as_str()).collect()
    }

    #[test]
    fn path_detection_covers_drive_unc_home_env_and_plain_text() {
        assert!(looks_like_local_path(r"C:\"));
        assert!(looks_like_local_path(r"d:/"));
        assert!(looks_like_local_path(r"C:\Users\Do"));
        assert!(looks_like_local_path(r"\\server\share"));
        assert!(looks_like_local_path(r"\\"));
        assert!(looks_like_local_path(r"~\"));
        assert!(looks_like_local_path("~/Documents"));
        assert!(looks_like_local_path(r"%APPDATA%"));
        assert!(looks_like_local_path(r"%APPDATA%\Microsoft"));
        assert!(looks_like_local_path("%ProgramFiles(x86)%"));
        assert!(!looks_like_local_path("a:b"));
        assert!(!looks_like_local_path("C:"));
        assert!(!looks_like_local_path("C:foo"));
        assert!(!looks_like_local_path("~"));
        assert!(!looks_like_local_path("~Documents"));
        assert!(!looks_like_local_path("%APPDATA"));
        assert!(!looks_like_local_path("%%"));
        assert!(!looks_like_local_path("readme"));
        assert!(!looks_like_local_path(r" C:\"));
        assert_eq!(load_system("a:b"), PathRead::NotPath);
        assert_eq!(load_system("C:"), PathRead::NotPath);
    }

    #[test]
    fn expansion_covers_home_every_variable_and_unknown_names() {
        let home = r"C:\Users\me";
        assert_eq!(
            expand_path(r"%APPDATA%\Lanwork", env, home),
            r"C:\Users\me\AppData\Roaming\Lanwork"
        );
        assert_eq!(
            expand_path(r"C:\Users\%USERNAME%\Desktop", env, home),
            r"C:\Users\me\Desktop"
        );
        assert_eq!(
            expand_path(r"~\Documents", env, home),
            r"C:\Users\me\Documents"
        );
        assert_eq!(expand_path("~/", env, home), r"C:\Users\me\");
        assert_eq!(expand_path(r"~\", env, home), r"C:\Users\me\");
        assert_eq!(expand_path("%NESTED%", env, home), "me");
        assert_eq!(expand_path(r"%NOPE%\a", env, home), r"%NOPE%\a");
        assert_eq!(expand_path("d:/项目", env, home), r"d:\项目");
        assert_eq!(
            expand_path(r"\\server\share\a", env, home),
            r"\\server\share\a"
        );
        assert!(!attributes_are_hidden_or_system(0));
        assert!(attributes_are_hidden_or_system(FILE_ATTRIBUTE_HIDDEN));
        assert!(attributes_are_hidden_or_system(FILE_ATTRIBUTE_SYSTEM));
    }

    #[test]
    fn folders_come_before_files_and_names_sort_within_each() {
        let entries = vec![
            item("b-file", false),
            item("a-dir", true),
            item("b-dir", true),
            item("a-file", false),
        ];
        let listed = arrange(r"C:\Work", &entries, "", |_, _| 0);
        assert_eq!(names(&listed), ["a-dir", "b-dir", "a-file", "b-file"]);
        assert!(listed[0].folder);
        assert!(!listed[2].folder);
        assert_eq!(listed[0].path, r"C:\Work\a-dir");
    }

    #[test]
    fn filter_uses_match_order_and_pinyin_for_names() {
        let entries = vec![
            item("readme", false),
            item("read", false),
            item("notes", false),
            item("文档", false),
        ];
        assert!(name_has_reading("文档"));
        let listed = arrange(r"C:\Work", &entries, "read", |_, _| 0);
        assert_eq!(names(&listed), ["read", "readme"]);
        assert_eq!(listed[0].kind, Some(HitKind::Exact));
        assert_eq!(listed[1].kind, Some(HitKind::Prefix));
        let by_pinyin = arrange(r"C:\Work", &entries, "wendang", |_, _| 0);
        assert_eq!(names(&by_pinyin), ["文档"]);
        assert_eq!(by_pinyin[0].kind, Some(HitKind::Pinyin));
        let usage = arrange(r"C:\Work", &entries, "read", |path, folder| {
            assert!(!folder);
            u64::from(path.ends_with("readme"))
        });
        assert_eq!(names(&usage), ["read", "readme"]);
        let both_prefix = vec![item("ab", false), item("aa", false)];
        let ranked = arrange(r"C:\Work", &both_prefix, "a", |path, _| {
            u64::from(path.ends_with("aa"))
        });
        assert_eq!(names(&ranked), ["aa", "ab"]);
        assert_eq!(ranked[0].kind, Some(HitKind::Prefix));
    }

    #[test]
    fn hidden_and_system_entries_are_left_out_of_the_cap() {
        let mut entries = vec![
            DirItem {
                name: "secret".to_owned(),
                folder: true,
                hidden: true,
                system: false,
            },
            DirItem {
                name: "pagefile".to_owned(),
                folder: false,
                hidden: false,
                system: true,
            },
            item("keep", false),
        ];
        entries.extend((0..200).map(|index| item(&format!("f{index:03}"), false)));
        let taken = take_visible(&entries);
        assert_eq!(taken.len(), 201);
        assert!(
            taken
                .iter()
                .all(|item| item.name != "secret" && item.name != "pagefile")
        );
        let listed = arrange(r"C:\", &entries, "", |_, _| 0);
        assert_eq!(listed.len(), PATH_LIST_LIMIT);
        assert_eq!(listed[0].name, "f000");
        assert!(
            listed.iter().all(|item| item.name != "secret"
                && item.name != "pagefile"
                && item.name != "keep")
        );
    }

    #[test]
    fn cap_is_applied_after_filtering_and_name_order() {
        let mut entries = vec![item("zzz", false)];
        entries.extend((0..204).map(|index| item(&format!("m{index:04}"), false)));
        entries.push(item("aaa", false));
        let listed = arrange(r"C:\Work", &entries, "", |_, _| 0);
        assert_eq!(listed.len(), PATH_LIST_LIMIT);
        assert_eq!(listed[0].name, "aaa");
        assert_eq!(listed[1].name, "m0000");
        assert!(listed.iter().all(|item| item.name != "zzz"));

        let mut scanned = Vec::new();
        scanned.extend((0..200).map(|index| item(&format!("n{index:04}"), false)));
        scanned.push(item("target", false));
        let matched = arrange(r"C:\Work", &scanned, "target", |_, _| 0);
        assert_eq!(names(&matched), ["target"]);
    }

    #[test]
    fn tab_completes_only_while_the_input_is_a_local_path() {
        assert_eq!(
            folder_tab_target(r"C:\Work", Some(r"C:\Work\a-dir")).as_deref(),
            Some(r"C:\Work\a-dir\")
        );
        assert_eq!(folder_tab_target("notes", Some(r"C:\Work\a-dir")), None);
        assert_eq!(folder_tab_target("a:b", Some(r"C:\a")), None);
        assert_eq!(folder_tab_target(r"C:\Work", None), None);
    }

    #[test]
    fn timeout_is_one_row_and_a_file_opens_first() {
        let timed = present(
            PathRead::Timeout {
                up: Some(r"C:\".to_owned()),
            },
            |_, _| 0,
        );
        assert!(timed.timeout);
        assert!(timed.listed.is_empty());
        assert!(timed.open_file.is_none());
        assert_eq!(timed.up.as_deref(), Some(r"C:\"));
        assert_eq!(READ_TIMEOUT_LABEL, "读取超时");
        let file = present(
            PathRead::File {
                path: r"C:\Windows\notepad.exe".to_owned(),
                up: Some(r"C:\Windows\".to_owned()),
            },
            |_, _| 0,
        );
        assert_eq!(file.open_file.as_deref(), Some(r"C:\Windows\notepad.exe"));
        assert!(!file.timeout);
        assert_eq!(OPEN_FILE_LABEL, "打开这个文件");
        assert_eq!(
            parent_directory(r"C:\Windows\notepad.exe").as_deref(),
            Some(r"C:\Windows\")
        );
    }

    #[test]
    fn parent_and_completion_move_one_level() {
        assert_eq!(
            parent_directory(r"C:\Users\Do").as_deref(),
            Some(r"C:\Users\")
        );
        assert_eq!(parent_directory(r"C:\Users").as_deref(), Some(r"C:\"));
        assert_eq!(parent_directory(r"C:\"), None);
        assert_eq!(
            parent_directory(r"\\server\share\folder").as_deref(),
            Some(r"\\server\share\")
        );
        assert_eq!(parent_directory(r"\\server\share"), None);
        assert_eq!(parent_directory(r"\\server\share\"), None);
        assert_eq!(folder_completion(r"C:\Users"), r"C:\Users\");
        assert_eq!(folder_completion(r"d:/项目"), r"d:\项目\");
        assert_eq!(folder_completion(r"C:\Users\"), r"C:\Users\");
    }

    #[test]
    fn slow_read_returns_timeout_and_a_fast_read_returns_the_value() {
        let timed = run_bounded(Duration::from_millis(30), || {
            std::thread::sleep(Duration::from_millis(200));
            7
        });
        assert!(timed.is_err());
        let ready = run_bounded(Duration::from_millis(200), || 7);
        assert_eq!(ready, Ok(7));
        assert_eq!(PATH_LIST_TIMEOUT, Duration::from_millis(1500));
        assert_eq!(PATH_LIST_LIMIT, 200);
    }

    #[test]
    fn reader_scans_the_directory_and_a_missed_deadline_drops_it() {
        let root = std::env::temp_dir().join(format!("lanwork-path-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a-dir")).unwrap();
        std::fs::create_dir_all(root.join("b-dir")).unwrap();
        std::fs::write(root.join("a-file"), b"a").unwrap();
        std::fs::write(root.join(".secret"), b"s").unwrap();
        let listed = read_one_level(&root, Instant::now() + Duration::from_secs(2)).unwrap();
        let mut names: Vec<_> = listed.iter().map(|item| item.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, [".secret", "a-dir", "a-file", "b-dir"]);
        assert!(
            listed
                .iter()
                .any(|item| item.name == "a-dir" && item.folder)
        );
        assert!(
            listed
                .iter()
                .any(|item| item.name == "a-file" && !item.folder)
        );

        let wide = root.join("wide");
        std::fs::create_dir_all(&wide).unwrap();
        for index in 0..205 {
            std::fs::write(wide.join(format!("f{index:04}.txt")), b"x").unwrap();
        }
        let scanned = read_one_level(&wide, Instant::now() + Duration::from_secs(2)).unwrap();
        assert_eq!(scanned.len(), 205);
        let timed = read_one_level(&wide, Instant::now() - Duration::from_millis(1));
        assert!(matches!(timed, Err(ReadFail::TimedOut)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
