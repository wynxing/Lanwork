//! 导出、备份和导入。
//!
//! 不读取环境变量。日志、`backups/` 和 `import.pending` 不进入包。
//! 调用方传入公历日和 UTC 毫秒，本服务不读时钟。

use std::path::{Path, PathBuf};

use super::error::BackupError;
use super::package::{self, PackageOverview, PackagePreview};
use super::tools;
use super::zipstore::{self, StoredFile};
use crate::capture::CivilDate;
use crate::storage::{self, BootHooks, EntityChanged, EntityKind, ImportPending, Store};

/// 同一天再次自动备份时的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchBackup {
    Created { path: PathBuf },
    AlreadyDone,
}

/// 计入 7 份上限的一份备份。导入前的 `import-` 不在这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupKind {
    Auto,
    Manual,
}

/// 设置页列出的一份自动或手动备份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedBackup {
    pub kind: BackupKind,
    pub at_ms: i64,
    /// 自动备份文件名里的公历日。手动备份没有。
    pub day: Option<CivilDate>,
    /// `backups/` 里的文件名，用来打开这一份再走导入确认。
    pub file_name: String,
}

impl ListedBackup {
    /// 自动用文件名里的公历日。手动用 UTC 的「YYYY-MM-DD HH:MM」。
    ///
    /// 时区不是产品规则。自动备份的日子是外壳传入的本地公历日，手动只有 UTC 毫秒。
    #[must_use]
    pub fn label(&self) -> String {
        match self.kind {
            BackupKind::Auto => match self.day {
                Some(day) => format!("自动 {:04}-{:02}-{:02}", day.year(), day.month(), day.day()),
                None => format!("自动 {}", format_utc_minute(self.at_ms)),
            },
            BackupKind::Manual => format!("手动 {}", format_utc_minute(self.at_ms)),
        }
    }

    #[must_use]
    pub fn package_path(&self, backups_dir: &Path) -> PathBuf {
        backups_dir.join(&self.file_name)
    }
}

#[derive(Clone, Debug)]
pub struct BackupService {
    store: Store,
}

