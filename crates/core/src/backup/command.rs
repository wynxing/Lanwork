//! 薄命令。规则在 [`super::service`]。

use std::path::{Path, PathBuf};

use super::error::BackupError;
use super::package::PackageOverview;
use super::service::{BackupService, LaunchBackup, ListedBackup};
use crate::capture::CivilDate;
use crate::storage::{BootHooks, Store};

/// 设置页的备份、导出和导入调用这些命令。
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

    pub fn export(&self, dest: &Path, now_ms: i64) -> Result<PackageOverview, BackupError> {
        self.service.export(dest, now_ms)
    }

    pub fn backup_manual(&self, now_ms: i64) -> Result<PathBuf, BackupError> {
        self.service.backup_manual(now_ms)
    }

    /// 自动备份和手动备份，从新到旧。不含 `import-`。合计不超过 7 份。
    pub fn list_retained(&self) -> Result<Vec<ListedBackup>, BackupError> {
        self.service.list_retained()
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

    /// 确认后的导入。结束后重载已打开的服务内存，再发布变更。
    pub fn import(&self, package: &Path, now_ms: i64) -> Result<PackageOverview, BackupError> {
        self.service.import(package, now_ms)
    }

    /// 把导入恢复注册到 [`crate::storage::Store::boot`]。
    pub fn register_boot_hooks(hooks: &mut BootHooks) {
        BackupService::register_boot_hooks(hooks);
    }
}
