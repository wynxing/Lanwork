//! 导出、备份和导入。
//!
//! 不读取环境变量。日志、`backups/` 和 `import.pending` 不进入包。
//! 调用方传入公历日和 UTC 毫秒，本服务不读时钟。

use std::path::{Path, PathBuf};

use super::error::BackupError;
use super::package::{self, PackageOverview};
use super::zipstore::{self, StoredFile};
use crate::capture::CivilDate;
use crate::storage::{self, BootHooks, ImportPending, Store};

/// 同一天再次自动备份时的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchBackup {
    Created { path: PathBuf },
    AlreadyDone,
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

    /// 写出一份导出包。`include_cache` 为假时不含 `github/cache`。
    pub fn export(
        &self,
        dest: &Path,
        include_cache: bool,
        now_ms: i64,
    ) -> Result<PackageOverview, BackupError> {
        let (files, overview) = self
            .store
            .with_write_lock(|| self.snapshot(include_cache, now_ms))?;
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

    /// 校验包并返回概览，不改当前数据。
    pub fn inspect(&self, package: &Path) -> Result<PackageOverview, BackupError> {
        let files = read_zip(package)?;
        package::inspect_files(&files)
    }

    /// 校验通过后先做导入备份，再替换。校验失败时当前数据不变。
    pub fn import(&self, package: &Path, now_ms: i64) -> Result<PackageOverview, BackupError> {
        let files = read_zip(package)?;
        let overview = package::inspect_files(&files)?;
        self.store
            .with_write_lock(|| self.import_holding(&files, now_ms))?;
        Ok(overview)
    }

    /// 启动时若有 `import.pending`，用其中的备份恢复并删掉该文件。
    pub fn register_boot_hooks(hooks: &mut BootHooks) {
        hooks.recover_import = Some(Box::new(|store, pending| {
            recover_import(store, pending).map_err(|err| err.to_string())
        }));
    }

    fn snapshot(
        &self,
        include_cache: bool,
        now_ms: i64,
    ) -> Result<(Vec<StoredFile>, PackageOverview), BackupError> {
        let files = package::collect(&self.store, include_cache)?;
        package::with_manifest(files, now_ms)
    }

    fn write_named(&self, file_name: &str, now_ms: i64) -> Result<PathBuf, BackupError> {
        let (files, _) = self.snapshot(true, now_ms)?;
        let path = self.store.backups_dir().join(file_name);
        write_bytes(&path, &zipstore::write_stored(&files)?)?;
        Ok(path)
    }

    fn import_holding(&self, files: &[StoredFile], now_ms: i64) -> Result<(), BackupError> {
        let backup = self.write_named(&format!("import-{now_ms}.zip"), now_ms)?;
        self.store.write_import_pending_holding_lock(&backup)?;
        if let Err(err) = apply(&self.store, files) {
            restore_backup(&self.store, &backup)?;
            self.store.clear_import_pending_holding_lock()?;
            return Err(err);
        }
        self.store.clear_import_pending_holding_lock()?;
        Ok(())
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
    apply(store, &files)
}

fn apply(store: &Store, files: &[StoredFile]) -> Result<(), BackupError> {
    let mut keep = std::collections::BTreeSet::new();
    for file in files {
        if file.name == "manifest.json" {
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
    }
    for (name, path) in package::list_data_files(store)? {
        if !keep.contains(&name) {
            storage::fs_remove_file(&path).map_err(|_| BackupError::io("无法删除", &path))?;
        }
    }
    Ok(())
}

fn read_zip(path: &Path) -> Result<Vec<StoredFile>, BackupError> {
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
}
