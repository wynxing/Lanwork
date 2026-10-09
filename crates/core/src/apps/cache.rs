//! `%LOCALAPPDATA%\Lanwork\cache\apps.json`。
//!
//! 写入走存储层的原子替换。无法解析，或 `schemaVersion` 不是 1 时，
//! 把文件改名为 `apps.json.corrupt-<UTC 毫秒>-<序号>` 并记日志。日志不含文件内容。
//! 这是当前实现选择：缓存可以重建，所以不认识的版本也隔离；数据目录里的业务文件不按这条处理。
//! 读取时的 IO 错误只记日志，不改名。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::model::{AppEntry, AppSource, LaunchTarget};

pub const CACHE_FILE_NAME: &str = "apps.json";
pub const CACHE_SCHEMA_VERSION: u32 = 1;

static QUARANTINE_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLoad {
    Missing,
    Loaded(Vec<AppEntry>),
    Discarded {
        quarantine: Option<PathBuf>,
    },
    /// 文件还在。读失败不是解析失败，不隔离。
    Unreadable,
}

/// 打开索引时缓存文件的状态。条目数只反映当时读到的文件，不含去重之后的变化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    Missing,
    Loaded(usize),
    Discarded,
    Unreadable,
}

impl From<&CacheLoad> for CacheStatus {
    fn from(load: &CacheLoad) -> Self {
        match load {
            CacheLoad::Missing => Self::Missing,
            CacheLoad::Loaded(entries) => Self::Loaded(entries.len()),
            CacheLoad::Discarded { .. } => Self::Discarded,
            CacheLoad::Unreadable => Self::Unreadable,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    #[serde(rename = "schemaVersion", default = "default_schema")]
    schema_version: u32,
    entries: Vec<CacheEntry>,
}

fn default_schema() -> u32 {
    CACHE_SCHEMA_VERSION
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    name: String,
    source: SourceName,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum SourceName {
    StartMenu,
    AppPaths,
    Path,
    Store,
    Portable,
    Alias,
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

pub(crate) fn load_cache_logged(path: &Path, log: Option<&crate::storage::Log>) -> CacheLoad {
    if !path.is_file() {
        return CacheLoad::Missing;
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            note(
                log,
                &format!(
                    "app cache read failed path={} os={:?}",
                    path.display(),
                    err.raw_os_error(),
                ),
            );
            return CacheLoad::Unreadable;
        }
    };
    match parse_cache(&bytes) {
        Ok(entries) => CacheLoad::Loaded(entries),
        Err(reason) => CacheLoad::Discarded {
            quarantine: quarantine(path, log, reason),
        },
    }
}

pub fn save_cache(path: &Path, entries: &[AppEntry]) -> Result<(), String> {
    let file = CacheFile {
        schema_version: CACHE_SCHEMA_VERSION,
        entries: entries.iter().map(CacheEntry::from_entry).collect(),
    };
    let mut bytes = serde_json::to_vec(&file).map_err(|_| "应用缓存编码失败".to_owned())?;
    bytes.push(b'\n');
    crate::storage::atomic_write(path, &bytes)
        .map_err(|err| format!("写入应用缓存失败 {}: {err}", path.display()))
}

fn parse_cache(bytes: &[u8]) -> Result<Vec<AppEntry>, &'static str> {
    let file: CacheFile = serde_json::from_slice(bytes).map_err(|_| "parse")?;
    if file.schema_version != CACHE_SCHEMA_VERSION {
        return Err("schema");
    }
    Ok(file
        .entries
        .iter()
        .filter_map(CacheEntry::to_entry)
        .collect())
}

fn note(log: Option<&crate::storage::Log>, message: &str) {
    if let Some(log) = log {
        log.warn(message);
    }
}

fn quarantine(path: &Path, log: Option<&crate::storage::Log>, reason: &str) -> Option<PathBuf> {
    let file_name = path.file_name()?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = QUARANTINE_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = std::ffi::OsString::from(file_name);
    name.push(format!(".corrupt-{millis}-{seq}"));
    let target = path.with_file_name(name);
    match std::fs::rename(path, &target) {
        Ok(()) => {
            note(
                log,
                &format!(
                    "app cache quarantined path={} quarantine={} reason={reason}",
                    path.display(),
                    target.display(),
                ),
            );
            Some(target)
        }
        Err(err) => {
            note(
                log,
                &format!(
                    "app cache quarantine rename failed path={} reason={reason} os={:?}",
                    path.display(),
                    err.raw_os_error(),
                ),
            );
            None
        }
    }
}

impl CacheEntry {
    fn from_entry(entry: &AppEntry) -> Self {
        Self {
            name: entry.name.clone(),
            source: SourceName::from(entry.source),
            target: TargetDto::from(&entry.target),
            icon_path: entry
                .icon_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            icon_index: entry.icon_index,
            alternate_names: entry.alternate_names.clone(),
        }
    }

    fn to_entry(&self) -> Option<AppEntry> {
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
            source: AppSource::from(self.source),
            target,
            icon_path: self
                .icon_path
                .as_ref()
                .map(|path| path.trim())
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            icon_index: self.icon_index,
            alternate_names: self
                .alternate_names
                .iter()
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
}

impl From<AppSource> for SourceName {
    fn from(source: AppSource) -> Self {
        match source {
            AppSource::StartMenu => Self::StartMenu,
            AppSource::AppPaths => Self::AppPaths,
            AppSource::Path => Self::Path,
            AppSource::Store => Self::Store,
            AppSource::Portable => Self::Portable,
            AppSource::Alias => Self::Alias,
        }
    }
}

impl From<SourceName> for AppSource {
    fn from(source: SourceName) -> Self {
        match source {
            SourceName::StartMenu => Self::StartMenu,
            SourceName::AppPaths => Self::AppPaths,
            SourceName::Path => Self::Path,
            SourceName::Store => Self::Store,
            SourceName::Portable => Self::Portable,
            SourceName::Alias => Self::Alias,
        }
    }
}

impl From<&LaunchTarget> for TargetDto {
    fn from(target: &LaunchTarget) -> Self {
        match target {
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
        }
    }
}

impl TargetDto {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_temp::TempDir;

    fn sample() -> AppEntry {
        AppEntry {
            name: "带参数".into(),
            source: AppSource::StartMenu,
            target: LaunchTarget::Path {
                path: PathBuf::from(r"C:\App\tool.exe"),
                args: "--kept".into(),
                working_directory: Some(PathBuf::from(r"C:\Work")),
            },
            icon_path: Some(PathBuf::from(r"C:\App\tool.exe")),
            icon_index: 2,
            alternate_names: vec!["tool.exe".into()],
        }
    }

    #[test]
    fn roundtrip_keeps_args_working_directory_and_aumid() {
        let temp = TempDir::new();
        let path = temp.path().join(CACHE_FILE_NAME);
        let aumid = AppEntry {
            name: "计算器".into(),
            source: AppSource::Store,
            target: LaunchTarget::Aumid {
                aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into(),
            },
            icon_path: None,
            icon_index: 0,
            alternate_names: Vec::new(),
        };
        let game = AppEntry {
            name: "游戏".into(),
            source: AppSource::StartMenu,
            target: LaunchTarget::Url {
                url: "steam://rungameid/570".into(),
            },
            icon_path: None,
            icon_index: 0,
            alternate_names: vec!["Game.url".into()],
        };
        save_cache(&path, &[sample(), aumid.clone(), game.clone()]).unwrap();
        let CacheLoad::Loaded(entries) = load_cache_logged(&path, None) else {
            panic!("cache should load");
        };
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].target, sample().target);
        assert_eq!(entries[0].icon_index, 2);
        assert_eq!(entries[0].alternate_names, vec!["tool.exe".to_owned()]);
        assert_eq!(entries[1], aumid);
        assert_eq!(entries[2], game);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"schemaVersion\":1"));
        assert!(text.contains("\"workingDirectory\":\"C:\\\\Work\""));
        assert!(text.contains("\"kind\":\"aumid\""));
        assert!(text.contains("\"kind\":\"url\""));
        assert!(text.contains("\"alternateNames\""));
        assert!(!temp.path().join("apps.json.tmp").exists());
    }

    #[test]
    fn old_cache_without_alternate_names_url_or_portable_still_loads() {
        let raw = r#"{
            "schemaVersion": 1,
            "entries": [
                {
                    "name": "记事本",
                    "source": "startMenu",
                    "target": {
                        "kind": "path",
                        "path": "C:\\Windows\\notepad.exe",
                        "args": "",
                        "workingDirectory": ""
                    },
                    "iconIndex": 0
                },
                {
                    "name": "计算器",
                    "source": "store",
                    "target": {
                        "kind": "aumid",
                        "aumid": "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
                    }
                }
            ]
        }"#;
        let entries = parse_cache(raw.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].alternate_names.is_empty());
        assert!(matches!(entries[0].target, LaunchTarget::Path { .. }));
        assert_eq!(entries[0].source, AppSource::StartMenu);
        assert_eq!(entries[1].source, AppSource::Store);
        assert!(matches!(entries[1].target, LaunchTarget::Aumid { .. }));
    }

    #[test]
    fn missing_schema_version_reads_as_current() {
        let raw = r#"{"entries":[{"name":"A","source":"path","target":{"kind":"path","path":"C:\\a.exe"}}]}"#;
        let entries = parse_cache(raw.as_bytes()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, AppSource::Path);
    }

    #[test]
    fn corrupt_json_is_renamed_and_not_left_in_place() {
        let temp = TempDir::new();
        let path = temp.path().join(CACHE_FILE_NAME);
        std::fs::write(&path, b"{").unwrap();
        let CacheLoad::Discarded { quarantine } = load_cache_logged(&path, None) else {
            panic!("corrupt cache should be discarded");
        };
        let quarantine = quarantine.expect("rename");
        assert!(!path.exists());
        assert!(
            quarantine
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("apps.json.corrupt-")
        );
        assert_eq!(std::fs::read(&quarantine).unwrap(), b"{");
    }

    #[test]
    fn unsupported_schema_is_discarded() {
        let temp = TempDir::new();
        let path = temp.path().join(CACHE_FILE_NAME);
        std::fs::write(&path, br#"{"schemaVersion":99,"entries":[]}"#).unwrap();
        assert!(matches!(
            load_cache_logged(&path, None),
            CacheLoad::Discarded { .. }
        ));
        assert!(!path.exists());
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let temp = TempDir::new();
        assert_eq!(
            load_cache_logged(&temp.path().join(CACHE_FILE_NAME), None),
            CacheLoad::Missing
        );
    }

    #[test]
    fn corrupt_cache_is_logged_without_file_contents() {
        let temp = TempDir::new();
        let path = temp.path().join(CACHE_FILE_NAME);
        let body = b"{not-json cache-body-do-not-log";
        std::fs::write(&path, body).unwrap();
        let log = test_log(temp.path());
        let CacheLoad::Discarded { quarantine } = load_cache_logged(&path, Some(&log)) else {
            panic!("corrupt cache should be discarded");
        };
        let quarantine = quarantine.expect("rename");
        assert!(!path.exists());
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("app cache quarantined"));
        assert!(text.contains("reason=parse"));
        assert!(text.contains(&quarantine.display().to_string()));
        assert!(!text.contains("cache-body-do-not-log"));
    }

    #[test]
    fn unsupported_schema_is_logged_as_quarantine() {
        let temp = TempDir::new();
        let path = temp.path().join(CACHE_FILE_NAME);
        std::fs::write(&path, br#"{"schemaVersion":99,"entries":[]}"#).unwrap();
        let log = test_log(temp.path());
        assert!(matches!(
            load_cache_logged(&path, Some(&log)),
            CacheLoad::Discarded { .. }
        ));
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("reason=schema"));
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn read_error_keeps_the_cache_file() {
        use std::os::windows::fs::OpenOptionsExt;

        let temp = TempDir::new();
        let path = temp.path().join(CACHE_FILE_NAME);
        std::fs::write(&path, b"{\"schemaVersion\":1,\"entries\":[]}").unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let log = test_log(temp.path());
        assert!(matches!(
            load_cache_logged(&path, Some(&log)),
            CacheLoad::Unreadable
        ));
        drop(held);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"{\"schemaVersion\":1,\"entries\":[]}"
        );
        let names: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            names
                .iter()
                .all(|name| !name.to_string_lossy().contains(".corrupt-"))
        );
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("app cache read failed"));
        assert!(!text.contains("schemaVersion"));
    }

    fn test_log(dir: &Path) -> crate::storage::Log {
        crate::storage::Log::open(
            dir.join("app.log"),
            crate::storage::LogSettings {
                max_bytes: 1024 * 1024,
                max_files: 2,
                secrets: Vec::new(),
            },
        )
        .unwrap()
    }
}
