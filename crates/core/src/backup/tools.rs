//! 导出、备份和迁移里的工具副本、存储、启用状态和窗口记录。
//!
//! 密钥在凭据管理器里，这些文件不含密钥原文。
//! 副本在写出或读回时都走 [`lanwork_toolhost::recheck_installed`]，和导入同一套
//! id、清单、路径和大小限制。授权按校验过的清单重新生成，不采用包里的 `grant.json`。
//! `tools/` 下以 `.` 开头的临时目录不进入名单。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::error::BackupError;
use super::package::{self, Kind};
use super::zipstore::StoredFile;
use crate::panel;
use crate::storage::{self, Store};

const TOOL_MAX_ENTRY: u64 = lanwork_toolhost::TOOL_MAX_ENTRY as u64;
const TOOL_MAX_BYTES: u64 = lanwork_toolhost::TOOL_MAX_BYTES as u64;
const TOOL_MAX_FILES: usize = lanwork_toolhost::TOOL_MAX_FILES;

pub(crate) struct CopyGroup {
    files: Vec<(String, Vec<u8>)>,
    total: u64,
    skip: Option<&'static str>,
}

impl CopyGroup {
    pub(crate) fn new() -> Self {
        Self {
            files: Vec::new(),
            total: 0,
            skip: None,
        }
    }
}

/// `tools/<id>/<相对路径>`。临时目录的 id 也拆得开，调用方再决定跳过。
pub(crate) fn split_copy(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("tools/")?;
    let (id, rel) = rest.split_once('/')?;
    if id.is_empty() || rel.is_empty() {
        return None;
    }
    Some((id, rel))
}

pub(crate) fn is_sidecar(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::ToolWindow | Kind::ToolStorage | Kind::ToolEnabled
    )
}

/// 窗口记录去掉 `open`。存储和启用状态去掉清单以外的字段。不合格则 `None`。
pub(crate) fn normalize_sidecar(kind: Kind, bytes: &[u8]) -> Option<Vec<u8>> {
    match kind {
        Kind::ToolWindow => panel::archived_window(bytes),
        Kind::ToolStorage => panel::canonical_storage(bytes),
        Kind::ToolEnabled => panel::canonical_enabled(bytes),
        _ => None,
    }
}

pub(crate) fn skip_line(action: &str, name: &str) -> String {
    let id = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .trim_end_matches(".json");
    format!("工具{action}：{}", show_id(id))
}

/// 把磁盘上的副本收成可放进包的文件。过大、过多或清单不合格的整份跳过。
pub(crate) fn export_copies(store: &Store, groups: BTreeMap<String, CopyGroup>) -> Vec<StoredFile> {
    let mut out = Vec::new();
    for (id, group) in &groups {
        if let Some(reason) = group.skip {
            store.log_warn(&format!("工具未导出：{}：{reason}", show_id(id)));
            continue;
        }
        match lanwork_toolhost::recheck_installed(id, &group.files) {
            Ok(tool) => out.extend(staged_files(&tool)),
            Err(err) => {
                store.log_warn(&format!(
                    "工具未导出：{}：{}",
                    show_id(id),
                    clip(err.detail())
                ));
            }
        }
    }
    out
}

/// 读入一组副本。返回要放进 `keep` 的相对路径，以及成功写回的 id。
pub(crate) fn restore_copies(
    store: &Store,
    groups: BTreeMap<String, Vec<(String, Vec<u8>)>>,
) -> Result<(Vec<String>, BTreeSet<String>), BackupError> {
    let mut keep = Vec::new();
    let mut ids = BTreeSet::new();
    let data_dir = store.data_dir();
    for (id, files) in groups {
        let checked = match lanwork_toolhost::recheck_installed(&id, &files) {
            Ok(tool) => tool,
            Err(err) => {
                store.log_warn(&format!(
                    "工具未恢复：{}：{}",
                    show_id(&id),
                    clip(err.detail())
                ));
                continue;
            }
        };
        let dest = data_dir.join(panel::TOOLS_DIR).join(&checked.id);
        panel::install_copy(
            &data_dir,
            &checked.id,
            &checked.name,
            &checked.description,
            &checked.files,
            &checked.grant,
        )
        .map_err(|_| BackupError::io("无法写入", dest))?;
        keep.extend(staged_names(&checked));
        ids.insert(checked.id);
    }
    Ok((keep, ids))
}

