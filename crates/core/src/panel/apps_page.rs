//! 面板「应用」页里不画窗口的决定：卡片、菜单项和启用状态。
//!
//! 可见行为见 `docs/tools.md`「应用页」。导入确认和卸载的句子在 `tool_install`。
//! 没有 `tools/` 时，列表是空的，界面只留「导入+」。
//! 启用状态写在数据目录的 `tool-enabled.json`。安装副本的路径规格还没定，
//! 这份文件名因此也还不是产品规则，并且不进导出白名单。

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::storage::{SCHEMA_VERSION, atomic_write, is_supported_schema, validate_id};

/// 数据目录里的启用状态。只记关掉的工具 id。缺了这份文件，或某个 id 不在里面，就是启用。
pub const ENABLED_FILE: &str = "tool-enabled.json";

pub const IMPORT_LABEL: &str = "导入+";
pub const FLOAT_LABEL: &str = "悬浮窗";

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// 已经安装、可以列在应用页上的工具。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedTool {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 工具文件夹里的 `icon.png`。没有这个文件时是 `None`。
    pub icon_path: Option<PathBuf>,
}

/// 应用页上的一张工具卡片。图标路径只在内容是 PNG 时保留。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub mark: String,
    pub icon_path: Option<PathBuf>,
}

/// 从字节读到的启用状态。坏 JSON 不当成「全部启用」后写回去。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnabledBytes {
    /// 关掉的 id。不在集合里的工具是启用。
    Known(BTreeSet<String>),
    /// `schemaVersion` 高于当前版本，或不是数字。调用方留着原文。
    Unsupported,
    Corrupt,
}

/// 已安装的工具，顺序与数据目录里的目录名相同。
#[must_use]
pub fn installed_tools(data_dir: &Path) -> Vec<ListedTool> {
    super::tool_install::list_installed(data_dir)
}

/// 全部工具卡片，顺序与 `tools` 相同。「导入+」不在这份列表里，由界面加在最后。
#[must_use]
pub fn visible_cards(tools: &[ListedTool], disabled: &BTreeSet<String>) -> Vec<ToolCard> {
    tools
        .iter()
        .map(|tool| ToolCard {
            id: tool.id.clone(),
            name: tool.name.clone(),
            description: tool.description.clone(),
            enabled: is_enabled(disabled, &tool.id),
            mark: name_mark(&tool.name),
            icon_path: tool
                .icon_path
                .as_ref()
                .filter(|path| usable_icon(path))
                .cloned(),
        })
        .collect()
}

/// 每张卡片的「⋯」都是「更新」「卸载」「查看权限」。
/// 声明了密钥时，最后再加上「填写密钥」。
#[must_use]
pub fn menu_items(has_secrets: bool) -> Vec<&'static str> {
    let mut items = vec!["更新", "卸载", "查看权限"];
    if has_secrets {
        items.push("填写密钥");
    }
    items
}

/// 名称去掉首尾空白后的第一个 Unicode 标量。没有可见字时是空字符串。
#[must_use]
pub fn name_mark(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map(|ch| ch.to_string())
        .unwrap_or_default()
}

/// 文件头是 PNG 签名。其它格式这一版不作为图标。
#[must_use]
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_SIGNATURE)
}

/// 没有文件、读不出来或内容不是 PNG 时返回假。调用方改用名称首字。
#[must_use]
pub fn usable_icon(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => is_png(&bytes),
        Err(_) => false,
    }
}

/// 不在集合里就是启用。新导入的工具默认启用，缺记录时同样如此。
#[must_use]
pub fn is_enabled(disabled: &BTreeSet<String>, id: &str) -> bool {
    !disabled.contains(id)
}

/// 退出时窗口开着，并且启用开关开着，下次启动才再打开。
#[must_use]
pub fn should_reopen(record_open: bool, enabled: bool) -> bool {
    record_open && enabled
}

/// 拨动开关后的集合。id 不能作为文件名时不改集合。
pub fn set_disabled(set: &mut BTreeSet<String>, id: &str, enabled: bool) -> bool {
    if validate_id(id).is_err() {
        return false;
    }
    if enabled {
        set.remove(id);
    } else {
        set.insert(id.to_owned());
    }
    true
}

/// 读启用状态。文件不存在时是空集合，不是错误。
pub fn read_enabled(data_dir: &Path) -> io::Result<EnabledBytes> {
    match std::fs::read(data_dir.join(ENABLED_FILE)) {
        Ok(bytes) => Ok(decode_enabled(&bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Ok(EnabledBytes::Known(BTreeSet::new()))
        }
        Err(err) => Err(err),
    }
}

/// 原子写入。不认识的 schema 不应调用这里覆盖。
pub fn write_enabled(data_dir: &Path, disabled: &BTreeSet<String>) -> io::Result<()> {
    for id in disabled {
        if validate_id(id).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "工具 id 不能作为文件名",
            ));
        }
    }
    atomic_write(&data_dir.join(ENABLED_FILE), &encode_enabled(disabled))
}