impl BackupService {
    #[must_use]
    pub fn open(store: Store) -> Self {
        Self { store }
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 写出一份导出包。不含 `github/watchlist.json`、`github/cache` 和 `cache/web-icons`。
    pub fn export(&self, dest: &Path, now_ms: i64) -> Result<PackageOverview, BackupError> {
        let (files, overview) = self.store.with_write_lock(|| self.snapshot(now_ms))?;
        write_bytes(dest, &zipstore::write_stored(&files)?)?;
        Ok(overview)
    }

    /// 手动备份。不改自动备份的日期记录。之后自动和手动合计只留 7 份。
    pub fn backup_manual(&self, now_ms: i64) -> Result<PathBuf, BackupError> {
        self.store.with_write_lock(|| {
            let path = self.write_named(&format!("manual-{now_ms}.zip"), now_ms)?;
            self.prune()?;
            Ok(path)
        })
    }

    /// 每个公历日的第一次调用写一份自动备份。同一天再调用不再写。
    pub fn backup_on_launch(
        &self,
        today: CivilDate,
        now_ms: i64,
    ) -> Result<LaunchBackup, BackupError> {
        self.store.with_write_lock(|| {
            let day = format_day(today);
            if self.read_stamp()? == Some(day.clone()) {
                return Ok(LaunchBackup::AlreadyDone);
            }
            let path = self.write_named(&format!("auto-{day}-{now_ms}.zip"), now_ms)?;
            write_bytes(&self.stamp_path(), format!("{day}\n").as_bytes())?;
            self.prune()?;
            Ok(LaunchBackup::Created { path })
        })
    }

    /// 自动和手动备份，从新到旧。目录不存在时是空列表。
    ///
    /// 在写锁里读文件名，和修剪用同一把锁，避免列出刚被删掉的最旧一份。
    pub fn list_retained(&self) -> Result<Vec<ListedBackup>, BackupError> {
        self.store
            .with_write_lock(|| list_retained_holding(&self.store))
    }

    /// 校验包并返回概览和工具权限，不改当前数据。
    pub fn inspect(&self, package: &Path) -> Result<PackagePreview, BackupError> {
        let files = read_zip(package)?;
        let overview = package::inspect_files(&files)?;
        Ok(PackagePreview {
            overview,
            tools: tools::preview_copies(&files),
        })
    }

    /// 校验通过后先做导入备份，再替换。校验失败时当前数据不变。
    ///
    /// 替换结束并删掉 `import.pending` 之后，已打开的服务从磁盘重新载入，然后才发布变更。
    pub fn import(&self, package: &Path, now_ms: i64) -> Result<PackageOverview, BackupError> {
        let files = read_zip(package)?;
        let overview = package::inspect_files(&files)?;
        let events = self
            .store
            .with_write_lock(|| self.import_holding(&files, now_ms))?;
        self.store.reload_memories()?;
        for event in events {
            self.store.publish_change(event);
        }
        Ok(overview)
    }

    /// 启动时若有 `import.pending`，用其中的备份恢复并删掉该文件。
    pub fn register_boot_hooks(hooks: &mut BootHooks) {
        hooks.recover_import = Some(Box::new(|store, pending| {
            recover_import(store, pending).map_err(|err| err.to_string())
        }));
    }

    fn snapshot(&self, now_ms: i64) -> Result<(Vec<StoredFile>, PackageOverview), BackupError> {
        let files = package::collect(&self.store)?;
        package::with_manifest(files, now_ms)
    }

    fn write_named(&self, file_name: &str, now_ms: i64) -> Result<PathBuf, BackupError> {
        let (files, _) = self.snapshot(now_ms)?;
        self.write_package(file_name, &files)
    }

    /// 导入失败时用来写回的原字节。不走 [`package::collect`] 的再校验。
    fn write_raw_named(&self, file_name: &str, now_ms: i64) -> Result<PathBuf, BackupError> {
        let files = package::collect_raw(&self.store)?;
        let (files, _) = package::with_manifest(files, now_ms)?;
        self.write_package(file_name, &files)
    }

    fn write_package(&self, file_name: &str, files: &[StoredFile]) -> Result<PathBuf, BackupError> {
        let path = self.store.backups_dir().join(file_name);
        write_bytes(&path, &zipstore::write_stored(files)?)?;
        Ok(path)
    }

    fn import_holding(
        &self,
        files: &[StoredFile],
        now_ms: i64,
    ) -> Result<Vec<EntityChanged>, BackupError> {
        let backup = self.write_raw_named(&format!("import-{now_ms}.zip"), now_ms)?;
        self.store.write_import_pending_holding_lock(&backup)?;
        let events = match apply(&self.store, files, false) {
            Ok(events) => events,
            Err(err) => {
                restore_backup(&self.store, &backup)?;
                self.store.clear_import_pending_holding_lock()?;
                return Err(err);
            }
        };
        self.store.clear_import_pending_holding_lock()?;
        Ok(events)
    }

    fn prune(&self) -> Result<(), BackupError> {
        let dir = self.store.backups_dir();
        let entries = storage::fs_read_dir(&dir).map_err(|_| BackupError::io("无法读取", &dir))?;
        let mut counted = Vec::new();
        for entry in entries {
            if !entry.is_file {
                continue;
            }
            let Some(name) = entry.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if let Some(millis) = package::retention_millis(name) {
                counted.push((millis, name.to_owned(), entry.path));
            }
        }
        counted.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        while counted.len() > package::RETENTION {
            let (_, _, path) = counted.remove(0);
            storage::fs_remove_file(&path).map_err(|_| BackupError::io("无法删除", &path))?;
        }
        Ok(())
    }

    fn read_stamp(&self) -> Result<Option<String>, BackupError> {
        let path = self.stamp_path();
        if !storage::fs_exists(&path) {
            return Ok(None);
        }
        let bytes = storage::fs_read(&path).map_err(|_| BackupError::io("无法读取", &path))?;
        let text =
            String::from_utf8(bytes).map_err(|_| BackupError::Rejected("自动备份记录无法读取"))?;
        let day = text.trim();
        if day.is_empty() || day.chars().any(char::is_control) {
            return Err(BackupError::Rejected("自动备份记录无法读取"));
        }
        Ok(Some(day.to_owned()))
    }

    fn stamp_path(&self) -> PathBuf {
        self.store.backups_dir().join("auto-day.txt")
    }
}

pub(crate) fn recover_import(store: &Store, pending: &ImportPending) -> Result<(), BackupError> {
    if !storage::fs_exists(&pending.backup_path) {
        return Err(BackupError::Rejected("备份文件不存在"));
    }
    store.with_write_lock(|| {
        restore_backup(store, &pending.backup_path)?;
        store.clear_import_pending_holding_lock()?;
        Ok(())
    })
}

fn restore_backup(store: &Store, path: &Path) -> Result<(), BackupError> {
    if !storage::fs_exists(path) {
        return Err(BackupError::Rejected("备份文件不存在"));
    }
    let files = read_zip(path)?;
    package::inspect_files(&files)?;
    apply(store, &files, true)?;
    Ok(())
}

/// `exact` 为真时按包内字节写回，供导入失败后的 `import-` 备份使用。不清除密钥。
/// 为假时整理侧车，并按导入规则重写工具副本。校验失败的工具留下本机副本和侧车。
fn apply(
    store: &Store,
    files: &[StoredFile],
    exact: bool,
) -> Result<Vec<EntityChanged>, BackupError> {
    let mut keep = std::collections::BTreeSet::new();
    let mut events = Vec::new();
    let mut copies: std::collections::BTreeMap<String, Vec<(String, Vec<u8>)>> =
        std::collections::BTreeMap::new();
    let mut sidecars = Vec::new();
    for file in files {
        if file.name == "manifest.json" || package::is_legacy_github(&file.name) {
            continue;
        }
        if let Some((id, rel)) = tools::split_copy(&file.name) {
            copies
                .entry(id.to_owned())
                .or_default()
                .push((rel.to_owned(), file.bytes.clone()));
            continue;
        }
        let kind = package::classify(&file.name)?;
        if !exact && tools::is_sidecar(kind) {
            sidecars.push(file);
            continue;
        }
        let path = package::disk_path(store, &file.name)?;
        if let Some(parent) = path.parent() {
            storage::fs_create_dir_all(parent)
                .map_err(|_| BackupError::io("无法创建目录", parent))?;
        }
        storage::atomic_write(&path, &file.bytes)
            .map_err(|_| BackupError::io("无法写入", &path))?;
        keep.insert(file.name.clone());
        if let Some(event) = change_for(&file.name) {
            events.push(event);
        }
    }
    let (names, restored, skipped, clear_secrets) = if exact {
        let (names, restored) = tools::write_raw_copies(store, copies)?;
        (
            names,
            restored,
            std::collections::BTreeSet::new(),
            Vec::new(),
        )
    } else {
        let restored = tools::restore_copies(store, copies)?;
        (
            restored.keep,
            restored.restored,
            restored.skipped,
            restored.clear_secrets,
        )
    };
    keep.extend(names);
    if !exact {
        let data_dir = store.data_dir();
        for id in &skipped {
            keep.extend(tools::files_under_tool(&data_dir, id)?);
            keep.extend(tools::existing_sidecars(&data_dir, id));
        }
        tools::write_restored_sidecars(store, &sidecars, &restored, &skipped, &mut keep)?;
    }
    for (name, path) in package::list_data_files(store)? {
        if !keep.contains(&name) {
            storage::fs_remove_file(&path).map_err(|_| BackupError::io("无法删除", &path))?;
            if let Some(event) = change_for(&name) {
                events.push(event);
            }
        }
    }
    let mut kept_tools = restored;
    kept_tools.extend(skipped);
    tools::finish_tools(&store.data_dir(), &kept_tools)?;
    if !exact {
        tools::clear_product_secrets(&clear_secrets)?;
    }
    Ok(events)
}

fn change_for(name: &str) -> Option<EntityChanged> {
    let (kind, id) = match name {
        "config.json" => (EntityKind::Config, "config".to_owned()),
        _ => {
            let (dir, file) = name.rsplit_once('/')?;
            let id = file.strip_suffix(".json")?.to_owned();
            let kind = match dir {
                "todos" => EntityKind::Todo,
                "notes" => EntityKind::Note,
                "shelves" => EntityKind::Shelf,
                _ => return None,
            };
            (kind, id)
        }
    };
    Some(EntityChanged {
        kind,
        id,
        revision: None,
    })
}

fn read_zip(path: &Path) -> Result<Vec<StoredFile>, BackupError> {
    let len = storage::fs_file_len(path).map_err(|_| BackupError::io("无法读取", path))?;
    if len > zipstore::PACKAGE_MAX_BYTES {
        return Err(BackupError::Rejected("压缩包过大"));
    }
    let bytes = storage::fs_read(path).map_err(|_| BackupError::io("无法读取", path))?;
    zipstore::read_stored(&bytes)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), BackupError> {
    if let Some(parent) = path.parent() {
        storage::fs_create_dir_all(parent).map_err(|_| BackupError::io("无法创建目录", parent))?;
    }
    storage::atomic_write(path, bytes).map_err(|_| BackupError::io("无法写入", path))
}

fn format_day(day: CivilDate) -> String {
    format!("{:04}-{:02}-{:02}", day.year(), day.month(), day.day())
}

fn list_retained_holding(store: &Store) -> Result<Vec<ListedBackup>, BackupError> {
    let dir = store.backups_dir();
    if !storage::fs_exists(&dir) {
        return Ok(Vec::new());
    }
    let entries = storage::fs_read_dir(&dir).map_err(|_| BackupError::io("无法读取", &dir))?;
    let mut rows = Vec::new();
    for entry in entries {
        if !entry.is_file {
            continue;
        }
        let Some(name) = entry.path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(item) = listed_from_name(name) else {
            continue;
        };
        rows.push((item, name.to_owned()));
    }
    rows.sort_by(|left, right| right.0.at_ms.cmp(&left.0.at_ms).then(right.1.cmp(&left.1)));
    Ok(rows.into_iter().map(|(item, _)| item).collect())
}

fn listed_from_name(name: &str) -> Option<ListedBackup> {
    let millis = package::retention_millis(name)?;
    let at_ms = i64::try_from(millis).ok()?;
    let stem = name.strip_suffix(".zip")?;
    if stem.strip_prefix("manual-").is_some() {
        return Some(ListedBackup {
            kind: BackupKind::Manual,
            at_ms,
            day: None,
            file_name: name.to_owned(),
        });
    }
    let rest = stem.strip_prefix("auto-")?;
    let (day_text, _) = rest.rsplit_once('-')?;
    Some(ListedBackup {
        kind: BackupKind::Auto,
        at_ms,
        day: parse_day(day_text),
        file_name: name.to_owned(),
    })
}

fn parse_day(text: &str) -> Option<CivilDate> {
    let mut parts = text.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    CivilDate::try_from_ymd(year, month, day)
}

/// UTC。`at_ms` 为负时仍给出 1970 年之前的日期。
fn format_utc_minute(at_ms: i64) -> String {
    let secs = at_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Howard Hinnant 的公历算法。`days` 是 Unix 纪元起的天数。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StorePaths;
    use crate::storage::test_temp::TempDir;

    fn store() -> (TempDir, Store) {
        let temp = TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            user_profile: temp.path().join("profile"),
            local_app_data: temp.path().join("local"),
        })
        .unwrap();
        (temp, store)
    }

    fn evil(temp: &TempDir, files: Vec<StoredFile>) -> PathBuf {
        let path = temp.path().join("evil.zip");
        std::fs::write(&path, zipstore::write_stored(&files).unwrap()).unwrap();
        path
    }

    #[test]
    fn rejected_path_leaves_data_and_pending_untouched() {
        let (temp, store) = store();
        std::fs::write(
            store.data_dir().join("config.json"),
            br#"{"schemaVersion":1,"title":"keep"}"#,
        )
        .unwrap();
        let package = evil(
            &temp,
            vec![StoredFile {
                name: "../secret.json".to_owned(),
                bytes: b"{}".to_vec(),
            }],
        );
        let err = BackupService::open(store.clone())
            .import(&package, 1)
            .unwrap_err();
        assert!(err.to_string().contains("不允许的路径"));
        assert!(store.read_import_pending().unwrap().is_none());
        let text = std::fs::read_to_string(store.data_dir().join("config.json")).unwrap();
        assert!(text.contains("keep"));
        assert!(!store.data_dir().join("secret.json").exists());
    }

    #[test]
    fn unsupported_schema_leaves_data_unchanged() {
        let (temp, store) = store();
        std::fs::write(
            store.data_dir().join("notes").join("n.json"),
            br#"{"schemaVersion":1,"body":"keep"}"#,
        )
        .unwrap();
        let note = br#"{"schemaVersion":99,"body":"nope"}"#;
        let package = evil(
            &temp,
            vec![
                StoredFile {
                    name: "manifest.json".to_owned(),
                    bytes: br#"{"packageSchemaVersion":1,"exportedAt":5,"appVersion":"0.1.0","counts":{"todos":0,"notes":1,"shelves":0,"config":0,"userApps":0,"watchlist":0,"githubCaches":0}}"#.to_vec(),
                },
                StoredFile {
                    name: "notes/n.json".to_owned(),
                    bytes: note.to_vec(),
                },
            ],
        );
        let err = BackupService::open(store.clone())
            .import(&package, 6)
            .unwrap_err();
        assert!(err.to_string().contains("schemaVersion"));
        assert!(store.read_import_pending().unwrap().is_none());
        let text = std::fs::read_to_string(store.data_dir().join("notes").join("n.json")).unwrap();
        assert!(text.contains("keep"));
        assert!(!text.contains("nope"));
    }

    #[test]
    fn import_reloads_note_memory_before_publish() {
        use std::sync::{Arc, Mutex};

        use crate::notes::{NoteError, NoteInput, NoteService};

        let (temp, store) = store();
        let notes = NoteService::open(store.clone()).unwrap();
        let created = notes
            .create(&NoteInput {
                title: "标题".to_owned(),
                body: "imported-body".to_owned(),
                tags: Vec::new(),
                pinned: false,
            })
            .unwrap();
        let package = temp.path().join("pkg.zip");
        let commands = BackupService::open(store.clone());
        commands.export(&package, 1).unwrap();
        let edited = notes
            .save(
                &created.id,
                created.revision,
                &NoteInput {
                    title: "标题".to_owned(),
                    body: "edited-body".to_owned(),
                    tags: Vec::new(),
                    pinned: false,
                },
            )
            .unwrap();

        let during = Arc::new(Mutex::new(None));
        let during_probe = Arc::clone(&during);
        let notes_probe = notes.clone();
        let id = created.id.clone();
        store.set_publish_probe(Some(Arc::new(move || {
            let body = notes_probe.get(&id).unwrap().body;
            *during_probe.lock().expect("probe") = Some(body);
        })));
        commands.import(&package, 2).unwrap();
        store.set_publish_probe(None);

        assert_eq!(
            during.lock().expect("probe").clone().expect("发布时已重载"),
            "imported-body"
        );
        assert_eq!(notes.get(&created.id).unwrap().body, "imported-body");
        let err = notes
            .save(
                &created.id,
                edited.revision,
                &NoteInput {
                    title: "标题".to_owned(),
                    body: "edited-body".to_owned(),
                    tags: Vec::new(),
                    pinned: false,
                },
            )
            .unwrap_err();
        assert!(matches!(err, NoteError::Conflict { .. }), "{err}");
    }

    #[test]
    fn import_accepts_old_packages_that_still_contain_github_files() {
        let (temp, store) = store();
        std::fs::create_dir_all(store.data_dir().join("github").join("cache")).unwrap();
        std::fs::write(
            store
                .data_dir()
                .join("github")
                .join("cache")
                .join("local.json"),
            br#"{"schemaVersion":1,"repo":"left-alone"}"#,
        )
        .unwrap();
        std::fs::write(
            store.data_dir().join("notes").join("old.json"),
            br#"{"schemaVersion":1,"body":"will-go"}"#,
        )
        .unwrap();
        let manifest = br#"{"packageSchemaVersion":1,"exportedAt":5,"appVersion":"0.1.0","counts":{"todos":0,"notes":1,"shelves":0,"config":0,"userApps":0,"watchlist":1,"githubCaches":1}}"#;
        let package = evil(
            &temp,
            vec![
                StoredFile {
                    name: "manifest.json".to_owned(),
                    bytes: manifest.to_vec(),
                },
                StoredFile {
                    name: "notes/n.json".to_owned(),
                    bytes: br#"{"schemaVersion":1,"body":"from-old-package"}"#.to_vec(),
                },
                StoredFile {
                    name: "github/watchlist.json".to_owned(),
                    bytes: br#"{"schemaVersion":1,"repos":["acme/app"]}"#.to_vec(),
                },
                StoredFile {
                    name: "github/cache/acme%2Fapp.json".to_owned(),
                    bytes: br#"{"schemaVersion":1,"repo":"from-package"}"#.to_vec(),
                },
            ],
        );
        let overview = BackupService::open(store.clone())
            .import(&package, 8)
            .unwrap();
        assert!(overview.watchlist);
        assert_eq!(overview.github_caches, 1);
        assert_eq!(overview.notes, 1);
        let note = std::fs::read_to_string(store.data_dir().join("notes").join("n.json")).unwrap();
        assert!(note.contains("from-old-package"));
        assert!(!store.data_dir().join("notes").join("old.json").exists());
        assert!(
            !store
                .data_dir()
                .join("github")
                .join("watchlist.json")
                .exists()
        );
        assert!(
            !store
                .data_dir()
                .join("github")
                .join("cache")
                .join("acme%2Fapp.json")
                .exists()
        );
        let left = std::fs::read_to_string(
            store
                .data_dir()
                .join("github")
                .join("cache")
                .join("local.json"),
        )
        .unwrap();
        assert!(left.contains("left-alone"));
        assert!(store.read_import_pending().unwrap().is_none());
    }

    #[test]
    fn export_omits_web_icon_cache_and_import_keeps_it() {
        let (temp, store) = store();
        std::fs::create_dir_all(store.data_dir().join("shelves")).unwrap();
        std::fs::write(
            store.data_dir().join("shelves").join("g.json"),
            br#"{"schemaVersion":1,"name":"shelf"}"#,
        )
        .unwrap();
        let icon = store
            .data_dir()
            .join("cache")
            .join("web-icons")
            .join("abc.bin");
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&icon, b"LWFI-keep").unwrap();
        std::fs::write(
            store.data_dir().join("todo-purge-pending.json"),
            br#"{"schemaVersion":1,"ids":["purge-marker-stay"]}"#,
        )
        .unwrap();
        std::fs::write(
            store.data_dir().join("token.json"),
            b"token-marker-not-exported",
        )
        .unwrap();

        let package = temp.path().join("pkg.zip");
        BackupService::open(store.clone())
            .export(&package, 3)
            .unwrap();
        let files = read_zip(&package).unwrap();
        assert!(files.iter().any(|file| file.name == "shelves/g.json"));
        assert!(
            files
                .iter()
                .all(|file| !file.name.replace('\\', "/").starts_with("cache/"))
        );
        assert!(
            files
                .iter()
                .all(|file| file.name != "todo-purge-pending.json")
        );
        assert!(files.iter().all(|file| file.name != "token.json"));
        let packed = String::from_utf8_lossy(&std::fs::read(&package).unwrap()).into_owned();
        assert!(!packed.contains("purge-marker-stay"));
        assert!(!packed.contains("token-marker-not-exported"));
        assert!(!packed.contains("LWFI-keep"));

        BackupService::open(store.clone())
            .import(&package, 4)
            .unwrap();
        assert_eq!(std::fs::read(&icon).unwrap(), b"LWFI-keep");
        let purge =
            std::fs::read_to_string(store.data_dir().join("todo-purge-pending.json")).unwrap();
        assert!(purge.contains("purge-marker-stay"));
    }

    fn package_manifest(notes: u64) -> Vec<u8> {
        format!(
            "{{\"packageSchemaVersion\":1,\"exportedAt\":1,\"appVersion\":\"test\",\"counts\":{{\"todos\":0,\"notes\":{notes},\"shelves\":0,\"config\":0,\"userApps\":0}}}}\n"
        )
        .into_bytes()
    }

    fn tool_manifest() -> Vec<u8> {
        r#"
id = "example.quota"
name = "API 额度"
description = "剩余额度"
entry = "main.lua"
services = ["storage"]

[[component]]
id = "summary"
component = "text"
text = "没有数据"
"#
        .as_bytes()
        .to_vec()
    }

    #[test]
    fn import_skips_a_bad_tool_and_refuses_a_path_that_escapes() {
        let (temp, store) = store();
        let note = br#"{"schemaVersion":1,"body":"kept-note"}"#;
        let mut good = vec![
            super::StoredFile {
                name: "manifest.json".to_string(),
                bytes: package_manifest(1),
            },
            super::StoredFile {
                name: "notes/n.json".to_string(),
                bytes: note.to_vec(),
            },
            super::StoredFile {
                name: "tools/example.quota/manifest.toml".to_string(),
                bytes: tool_manifest(),
            },
            super::StoredFile {
                name: "tools/example.quota/main.lua".to_string(),
                bytes: b"function on_load() end\n".to_vec(),
            },
            super::StoredFile {
                name: "tools/example.quota/helper.lua".to_string(),
                bytes: b"helper-secret-marker".to_vec(),
            },
            super::StoredFile {
                name: "tools/example.quota/grant.json".to_string(),
                bytes: br#"{"schemaVersion":1,"services":["notify","storage"],"domains":[],"secrets":[],"value":"secret-marker-77"}"#.to_vec(),
            },
            super::StoredFile {
                name: "tools/example.bad/manifest.toml".to_string(),
                bytes: b"bad-body-marker".to_vec(),
            },
            super::StoredFile {
                name: "tools/.incoming-example.quota/leak.txt".to_string(),
                bytes: b"scratch-marker-77".to_vec(),
            },
        ];
        good.sort_by(|left, right| left.name.cmp(&right.name));
        let package = temp.path().join("tools.zip");
        std::fs::write(
            &package,
            super::super::zipstore::write_stored(&good).unwrap(),
        )
        .unwrap();
        BackupService::open(store.clone())
            .import(&package, 5)
            .unwrap();
        assert!(
            std::fs::read_to_string(store.data_dir().join("notes").join("n.json"))
                .unwrap()
                .contains("kept-note")
        );
        let grant = std::fs::read(store.data_dir().join("tools/example.quota/grant.json")).unwrap();
        let grant_text = String::from_utf8(grant).unwrap();
        assert!(grant_text.contains("storage"));
        assert!(!grant_text.contains("notify"));
        assert!(!grant_text.contains("secret-marker-77"));
        assert!(
            !store
                .data_dir()
                .join("tools/example.quota/helper.lua")
                .exists()
        );
        assert!(!store.data_dir().join("tools/example.bad").exists());
        assert!(
            !store
                .data_dir()
                .join("tools/.incoming-example.quota")
                .exists()
        );
        let log = std::fs::read_to_string(store.log_path()).unwrap_or_default();
        assert!(log.contains("工具未恢复"));
        assert!(log.contains("example.bad"));
        assert!(!log.contains("bad-body-marker"));
        assert!(!log.contains("secret-marker-77"));
        assert!(!log.contains("scratch-marker-77"));

        let escaped = vec![
            super::StoredFile {
                name: "manifest.json".to_string(),
                bytes: package_manifest(1),
            },
            super::StoredFile {
                name: "notes/n.json".to_string(),
                bytes: note.to_vec(),
            },
            super::StoredFile {
                name: "tools/example.quota/../../notes/pwn.json".to_string(),
                bytes: b"pwn-marker".to_vec(),
            },
        ];
        let evil = temp.path().join("evil.zip");
        std::fs::write(
            &evil,
            super::super::zipstore::write_stored(&escaped).unwrap(),
        )
        .unwrap();
        assert!(BackupService::open(store.clone()).import(&evil, 6).is_err());
        assert!(!store.data_dir().join("notes").join("pwn.json").exists());
        assert!(
            std::fs::read_to_string(store.data_dir().join("notes").join("n.json"))
                .unwrap()
                .contains("kept-note")
        );
    }

    #[test]
    fn import_rollback_writes_the_original_tool_bytes() {
        let (_temp, store) = store();
        let data = store.data_dir();
        std::fs::create_dir_all(data.join("notes")).unwrap();
        std::fs::write(
            data.join("notes/n.json"),
            br#"{"schemaVersion":1,"body":"old-note"}"#,
        )
        .unwrap();
        let tool = data.join("tools/example.quota");
        std::fs::create_dir_all(&tool).unwrap();
        std::fs::write(tool.join("manifest.toml"), tool_manifest()).unwrap();
        std::fs::write(tool.join("main.lua"), b"function on_load() end\n").unwrap();
        std::fs::write(tool.join("helper.lua"), b"helper-secret-marker").unwrap();
        std::fs::write(tool.join("grant.json"), br#"{"value":"secret-marker-77"}"#).unwrap();
        std::fs::create_dir_all(data.join("tool-windows")).unwrap();
        std::fs::write(
            data.join("tool-windows/example.quota.json"),
            br#"{"schemaVersion":1,"open":true,"floatWindow":{"x":12,"y":4,"width":200,"height":120}}"#,
        )
        .unwrap();

        let raw = super::super::package::collect_raw(&store).unwrap();
        assert!(raw.iter().any(|file| {
            file.name == "tools/example.quota/helper.lua" && file.bytes == b"helper-secret-marker"
        }));
        assert!(raw.iter().any(|file| {
            file.name == "tools/example.quota/grant.json"
                && String::from_utf8_lossy(&file.bytes).contains("secret-marker-77")
        }));
        assert!(raw.iter().any(|file| {
            file.name == "tool-windows/example.quota.json"
                && String::from_utf8_lossy(&file.bytes).contains("\"open\":true")
        }));
        let clean = super::super::package::collect(&store).unwrap();
        assert!(
            clean
                .iter()
                .all(|file| file.bytes != b"helper-secret-marker")
        );
        assert!(
            clean
                .iter()
                .all(|file| !String::from_utf8_lossy(&file.bytes).contains("secret-marker-77"))
        );
        assert!(
            clean
                .iter()
                .all(|file| { !String::from_utf8_lossy(&file.bytes).contains("\"open\":true") })
        );

        std::fs::remove_file(tool.join("helper.lua")).unwrap();
        std::fs::write(tool.join("grant.json"), b"{}\n").unwrap();
        std::fs::write(tool.join("extra.lua"), b"new-extra").unwrap();
        std::fs::write(
            data.join("tool-windows/example.quota.json"),
            br#"{"schemaVersion":1,"open":false}"#,
        )
        .unwrap();
        let (files, _) = super::super::package::with_manifest(raw, 1).unwrap();
        super::apply(&store, &files, true).unwrap();
        assert_eq!(
            std::fs::read(tool.join("helper.lua")).unwrap(),
            b"helper-secret-marker"
        );
        assert!(
            std::fs::read_to_string(tool.join("grant.json"))
                .unwrap()
                .contains("secret-marker-77")
        );
        assert!(
            std::fs::read_to_string(data.join("tool-windows/example.quota.json"))
                .unwrap()
                .contains("\"open\":true")
        );
        assert!(!tool.join("extra.lua").exists());
        assert!(
            std::fs::read_to_string(data.join("notes/n.json"))
                .unwrap()
                .contains("old-note")
        );
        let capture = tools::SecretCapture::begin();
        super::apply(&store, &files, true).unwrap();
        assert!(
            capture.ids().is_empty(),
            "原样写回不清除密钥：{:?}",
            capture.ids()
        );
    }

    #[test]
    fn a_zip_longer_than_the_package_cap_is_rejected_before_reading() {
        let (temp, store) = store();
        let path = temp.path().join("huge.zip");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(super::super::zipstore::PACKAGE_MAX_BYTES + 1)
            .unwrap();
        let err = BackupService::open(store).import(&path, 1).unwrap_err();
        assert!(err.to_string().contains("压缩包过大"), "{err}");
    }

    #[test]
    fn sidecar_ids_use_the_tool_id_rules() {
        let (temp, store) = store();
        std::fs::write(
            store.data_dir().join("config.json"),
            b"{\"schemaVersion\":1}\n",
        )
        .unwrap();
        let window = br#"{"schemaVersion":1,"floatWindow":{"x":1,"y":2,"width":200,"height":120}}"#;
        for name in [
            "tool-windows/lanwork.float-shell-demo.json",
            "tool-storage/.hidden.json",
            "tool-windows/.hidden.json",
        ] {
            let package = evil(
                &temp,
                vec![
                    super::StoredFile {
                        name: "manifest.json".to_owned(),
                        bytes: package_manifest(0),
                    },
                    super::StoredFile {
                        name: name.to_owned(),
                        bytes: window.to_vec(),
                    },
                ],
            );
            let err = BackupService::open(store.clone())
                .import(&package, 2)
                .unwrap_err();
            assert!(err.to_string().contains("不允许的路径"), "{name}: {err}");
        }
        let enabled = evil(
            &temp,
            vec![
                super::StoredFile {
                    name: "manifest.json".to_owned(),
                    bytes: package_manifest(0),
                },
                super::StoredFile {
                    name: "tool-enabled.json".to_owned(),
                    bytes: br#"{"schemaVersion":1,"disabled":["lanwork.float-shell-demo"]}"#
                        .to_vec(),
                },
            ],
        );
        let err = BackupService::open(store.clone())
            .import(&enabled, 3)
            .unwrap_err();
        assert!(err.to_string().contains("不允许的路径"), "{err}");
        assert!(store.data_dir().join("config.json").is_file());
        assert!(!store.data_dir().join("tool-windows").exists());

        std::fs::create_dir_all(store.data_dir().join("tool-windows")).unwrap();
        std::fs::write(
            store
                .data_dir()
                .join("tool-windows/lanwork.float-shell-demo.json"),
            window,
        )
        .unwrap();
        std::fs::write(
            store.data_dir().join("tool-enabled.json"),
            br#"{"schemaVersion":1,"disabled":["example.quota","lanwork.float-shell-demo"]}"#,
        )
        .unwrap();
        let packed = super::super::package::collect(&store).unwrap();
        assert!(packed.iter().all(|file| {
            !file.name.contains("lanwork.")
                && !file.bytes.windows(8).any(|window| window == b"lanwork.")
        }));
        let enabled = packed
            .iter()
            .find(|file| file.name == "tool-enabled.json")
            .unwrap();
        let text = String::from_utf8(enabled.bytes.clone()).unwrap();
        assert!(text.contains("example.quota"));
        assert!(!text.contains("lanwork."));
    }

    #[test]
    fn a_skipped_tool_keeps_its_local_copy_and_an_absent_tool_is_replaced() {
        let (temp, store) = store();
        let data = store.data_dir();
        write_local_tool(
            &data,
            "example.bad",
            b"local-bad-manifest",
            b"local-bad-entry",
        );
        write_local_tool(
            &data,
            "example.gone",
            b"local-gone-manifest",
            b"local-gone-entry",
        );
        for id in ["example.bad", "example.gone"] {
            std::fs::create_dir_all(data.join("tool-storage")).unwrap();
            std::fs::write(
                data.join(format!("tool-storage/{id}.json")),
                format!(r#"{{"schemaVersion":1,"entries":{{"note":"keep-{id}"}}}}"#),
            )
            .unwrap();
            std::fs::create_dir_all(data.join("tool-windows")).unwrap();
            std::fs::write(
                data.join(format!("tool-windows/{id}.json")),
                format!(r#"{{"schemaVersion":1,"open":true,"mark":"keep-{id}"}}"#),
            )
            .unwrap();
        }
        std::fs::write(
            data.join("tool-enabled.json"),
            br#"{"schemaVersion":1,"disabled":["example.bad","example.gone"]}"#,
        )
        .unwrap();
        let capture = tools::SecretCapture::begin();
        let mut files = quota_files("剩余额度", "function on_load() end\n");
        files.push(super::StoredFile {
            name: "tools/example.bad/manifest.toml".to_owned(),
            bytes: b"not-a-manifest".to_vec(),
        });
        files.push(super::StoredFile {
            name: "tool-storage/example.bad.json".to_owned(),
            bytes: br#"{"schemaVersion":1,"entries":{"note":"from-package"}}"#.to_vec(),
        });
        files.push(super::StoredFile {
            name: "tool-storage/example.quota.json".to_owned(),
            bytes: br#"{"schemaVersion":1,"entries":{"note":"quota-storage"}}"#.to_vec(),
        });
        files.push(super::StoredFile {
            name: "tool-windows/example.quota.json".to_owned(),
            bytes: br#"{"schemaVersion":1,"open":true,"floatWindow":{"x":8,"y":9,"width":200,"height":120}}"#.to_vec(),
        });
        files.push(super::StoredFile {
            name: "tool-enabled.json".to_owned(),
            bytes: br#"{"schemaVersion":1,"disabled":["example.quota"]}"#.to_vec(),
        });
        let package = temp.path().join("skip.zip");
        std::fs::write(
            &package,
            super::super::zipstore::write_stored(&files).unwrap(),
        )
        .unwrap();
        BackupService::open(store.clone())
            .import(&package, 4)
            .unwrap();
        assert_eq!(
            capture.ids(),
            vec!["example.quota".to_owned()],
            "只有新恢复的工具清密钥，跳过的 example.bad 不清：{:?}",
            capture.ids()
        );
        assert_eq!(
            std::fs::read(data.join("tools/example.bad/manifest.toml")).unwrap(),
            b"local-bad-manifest"
        );
        assert!(
            std::fs::read_to_string(data.join("tool-storage/example.bad.json"))
                .unwrap()
                .contains("keep-example.bad")
        );
        assert!(
            std::fs::read_to_string(data.join("tool-windows/example.bad.json"))
                .unwrap()
                .contains("keep-example.bad")
        );
        assert!(!data.join("tools/example.gone").exists());
        assert!(!data.join("tool-storage/example.gone.json").exists());
        assert!(!data.join("tool-windows/example.gone.json").exists());
        assert!(
            std::fs::read_to_string(data.join("tools/example.quota/main.lua"))
                .unwrap()
                .contains("on_load")
        );
        assert!(
            std::fs::read_to_string(data.join("tool-storage/example.quota.json"))
                .unwrap()
                .contains("quota-storage")
        );
        let window = std::fs::read_to_string(data.join("tool-windows/example.quota.json")).unwrap();
        assert!(window.contains("\"x\":8"));
        assert!(!window.contains("\"open\":true"));
        let enabled = std::fs::read_to_string(data.join("tool-enabled.json")).unwrap();
        assert!(enabled.contains("example.quota"));
        assert!(enabled.contains("example.bad"));
        assert!(!enabled.contains("example.gone"));
    }

    #[test]
    fn secrets_clear_only_when_the_manifest_or_entry_changes() {
        let (temp, store) = store();
        let entry = "function on_load() end\n";
        let changed = "function on_load() end\n--changed\n";
        let first = temp.path().join("first.zip");
        std::fs::write(
            &first,
            super::super::zipstore::write_stored(&quota_files("剩余额度", entry)).unwrap(),
        )
        .unwrap();
        let capture = tools::SecretCapture::begin();
        BackupService::open(store.clone())
            .import(&first, 11)
            .unwrap();
        assert_eq!(capture.ids(), vec!["example.quota".to_owned()]);
        drop(capture);

        let capture = tools::SecretCapture::begin();
        BackupService::open(store.clone())
            .import(&first, 12)
            .unwrap();
        assert!(capture.ids().is_empty(), "{:?}", capture.ids());
        drop(capture);

        let next = temp.path().join("entry.zip");
        std::fs::write(
            &next,
            super::super::zipstore::write_stored(&quota_files("剩余额度", changed)).unwrap(),
        )
        .unwrap();
        let capture = tools::SecretCapture::begin();
        BackupService::open(store.clone())
            .import(&next, 13)
            .unwrap();
        assert_eq!(capture.ids(), vec!["example.quota".to_owned()]);
        drop(capture);

        let manifest_only = temp.path().join("manifest.zip");
        std::fs::write(
            &manifest_only,
            super::super::zipstore::write_stored(&quota_files("另一句", changed)).unwrap(),
        )
        .unwrap();
        let capture = tools::SecretCapture::begin();
        BackupService::open(store.clone())
            .import(&manifest_only, 14)
            .unwrap();
        assert_eq!(capture.ids(), vec!["example.quota".to_owned()]);
        drop(capture);

        let capture = tools::SecretCapture::begin();
        BackupService::open(store)
            .import(&manifest_only, 15)
            .unwrap();
        assert!(capture.ids().is_empty(), "{:?}", capture.ids());
    }

    #[test]
    fn inspect_lists_permission_lines_and_writes_nothing_until_import() {
        let (temp, store) = store();
        let data = store.data_dir();
        std::fs::create_dir_all(data.join("notes")).unwrap();
        std::fs::write(
            data.join("notes/n.json"),
            br#"{"schemaVersion":1,"body":"kept-note"}"#,
        )
        .unwrap();
        let bad = data.join("tools/example.bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("keep.txt"), b"kept-bad-local").unwrap();
        let manifest = rich_manifest();
        let package = temp.path().join("preview.zip");
        std::fs::write(
            &package,
            zipstore::write_stored(&preview_files(&manifest)).unwrap(),
        )
        .unwrap();

        let before = std::fs::read(data.join("notes/n.json")).unwrap();
        let preview = BackupService::open(store.clone())
            .inspect(&package)
            .unwrap();
        assert_eq!(
            std::fs::read(data.join("notes/n.json")).unwrap(),
            before,
            "没确认就不写入"
        );
        assert!(store.read_import_pending().unwrap().is_none());
        assert!(!data.join("tools/example.quota").exists());
        assert_eq!(
            std::fs::read(bad.join("keep.txt")).unwrap(),
            b"kept-bad-local"
        );
        assert!(
            preview
                .tools
                .iter()
                .all(|tool| !tool.name.contains(".incoming"))
        );

        let loaded = lanwork_toolhost::load_dir(&materialize_tool(temp.path(), &manifest)).unwrap();
        let expected = lanwork_toolhost::permission_lines(&loaded);
        assert!(expected.iter().any(|line| line == "访问 api.example.com"));
        assert!(expected.iter().any(|line| line == "访问 cdn.example.com"));
        assert!(
            expected
                .iter()
                .any(|line| line == "使用你填写的 令牌，请求头 Authorization，前缀 Bearer ")
        );
        assert!(
            expected
                .iter()
                .any(|line| line == "使用你填写的 备用令牌，请求头 X-Spare")
        );
        assert!(expected.iter().any(|line| line == "发送通知"));
        assert_eq!(preview.tools.len(), 2);
        assert_eq!(preview.tools[0].name, "example.bad");
        let skipped = preview.tools[0].confirmation_lines();
        assert_eq!(skipped[0].text, "example.bad");
        assert!(skipped[0].heading);
        assert_eq!(skipped[1].text, "不会恢复：清单无法解析");
        assert!(preview.tools[0].permissions.is_empty());
        assert_eq!(preview.tools[1].name, "API 额度");
        assert_eq!(preview.tools[1].permissions, expected);
        let shown: Vec<String> = preview.tools[1]
            .confirmation_lines()
            .into_iter()
            .filter(|line| !line.heading)
            .map(|line| line.text)
            .collect();
        assert_eq!(shown, expected);
        let joined = shown.join("\n");
        assert!(!joined.contains("evil.example"));
        assert!(!joined.contains("后门"));

        BackupService::open(store.clone())
            .import(&package, 21)
            .unwrap();
        assert!(data.join("tools/example.quota/manifest.toml").is_file());
        let grant = std::fs::read_to_string(data.join("tools/example.quota/grant.json")).unwrap();
        assert!(grant.contains("api.example.com"));
        assert!(grant.contains("令牌"));
        assert!(!grant.contains("evil.example"));
        assert!(!grant.contains("后门"));
        assert!(!grant.contains("notify-from-grant"));
        assert_eq!(
            std::fs::read(bad.join("keep.txt")).unwrap(),
            b"kept-bad-local"
        );
        assert!(!data.join("tools/.incoming-example.quota").exists());
    }

    #[test]
    fn retained_backups_preview_the_same_lines_and_wait_for_import() {
        let (temp, store) = store();
        let data = store.data_dir();
        let manifest = rich_manifest();
        let tool = materialize_tool(&data.join("tools/example.quota"), &manifest);
        std::fs::write(tool.join("main.lua"), b"function on_load() end\n").unwrap();
        let manual = BackupService::open(store.clone())
            .backup_manual(30)
            .unwrap();
        let auto = match BackupService::open(store.clone())
            .backup_on_launch(
                crate::capture::CivilDate::try_from_ymd(2026, 10, 16).unwrap(),
                31,
            )
            .unwrap()
        {
            LaunchBackup::Created { path } => path,
            LaunchBackup::AlreadyDone => panic!("这一天还没有自动备份"),
        };
        std::fs::remove_dir_all(data.join("tools")).unwrap();
        let loaded = lanwork_toolhost::load_dir(&materialize_tool(temp.path(), &manifest)).unwrap();
        let expected = lanwork_toolhost::permission_lines(&loaded);
        for path in [&manual, &auto] {
            let preview = BackupService::open(store.clone()).inspect(path).unwrap();
            assert!(!data.join("tools").exists(), "没确认就不写入");
            assert_eq!(preview.tools.len(), 1);
            assert_eq!(preview.tools[0].permissions, expected);
            assert!(preview.tools[0].skip_reason.is_none());
        }
        BackupService::open(store.clone())
            .import(&manual, 32)
            .unwrap();
        assert!(data.join("tools/example.quota/manifest.toml").is_file());
        let grant = std::fs::read_to_string(data.join("tools/example.quota/grant.json")).unwrap();
        assert!(grant.contains("api.example.com"));
        assert!(!grant.contains("evil.example"));
    }

    fn rich_manifest() -> String {
        "\
id = \"example.quota\"
name = \"API 额度\"
description = \"剩余额度\"
entry = \"main.lua\"
services = [\"http\", \"notify\", \"secret\", \"storage\", \"timer\", \"todo.complete\", \"todo.read\"]

[http]
domains = [\"api.example.com\", \"cdn.example.com\"]

[[secret]]
id = \"token\"
label = \"令牌\"
header = \"Authorization\"
prefix = \"Bearer \"

[[secret]]
id = \"spare\"
label = \"备用令牌\"
header = \"X-Spare\"

[[component]]
id = \"summary\"
component = \"text\"
text = \"没有数据\"
"
        .to_string()
    }

    fn preview_files(manifest: &str) -> Vec<super::StoredFile> {
        vec![
            super::StoredFile {
                name: "manifest.json".to_owned(),
                bytes: package_manifest(0),
            },
            super::StoredFile {
                name: "tools/example.bad/manifest.toml".to_owned(),
                bytes: b"not-a-manifest".to_vec(),
            },
            super::StoredFile {
                name: "tools/example.quota/manifest.toml".to_owned(),
                bytes: manifest.as_bytes().to_vec(),
            },
            super::StoredFile {
                name: "tools/example.quota/main.lua".to_owned(),
                bytes: b"function on_load() end\n".to_vec(),
            },
            super::StoredFile {
                name: "tools/example.quota/grant.json".to_owned(),
                bytes: r#"{"schemaVersion":1,"services":["notify"],"domains":["evil.example"],"secrets":[{"id":"x","label":"后门"}],"marker":"notify-from-grant"}"#
                    .as_bytes()
                    .to_vec(),
            },
            super::StoredFile {
                name: "tools/.incoming-example.quota/leak.txt".to_owned(),
                bytes: b"scratch".to_vec(),
            },
        ]
    }

    fn materialize_tool(dir: &Path, manifest: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("manifest.toml"), manifest).unwrap();
        std::fs::write(dir.join("main.lua"), b"function on_load() end\n").unwrap();
        dir.to_path_buf()
    }

    fn quota_files(description: &str, entry: &str) -> Vec<super::StoredFile> {
        vec![
            super::StoredFile {
                name: "manifest.json".to_owned(),
                bytes: package_manifest(0),
            },
            super::StoredFile {
                name: "tools/example.quota/manifest.toml".to_owned(),
                bytes: format!(
                    "id = \"example.quota\"\nname = \"API 额度\"\ndescription = \"{description}\"\nentry = \"main.lua\"\nservices = [\"storage\"]\n\n[[component]]\nid = \"summary\"\ncomponent = \"text\"\ntext = \"没有数据\"\n"
                )
                .into_bytes(),
            },
            super::StoredFile {
                name: "tools/example.quota/main.lua".to_owned(),
                bytes: entry.as_bytes().to_vec(),
            },
        ]
    }

    fn write_local_tool(data: &std::path::Path, id: &str, manifest: &[u8], entry: &[u8]) {
        let dir = data.join("tools").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.toml"), manifest).unwrap();
        std::fs::write(dir.join("main.lua"), entry).unwrap();
    }

    #[test]
    fn utc_labels_use_the_unix_epoch() {
        assert_eq!(format_utc_minute(0), "1970-01-01 00:00");
        assert_eq!(format_utc_minute(86_400_000), "1970-01-02 00:00");
        assert_eq!(format_utc_minute(1_700_000_000_000), "2023-11-14 22:13");
    }
}
