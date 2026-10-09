//! 用户添加的便携应用、别名和隐藏列表。
//!
//! 文件是数据目录下的 `user-apps.json`。调用方传入路径，索引不猜测。
//! 这份文件不是可重建的 `apps.json`：无法解析或版本更高时返回错误，不改名、不隔离。
//! 文件不存在视为空目录。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{AppEntry, AppSource, LaunchTarget};

/// 调用方持有的用户目录。索引不猜测它写在哪。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserCatalog {
    pub portable: Vec<AppEntry>,
    pub aliases: Vec<AppAlias>,
    pub hidden: Vec<HiddenApp>,
}

/// 挂到已有启动目标上的可搜索名称。不单独成为一条结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAlias {
    pub name: String,
    pub target: LaunchTarget,
}

/// 设置页用来列出和恢复的一条隐藏记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenApp {
    pub launch_key: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CatalogFile {
    #[serde(rename = "schemaVersion", default = "current_schema")]
    schema_version: u32,
    #[serde(default)]
    portable: Vec<PortableDto>,
    #[serde(default)]
    aliases: Vec<AliasDto>,
    #[serde(default)]
    hidden: Vec<HiddenDto>,
}

const CATALOG_SCHEMA_VERSION: u32 = 1;

fn current_schema() -> u32 {
    CATALOG_SCHEMA_VERSION
}

#[derive(Debug, Serialize, Deserialize)]
struct PortableDto {
    name: String,
    target: TargetDto,
    #[serde(default, rename = "iconPath", skip_serializing_if = "Option::is_none")]
    icon_path: Option<String>,
    #[serde(default, rename = "iconIndex")]
    icon_index: i32,
    #[serde(
        default,
        rename = "alternateNames",
        skip_serializing_if = "Vec::is_empty"
    )]
    alternate_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AliasDto {
    name: String,
    target: TargetDto,
}

#[derive(Debug, Serialize, Deserialize)]
struct HiddenDto {
    #[serde(rename = "launchKey")]
    launch_key: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum TargetDto {
    Path {
        path: String,
        #[serde(default)]
        args: String,
        #[serde(default, rename = "workingDirectory")]
        working_directory: String,
    },
    Aumid {
        aumid: String,
    },
    Url {
        url: String,
    },
}

pub fn load_user_catalog(path: &Path) -> Result<UserCatalog, String> {
    if !path.exists() {
        return Ok(UserCatalog::default());
    }
    let bytes = std::fs::read(path)
        .map_err(|err| format!("读不到用户应用目录 {}: {err}", path.display()))?;
    parse_catalog(&bytes)
}

pub fn save_user_catalog(path: &Path, catalog: &UserCatalog) -> Result<(), String> {
    let file = CatalogFile::from_catalog(catalog);
    let mut bytes = serde_json::to_vec(&file).map_err(|_| "用户应用目录编码失败".to_owned())?;
    bytes.push(b'\n');
    crate::storage::atomic_write(path, &bytes)
        .map_err(|err| format!("写入用户应用目录失败 {}: {err}", path.display()))
}

fn parse_catalog(bytes: &[u8]) -> Result<UserCatalog, String> {
    parse_catalog_logged(bytes, None)
}

fn parse_catalog_logged(
    bytes: &[u8],
    log: Option<&crate::storage::Log>,
) -> Result<UserCatalog, String> {
    let file: CatalogFile =
        serde_json::from_slice(bytes).map_err(|_| "用户应用目录无法解析".to_owned())?;
    if file.schema_version != CATALOG_SCHEMA_VERSION {
        return Err("用户应用目录的 schemaVersion 不受支持".to_owned());
    }
    Ok(file.into_catalog(log))
}

impl CatalogFile {
    fn from_catalog(catalog: &UserCatalog) -> Self {
        Self {
            schema_version: CATALOG_SCHEMA_VERSION,
            portable: catalog
                .portable
                .iter()
                .filter_map(PortableDto::from_entry)
                .collect(),
            aliases: catalog
                .aliases
                .iter()
                .filter_map(AliasDto::from_alias)
                .collect(),
            hidden: catalog
                .hidden
                .iter()
                .filter(|item| !item.launch_key.trim().is_empty())
                .map(|item| HiddenDto {
                    launch_key: item.launch_key.clone(),
                    name: item.name.clone(),
                })
                .collect(),
        }
    }

