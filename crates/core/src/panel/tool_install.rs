//! 已安装工具的副本和存储。
//!
//! 副本放在数据目录的 `tools/<工具 id>/`，存储放在 `tool-storage/<工具 id>.json`。
//! 这两条路径规格仍待定，这里只是这一版的放法，不写成产品规则。
//! 窗口记录在 `tool-windows/<工具 id>.json`。卸载时把这份记录一起删掉。
//! 启用状态仍在 `tool-enabled.json`。密钥不在这里。这个模块不读清单，卡片名称来自安装时写下的 `card.json`。
//! 导出和备份带走副本、存储和窗口记录。临时目录不在名单里。

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::apps_page::ListedTool;
use crate::storage::{SCHEMA_VERSION, atomic_write, is_supported_schema, validate_id};

/// 安装副本所在的目录名。规格仍待定。
pub const TOOLS_DIR: &str = "tools";
/// 工具自己的文本。不放进工具文件夹。规格仍待定。
pub const STORAGE_DIR: &str = "tool-storage";
pub const CARD_FILE: &str = "card.json";
pub const GRANT_FILE: &str = "grant.json";

pub const UPDATE_NOTE: &str = "保留数据";
pub const UNINSTALL_BODY: &str = "会删除这个工具的存储和密钥。";
pub const INVALID_PACKAGE: &str = "清单无效";
pub const UNAUTHORIZED: &str = "未授权";
/// 从某一张卡片的「更新」选到的包，id 不是这张卡片。
pub const WRONG_UPDATE: &str = "不是这个工具的更新包";

const STORAGE_READ_LIMIT: u64 = 8 * 1024 * 1024;

/// 新副本已经在正式目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    Ready,
    /// 旧目录删不掉，还留在 `tools/.previous-<id>/` 或 `tools/.hold-<id>.N/`。
    BackupRemains,
}

/// 把校验过的文件装进数据目录。`files` 的路径相对工具根，只用 `/`。
///
/// 先写入 `tools/.incoming-<id>/`。正式目录换成新副本之后，才删除旧目录。
/// 旧目录删不掉时新副本仍然留着，返回 [`Installed::BackupRemains`]。
/// 不改存储、密钥和窗口记录。
pub fn install_copy(
    data_dir: &Path,
    id: &str,
    name: &str,
    description: &str,
    files: &[(String, Vec<u8>)],
    grant: &[u8],
) -> io::Result<Installed> {
    check_id(id)?;
    let tools = data_dir.join(TOOLS_DIR);
    std::fs::create_dir_all(&tools)?;
    let incoming = scratch_path(&tools, ".incoming-", id);
    let dest = tools.join(id);
    let previous = scratch_path(&tools, ".previous-", id);
    if incoming.exists() {
        std::fs::remove_dir_all(&incoming)?;
    }
    std::fs::create_dir_all(&incoming)?;
    let written = (|| {
        for (relative, bytes) in files {
            if relative == CARD_FILE || relative == GRANT_FILE {
                continue;
            }
            write_relative(&incoming, relative, bytes)?;
        }
        atomic_write(
            &incoming.join(CARD_FILE),
            &encode_card(id, name, description),
        )?;
        atomic_write(&incoming.join(GRANT_FILE), grant)?;
        Ok(())
    })();
    if let Err(err) = written {
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(err);
    }
    // 正式目录还在时，先把占着 `.previous-` 名字的旧备份挪开，不删除。
    let parked = if dest.exists() && previous.exists() {
        let hold = vacant_hold(&tools, id)?;
        if let Err(err) = std::fs::rename(&previous, &hold) {
            let _ = std::fs::remove_dir_all(&incoming);
            return Err(err);
        }
        Some(hold)
    } else {
        None
    };
    if dest.exists()
        && let Err(err) = std::fs::rename(&dest, &previous)
    {
        restore_parked(parked.as_deref(), &previous);
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&incoming, &dest) {
        if !dest.exists() && previous.exists() {
            let _ = std::fs::rename(&previous, &dest);
        }
        restore_parked(parked.as_deref(), &previous);
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(err);
    }
    let mut remains = !discard_tree(&previous);
    if !discard_holds(&tools, id) {
        remains = true;
    }
    Ok(if remains {
        Installed::BackupRemains
    } else {
        Installed::Ready
    })
}

