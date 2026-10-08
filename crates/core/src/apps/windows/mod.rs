//! Windows 上的应用来源。COM 只在枚举线程初始化，不创建窗口。
//!
//! 便携应用和别名（规格缺口 #9 第 1 项）没有产品规则，这里没有这个来源。

mod app_paths;
mod comutil;
pub(crate) mod launch;
mod path_env;
mod shortcut;
mod start_menu;
mod store;
pub(crate) mod watch;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use super::index::SourceEnumerator;
use super::model::{AppEntry, AppSource};

pub(crate) struct WindowsSources {
    extra: Option<PathBuf>,
    user: Vec<AppEntry>,
    common: Vec<AppEntry>,
    extra_entries: Vec<AppEntry>,
}

impl WindowsSources {
    pub(crate) fn new(extra: Option<PathBuf>) -> Self {
        Self {
            extra,
            user: Vec::new(),
            common: Vec::new(),
            extra_entries: Vec::new(),
        }
    }

    fn read_start_menu(&mut self) -> Result<Vec<AppEntry>, String> {
        let mut errors = Vec::new();
        match start_menu::read_user() {
            Ok(entries) => self.user = entries,
            Err(message) => errors.push(message),
        }
        match start_menu::read_common() {
            Ok(entries) => self.common = entries,
            Err(message) => errors.push(message),
        }
        if let Some(extra) = &self.extra {
            match start_menu::read_shortcut_dir(extra) {
                Ok(entries) => self.extra_entries = entries,
                Err(message) => errors.push(message),
            }
        }
        let configured = 2 + usize::from(self.extra.is_some());
        if errors.len() == configured {
            return Err(errors.join("; "));
        }
        let mut entries = Vec::new();
        entries.extend(self.user.iter().cloned());
        entries.extend(self.common.iter().cloned());
        entries.extend(self.extra_entries.iter().cloned());
        Ok(entries)
    }
}

impl SourceEnumerator for WindowsSources {
    fn enumerate(&mut self, source: AppSource) -> Result<Vec<AppEntry>, String> {
        comutil::ensure_com()?;
        match source {
            AppSource::StartMenu => self.read_start_menu(),
            AppSource::AppPaths => app_paths::read_app_paths(),
            AppSource::Path => path_env::read_path_entries(),
            AppSource::Store => store::read_store(),
        }
    }

    fn watch_directories(&self) -> Vec<PathBuf> {
        let _ = comutil::ensure_com();
        start_menu::watch_directories(self.extra.as_deref())
    }
}
