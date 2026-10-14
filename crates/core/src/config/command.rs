//! 薄命令。校验在 [`super::model::normalize`]，写盘在 [`super::ConfigService`]。
//!
//! 运行中修改热键只经过 [`crate::shell::save_hotkeys`]：先注册，成功后再 [`ConfigCommands::replace`]，然后卸掉旧热键。
//! 注册失败时不写盘。[`ConfigCommands::replace`] 在发布变更之前更新内存。

use super::error::ConfigError;
use super::model::{Config, HotCorner, Theme};
use super::quiet::QuietHours;
use super::service::{ConfigService, Fallback};
use crate::search::WebSearchEngine;
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

    /// 只改开机启动。值没变时不写盘。写盘失败时内存和 `config.json` 都保持原样。
    ///
    /// 注册表的 Run 项不在这里写：调用方在这次返回成功之后按 [`crate::shell::run_value_action`] 处理。
    pub fn set_launch_at_startup(&self, enabled: bool) -> Result<Config, ConfigError> {
        self.change(|config| config.launch_at_startup = enabled)
    }

    /// 只改热角。值没变时不写盘。写盘失败时内存和 `config.json` 都保持原样。
    ///
    /// 让轮询立即换角是调用方在这次返回成功之后的事。
    pub fn set_hot_corner(&self, corner: HotCorner) -> Result<Config, ConfigError> {
        self.change(|config| config.hot_corner = corner)
    }

    /// 只改主题。值没变时不写盘。界面换色是调用方在这次返回成功之后的事。
    pub fn set_theme(&self, theme: Theme) -> Result<Config, ConfigError> {
        self.change(|config| config.theme = theme)
    }

    /// 只改网页搜索引擎。值没变时不写盘。搜索条换名称是调用方在这次返回成功之后的事。
    pub fn set_web_search_engine(&self, engine: WebSearchEngine) -> Result<Config, ConfigError> {
        self.change(|config| config.web_search_engine = engine)
    }

    /// 只改安静时段。`None` 表示关掉。值没变时不写盘。
    pub fn set_quiet_hours(&self, hours: Option<QuietHours>) -> Result<Config, ConfigError> {
        self.change(|config| config.quiet_hours = hours)
    }

    /// 只改处理模式「稍后」天数。超出 1 至 30 时不写盘。
    pub fn set_process_snooze_days(&self, days: u32) -> Result<Config, ConfigError> {
        self.change(|config| config.process_snooze_days = days)
    }

    fn change(&self, edit: impl FnOnce(&mut Config)) -> Result<Config, ConfigError> {
        let current = self.service.current();
        let mut next = current.clone();
        edit(&mut next);
        if next == current {
            return Ok(current);
        }
        self.service.replace(next)
    }
}