/// 正式目录不在、而 `tools/.previous-<id>/` 还在时，把备份放回正式目录。
///
/// 正式目录还在时不动备份。恢复失败的条目留在原处，返回给调用方写日志。
#[must_use]
pub fn recover_previous(data_dir: &Path) -> Vec<String> {
    let tools = data_dir.join(TOOLS_DIR);
    let Ok(read) = std::fs::read_dir(&tools) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    let mut warnings = Vec::new();
    for name in names {
        let Some(id) = name.strip_prefix(".previous-") else {
            continue;
        };
        if check_id(id).is_err() {
            continue;
        }
        let dest = tools.join(id);
        if dest.exists() {
            continue;
        }
        let backup = tools.join(&name);
        if let Err(err) = std::fs::rename(&backup, &dest) {
            warnings.push(format!("工具副本没有恢复：{id}：{err}"));
        }
    }
    warnings
}

/// 删除副本、半成品和换下来的旧目录、存储，以及 `tool-windows/<id>.json`。
///
/// 不删除 `tool-windows` 目录本身，也不删除密钥。
pub fn remove_installed(data_dir: &Path, id: &str) -> io::Result<()> {
    check_id(id)?;
    let tools = data_dir.join(TOOLS_DIR);
    let dest = tools.join(id);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    if tools.is_dir() {
        let read = std::fs::read_dir(&tools)?;
        let mut scratch = Vec::new();
        for entry in read {
            let entry = entry?;
            let Some(name) = entry.file_name().into_string().ok() else {
                continue;
            };
            if is_scratch(&name, id) {
                scratch.push(entry.path());
            }
        }
        for path in scratch {
            if path.exists() {
                std::fs::remove_dir_all(path)?;
            }
        }
    }
    let storage = storage_path(data_dir, id);
    if storage.exists() {
        std::fs::remove_file(storage)?;
    }
    if let Some(relative) = super::float_shell::record_relative(id) {
        let record = data_dir.join(relative);
        if record.exists() {
            std::fs::remove_file(record)?;
        }
    }
    Ok(())
}

/// 已安装的工具。跳过以 `.` 开头的临时目录，以及卡片 id 和目录名不一致的目录。
#[must_use]
pub fn list_installed(data_dir: &Path) -> Vec<ListedTool> {
    let _ = recover_previous(data_dir);
    let tools = data_dir.join(TOOLS_DIR);
    let Ok(read) = std::fs::read_dir(&tools) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    let mut listed = Vec::new();
    for entry in entries {
        let Some(dir_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if dir_name.starts_with('.') || validate_id(&dir_name).is_err() {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path().join(CARD_FILE)) else {
            continue;
        };
        let Some(card) = decode_card(&bytes) else {
            continue;
        };
        if card.id != dir_name {
            continue;
        }
        let icon = entry.path().join("icon.png");
        listed.push(ListedTool {
            id: card.id,
            name: card.name,
            description: card.description,
            icon_path: icon.is_file().then_some(icon),
        });
    }
    listed
}

#[must_use]
pub fn tool_dir(data_dir: &Path, id: &str) -> Option<PathBuf> {
    if check_id(id).is_err() {
        return None;
    }
    Some(data_dir.join(TOOLS_DIR).join(id))
}

#[must_use]
pub fn storage_path(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join(STORAGE_DIR).join(format!("{id}.json"))
}

/// 文件不存在时是空映射。不认识的 schema 返回错误，调用方不要覆盖。
pub fn read_tool_storage(data_dir: &Path, id: &str) -> io::Result<BTreeMap<String, String>> {
    check_id(id)?;
    let path = storage_path(data_dir, id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => return Err(err),
    };
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > STORAGE_READ_LIMIT {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "存储过大"));
    }
    decode_storage(&bytes).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "存储读不了"))
}

pub fn write_tool_storage(
    data_dir: &Path,
    id: &str,
    entries: &BTreeMap<String, String>,
) -> io::Result<()> {
    check_id(id)?;
    let dir = data_dir.join(STORAGE_DIR);
    std::fs::create_dir_all(&dir)?;
    atomic_write(&storage_path(data_dir, id), &encode_storage(entries))
}

fn check_id(id: &str) -> io::Result<()> {
    if id.starts_with('.') || validate_id(id).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "工具 id 不能作为文件名",
        ));
    }
    Ok(())
}

fn scratch_path(tools: &Path, prefix: &str, id: &str) -> PathBuf {
    tools.join(format!("{prefix}{id}"))
}

fn is_scratch(name: &str, id: &str) -> bool {
    name == format!(".previous-{id}")
        || name == format!(".incoming-{id}")
        || name == format!(".hold-{id}")
        || name
            .strip_prefix(&format!(".hold-{id}."))
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
}

