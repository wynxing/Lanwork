//! 在隔离目录生成性能测量用的固定数据。
//!
//! 规模默认值来自 architecture.md「性能测量」。清单个数协议没有写，默认 20 是夹具参数。
//! JSON 带 `schemaVersion` 1。待办、便签和收纳的字段还不是 architecture.md 的 `ExampleDocument`，按 #11、#12、#13、#14 已经列出的模型来写。
//! 那些 issue 落地后如果改了字段，夹具要跟着改。

mod lnk;

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::lnk::{ShellLink, shell_path, write_shell_link};

pub const PROTOCOL_TODOS: usize = 10_000;
pub const PROTOCOL_NOTES: usize = 1_000;
pub const PROTOCOL_NOTE_BYTES: usize = 2048;
pub const PROTOCOL_SHELVES: usize = 20;
pub const PROTOCOL_REFS: usize = 1_000;
pub const PROTOCOL_SHORTCUTS: usize = 5_000;
/// 协议只规定待办条数，没有规定清单个数。
pub const DEFAULT_LISTS: usize = 20;
const FIXTURE_TIME: &str = "2026-01-01T00:00:00Z";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<io::Error> for ToolError {
    fn from(err: io::Error) -> Self {
        Self::new(err.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(err: serde_json::Error) -> Self {
        Self::new(err.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    pub todos: usize,
    pub lists: usize,
    pub notes: usize,
    pub note_bytes: usize,
    pub shelves: usize,
    pub refs: usize,
    pub shortcuts: usize,
}

impl Scale {
    pub const fn protocol() -> Self {
        Self {
            todos: PROTOCOL_TODOS,
            lists: DEFAULT_LISTS,
            notes: PROTOCOL_NOTES,
            note_bytes: PROTOCOL_NOTE_BYTES,
            shelves: PROTOCOL_SHELVES,
            refs: PROTOCOL_REFS,
            shortcuts: PROTOCOL_SHORTCUTS,
        }
    }
}

impl Default for Scale {
    fn default() -> Self {
        Self::protocol()
    }
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub out: PathBuf,
    pub scale: Scale,
    /// `%USERPROFILE%`。用来拒绝正式数据目录和 MayDolist 目录。测试可以传入替身。
    pub user_profile: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GenerateReport {
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub files_dir: PathBuf,
    pub shortcuts_dir: PathBuf,
    pub todos: usize,
    pub lists: usize,
    pub notes: usize,
    pub note_bytes: usize,
    pub shelves: usize,
    pub refs: usize,
    pub shortcuts: usize,
}

pub fn generate(request: &GenerateRequest) -> Result<GenerateReport, ToolError> {
    validate_scale(&request.scale)?;
    let root = lexical_absolute(&request.out);
    refuse_protected(&root, request.user_profile.as_deref())?;
    if let Some(real) = canonical_destination(&root) {
        refuse_protected(&real, request.user_profile.as_deref())?;
    }

    let data_dir = root.join("data");
    let files_dir = root.join("files");
    let shortcuts_dir = root.join("shortcuts");
    let targets_dir = shortcuts_dir.join("targets");
    fs::create_dir_all(data_dir.join("todos"))?;
    fs::create_dir_all(data_dir.join("notes"))?;
    fs::create_dir_all(data_dir.join("shelves"))?;
    fs::create_dir_all(&files_dir)?;
    fs::create_dir_all(&targets_dir)?;

    write_todos(&data_dir, request.scale.todos, request.scale.lists)?;
    write_notes(&data_dir, request.scale.notes, request.scale.note_bytes)?;
    write_shelves(
        &data_dir,
        &files_dir,
        request.scale.shelves,
        request.scale.refs,
    )?;
    write_shortcuts(&shortcuts_dir, &targets_dir, request.scale.shortcuts)?;

    let report = GenerateReport {
        root: root.clone(),
        data_dir,
        files_dir,
        shortcuts_dir,
        todos: request.scale.todos,
        lists: request.scale.lists,
        notes: request.scale.notes,
        note_bytes: request.scale.note_bytes,
        shelves: request.scale.shelves,
        refs: request.scale.refs,
        shortcuts: request.scale.shortcuts,
    };
    write_json(&root.join("manifest.json"), &report)?;
    Ok(report)
}

fn validate_scale(scale: &Scale) -> Result<(), ToolError> {
    if scale.todos > 0 && scale.lists == 0 {
        return Err(ToolError::new("有待办时清单数至少为 1"));
    }
    if scale.refs > 0 && scale.shelves == 0 {
        return Err(ToolError::new("有引用时收纳分组数至少为 1"));
    }
    Ok(())
}

fn refuse_protected(path: &Path, user_profile: Option<&Path>) -> Result<(), ToolError> {
    let Some(profile) = user_profile else {
        return Ok(());
    };
    let path_key = comparison_key(&lexical_absolute(path));
    let profile = lexical_absolute(profile);
    let lanwork = profile.join("Documents").join("Lanwork");
    let maydolist = profile.join("Documents").join("MayDolist");
    if is_within(&path_key, &comparison_key(&lanwork)) {
        return Err(ToolError::new(format!(
            "拒绝写入 {}。测量数据不能放进 %USERPROFILE%\\Documents\\Lanwork",
            path.display()
        )));
    }
    if is_within(&path_key, &comparison_key(&maydolist)) {
        return Err(ToolError::new(format!(
            "拒绝写入 {}。不能碰 %USERPROFILE%\\Documents\\MayDolist",
            path.display()
        )));
    }
    Ok(())
}

fn canonical_destination(path: &Path) -> Option<PathBuf> {
    let mut ancestor = path.to_path_buf();
    let mut popped = Vec::new();
    loop {
        if ancestor.exists() {
            let mut full = fs::canonicalize(&ancestor).ok()?;
            for name in popped.iter().rev() {
                full.push(name);
            }
            return Some(full);
        }
        let name = ancestor.file_name()?.to_os_string();
        if !ancestor.pop() {
            return None;
        }
        popped.push(name);
    }
}

fn write_todos(data_dir: &Path, todos: usize, lists: usize) -> Result<(), ToolError> {
    let counts = spread(todos, lists);
    let todo_width = digits(todos.max(1));
    let list_width = digits(lists.max(1));
    let mut next_todo = 1usize;
    for (index, count) in counts.iter().copied().enumerate() {
        let number = index + 1;
        let id = format!("list-{number:0list_width$}");
        let (name, kind) = if index == 0 {
            ("收件箱".to_string(), "inbox")
        } else {
            (format!("清单 {number}"), "normal")
        };
        let mut items = Vec::with_capacity(count);
        for offset in 0..count {
            let todo_number = next_todo + offset;
            items.push(TodoItem {
                id: format!("todo-{todo_number:0todo_width$}"),
                title: format!("待办 {todo_number:0todo_width$}"),
                completed: false,
                sort: u32::try_from(offset).unwrap_or(u32::MAX),
                due: None,
                remind_at: None,
                recurrence: None,
                source: None,
                current: false,
                current_since: None,
                deleted_at: None,
                original_list_id: None,
                moved_at: None,
            });
        }
        next_todo += count;
        write_json(
            &data_dir.join("todos").join(format!("{id}.json")),
            &TodoList {
                schema_version: SCHEMA_VERSION,
                id,
                name,
                kind,
                sort: u32::try_from(index).unwrap_or(u32::MAX),
                items,
            },
        )?;
    }
    Ok(())
}

fn write_notes(data_dir: &Path, notes: usize, note_bytes: usize) -> Result<(), ToolError> {
    if notes == 0 {
        return Ok(());
    }
    let width = digits(notes);
    for number in 1..=notes {
        let id = format!("note-{number:0width$}");
        write_json(
            &data_dir.join("notes").join(format!("{id}.json")),
            &Note {
                schema_version: SCHEMA_VERSION,
                id: id.clone(),
                title: format!("便签 {number:0width$}"),
                body: note_body(number, note_bytes),
                tags: Vec::new(),
                pinned: false,
                created_at: FIXTURE_TIME.to_string(),
                updated_at: FIXTURE_TIME.to_string(),
                deleted_at: None,
                revision: 1,
            },
        )?;
    }
    Ok(())
}

fn write_shelves(
    data_dir: &Path,
    files_dir: &Path,
    shelves: usize,
    refs: usize,
) -> Result<(), ToolError> {
    let counts = spread(refs, shelves);
    let shelf_width = digits(shelves.max(1));
    let ref_width = digits(refs.max(1));
    let mut next_ref = 1usize;
    for (index, count) in counts.iter().copied().enumerate() {
        let number = index + 1;
        let id = format!("shelf-{number:0shelf_width$}");
        let mut items = Vec::with_capacity(count);
        for offset in 0..count {
            let ref_number = next_ref + offset;
            let file_name = format!("ref-{ref_number:0ref_width$}.txt");
            let path = files_dir.join(&file_name);
            fs::write(&path, format!("ref {ref_number}\n"))?;
            items.push(ShelfRef {
                path: path.to_string_lossy().into_owned(),
                name: file_name,
                folder: false,
                added_at: FIXTURE_TIME.to_string(),
            });
        }
        next_ref += count;
        write_json(
            &data_dir.join("shelves").join(format!("{id}.json")),
            &Shelf {
                schema_version: SCHEMA_VERSION,
                id,
                name: format!("分组 {number}"),
                sort: u32::try_from(index).unwrap_or(u32::MAX),
                todo_id: None,
                refs: items,
            },
        )?;
    }
    Ok(())
}

fn write_shortcuts(
    shortcuts_dir: &Path,
    targets_dir: &Path,
    shortcuts: usize,
) -> Result<(), ToolError> {
    if shortcuts == 0 {
        return Ok(());
    }
    let width = digits(shortcuts);
    let working_dir = shell_path(targets_dir);
    for number in 1..=shortcuts {
        let stem = format!("App {number:0width$}");
        let target_path = targets_dir.join(format!("{stem}.exe"));
        fs::write(&target_path, b"")?;
        let bytes = write_shell_link(&ShellLink {
            target: shell_path(&target_path),
            working_dir: working_dir.clone(),
            arguments: "fixture".to_string(),
        })?;
        fs::write(shortcuts_dir.join(format!("{stem}.lnk")), bytes)?;
    }
    Ok(())
}

fn note_body(index: usize, bytes: usize) -> String {
    let mut body = format!("note-{index}\n");
    if body.len() >= bytes {
        body.truncate(bytes);
        return body;
    }
    body.push_str(&"x".repeat(bytes - body.len()));
    body
}

fn spread(total: usize, buckets: usize) -> Vec<usize> {
    if buckets == 0 {
        return Vec::new();
    }
    let base = total / buckets;
    let extra = total % buckets;
    (0..buckets)
        .map(|index| base + usize::from(index < extra))
        .collect()
}

fn digits(value: usize) -> usize {
    value.to_string().len().max(4)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| path_error(parent, err))?;
    }
    let file = fs::File::create(path).map_err(|err| path_error(path, err))?;
    let mut writer = io::BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn path_error(path: &Path, err: io::Error) -> ToolError {
    ToolError::new(format!("{}：{err}", path.display()))
}

fn lexical_absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn comparison_key(path: &Path) -> Vec<String> {
    let mut key = Vec::new();
    let path = strip_verbatim(&lexical_absolute(path));
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                key.push(prefix.as_os_str().to_string_lossy().to_ascii_lowercase());
            }
            Component::RootDir => key.push("/".to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if key
                    .last()
                    .is_some_and(|part| part != "/" && !part.ends_with(':'))
                {
                    key.pop();
                }
            }
            Component::Normal(part) => {
                key.push(part.to_string_lossy().to_ascii_lowercase());
            }
        }
    }
    key
}

fn is_within(path: &[String], root: &[String]) -> bool {
    path.len() >= root.len() && path[..root.len()] == root[..]
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TodoList {
    schema_version: u32,
    id: String,
    name: String,
    kind: &'static str,
    sort: u32,
    items: Vec<TodoItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TodoItem {
    id: String,
    title: String,
    completed: bool,
    sort: u32,
    due: Option<String>,
    remind_at: Option<String>,
    recurrence: Option<String>,
    source: Option<String>,
    current: bool,
    current_since: Option<String>,
    deleted_at: Option<String>,
    original_list_id: Option<String>,
    moved_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Note {
    schema_version: u32,
    id: String,
    title: String,
    body: String,
    tags: Vec<String>,
    pinned: bool,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    revision: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Shelf {
    schema_version: u32,
    id: String,
    name: String,
    sort: u32,
    todo_id: Option<String>,
    refs: Vec<ShelfRef>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShelfRef {
    path: String,
    name: String,
    folder: bool,
    added_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lnk::parse_shell_link;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lanwork-fixture-{nanos}-{n}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn small_scale() -> Scale {
        Scale {
            todos: 5,
            lists: 2,
            notes: 2,
            note_bytes: 32,
            shelves: 2,
            refs: 3,
            shortcuts: 2,
        }
    }

    #[test]
    fn protocol_defaults_match_the_measurement_section() {
        let scale = Scale::protocol();
        assert_eq!(scale.todos, 10_000);
        assert_eq!(scale.notes, 1_000);
        assert_eq!(scale.note_bytes, 2048);
        assert_eq!(scale.shelves, 20);
        assert_eq!(scale.refs, 1_000);
        assert_eq!(scale.shortcuts, 5_000);
        assert_eq!(scale.lists, DEFAULT_LISTS);
    }

    #[test]
    fn spread_puts_remainder_on_earlier_buckets() {
        assert_eq!(spread(5, 2), vec![3, 2]);
        assert_eq!(spread(1_000, 20), vec![50; 20]);
    }

    #[test]
    fn generates_adjustable_scale_without_github_or_config() {
        let root = scratch();
        let profile = root.join("profile");
        let out = root.join("out");
        let report = generate(&GenerateRequest {
            out: out.clone(),
            scale: small_scale(),
            user_profile: Some(profile),
        })
        .unwrap();

        assert!(!report.data_dir.join("config.json").exists());
        assert!(!report.data_dir.join("github").exists());
        assert!(!report.shortcuts_dir.starts_with(&report.data_dir));
        assert!(!report.files_dir.starts_with(&report.data_dir));

        let list_one: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(report.data_dir.join("todos/list-0001.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(list_one["schemaVersion"], 1);
        assert_eq!(list_one["kind"], "inbox");
        assert_eq!(list_one["name"], "收件箱");
        assert_eq!(list_one["items"].as_array().unwrap().len(), 3);
        assert_eq!(list_one["items"][0]["completed"], false);
        assert!(list_one["items"][0]["deletedAt"].is_null());

        let list_two: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(report.data_dir.join("todos/list-0002.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(list_two["kind"], "normal");
        assert_eq!(list_two["items"].as_array().unwrap().len(), 2);

        let note: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(report.data_dir.join("notes/note-0001.json")).unwrap(),
        )
        .unwrap();
        let body = note["body"].as_str().unwrap();
        assert_eq!(body.len(), 32);
        assert!(body.is_ascii());
        assert_eq!(note["revision"], 1);
        assert!(note["deletedAt"].is_null());

        let shelf: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(report.data_dir.join("shelves/shelf-0001.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(shelf["refs"].as_array().unwrap().len(), 2);
        assert!(shelf["todoId"].is_null());
        for item in shelf["refs"].as_array().unwrap() {
            let path = PathBuf::from(item["path"].as_str().unwrap());
            assert!(path.is_absolute());
            assert!(path.starts_with(&report.files_dir));
            assert!(path.is_file());
            assert_eq!(item["folder"], false);
        }

        let link_path = report.shortcuts_dir.join("App 0001.lnk");
        let parsed = parse_shell_link(&fs::read(&link_path).unwrap()).unwrap();
        let target = report.shortcuts_dir.join("targets").join("App 0001.exe");
        assert!(target.is_file());
        assert_eq!(parsed.target, shell_path(&target));
        assert_eq!(parsed.arguments, "fixture");
        assert_eq!(
            parsed.working_dir,
            shell_path(&report.shortcuts_dir.join("targets"))
        );
        assert!(report.root.join("manifest.json").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn does_not_write_user_documents_or_maydolist() {
        let root = scratch();
        let profile = root.join("Profile");
        let lanwork = profile.join("Documents").join("Lanwork");
        let maydolist = profile.join("Documents").join("MayDolist");
        fs::create_dir_all(&lanwork).unwrap();
        fs::create_dir_all(&maydolist).unwrap();
        fs::write(lanwork.join("marker"), "keep").unwrap();
        fs::write(maydolist.join("marker"), "keep").unwrap();

        let out = root.join("fixture-out");
        generate(&GenerateRequest {
            out,
            scale: small_scale(),
            user_profile: Some(profile.clone()),
        })
        .unwrap();
        assert_eq!(fs::read_to_string(lanwork.join("marker")).unwrap(), "keep");
        assert_eq!(
            fs::read_to_string(maydolist.join("marker")).unwrap(),
            "keep"
        );
        assert_eq!(fs::read_dir(&lanwork).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&maydolist).unwrap().count(), 1);

        let missing = root
            .join("absent-profile")
            .join("Documents")
            .join("Lanwork");
        assert!(!missing.exists());
        generate(&GenerateRequest {
            out: root.join("elsewhere"),
            scale: Scale {
                todos: 0,
                lists: 0,
                notes: 0,
                note_bytes: 0,
                shelves: 0,
                refs: 0,
                shortcuts: 0,
            },
            user_profile: Some(root.join("absent-profile")),
        })
        .unwrap();
        assert!(!missing.exists());
        assert!(
            !root
                .join("absent-profile")
                .join("Documents")
                .join("MayDolist")
                .exists()
        );

        for forbidden in [
            lanwork.clone(),
            lanwork.join("nested"),
            maydolist.clone(),
            profile.join("documents").join("LANWORK"),
            profile.join("Documents").join("maydolist"),
        ] {
            let before = directory_entries(&profile);
            let err = generate(&GenerateRequest {
                out: forbidden,
                scale: small_scale(),
                user_profile: Some(profile.clone()),
            })
            .unwrap_err();
            assert!(
                err.to_string().contains("拒绝写入"),
                "unexpected error: {err}"
            );
            assert_eq!(directory_entries(&profile), before);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_into_protected_directory() {
        let root = scratch();
        let profile = root.join("profile");
        let lanwork = profile.join("Documents").join("Lanwork");
        fs::create_dir_all(&lanwork).unwrap();
        fs::write(lanwork.join("marker"), "keep").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&lanwork, &link).unwrap();
        let err = generate(&GenerateRequest {
            out: link,
            scale: small_scale(),
            user_profile: Some(profile),
        })
        .unwrap_err();
        assert!(err.to_string().contains("拒绝写入"));
        assert_eq!(fs::read_to_string(lanwork.join("marker")).unwrap(), "keep");
        assert_eq!(fs::read_dir(&lanwork).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    fn directory_entries(root: &Path) -> Vec<String> {
        let mut entries = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(read) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                entries.push(entry.path().to_string_lossy().into_owned());
                if entry.path().is_dir() {
                    stack.push(entry.path());
                }
            }
        }
        entries.sort();
        entries
    }
}
