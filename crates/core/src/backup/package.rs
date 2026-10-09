//! 导出包的白名单、清单和数据文件集合。
//!
//! 包内路径只用 `/`。拒绝 `..` 和绝对路径。JSON 缺少 `schemaVersion` 时按 1。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::error::BackupError;
use super::zipstore::StoredFile;
use crate::storage::{self, DocumentId, Store};

pub(crate) const PACKAGE_SCHEMA: u64 = 1;
pub(crate) const RETENTION: usize = 7;

const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageOverview {
    pub exported_at: i64,
    pub app_version: String,
    pub todos: u64,
    pub notes: u64,
    pub shelves: u64,
    pub config: bool,
    pub user_apps: bool,
    pub watchlist: bool,
    pub github_caches: u64,
}

#[derive(Clone, Copy)]
enum Kind {
    Manifest,
    Config,
    UserApps,
    Watchlist,
    Todo,
    Note,
    Shelf,
    GithubCache,
}

pub(crate) fn collect(store: &Store, include_cache: bool) -> Result<Vec<StoredFile>, BackupError> {
    let mut files = Vec::new();
    push_optional(
        &mut files,
        "config.json",
        &store.data_dir().join("config.json"),
    )?;
    push_optional(&mut files, "user-apps.json", &store.user_apps_path())?;
    push_dir(&mut files, store, "todos")?;
    push_dir(&mut files, store, "notes")?;
    push_dir(&mut files, store, "shelves")?;
    push_optional(
        &mut files,
        "github/watchlist.json",
        &store.data_dir().join("github").join("watchlist.json"),
    )?;
    if include_cache {
        push_dir(&mut files, store, "github/cache")?;
    }
    for file in &files {
        check_entry(&file.name, &file.bytes)?;
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

pub(crate) fn with_manifest(
    files: Vec<StoredFile>,
    exported_at: i64,
) -> Result<(Vec<StoredFile>, PackageOverview), BackupError> {
    let overview = recount(&files, exported_at, env!("CARGO_PKG_VERSION"))?;
    let manifest = StoredFile {
        name: MANIFEST.to_owned(),
        bytes: manifest_bytes(&overview)?,
    };
    let mut all = vec![manifest];
    all.extend(files);
    Ok((all, overview))
}

pub(crate) fn inspect_files(files: &[StoredFile]) -> Result<PackageOverview, BackupError> {
    let mut seen = std::collections::BTreeSet::new();
    let mut manifest = None;
    let mut data = Vec::new();
    for file in files {
        if !seen.insert(file.name.clone()) {
            return Err(BackupError::entry(&file.name, "压缩包含有重复路径"));
        }
        let kind = classify(&file.name)?;
        if matches!(kind, Kind::Manifest) {
            manifest = Some(file);
        } else {
            check_document(&file.bytes).map_err(|err| match err {
                BackupError::Rejected(message) => BackupError::entry(&file.name, message),
                other => other,
            })?;
            data.push(file);
        }
    }
    let Some(manifest) = manifest else {
        return Err(BackupError::Rejected("压缩包缺少 manifest.json"));
    };
    let declared = parse_manifest(&manifest.bytes)?;
    let actual = recount_refs(&data, declared.exported_at, &declared.app_version)?;
    if declared != actual {
        return Err(BackupError::Rejected("清单与内容不一致"));
    }
    Ok(declared)
}

/// 磁盘上的白名单数据文件，不解析 JSON。供导入时删除包里没有的文件。
pub(crate) fn list_data_files(store: &Store) -> Result<Vec<(String, PathBuf)>, BackupError> {
    let mut names = Vec::new();
    if storage::fs_exists(&store.data_dir().join("config.json")) {
        names.push("config.json".to_owned());
    }
    if storage::fs_exists(&store.user_apps_path()) {
        names.push("user-apps.json".to_owned());
    }
    push_dir_names(&mut names, store, "todos")?;
    push_dir_names(&mut names, store, "notes")?;
    push_dir_names(&mut names, store, "shelves")?;
    if storage::fs_exists(&store.data_dir().join("github").join("watchlist.json")) {
        names.push("github/watchlist.json".to_owned());
    }
    push_dir_names(&mut names, store, "github/cache")?;
    let mut found = Vec::new();
    for name in names {
        found.push((name.clone(), disk_path(store, &name)?));
    }
    Ok(found)
}

pub(crate) fn disk_path(store: &Store, name: &str) -> Result<PathBuf, BackupError> {
    let kind = classify(name)?;
    let path = match kind {
        Kind::Manifest => {
            return Err(BackupError::entry(name, "压缩包含有不允许的路径"));
        }
        Kind::Config => store.document_path(&DocumentId::Config)?,
        Kind::UserApps => store.user_apps_path(),
        Kind::Watchlist => store.document_path(&DocumentId::GithubWatchlist)?,
        Kind::Todo => store.document_path(&DocumentId::Todo(id_of(name)?))?,
        Kind::Note => store.document_path(&DocumentId::Note(id_of(name)?))?,
        Kind::Shelf => store.document_path(&DocumentId::Shelf(id_of(name)?))?,
        Kind::GithubCache => store.document_path(&DocumentId::GithubCache(id_of(name)?))?,
    };
    Ok(path)
}

pub(crate) fn retention_millis(file_name: &str) -> Option<u64> {
    let stem = file_name.strip_suffix(".zip")?;
    if let Some(rest) = stem.strip_prefix("manual-") {
        return rest.parse().ok();
    }
    if let Some(rest) = stem.strip_prefix("auto-") {
        let millis = rest.rsplit_once('-')?.1;
        return millis.parse().ok();
    }
    None
}

fn recount(
    files: &[StoredFile],
    exported_at: i64,
    app_version: &str,
) -> Result<PackageOverview, BackupError> {
    recount_refs(&files.iter().collect::<Vec<_>>(), exported_at, app_version)
}

fn recount_refs(
    files: &[&StoredFile],
    exported_at: i64,
    app_version: &str,
) -> Result<PackageOverview, BackupError> {
    let mut overview = PackageOverview {
        exported_at,
        app_version: app_version.to_owned(),
        todos: 0,
        notes: 0,
        shelves: 0,
        config: false,
        user_apps: false,
        watchlist: false,
        github_caches: 0,
    };
    for file in files {
        match classify(&file.name)? {
            Kind::Manifest => return Err(BackupError::entry(&file.name, "压缩包含有重复路径")),
            Kind::Config => overview.config = true,
            Kind::UserApps => overview.user_apps = true,
            Kind::Watchlist => overview.watchlist = true,
            Kind::Todo => overview.todos += 1,
            Kind::Note => overview.notes += 1,
            Kind::Shelf => overview.shelves += 1,
            Kind::GithubCache => overview.github_caches += 1,
        }
    }
    Ok(overview)
}

fn manifest_bytes(overview: &PackageOverview) -> Result<Vec<u8>, BackupError> {
    let mut counts = Map::new();
    counts.insert("todos".to_owned(), Value::from(overview.todos));
    counts.insert("notes".to_owned(), Value::from(overview.notes));
    counts.insert("shelves".to_owned(), Value::from(overview.shelves));
    counts.insert("config".to_owned(), Value::from(u64::from(overview.config)));
    counts.insert(
        "userApps".to_owned(),
        Value::from(u64::from(overview.user_apps)),
    );
    counts.insert(
        "watchlist".to_owned(),
        Value::from(u64::from(overview.watchlist)),
    );
    counts.insert(
        "githubCaches".to_owned(),
        Value::from(overview.github_caches),
    );
    let mut root = Map::new();
    root.insert(
        "packageSchemaVersion".to_owned(),
        Value::from(PACKAGE_SCHEMA),
    );
    root.insert("exportedAt".to_owned(), Value::from(overview.exported_at));
    root.insert(
        "appVersion".to_owned(),
        Value::String(overview.app_version.clone()),
    );
    root.insert("counts".to_owned(), Value::Object(counts));
    let mut bytes = serde_json::to_vec(&Value::Object(root)).map_err(|_| storage::Error::Encode)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn parse_manifest(bytes: &[u8]) -> Result<PackageOverview, BackupError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| BackupError::entry(MANIFEST, "压缩包中的 JSON 无法解析"))?;
    let obj = value
        .as_object()
        .ok_or(BackupError::entry(MANIFEST, "压缩包中的 JSON 无法解析"))?;
    let version = obj
        .get("packageSchemaVersion")
        .and_then(Value::as_u64)
        .ok_or(BackupError::Rejected("packageSchemaVersion 不受支持"))?;
    if version != PACKAGE_SCHEMA {
        return Err(BackupError::Rejected("packageSchemaVersion 不受支持"));
    }
    let exported_at = obj
        .get("exportedAt")
        .and_then(Value::as_i64)
        .ok_or(BackupError::entry(MANIFEST, "压缩包中的 JSON 无法解析"))?;
    let app_version = obj
        .get("appVersion")
        .and_then(Value::as_str)
        .ok_or(BackupError::entry(MANIFEST, "压缩包中的 JSON 无法解析"))?
        .to_owned();
    let counts = obj
        .get("counts")
        .and_then(Value::as_object)
        .ok_or(BackupError::entry(MANIFEST, "压缩包中的 JSON 无法解析"))?;
    Ok(PackageOverview {
        exported_at,
        app_version,
        todos: count_field(counts, "todos")?,
        notes: count_field(counts, "notes")?,
        shelves: count_field(counts, "shelves")?,
        config: count_field(counts, "config")? == 1,
        user_apps: count_field(counts, "userApps")? == 1,
        watchlist: count_field(counts, "watchlist")? == 1,
        github_caches: count_field(counts, "githubCaches")?,
    })
}

fn count_field(counts: &Map<String, Value>, name: &str) -> Result<u64, BackupError> {
    counts
        .get(name)
        .and_then(Value::as_u64)
        .ok_or(BackupError::entry(MANIFEST, "压缩包中的 JSON 无法解析"))
}

fn check_entry(name: &str, bytes: &[u8]) -> Result<(), BackupError> {
    classify(name)?;
    check_document(bytes).map_err(|err| match err {
        BackupError::Rejected(message) => BackupError::entry(name, message),
        other => other,
    })
}

fn check_document(bytes: &[u8]) -> Result<(), BackupError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| BackupError::Rejected("压缩包中的 JSON 无法解析"))?;
    let obj = value
        .as_object()
        .ok_or(BackupError::Rejected("压缩包中的 JSON 无法解析"))?;
    match obj.get("schemaVersion") {
        None => Ok(()),
        Some(Value::Number(number)) => match number.as_u64() {
            Some(1) => Ok(()),
            Some(_) => Err(BackupError::Rejected("schemaVersion 不受支持")),
            None => Err(BackupError::Rejected("schemaVersion 不受支持")),
        },
        Some(_) => Err(BackupError::Rejected("schemaVersion 不受支持")),
    }
}