fn vacant_hold(tools: &Path, id: &str) -> io::Result<PathBuf> {
    for index in 0..64 {
        let path = tools.join(format!(".hold-{id}.{index}"));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "旧副本太多"))
}

fn restore_parked(parked: Option<&Path>, previous: &Path) {
    let Some(parked) = parked else {
        return;
    };
    if !previous.exists() {
        let _ = std::fs::rename(parked, previous);
    }
}

fn discard_tree(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    #[cfg(test)]
    if FAIL_OLD_DELETE.with(std::cell::Cell::get) {
        return false;
    }
    std::fs::remove_dir_all(path).is_ok()
}

fn discard_holds(tools: &Path, id: &str) -> bool {
    let Ok(read) = std::fs::read_dir(tools) else {
        return true;
    };
    let mut ok = true;
    for entry in read.filter_map(Result::ok) {
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        if !is_scratch(&name, id) || !name.starts_with(".hold-") {
            continue;
        }
        if !discard_tree(&entry.path()) {
            ok = false;
        }
    }
    ok
}

#[cfg(test)]
std::thread_local! {
    static FAIL_OLD_DELETE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn write_relative(root: &Path, relative: &str, bytes: &[u8]) -> io::Result<()> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative.contains(':')
        || relative.contains('\0')
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "路径越界"));
    }
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "路径越界"));
        }
        path.push(part);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

struct Card {
    id: String,
    name: String,
    description: String,
}

pub(crate) fn card_bytes(id: &str, name: &str, description: &str) -> Vec<u8> {
    encode_card(id, name, description)
}

/// 导出和恢复用。只保留 `entries` 里的文本。读不懂时是 `None`。
pub(crate) fn canonical_storage(bytes: &[u8]) -> Option<Vec<u8>> {
    decode_storage(bytes).map(|entries| encode_storage(&entries))
}

fn encode_card(id: &str, name: &str, description: &str) -> Vec<u8> {
    let on_disk = CardOnDisk {
        schema_version: SCHEMA_VERSION,
        id,
        name,
        description,
    };
    let mut bytes = serde_json::to_vec(&on_disk).expect("卡片可以编码");
    bytes.push(b'\n');
    bytes
}

fn decode_card(bytes: &[u8]) -> Option<Card> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let obj = value.as_object()?;
    match obj.get("schemaVersion") {
        None => {}
        Some(Value::Number(number)) => match number.as_u64() {
            Some(version) if is_supported_schema(u32::try_from(version).unwrap_or(u32::MAX)) => {}
            _ => return None,
        },
        Some(_) => return None,
    }
    let id = obj.get("id")?.as_str()?.to_string();
    if validate_id(&id).is_err() {
        return None;
    }
    let name = obj.get("name")?.as_str()?.to_string();
    let description = obj.get("description")?.as_str()?.to_string();
    if name.trim().is_empty() {
        return None;
    }
    Some(Card {
        id,
        name,
        description,
    })
}

fn encode_storage(entries: &BTreeMap<String, String>) -> Vec<u8> {
    let on_disk = StorageOnDisk {
        schema_version: SCHEMA_VERSION,
        entries,
    };
    let mut bytes = serde_json::to_vec(&on_disk).expect("存储可以编码");
    bytes.push(b'\n');
    bytes
}

fn decode_storage(bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let obj = value.as_object()?;
    match obj.get("schemaVersion") {
        None => {}
        Some(Value::Number(number)) => match number.as_u64() {
            Some(version) if is_supported_schema(u32::try_from(version).unwrap_or(u32::MAX)) => {}
            _ => return None,
        },
        Some(_) => return None,
    }
    let mut entries = BTreeMap::new();
    let map = obj.get("entries")?.as_object()?;
    for (key, value) in map {
        entries.insert(key.clone(), value.as_str()?.to_string());
    }
    Some(entries)
}

#[derive(Serialize)]
struct CardOnDisk<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    id: &'a str,
    name: &'a str,
    description: &'a str,
}