#[must_use]
pub fn decode_enabled(bytes: &[u8]) -> EnabledBytes {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return EnabledBytes::Corrupt,
    };
    let Some(obj) = value.as_object() else {
        return EnabledBytes::Corrupt;
    };
    match obj.get("schemaVersion") {
        None => {}
        Some(Value::Number(number)) => match number.as_u64() {
            Some(version) if is_supported_schema(u32::try_from(version).unwrap_or(u32::MAX)) => {}
            _ => return EnabledBytes::Unsupported,
        },
        Some(_) => return EnabledBytes::Unsupported,
    }
    let Some(list) = obj.get("disabled") else {
        return EnabledBytes::Known(BTreeSet::new());
    };
    let Some(list) = list.as_array() else {
        return EnabledBytes::Corrupt;
    };
    let mut disabled = BTreeSet::new();
    for item in list {
        let Some(id) = item.as_str() else {
            return EnabledBytes::Corrupt;
        };
        if validate_id(id).is_err() {
            return EnabledBytes::Corrupt;
        }
        disabled.insert(id.to_owned());
    }
    EnabledBytes::Known(disabled)
}

#[must_use]
pub fn encode_enabled(disabled: &BTreeSet<String>) -> Vec<u8> {
    let ids: Vec<&str> = disabled.iter().map(String::as_str).collect();
    let on_disk = OnDisk {
        schema_version: SCHEMA_VERSION,
        disabled: &ids,
    };
    let mut bytes = serde_json::to_vec(&on_disk).expect("启用状态可以编码");
    bytes.push(b'\n');
    bytes
}

#[derive(Serialize)]
struct OnDisk<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    disabled: &'a [&'a str],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, name: &str) -> ListedTool {
        ListedTool {
            id: id.to_owned(),
            name: name.to_owned(),
            description: format!("{name}的说明"),
            icon_path: None,
        }
    }

    #[test]
    fn empty_list_has_no_cards_and_import_label_stays() {
        let cards = visible_cards(&[], &BTreeSet::new());
        assert!(cards.is_empty());
        assert_eq!(IMPORT_LABEL, "导入+");
        assert_eq!(FLOAT_LABEL, "悬浮窗");
    }

    #[test]
    fn cards_keep_install_order_and_share_one_menu() {
        let tools = vec![
            tool("example.quota", "API 额度"),
            tool("example.daily", "日报"),
        ];
        let mut disabled = BTreeSet::new();
        assert!(set_disabled(&mut disabled, "example.daily", false));
        let cards = visible_cards(&tools, &disabled);
        assert_eq!(
            cards
                .iter()
                .map(|card| card.name.as_str())
                .collect::<Vec<_>>(),
            ["API 额度", "日报"]
        );
        assert!(cards[0].enabled);
        assert!(!cards[1].enabled);
        assert_eq!(cards[0].description, "API 额度的说明");
        assert_eq!(menu_items(false), ["更新", "卸载", "查看权限"]);
        assert_eq!(menu_items(true), ["更新", "卸载", "查看权限", "填写密钥"]);
        assert!(!menu_items(false).contains(&"填写密钥"));
    }

    #[test]
    fn mark_uses_the_first_scalar_after_trim_and_png_rejects_other_bytes() {
        assert_eq!(name_mark("  日报"), "日");
        assert_eq!(name_mark(" API 额度 "), "A");
        assert_eq!(name_mark("   "), "");
        assert!(is_png(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!is_png(b"\xFF\xD8\xFF"));
        assert!(!is_png(b""));
        let dir =
            std::env::temp_dir().join(format!("lanwork-icon-{}-{}", std::process::id(), "mark"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("icon.png");
        let jpeg = dir.join("icon.jpg");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nbody").unwrap();
        std::fs::write(&jpeg, b"\xFF\xD8\xFF").unwrap();
        assert!(usable_icon(&png));
        assert!(!usable_icon(&jpeg));
        assert!(!usable_icon(&dir.join("missing.png")));
        let mut listed = tool("example.quota", "  API 额度");
        listed.icon_path = Some(jpeg);
        let cards = visible_cards(&[listed.clone()], &BTreeSet::new());
        assert_eq!(cards[0].mark, "A");
        assert!(cards[0].icon_path.is_none());
        listed.icon_path = Some(png);
        let cards = visible_cards(&[listed], &BTreeSet::new());
        assert!(cards[0].icon_path.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enabled_file_roundtrip_keeps_off_and_does_not_rewrite_a_bad_file() {
        let dir = std::env::temp_dir().join(format!("lanwork-enabled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            read_enabled(&dir).unwrap(),
            EnabledBytes::Known(BTreeSet::new())
        );
        let mut disabled = BTreeSet::new();
        assert!(set_disabled(&mut disabled, "example.quota", false));
        assert!(!set_disabled(&mut disabled, "", false));
        write_enabled(&dir, &disabled).unwrap();
        let EnabledBytes::Known(loaded) = read_enabled(&dir).unwrap() else {
            panic!("应读回关掉的 id");
        };
        assert!(!is_enabled(&loaded, "example.quota"));
        assert!(is_enabled(&loaded, "example.daily"));
        assert!(!should_reopen(true, false));
        assert!(should_reopen(true, true));
        assert!(!should_reopen(false, true));
        let path = dir.join(ENABLED_FILE);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"not-json").unwrap();
        assert_eq!(read_enabled(&dir).unwrap(), EnabledBytes::Corrupt);
        assert_eq!(std::fs::read(&path).unwrap(), b"not-json");
        std::fs::write(&path, b"{\"schemaVersion\":99,\"disabled\":[]}").unwrap();
        assert_eq!(read_enabled(&dir).unwrap(), EnabledBytes::Unsupported);
        std::fs::write(&path, original).unwrap();
        assert!(installed_tools(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
