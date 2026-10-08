//! 启动顺序：导入恢复、加载、加载修复，之后才允许建索引和处理通知点击。
//!
//! `movedAt`、`currentSince` 的修复规则不在这里，由待办服务注册。
//! 导入包的解压和备份格式不在这里，由备份与导入注册恢复钩子。

use std::path::PathBuf;

use super::Store;

/// `import.pending` 里记录的备份路径。文件内容就是该路径的 UTF-8 文本，不是 JSON。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPending {
    pub backup_path: PathBuf,
}

/// 启动成功后的结果。`import_recovered` 为真时，界面显示「导入未完成」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootReport {
    pub import_recovered: bool,
}

/// 业务数据加载之后、建索引之前运行的修复。
pub trait LoadRepair: Send {
    fn name(&self) -> &str;
    fn repair(&self, store: &Store) -> Result<(), String>;
}

/// 用函数注册一条加载修复。
pub struct FnRepair<F> {
    name: &'static str,
    func: F,
}

impl<F> FnRepair<F> {
    pub fn new(name: &'static str, func: F) -> Self {
        Self { name, func }
    }
}

impl<F> LoadRepair for FnRepair<F>
where
    F: for<'a> Fn(&'a Store) -> Result<(), String> + Send,
{
    fn name(&self) -> &str {
        self.name
    }

    fn repair(&self, store: &Store) -> Result<(), String> {
        (self.func)(store)
    }
}

/// 启动时发现 `import.pending` 后调用。成功时必须删掉该文件。
pub type ImportRecover = Box<dyn FnOnce(&Store, &ImportPending) -> Result<(), String> + Send>;

/// 导入恢复之后、加载修复之前加载业务数据。
pub type DataLoad = Box<dyn FnOnce(&Store) -> Result<(), String> + Send>;

/// 一次启动要调用的钩子。本层只保证顺序，不提供默认文档。
#[derive(Default)]
pub struct BootHooks {
    pub recover_import: Option<ImportRecover>,
    pub load: Option<DataLoad>,
    pub repairs: Vec<Box<dyn LoadRepair>>,
}

impl BootHooks {
    pub fn new() -> Self {
        Self::default()
    }
}