#[derive(Serialize)]
struct StorageOnDisk<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    entries: &'a BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lanwork-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn install_lists_the_card_and_update_keeps_storage() {
        let dir = scratch("keep");
        let files = vec![
            (
                "manifest.toml".to_string(),
                b"id = \"example.quota\"".to_vec(),
            ),
            ("main.lua".to_string(), b"function on_load() end".to_vec()),
            ("helper.lua".to_string(), b"no".to_vec()),
        ];
        install_copy(
            &dir,
            "example.quota",
            "API 额度",
            "剩余额度",
            &files,
            b"{\"schemaVersion\":1}\n",
        )
        .unwrap();
        let listed = list_installed(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "example.quota");
        assert_eq!(listed[0].name, "API 额度");
        assert!(dir.join("tools/example.quota/helper.lua").is_file());
        let mut stored = BTreeMap::new();
        stored.insert("used".to_string(), "20".to_string());
        write_tool_storage(&dir, "example.quota", &stored).unwrap();
        install_copy(
            &dir,
            "example.quota",
            "新名称",
            "新说明",
            &files,
            b"{\"schemaVersion\":1}\n",
        )
        .unwrap();
        assert_eq!(list_installed(&dir)[0].name, "新名称");
        assert_eq!(
            read_tool_storage(&dir, "example.quota")
                .unwrap()
                .get("used")
                .map(String::as_str),
            Some("20")
        );
        let windows = dir.join("tool-windows");
        std::fs::create_dir_all(&windows).unwrap();
        std::fs::write(windows.join("example.quota.json"), b"{}\n").unwrap();
        std::fs::write(windows.join("example.other.json"), b"{}\n").unwrap();
        remove_installed(&dir, "example.quota").unwrap();
        assert!(list_installed(&dir).is_empty());
        assert!(!dir.join("tools/example.quota").exists());
        assert!(!storage_path(&dir, "example.quota").exists());
        assert!(!windows.join("example.quota.json").exists());
        assert!(windows.join("example.other.json").is_file());
        assert!(windows.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_dirs_mismatched_ids_and_traversal_are_not_installed() {
        let dir = scratch("skip");
        std::fs::create_dir_all(dir.join("tools/.incoming-example.quota")).unwrap();
        std::fs::create_dir_all(dir.join("tools/example.other")).unwrap();
        std::fs::write(
            dir.join("tools/example.other/card.json"),
            "{\"schemaVersion\":1,\"id\":\"example.nope\",\"name\":\"别的\",\"description\":\"说明\"}\n",
        )
        .unwrap();
        assert!(list_installed(&dir).is_empty());
        let err = install_copy(
            &dir,
            "example.quota",
            "名称",
            "说明",
            &[("../secret.txt".to_string(), b"x".to_vec())],
            b"{}",
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!dir.join("tools/example.quota").exists());
        assert!(install_copy(&dir, "lanwork.", "名称", "说明", &[], b"{}").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_match_the_spec_sentences() {
        assert_eq!(UPDATE_NOTE, "保留数据");
        assert_eq!(UNINSTALL_BODY, "会删除这个工具的存储和密钥。");
        assert_eq!(INVALID_PACKAGE, "清单无效");
        assert_eq!(UNAUTHORIZED, "未授权");
        assert_eq!(WRONG_UPDATE, "不是这个工具的更新包");
    }

    #[test]
    fn dot_ids_are_rejected_and_previous_is_restored_when_dest_is_missing() {
        let dir = scratch("dot");
        assert!(install_copy(&dir, ".hidden", "名称", "说明", &[], b"{}").is_err());
        assert!(tool_dir(&dir, ".hidden").is_none());
        install_copy(&dir, "example.quota", "API 额度", "说明", &[], b"{}").unwrap();
        let live = dir.join("tools/example.quota");
        let backup = dir.join("tools/.previous-example.quota");
        std::fs::rename(&live, &backup).unwrap();
        assert_eq!(list_installed(&dir).len(), 1);
        assert!(live.join("card.json").is_file());
        assert!(!backup.exists());
        std::fs::create_dir_all(dir.join("tools/.incoming-example.quota")).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        remove_installed(&dir, "example.quota").unwrap();
        assert!(!live.exists());
        assert!(!backup.exists());
        assert!(!dir.join("tools/.incoming-example.quota").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_backup_delete_keeps_the_new_copy() {
        let dir = scratch("keep-backup");
        install_copy(&dir, "example.quota", "旧名称", "说明", &[], b"{}").unwrap();
        FAIL_OLD_DELETE.with(|flag| flag.set(true));
        let installed = install_copy(&dir, "example.quota", "新名称", "说明", &[], b"{}").unwrap();
        FAIL_OLD_DELETE.with(|flag| flag.set(false));
        assert_eq!(installed, Installed::BackupRemains);
        assert_eq!(list_installed(&dir)[0].name, "新名称");
        assert!(dir.join("tools/.previous-example.quota").is_dir());
        assert!(dir.join("tools/example.quota/card.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
