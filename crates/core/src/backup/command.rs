//! 薄命令。规则在 [`super::service`]。

use std::path::{Path, PathBuf};

use super::error::BackupError;
use super::package::PackageOverview;
use super::service::{BackupService, LaunchBackup};
use crate::capture::CivilDate;
use crate::storage::{BootHooks, Store};

/// 设置里的备份、导出和导入调用这些命令。界面尚未接入。
#[derive(Clone, Debug)]
pub struct BackupCommands {
    service: BackupService,
}

impl BackupCommands {
    #[must_use]
    pub fn open(store: Store) -> Self {
        Self {
            service: BackupService::open(store),
        }
    }

    #[must_use]
    pub fn service(&self) -> &BackupService {
        &self.service
    }

    pub fn export(
        &self,
        dest: &Path,
        include_cache: bool,
        now_ms: i64,
    ) -> Result<PackageOverview, BackupError> {
        self.service.export(dest, include_cache, now_ms)
    }

    pub fn backup_manual(&self, now_ms: i64) -> Result<PathBuf, BackupError> {
        self.service.backup_manual(now_ms)
    }

    pub fn backup_on_launch(
        &self,
        today: CivilDate,
        now_ms: i64,
    ) -> Result<LaunchBackup, BackupError> {
        self.service.backup_on_launch(today, now_ms)
    }

    pub fn inspect(&self, package: &Path) -> Result<PackageOverview, BackupError> {
        self.service.inspect(package)
    }

    pub fn import(&self, package: &Path, now_ms: i64) -> Result<PackageOverview, BackupError> {
        self.service.import(package, now_ms)
    }

    /// 把导入恢复注册到 [`crate::storage::Store::boot`]。
    pub fn register_boot_hooks(hooks: &mut BootHooks) {
        BackupService::register_boot_hooks(hooks);
    }
}
