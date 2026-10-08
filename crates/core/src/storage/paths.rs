//! 数据目录、缓存目录和引导文件。
//!
//! 解析顺序：`LANWORK_DATA_DIR` → `%LOCALAPPDATA%\Lanwork\bootstrap.json` →
//! `%USERPROFILE%\Documents\Lanwork`。引导文件不放在数据目录里。
//! 任何结果只要落在 `%USERPROFILE%\Documents\MayDolist` 里就拒绝，并且不创建该目录。

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::Error;
use super::schema::{
    SCHEMA_VERSION, default_schema_version, is_supported_schema, require_written_schema,
};

pub const ENV_DATA_DIR: &str = "LANWORK_DATA_DIR";
pub const BOOTSTRAP_FILE: &str = "bootstrap.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirSource {
    Env,
    Bootstrap,
    Default,
}

#[derive(Debug, Clone)]
pub struct ResolveInput {
    pub user_profile: PathBuf,
    pub local_app_data: PathBuf,
    /// 进程里的 `LANWORK_DATA_DIR`。空白视为没有设置。不要把这个值写进日志。
    pub env_data_dir: Option<String>,
    pub current_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ResolvedPaths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub bootstrap_path: PathBuf,
    pub user_profile: PathBuf,
    pub local_app_data: PathBuf,
    pub source: DataDirSource,
}

#[derive(Debug, Serialize, Deserialize)]
struct BootstrapFile {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    schema_version: u32,
    #[serde(rename = "dataDir")]
    data_dir: String,
}

pub fn resolve_from_process() -> Result<ResolvedPaths, Error> {
    let user_profile = require_env_path("USERPROFILE")?;
    let local_app_data = require_env_path("LOCALAPPDATA")?;
    let env_data_dir = std::env::var(ENV_DATA_DIR).ok();
    let current_dir = std::env::current_dir()
        .map_err(|source| Error::io(super::error::IoAction::Read, PathBuf::from("."), source))?;
    resolve(&ResolveInput {
        user_profile,
        local_app_data,
        env_data_dir,
        current_dir,
    })
}

pub fn resolve(input: &ResolveInput) -> Result<ResolvedPaths, Error> {
    let bootstrap_path = bootstrap_path(&input.local_app_data);
    let cache_dir = cache_dir(&input.local_app_data);
    let (raw, source) = if let Some(dir) = non_empty_trimmed(input.env_data_dir.as_deref()) {
        (PathBuf::from(dir), DataDirSource::Env)
    } else if bootstrap_path.is_file() {
        let data_dir = read_bootstrap(&bootstrap_path, &input.current_dir)?;
        (data_dir, DataDirSource::Bootstrap)
    } else {
        (
            input.user_profile.join("Documents").join("Lanwork"),
            DataDirSource::Default,
        )
    };
    let data_dir = normalize_lexical(&make_absolute(&raw, &input.current_dir));
    reject_maydolist(&data_dir, &input.user_profile)?;
    Ok(ResolvedPaths {
        data_dir,
        cache_dir,
        bootstrap_path,
        user_profile: input.user_profile.clone(),
        local_app_data: input.local_app_data.clone(),
        source,
    })
}

pub fn bootstrap_path(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Lanwork").join(BOOTSTRAP_FILE)
}

pub fn cache_dir(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Lanwork").join("cache")
}

pub fn maydolist_dir(user_profile: &Path) -> PathBuf {
    user_profile.join("Documents").join("MayDolist")
}

/// 把迁移后的数据目录记入引导文件。不切换已经打开的存储。
pub fn write_bootstrap(local_app_data: &Path, data_dir: &Path) -> Result<(), Error> {
    if !data_dir.is_absolute() {
        return Err(Error::Bootstrap {
            path: bootstrap_path(local_app_data),
            message: "dataDir 不是绝对路径",
        });
    }
    let path = bootstrap_path(local_app_data);
    if let Some(parent) = path.parent() {
        super::fsutil::create_dir_all(parent)
            .map_err(|source| Error::io(super::error::IoAction::CreateDir, parent, source))?;
    }
    let file = BootstrapFile {
        schema_version: SCHEMA_VERSION,
        data_dir: path_to_utf8(data_dir)?,
    };
    let mut bytes = serde_json::to_vec(&file).map_err(|_| Error::Encode)?;
    require_written_schema(&bytes)?;
    bytes.push(b'\n');
    super::atomic::atomic_write(&path, &bytes)
        .map_err(|source| Error::io(super::error::IoAction::Replace, &path, source))?;
    Ok(())
}