    fn into_catalog(self, log: Option<&crate::storage::Log>) -> UserCatalog {
        UserCatalog {
            portable: self
                .portable
                .iter()
                .filter_map(|entry| entry.to_entry(log))
                .collect(),
            aliases: self.aliases.iter().filter_map(AliasDto::to_alias).collect(),
            hidden: self
                .hidden
                .into_iter()
                .filter_map(|item| {
                    let launch_key = item.launch_key.trim();
                    if launch_key.is_empty() {
                        None
                    } else {
                        Some(HiddenApp {
                            launch_key: launch_key.to_owned(),
                            name: item.name.trim().to_owned(),
                        })
                    }
                })
                .collect(),
        }
    }
}

impl PortableDto {
    fn from_entry(entry: &AppEntry) -> Option<Self> {
        let name = entry.name.trim();
        if name.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            target: TargetDto::from_target(&entry.target)?,
            icon_path: entry
                .icon_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            icon_index: entry.icon_index,
            alternate_names: entry.alternate_names.clone(),
        })
    }

    fn to_entry(&self, log: Option<&crate::storage::Log>) -> Option<AppEntry> {
        if !matches!(self.target, TargetDto::Path { .. }) {
            note_portable_skip(log, target_dto_kind(&self.target));
            return None;
        }
        let name = self.name.trim();
        if name.is_empty() {
            return None;
        }
        let target = self.target.to_target()?;
        if super::model::launch_key(&target).is_empty() {
            return None;
        }
        Some(AppEntry {
            name: name.to_owned(),
            source: AppSource::Portable,
            target,
            icon_path: self
                .icon_path
                .as_ref()
                .map(|path| path.trim())
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            icon_index: self.icon_index,
            alternate_names: self.alternate_names.clone(),
        })
    }
}

impl AliasDto {
    fn from_alias(alias: &AppAlias) -> Option<Self> {
        let name = alias.name.trim();
        if name.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            target: TargetDto::from_target(&alias.target)?,
        })
    }

    fn to_alias(&self) -> Option<AppAlias> {
        let name = self.name.trim();
        if name.is_empty() {
            return None;
        }
        let target = self.target.to_target()?;
        if super::model::launch_key(&target).is_empty() {
            None
        } else {
            Some(AppAlias {
                name: name.to_owned(),
                target,
            })
        }
    }
}

impl TargetDto {
    fn from_target(target: &LaunchTarget) -> Option<Self> {
        if super::model::launch_key(target).is_empty() {
            return None;
        }
        Some(match target {
            LaunchTarget::Path {
                path,
                args,
                working_directory,
            } => Self::Path {
                path: path.to_string_lossy().into_owned(),
                args: args.clone(),
                working_directory: working_directory
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            LaunchTarget::Aumid { aumid } => Self::Aumid {
                aumid: aumid.clone(),
            },
            LaunchTarget::Url { url } => Self::Url { url: url.clone() },
        })
    }

    fn to_target(&self) -> Option<LaunchTarget> {
        match self {
            Self::Path {
                path,
                args,
                working_directory,
            } => {
                let path = path.trim();
                if path.is_empty() {
                    return None;
                }
                let working_directory = working_directory.trim();
                Some(LaunchTarget::Path {
                    path: PathBuf::from(path),
                    args: args.trim().to_owned(),
                    working_directory: if working_directory.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(working_directory))
                    },
                })
            }
            Self::Aumid { aumid } => {
                let aumid = aumid.trim();
                if aumid.is_empty() {
                    None
                } else {
                    Some(LaunchTarget::Aumid {
                        aumid: aumid.to_owned(),
                    })
                }
            }
            Self::Url { url } => {
                let url = url.trim();
                if url.is_empty() {
                    None
                } else {
                    Some(LaunchTarget::Url {
                        url: url.to_owned(),
                    })
                }
            }
        }
    }
}