fn classify(name: &str) -> Result<Kind, BackupError> {
    if name.is_empty()
        || name.as_bytes().contains(&0)
        || name.contains('\\')
        || name.contains(':')
        || name.starts_with('/')
    {
        return Err(BackupError::entry(name, "压缩包含有不允许的路径"));
    }
    let parts: Vec<&str> = name.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(BackupError::entry(name, "压缩包含有不允许的路径"));
    }
    let kind = match name {
        MANIFEST => Kind::Manifest,
        "config.json" => Kind::Config,
        "user-apps.json" => Kind::UserApps,
        "github/watchlist.json" => Kind::Watchlist,
        _ => kind_from_collection(name)?,
    };
    Ok(kind)
}

fn kind_from_collection(name: &str) -> Result<Kind, BackupError> {
    let (dir, file) = name
        .rsplit_once('/')
        .ok_or_else(|| BackupError::entry(name, "压缩包含有不允许的路径"))?;
    let id = file
        .strip_suffix(".json")
        .ok_or_else(|| BackupError::entry(name, "压缩包含有不允许的路径"))?;
    storage::validate_id(id).map_err(|_| BackupError::entry(name, "压缩包含有不允许的路径"))?;
    match dir {
        "todos" => Ok(Kind::Todo),
        "notes" => Ok(Kind::Note),
        "shelves" => Ok(Kind::Shelf),
        "github/cache" => Ok(Kind::GithubCache),
        _ => Err(BackupError::entry(name, "压缩包含有不允许的路径")),
    }
}

