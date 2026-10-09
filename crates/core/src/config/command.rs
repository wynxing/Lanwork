//! 薄命令。校验在 [`super::model::normalize`]，写盘在 [`super::ConfigService`]。
//!
//! 运行中修改热键只经过 [`crate::shell::save_hotkeys`]：先注册，成功后再 [`ConfigCommands::replace`]，然后卸掉旧热键。
//! 注册失败时不写盘。[`ConfigCommands::replace`] 在发布变更之前更新内存。

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

    /// 校验后整份写入。运行中改热键不从这里开始，先注册的入口是 [`crate::shell::save_hotkeys`]。
    pub fn replace(&self, next: Config) -> Result<Config, ConfigError> {
        self.service.replace(next)
    }

    #[must_use]
    pub fn github_settings(&self) -> GithubSettings {
        self.current().github_settings()
    }
}