pub(crate) fn portable_entries(catalog: &UserCatalog) -> Vec<AppEntry> {
    portable_entries_logged(catalog, None)
}

fn portable_entries_logged(
    catalog: &UserCatalog,
    log: Option<&crate::storage::Log>,
) -> Vec<AppEntry> {
    catalog
        .portable
        .iter()
        .filter_map(|entry| {
            if !matches!(entry.target, LaunchTarget::Path { .. }) {
                note_portable_skip(log, target_kind_name(&entry.target));
                return None;
            }
            let name = entry.name.trim();
            if name.is_empty() || super::model::launch_key(&entry.target).is_empty() {
                return None;
            }
            let mut owned = entry.clone();
            owned.name = name.to_owned();
            owned.source = AppSource::Portable;
            Some(owned)
        })
        .collect()
}

fn note_portable_skip(log: Option<&crate::storage::Log>, kind: &str) {
    const MESSAGE_PREFIX: &str = "portable app skipped reason=not-path kind=";
    if let Some(log) = log {
        log.warn(&format!("{MESSAGE_PREFIX}{kind}"));
        return;
    }
    let Some(paths) = crate::storage::resolve_from_process().ok() else {
        return;
    };
    let Ok(opened) = crate::storage::Log::open(
        paths.data_dir.join("logs").join("app.log"),
        crate::storage::LogSettings::default(),
    ) else {
        return;
    };
    opened.warn(&format!("{MESSAGE_PREFIX}{kind}"));
}

fn target_dto_kind(target: &TargetDto) -> &'static str {
    match target {
        TargetDto::Path { .. } => "path",
        TargetDto::Aumid { .. } => "aumid",
        TargetDto::Url { .. } => "url",
    }
}

fn target_kind_name(target: &LaunchTarget) -> &'static str {
    match target {
        LaunchTarget::Path { .. } => "path",
        LaunchTarget::Aumid { .. } => "aumid",
        LaunchTarget::Url { .. } => "url",
    }
}