pub fn reject_maydolist(path: &Path, user_profile: &Path) -> Result<(), Error> {
    let forbidden = maydolist_dir(user_profile);
    if is_same_or_under(path, &forbidden) {
        return Err(Error::MayDolist {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// 文件名用的 id。必须是单个路径分量。
pub fn validate_id(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || id.len() > 200
        || id == "."
        || id == ".."
        || id.starts_with(' ')
        || id.ends_with(' ')
        || id.ends_with('.')
        || is_reserved_windows_name(id)
        || id.chars().any(|ch| {
            matches!(ch, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*') || ch.is_control()
        })
    {
        return Err(Error::InvalidId { id: id.to_owned() });
    }
    Ok(())
}

pub fn absolute_lexical(path: &Path) -> Result<PathBuf, Error> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let current = std::env::current_dir().map_err(|source| {
            Error::io(super::error::IoAction::Read, PathBuf::from("."), source)
        })?;
        current.join(path)
    };
    Ok(normalize_lexical(&absolute))
}

pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn is_same_or_under(path: &Path, parent: &Path) -> bool {
    let path = fold_key(path);
    let parent = fold_key(parent);
    path == parent
        || path.starts_with(&(parent.clone() + "\\"))
        || path.starts_with(&(parent + "/"))
}

fn read_bootstrap(path: &Path, current_dir: &Path) -> Result<PathBuf, Error> {
    let bytes = super::fsutil::read(path)
        .map_err(|source| Error::io(super::error::IoAction::Read, path, source))?;
    let file: BootstrapFile = serde_json::from_slice(&bytes).map_err(|_| Error::Bootstrap {
        path: path.to_path_buf(),
        message: "无法解析",
    })?;
    if !is_supported_schema(file.schema_version) {
        return Err(Error::Bootstrap {
            path: path.to_path_buf(),
            message: "schemaVersion 不受支持",
        });
    }
    let data_dir = non_empty_trimmed(Some(&file.data_dir)).ok_or_else(|| Error::Bootstrap {
        path: path.to_path_buf(),
        message: "dataDir 为空",
    })?;
    Ok(make_absolute(Path::new(data_dir), current_dir))
}

fn require_env_path(name: &'static str) -> Result<PathBuf, Error> {
    let value = std::env::var(name).map_err(|_| Error::MissingEnv { name })?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::MissingEnv { name });
    }
    Ok(PathBuf::from(trimmed))
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

fn make_absolute(path: &Path, current_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    }
}

fn path_to_utf8(path: &Path) -> Result<String, Error> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::Bootstrap {
            path: path.to_path_buf(),
            message: "dataDir 不是 UTF-8",
        })
}

fn fold_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = text.trim_end_matches('\\');
    text.to_lowercase()
}

fn is_reserved_windows_name(id: &str) -> bool {
    let stem = id.split('.').next().unwrap_or(id);
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_temp::TempDir;

    fn input(root: &Path, env: Option<&str>) -> ResolveInput {
        ResolveInput {
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
            env_data_dir: env.map(str::to_owned),
            current_dir: root.join("cwd"),
        }
    }

    #[test]
    fn default_is_documents_lanwork_and_not_maydolist() {
        let temp = TempDir::new();
        let resolved = resolve(&input(temp.path(), None)).unwrap();
        assert_eq!(resolved.source, DataDirSource::Default);
        assert_eq!(
            resolved.data_dir,
            temp.path()
                .join("profile")
                .join("Documents")
                .join("Lanwork")
        );
        assert_eq!(
            resolved.cache_dir,
            temp.path().join("local").join("Lanwork").join("cache")
        );
        assert!(!maydolist_dir(&temp.path().join("profile")).exists());
    }

    #[test]
    fn env_overrides_bootstrap() {
        let temp = TempDir::new();
        let local = temp.path().join("local");
        write_bootstrap(&local, &temp.path().join("from-bootstrap")).unwrap();
        let resolved = resolve(&input(
            temp.path(),
            Some(temp.path().join("from-env").to_str().unwrap()),
        ))
        .unwrap();
        assert_eq!(resolved.source, DataDirSource::Env);
        assert_eq!(resolved.data_dir, temp.path().join("from-env"));
    }

    #[test]
    fn blank_env_uses_bootstrap() {
        let temp = TempDir::new();
        let local = temp.path().join("local");
        let custom = temp.path().join("migrated");
        write_bootstrap(&local, &custom).unwrap();
        let resolved = resolve(&input(temp.path(), Some("  "))).unwrap();
        assert_eq!(resolved.source, DataDirSource::Bootstrap);
        assert_eq!(resolved.data_dir, custom);
        assert!(resolved.bootstrap_path.starts_with(&local));
        assert!(!resolved.bootstrap_path.starts_with(&custom));
    }

    #[test]
    fn maydolist_is_rejected_and_not_created() {
        let temp = TempDir::new();
        let profile = temp.path().join("profile");
        let forbidden = maydolist_dir(&profile);
        let mut spec = input(temp.path(), None);
        spec.env_data_dir = Some(forbidden.to_str().unwrap().to_owned());
        let err = resolve(&spec).unwrap_err();
        assert!(err.to_string().contains("不能使用该数据目录"));
        assert!(!forbidden.exists());
    }

    #[test]
    fn corrupt_bootstrap_does_not_fall_back_to_default() {
        let temp = TempDir::new();
        let path = bootstrap_path(&temp.path().join("local"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{").unwrap();
        let err = resolve(&input(temp.path(), None)).unwrap_err();
        assert!(err.to_string().contains("引导文件错误"));
    }

    #[test]
    fn ids_allow_cjk_and_spaces_but_not_separators() {
        assert!(validate_id("便签 一").is_ok());
        assert!(validate_id("a/b").is_err());
        assert!(validate_id("a\\b").is_err());
        assert!(validate_id("..").is_err());
        assert!(validate_id("CON").is_err());
        assert!(validate_id(" trailing").is_err());
    }
}
