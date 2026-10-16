//! 导出、备份和迁移里的工具副本、存储、启用状态和窗口记录。
//!
//! 密钥在凭据管理器里，这些文件不含密钥原文。
//! 副本在写出或读回时都走 [`lanwork_toolhost::recheck_installed`]，和导入同一套
//! id、清单、路径和大小限制。授权按校验过的清单重新生成，不采用包里的 `grant.json`。
//! 非原样恢复时，入口脚本或清单与本机同 id 副本的内容不同就清除该 id 的密钥。
//! `tools/` 下以 `.` 开头的临时目录不进入名单。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::BackupError;
use super::package::{self, Kind};
use super::zipstore::StoredFile;
use crate::panel::{self, EnabledBytes};
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

/// 读入一组副本。
///
/// 校验失败的 id 放进 `skipped`，调用方保留本机副本和侧车。以 `.` 开头的临时目录不保留。
/// `clear_secrets` 是内容与本机不同、要在整次写回成功后再清密钥的 id。这里还不碰凭据管理器。
pub(crate) struct CopyRestore {
    pub keep: Vec<String>,
    pub restored: BTreeSet<String>,
    pub skipped: BTreeSet<String>,
    pub clear_secrets: Vec<String>,
}

pub(crate) fn restore_copies(
    store: &Store,
    groups: BTreeMap<String, Vec<(String, Vec<u8>)>>,
) -> Result<CopyRestore, BackupError> {
    let mut keep = Vec::new();
    let mut restored = BTreeSet::new();
    let mut skipped = BTreeSet::new();
    let mut clear_secrets = Vec::new();
    let data_dir = store.data_dir();
    for (id, files) in groups {
        let before = snapshot_local(&data_dir, &id);
        let checked = match lanwork_toolhost::recheck_installed(&id, &files) {
            Ok(tool) => tool,
            Err(err) => {
                store.log_warn(&format!(
                    "工具未恢复：{}：{}",
                    show_id(&id),
                    clip(err.detail())
                ));
                if !id.starts_with('.') {
                    skipped.insert(id);
                }
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
        if !same_content(before.as_ref(), &files) {
            clear_secrets.push(checked.id.clone());
        }
        keep.extend(staged_names(&checked));
        restored.insert(checked.id);
    }
    Ok(CopyRestore {
        keep,
        restored,
        skipped,
        clear_secrets,
    })
}

/// 写回已经成功恢复的工具的存储和窗口，并按启用状态合并。
///
/// 校验失败的 id 不写包里的侧车。启用文件读不懂时记日志，成功恢复的工具按启用处理。
pub(crate) fn write_restored_sidecars(
    store: &Store,
    sidecars: &[&StoredFile],
    restored: &BTreeSet<String>,
    skipped: &BTreeSet<String>,
    keep: &mut BTreeSet<String>,
) -> Result<(), BackupError> {
    let mut saw_enabled = false;
    let mut package_disabled = None;
    for file in sidecars {
        let kind = package::classify(&file.name)?;
        if matches!(kind, Kind::ToolEnabled) {
            saw_enabled = true;
            match panel::decode_enabled(&file.bytes) {
                EnabledBytes::Known(set) => package_disabled = Some(set),
                EnabledBytes::Corrupt | EnabledBytes::Unsupported => {
                    store.log_warn(&skip_line("未恢复", &file.name));
                }
            }
            continue;
        }
        let Some(id) = sidecar_id(&file.name) else {
            continue;
        };
        if !restored.contains(&id) {
            continue;
        }
        match normalize_sidecar(kind, &file.bytes) {
            Some(bytes) => {
                write_named(store, &file.name, &bytes)?;
                keep.insert(file.name.clone());
            }
            None => {
                store.log_warn(&skip_line("未恢复", &file.name));
                if storage::fs_exists(&package::join(&store.data_dir(), &file.name)) {
                    keep.insert(file.name.clone());
                }
            }
        }
    }
    let local = local_disabled(&store.data_dir());
    let package = if saw_enabled {
        package_disabled.as_ref()
    } else {
        None
    };
    let merged = merge_enabled(&local, package, restored, skipped);
    if !merged.is_empty() {
        let bytes = panel::encode_enabled(&merged);
        write_named(store, "tool-enabled.json", &bytes)?;
        keep.insert("tool-enabled.json".to_owned());
    }
    Ok(())
}

/// 成功恢复的工具按包里的关闭名单；包里没有这份文件时它们是启用的。
/// 校验失败的工具留在本机原来的关闭名单里。两边都没有的 id 不留下。
pub(crate) fn merge_enabled(
    local: &BTreeSet<String>,
    package: Option<&BTreeSet<String>>,
    restored: &BTreeSet<String>,
    skipped: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(package) = package {
        for id in restored {
            if package.contains(id) {
                out.insert(id.clone());
            }
        }
    }
    for id in skipped {
        if local.contains(id) {
            out.insert(id.clone());
        }
    }
    out
}

/// 本机这个工具目录下的白名单相对路径。没有目录时是空的。
pub(crate) fn files_under_tool(data_dir: &Path, id: &str) -> Result<Vec<String>, BackupError> {
    let dir = data_dir.join(panel::TOOLS_DIR).join(id);
    if !is_real_dir(&dir) {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    walk_files(&dir, &format!("tools/{id}"), 0, &mut names)?;
    Ok(names)
}

pub(crate) fn existing_sidecars(data_dir: &Path, id: &str) -> Vec<String> {
    let mut names = Vec::new();
    for name in [
        format!("tool-storage/{id}.json"),
        format!("tool-windows/{id}.json"),
    ] {
        if storage::fs_exists(&package::join(data_dir, &name)) {
            names.push(name);
        }
    }
    names
}

/// 导出时丢掉 `lanwork.` 前缀和以 `.` 开头的 id，其余仍写出。读不懂时是 `None`。
pub(crate) fn export_enabled(bytes: &[u8]) -> Option<Vec<u8>> {
    let EnabledBytes::Known(set) = panel::decode_enabled(bytes) else {
        return None;
    };
    let kept = set.into_iter().filter(|id| tool_id_allowed(id)).collect();
    Some(panel::encode_enabled(&kept))
}

/// 启用文件里有不允许的工具 id 时拒绝整包。读不懂的文件仍留给后面跳过。
pub(crate) fn reject_disallowed_enabled(name: &str, bytes: &[u8]) -> Result<(), BackupError> {
    let EnabledBytes::Known(set) = panel::decode_enabled(bytes) else {
        return Ok(());
    };
    if set.iter().any(|id| !tool_id_allowed(id)) {
        return Err(BackupError::entry(name, "压缩包含有不允许的路径"));
    }
    Ok(())
}

pub(crate) fn tool_id_allowed(id: &str) -> bool {
    lanwork_toolhost::valid_id(id) && !lanwork_toolhost::imported_prefix_forbidden(id)
}

/// 记下这次要清的工具 id，并调用产品密钥删除。测试用的记录不影响非 Windows 上的空操作。
pub(crate) fn clear_product_secrets(ids: &[String]) -> Result<(), BackupError> {
    for id in ids {
        lanwork_toolhost::delete_product_secrets(id)
            .map_err(|_| BackupError::Rejected("无法清除密钥"))?;
        #[cfg(test)]
        record_cleared_secret(id);
    }
    Ok(())
}

#[cfg(test)]
fn record_cleared_secret(id: &str) {
    SECRET_CAPTURE.with(|slot| {
        if let Some(list) = slot.borrow_mut().as_mut() {
            list.push(id.to_owned());
        }
    });
}

#[cfg(test)]
thread_local! {
    static SECRET_CAPTURE: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct SecretCapture;

#[cfg(test)]
impl SecretCapture {
    pub(crate) fn begin() -> Self {
        SECRET_CAPTURE.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
        Self
    }

    pub(crate) fn ids(&self) -> Vec<String> {
        SECRET_CAPTURE.with(|slot| slot.borrow().clone().unwrap_or_default())
    }
}

#[cfg(test)]
impl Drop for SecretCapture {
    fn drop(&mut self) {
        SECRET_CAPTURE.with(|slot| *slot.borrow_mut() = None);
    }
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

/// 删掉临时目录、符号链接和散落文件，以及既没有成功恢复、也不是校验失败而留下的工具目录。
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

fn write_named(store: &Store, name: &str, bytes: &[u8]) -> Result<(), BackupError> {
    let path = package::disk_path(store, name)?;
    if let Some(parent) = path.parent() {
        storage::fs_create_dir_all(parent).map_err(|_| BackupError::io("无法创建目录", parent))?;
    }
    storage::atomic_write(&path, bytes).map_err(|_| BackupError::io("无法写入", &path))
}

fn local_disabled(data_dir: &Path) -> BTreeSet<String> {
    match panel::read_enabled(data_dir) {
        Ok(EnabledBytes::Known(set)) => set,
        Ok(EnabledBytes::Corrupt | EnabledBytes::Unsupported) | Err(_) => BTreeSet::new(),
    }
}

fn sidecar_id(name: &str) -> Option<String> {
    let file = name.rsplit('/').next()?;
    let id = file.strip_suffix(".json")?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_owned())
    }
}

struct ContentSnapshot {
    manifest: [u8; 32],
    entry_name: String,
    entry: [u8; 32],
}

fn snapshot_local(data_dir: &Path, id: &str) -> Option<ContentSnapshot> {
    let dir = data_dir.join(panel::TOOLS_DIR).join(id);
    let manifest = storage::fs_read(&dir.join("manifest.toml")).ok()?;
    let entry_name = entry_script_name(&manifest)?;
    let path = tool_file(&dir, &entry_name)?;
    let entry = storage::fs_read(&path).ok()?;
    Some(ContentSnapshot {
        manifest: sha256(&manifest),
        entry_name,
        entry: sha256(&entry),
    })
}

fn same_content(local: Option<&ContentSnapshot>, files: &[(String, Vec<u8>)]) -> bool {
    let Some(local) = local else {
        return false;
    };
    let Some(manifest) = file_bytes(files, "manifest.toml") else {
        return false;
    };
    if sha256(manifest) != local.manifest {
        return false;
    }
    let Some(entry_name) = entry_script_name(manifest) else {
        return false;
    };
    if entry_name != local.entry_name {
        return false;
    }
    let Some(entry) = file_bytes(files, &entry_name) else {
        return false;
    };
    sha256(entry) == local.entry
}

fn file_bytes<'a>(files: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a [u8]> {
    files
        .iter()
        .find(|(item, _)| item == name)
        .map(|(_, bytes)| bytes.as_slice())
}

fn entry_script_name(manifest: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(manifest).ok()?;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("entry") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        return quoted(rest.trim());
    }
    None
}

fn quoted(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    for ch in chars {
        if ch == quote {
            if out.is_empty() || !out.ends_with(".lua") {
                return None;
            }
            return Some(out);
        }
        out.push(ch);
    }
    None
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
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