pub(crate) fn alias_entries(catalog: &UserCatalog) -> Vec<AppEntry> {
    catalog
        .aliases
        .iter()
        .filter_map(|alias| {
            let name = alias.name.trim();
            if name.is_empty() || super::model::launch_key(&alias.target).is_empty() {
                return None;
            }
            Some(AppEntry {
                name: name.to_owned(),
                source: AppSource::Alias,
                target: alias.target.clone(),
                icon_path: None,
                icon_index: 0,
                alternate_names: Vec::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_temp::TempDir;

    fn sample() -> UserCatalog {
        UserCatalog {
            portable: vec![AppEntry {
                name: "便携工具".into(),
                source: AppSource::Portable,
                target: LaunchTarget::Path {
                    path: PathBuf::from(r"D:\Tools\tool.exe"),
                    args: "--kept".into(),
                    working_directory: Some(PathBuf::from(r"D:\Tools")),
                },
                icon_path: Some(PathBuf::from(r"D:\Tools\tool.exe")),
                icon_index: 1,
                alternate_names: vec!["tool.exe".into()],
            }],
            aliases: vec![AppAlias {
                name: "笔记".into(),
                target: LaunchTarget::Path {
                    path: PathBuf::from(r"C:\Apps\note.exe"),
                    args: String::new(),
                    working_directory: None,
                },
            }],
            hidden: vec![HiddenApp {
                launch_key: "path\u{1}c:\\apps\\note.exe".into(),
                name: "记事本".into(),
            }],
        }
    }

    #[test]
    fn roundtrip_keeps_portable_alias_and_hidden() {
        let temp = TempDir::new();
        let path = temp.path().join("user-apps.json");
        save_user_catalog(&path, &sample()).unwrap();
        let loaded = load_user_catalog(&path).unwrap();
        assert_eq!(loaded, sample());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"schemaVersion\":1"));
        assert!(text.contains("\"launchKey\""));
        assert!(!temp.path().join("user-apps.json.tmp").exists());
    }

    #[test]
    fn missing_file_is_an_empty_catalog() {
        let temp = TempDir::new();
        let path = temp.path().join("missing.json");
        assert_eq!(load_user_catalog(&path).unwrap(), UserCatalog::default());
        assert!(!path.exists());
    }

    #[test]
    fn corrupt_or_newer_schema_is_left_in_place() {
        let temp = TempDir::new();
        let path = temp.path().join("user-apps.json");
        std::fs::write(&path, b"{not-json").unwrap();
        assert!(load_user_catalog(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not-json");
        std::fs::write(&path, br#"{"schemaVersion":99,"portable":[]}"#).unwrap();
        let err = load_user_catalog(&path).unwrap_err();
        assert!(err.contains("schemaVersion"));
        assert!(path.is_file());
        let names: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            names
                .iter()
                .all(|name| !name.to_string_lossy().contains(".corrupt-"))
        );
    }

    #[test]
    fn missing_schema_version_reads_as_current() {
        let raw = r#"{"portable":[{"name":"A","target":{"kind":"path","path":"D:\\a.exe"}}],"aliases":[{"name":"别名","target":{"kind":"url","url":"steam://rungameid/1"}}]}"#;
        let catalog = parse_catalog(raw.as_bytes()).unwrap();
        assert_eq!(catalog.portable.len(), 1);
        assert_eq!(catalog.portable[0].source, AppSource::Portable);
        assert_eq!(catalog.aliases[0].name, "别名");
        assert!(matches!(
            catalog.aliases[0].target,
            LaunchTarget::Url { .. }
        ));
    }

    #[test]
    fn portable_rejects_url_and_aumid_and_logs_the_kind() {
        let temp = TempDir::new();
        let log = crate::storage::Log::open(
            temp.path().join("app.log"),
            crate::storage::LogSettings {
                max_bytes: 1024 * 1024,
                max_files: 2,
                secrets: Vec::new(),
            },
        )
        .unwrap();
        let raw = r#"{"schemaVersion":1,"portable":[
            {"name":"网页","target":{"kind":"url","url":"steam://rungameid/999"}},
            {"name":"商店","target":{"kind":"aumid","aumid":"App"}},
            {"name":"工具","target":{"kind":"path","path":"D:\\tool.exe"}}
        ]}"#;
        let catalog = parse_catalog_logged(raw.as_bytes(), Some(&log)).unwrap();
        assert_eq!(catalog.portable.len(), 1);
        assert_eq!(catalog.portable[0].name, "工具");
        assert!(matches!(
            catalog.portable[0].target,
            LaunchTarget::Path { .. }
        ));
        let mut in_memory = catalog;
        in_memory.portable.push(AppEntry {
            name: "另一条链接".into(),
            source: AppSource::Portable,
            target: LaunchTarget::Url {
                url: "com.epicgames.launcher://apps/Foo".into(),
            },
            icon_path: None,
            icon_index: 0,
            alternate_names: Vec::new(),
        });
        let entries = portable_entries_logged(&in_memory, Some(&log));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "工具");
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("portable app skipped reason=not-path kind=url"));
        assert!(text.contains("portable app skipped reason=not-path kind=aumid"));
        assert!(!text.contains("steam://rungameid/999"));
        assert!(!text.contains("com.epicgames.launcher://apps/Foo"));
    }
}