fn id_of(name: &str) -> Result<String, BackupError> {
    let file = name
        .rsplit_once('/')
        .map(|(_, file)| file)
        .ok_or_else(|| BackupError::entry(name, "压缩包含有不允许的路径"))?;
    let id = file
        .strip_suffix(".json")
        .ok_or_else(|| BackupError::entry(name, "压缩包含有不允许的路径"))?;
    Ok(id.to_owned())
}

fn push_optional(files: &mut Vec<StoredFile>, name: &str, path: &Path) -> Result<(), BackupError> {
    if !storage::fs_exists(path) {
        return Ok(());
    }
    files.push(StoredFile {
        name: name.to_owned(),
        bytes: read_file(path)?,
    });
    Ok(())
}

fn push_dir(files: &mut Vec<StoredFile>, store: &Store, relative: &str) -> Result<(), BackupError> {
    for (name, path) in dir_files(store, relative)? {
        files.push(StoredFile {
            name,
            bytes: read_file(&path)?,
        });
    }
    Ok(())
}

fn push_dir_names(
    names: &mut Vec<String>,
    store: &Store,
    relative: &str,
) -> Result<(), BackupError> {
    for (name, _) in dir_files(store, relative)? {
        names.push(name);
    }
    Ok(())
}

fn dir_files(store: &Store, relative: &str) -> Result<Vec<(String, PathBuf)>, BackupError> {
    let dir = join(store.data_dir(), relative);
    if !storage::fs_exists(&dir) {
        return Ok(Vec::new());
    }
    let entries = storage::fs_read_dir(&dir).map_err(|_| BackupError::io("无法读取", &dir))?;
    let mut found = Vec::new();
    for entry in entries {
        if let Some(pair) = named_json(relative, &entry) {
            found.push(pair);
        }
    }
    Ok(found)
}

fn named_json(relative: &str, entry: &storage::FsDirEntry) -> Option<(String, PathBuf)> {
    if !entry.is_file {
        return None;
    }
    let file_name = entry.path.file_name().and_then(|name| name.to_str())?;
    if !file_name.ends_with(".json") {
        return None;
    }
    let name = format!("{relative}/{file_name}");
    classify(&name).ok()?;
    Some((name, entry.path.clone()))
}

fn read_file(path: &Path) -> Result<Vec<u8>, BackupError> {
    storage::fs_read(path).map_err(|_| BackupError::io("无法读取", path))
}

fn join(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}