pub(crate) fn push_copy(
    groups: &mut BTreeMap<String, CopyGroup>,
    id: &str,
    rel: &str,
    path: &Path,
) -> Result<(), BackupError> {
    let group = groups.entry(id.to_owned()).or_insert_with(CopyGroup::new);
    if group.skip.is_some() {
        return Ok(());
    }
    let len = storage::fs_file_len(path).map_err(|_| BackupError::io("无法读取", path))?;
    if len > TOOL_MAX_ENTRY || group.total.saturating_add(len) > TOOL_MAX_BYTES {
        group.skip = Some("文件过大");
        group.files.clear();
        group.total = 0;
        return Ok(());
    }
    if group.files.len() >= TOOL_MAX_FILES {
        group.skip = Some("文件过多");
        group.files.clear();
        group.total = 0;
        return Ok(());
    }
    let bytes = storage::fs_read(path).map_err(|_| BackupError::io("无法读取", path))?;
    if bytes.len() as u64 != len {
        group.skip = Some("文件过大");
        group.files.clear();
        group.total = 0;
        return Ok(());
    }
    group.total = group.total.saturating_add(len);
    group.files.push((rel.to_owned(), bytes));
    Ok(())
}

/// 按包内字节写回副本，不重新校验，也不改 `grant.json`。
///
/// 给导入失败后的 `import-` 备份用。路径仍拒绝 `..`。以 `.` 开头或 id 不合格的不写。
pub(crate) fn write_raw_copies(
    store: &Store,
    groups: BTreeMap<String, Vec<(String, Vec<u8>)>>,
) -> Result<(Vec<String>, BTreeSet<String>), BackupError> {
    let mut keep = Vec::new();
    let mut ids = BTreeSet::new();
    let data_dir = store.data_dir();
    for (id, files) in groups {
        if id.starts_with('.') || storage::validate_id(&id).is_err() {
            store.log_warn(&format!("工具未恢复：{}：路径不合格", show_id(&id)));
            continue;
        }
        let dest = data_dir.join(panel::TOOLS_DIR).join(&id);
        for (rel, bytes) in &files {
            let path = match tool_file(&dest, rel) {
                Some(path) => path,
                None => {
                    return Err(BackupError::entry(
                        &format!("tools/{id}/{rel}"),
                        "压缩包含有不允许的路径",
                    ));
                }
            };
            if let Some(parent) = path.parent() {
                storage::fs_create_dir_all(parent)
                    .map_err(|_| BackupError::io("无法创建目录", parent))?;
            }
            storage::atomic_write(&path, bytes).map_err(|_| BackupError::io("无法写入", &path))?;
            keep.push(format!("tools/{id}/{rel}"));
        }
        ids.insert(id);
    }
    Ok((keep, ids))
}

