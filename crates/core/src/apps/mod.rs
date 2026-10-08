//! 应用索引：开始菜单、App Paths、PATH、商店应用、缓存和启动。
//!
//! 架构把应用枚举放在 `crates/core`，同时要求这里不依赖 Slint，也不依赖 Win32 窗口 API。
//! 枚举用到的是快捷方式、注册表、`shell:AppsFolder`、目录变更和 `ShellExecuteExW`，
//! 不创建窗口，所以留在本模块，不放进 `crates/app`。界面以后只调用这里。
//!
//! 规格缺口 #9 第 1 项（便携应用和别名的入口与存储）和第 2 项（手动刷新的界面入口）
//! 还没有写进 product.md。这两项没有实现。`AppIndex::refresh` 只是进程内的重建函数。

mod cache;
mod index;
mod model;

#[cfg(windows)]
mod windows;

pub use cache::{CACHE_FILE_NAME, CACHE_SCHEMA_VERSION, CacheStatus};
pub use index::{AppHit, AppIndex, IndexError, RefreshReport};
pub use model::{AppEntry, AppSource, LaunchTarget, SourceError, launch_key};

#[cfg(test)]
pub(crate) use index::{OpenOptions, open_with};

use std::time::Duration;

/// 测量用快捷方式目录。正式构建读取它，界面不暴露。
pub const EXTRA_SHORTCUT_DIR_ENV: &str = "LANWORK_EXTRA_SHORTCUT_DIR";

/// 开始菜单目录的变化在这段安静时间后合并成一次重建，另有 2 秒上限。
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(400);

#[cfg(windows)]
pub use windows::launch::launch;

#[cfg(all(windows, test))]
pub(crate) use windows::launch::launch_and_wait;

#[cfg(not(windows))]
#[derive(Debug)]
pub struct LaunchError {
    pub message: String,
}

#[cfg(not(windows))]
pub fn launch(_target: &LaunchTarget) -> Result<(), LaunchError> {
    Err(LaunchError {
        message: "应用启动只在 Windows 上可用".to_owned(),
    })
}

#[cfg(windows)]
pub use windows::launch::LaunchError;
