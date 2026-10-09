//! 薄命令。校验在 [`super::model::normalize`]，写盘在 [`super::ConfigService`]。
//!
//! 热键是否能在系统里注册由外壳做。注册失败时外壳不调用 [`ConfigCommands::replace`]。
//! 顺序在 [`crate::shell::save_hotkeys`]：先注册新热键，成功后再写入，然后卸掉旧热键。

use super::error::ConfigError;
use super::model::Config;
use super::service::{ConfigService, Fallback};
use crate::github::GithubSettings;
use crate::storage::Store;

/// 界面和外壳调用的配置命令。
#[derive(Clone, Debug)]
pub struct ConfigCommands {
    service: ConfigService,
}

impl ConfigCommands {
    pub fn open(store: Store) -> Result<Self, ConfigError> {
        Ok(Self {
            service: ConfigService::open(store)?,
        })
    }

    #[must_use]
    pub fn service(&self) -> &ConfigService {
        &self.service
    }

    #[must_use]
    pub fn current(&self) -> Config {
        self.service.current()
    }

    #[must_use]
    pub fn fallback(&self) -> Option<Fallback> {
        self.service.fallback()
    }

    pub fn replace(&self, next: Config) -> Result<Config, ConfigError> {
        self.service.replace(next)
    }

    /// 只改两个热键。其他字段保持当前值。失败不写盘。
    pub fn set_hotkeys(&self, search: &str, panel: Option<&str>) -> Result<Config, ConfigError> {
        let mut next = self.current();
        next.search_hotkey = search.to_owned();
        next.panel_hotkey = panel.map(str::to_owned);
        self.replace(next)
    }

    #[must_use]
    pub fn github_settings(&self) -> GithubSettings {
        self.current().github_settings()
    }
}