/// 删掉临时目录，以及这次没有成功恢复的工具目录。符号链接只删链接本身。
pub(crate) fn finish_tools(
    data_dir: &Path,
    restored: &BTreeSet<String>,
) -> Result<(), BackupError> {
    let tools = data_dir.join(panel::TOOLS_DIR);
    if !storage::fs_exists(&tools) {
        return Ok(());
    }
    let entries = storage::fs_read_dir(&tools).map_err(|_| BackupError::io("无法读取", &tools))?;
    for entry in entries {
        let Some(name) = entry.path.file_name().and_then(|item| item.to_str()) else {
            continue;
        };
        let meta = match std::fs::symlink_metadata(&entry.path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let remove = meta.file_type().is_symlink()
            || name.starts_with('.')
            || (meta.is_dir() && !restored.contains(name))
            || meta.is_file();
        if !remove {
            continue;
        }
        remove_path(&entry.path)?;
    }
    Ok(())
}

fn staged_files(tool: &lanwork_toolhost::RecheckedTool) -> Vec<StoredFile> {
    let mut out = Vec::new();
    for (rel, bytes) in &tool.files {
        out.push(StoredFile {
            name: format!("tools/{}/{rel}", tool.id),
            bytes: bytes.clone(),
        });
    }
    out.push(StoredFile {
        name: format!("tools/{}/{}", tool.id, panel::CARD_FILE),
        bytes: panel::card_bytes(&tool.id, &tool.name, &tool.description),
    });
    out.push(StoredFile {
        name: format!("tools/{}/{}", tool.id, panel::GRANT_FILE),
        bytes: tool.grant.clone(),
    });
    out
}

fn staged_names(tool: &lanwork_toolhost::RecheckedTool) -> Vec<String> {
    let mut names: Vec<String> = tool
        .files
        .iter()
        .map(|(rel, _)| format!("tools/{}/{rel}", tool.id))
        .collect();
    names.push(format!("tools/{}/{}", tool.id, panel::CARD_FILE));
    names.push(format!("tools/{}/{}", tool.id, panel::GRANT_FILE));
    names
}

fn tool_file(dest: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = dest.to_path_buf();
    if relative.is_empty() {
        return None;
    }
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

fn remove_path(path: &Path) -> Result<(), BackupError> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| BackupError::io("无法删除", path))?;
    let result = if meta.file_type().is_symlink() || meta.is_file() {
        storage::fs_remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    };
    result.map_err(|_| BackupError::io("无法删除", path))
}

fn show_id(id: &str) -> String {
    let mut shown: String = id.chars().filter(|ch| !ch.is_control()).take(80).collect();
    if id.chars().filter(|ch| !ch.is_control()).count() > 80 {
        shown.push('…');
    }
    if shown.is_empty() {
        "路径不合格".to_owned()
    } else {
        shown
    }
}

fn clip(detail: &str) -> String {
    let mut shown: String = detail
        .chars()
        .filter(|ch| !ch.is_control())
        .take(120)
        .collect();
    if detail.chars().filter(|ch| !ch.is_control()).count() > 120 {
        shown.push('…');
    }
    shown
}

/// 供 [`super::package::data_file_names`] 列出已安装副本。临时目录不在里面。
pub(crate) fn copy_names(data_dir: &Path) -> Result<Vec<String>, BackupError> {
    let root = package::join(data_dir, "tools");
    if !storage::fs_exists(&root) {
        return Ok(Vec::new());
    }
    let entries = storage::fs_read_dir(&root).map_err(|_| BackupError::io("无法读取", &root))?;
    let mut names = Vec::new();
    for entry in entries {
        let Some(id) = entry.path.file_name().and_then(|item| item.to_str()) else {
            continue;
        };
        if id.starts_with('.') || storage::validate_id(id).is_err() || !is_real_dir(&entry.path) {
            continue;
        }
        walk_files(&entry.path, &format!("tools/{id}"), 0, &mut names)?;
    }
    Ok(names)
}

fn walk_files(
    dir: &Path,
    prefix: &str,
    depth: u32,
    names: &mut Vec<String>,
) -> Result<(), BackupError> {
    if depth > 32 {
        return Err(BackupError::Rejected("工具副本层级过深"));
    }
    let entries = storage::fs_read_dir(dir).map_err(|_| BackupError::io("无法读取", dir))?;
    for entry in entries {
        let Some(file_name) = entry.path.file_name().and_then(|item| item.to_str()) else {
            continue;
        };
        if file_name.is_empty() || file_name == "." || file_name == ".." {
            continue;
        }
        let rel = format!("{prefix}/{file_name}");
        if is_real_dir(&entry.path) {
            walk_files(&entry.path, &rel, depth + 1, names)?;
        } else if is_real_file(&entry.path) {
            names.push(rel);
        }
    }
    Ok(())
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}
