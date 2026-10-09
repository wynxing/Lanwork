//! `config.json`。
//!
//! 字段按架构文档「数据」列出的配置项。名字用 camelCase。
//! 新字段可选并有默认值。缺少 `schemaVersion` 时按 1 读取。
//!
//! 规格没有写默认值的项用下面的缺省，并在架构文档里标明。它们不是产品规则。
//! 安静时段只接受缺省的空值。数据目录字段只是记录，不参与目录解析。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::github::{DEFAULT_STALE_DAYS, GithubSettings};
use crate::storage::{SCHEMA_VERSION, is_supported_schema};
use crate::todos::{DEFAULT_DEFER_DAYS, MAX_DEFER_DAYS, MIN_DEFER_DAYS};

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

use super::error::ConfigError;
use super::hotkey::{Hotkey, default_search_hotkey, parse_hotkey};

/// 番茄钟工作时长的下限和上限，单位是分钟。产品规格写明 1 至 120。
pub const MIN_WORK_MINUTES: u32 = 1;
pub const MAX_WORK_MINUTES: u32 = 120;
pub const DEFAULT_WORK_MINUTES: u32 = 25;

/// 休息时长和长休息时长的下限和上限，单位是分钟。产品规格写明 1 至 60。
pub const MIN_BREAK_MINUTES: u32 = 1;
pub const MAX_BREAK_MINUTES: u32 = 60;
pub const DEFAULT_BREAK_MINUTES: u32 = 5;
pub const DEFAULT_LONG_BREAK_MINUTES: u32 = 15;

const WORK_RANGE: &str = "工作时长只接受 1 至 120 分钟";
const BREAK_RANGE: &str = "休息时长只接受 1 至 60 分钟";
const LONG_BREAK_RANGE: &str = "长休息时长只接受 1 至 60 分钟";
const SNOOZE_RANGE: &str = "处理模式顺延天数只接受 1 至 30";

fn default_stale_days() -> u32 {
    DEFAULT_STALE_DAYS
}

fn default_snooze_days() -> u32 {
    DEFAULT_DEFER_DAYS
}

fn default_work_minutes() -> u32 {
    DEFAULT_WORK_MINUTES
}

fn default_break_minutes() -> u32 {
    DEFAULT_BREAK_MINUTES
}

fn default_long_break_minutes() -> u32 {
    DEFAULT_LONG_BREAK_MINUTES
}

/// 热角。默认右上。关闭表示不触发。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HotCorner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
    Off,
}

/// 主题。缺省是跟随系统。产品规格没有写缺省值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

/// 已经校验过的配置。热键字符串是规范形式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
    /// 用户写下的数据目录。空字符串表示没有另写。
    /// 启动时的目录仍按环境变量、引导文件、默认目录解析，不读这个字段。
    #[serde(rename = "dataDir", default)]
    pub data_dir: String,
    #[serde(rename = "hotCorner", default)]
    pub hot_corner: HotCorner,
    #[serde(rename = "searchHotkey", default = "default_search_hotkey")]
    pub search_hotkey: String,
    #[serde(
        rename = "panelHotkey",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub panel_hotkey: Option<String>,
    /// 产品规格没有写起止格式。只能是空。
    #[serde(rename = "quietHours", default)]
    pub quiet_hours: Option<Value>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(rename = "launchAtStartup", default)]
    pub launch_at_startup: bool,
    /// 毫秒。缺省 0。GitHub 服务不解释这个数，外壳也不为此建定时器。
    #[serde(rename = "githubRefreshIntervalMs", default)]
    pub github_refresh_interval_ms: u64,
    #[serde(rename = "staleDays", default = "default_stale_days")]
    pub stale_days: u32,
    #[serde(rename = "sourceSync", default)]
    pub source_sync: bool,
    #[serde(rename = "autoCompleteOnClose", default)]
    pub auto_complete_on_close: bool,
    #[serde(rename = "processSnoozeDays", default = "default_snooze_days")]
    pub process_snooze_days: u32,
    #[serde(rename = "pomodoroWorkMinutes", default = "default_work_minutes")]
    pub pomodoro_work_minutes: u32,
    #[serde(rename = "pomodoroBreakMinutes", default = "default_break_minutes")]
    pub pomodoro_break_minutes: u32,
    #[serde(
        rename = "pomodoroLongBreakMinutes",
        default = "default_long_break_minutes"
    )]
    pub pomodoro_long_break_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            data_dir: String::new(),
            hot_corner: HotCorner::TopRight,
            search_hotkey: default_search_hotkey(),
            panel_hotkey: None,
            quiet_hours: None,
            theme: Theme::System,
            launch_at_startup: false,
            github_refresh_interval_ms: 0,
            stale_days: DEFAULT_STALE_DAYS,
            source_sync: false,
            auto_complete_on_close: false,
            process_snooze_days: DEFAULT_DEFER_DAYS,
            pomodoro_work_minutes: DEFAULT_WORK_MINUTES,
            pomodoro_break_minutes: DEFAULT_BREAK_MINUTES,
            pomodoro_long_break_minutes: DEFAULT_LONG_BREAK_MINUTES,
        }
    }
}

impl Config {
    /// 交给 GitHub 服务的那一组值。不在这里解释刷新间隔，包括 0。
    #[must_use]
    pub fn github_settings(&self) -> GithubSettings {
        GithubSettings {
            stale_days: self.stale_days,
            auto_complete_on_close: self.auto_complete_on_close,
            source_sync: self.source_sync,
            refresh_interval_ms: self.github_refresh_interval_ms,
        }
    }

    #[must_use]
    pub fn search_hotkey_parsed(&self) -> Option<Hotkey> {
        parse_hotkey(&self.search_hotkey).ok()
    }

    #[must_use]
    pub fn panel_hotkey_parsed(&self) -> Option<Hotkey> {
        self.panel_hotkey
            .as_deref()
            .and_then(|text| parse_hotkey(text).ok())
    }
}

/// 把热键收成规范形式，并检查产品规格已经写明的范围。
///
/// 不写盘。`schemaVersion` 改成当前版本。调用方应先拒绝更高的版本。
pub fn normalize(mut config: Config) -> Result<Config, ConfigError> {
    if !is_supported_schema(config.schema_version) {
        return Err(ConfigError::UnsupportedSchema {
            found: config.schema_version,
        });
    }
    config.schema_version = SCHEMA_VERSION;
    config.data_dir = config.data_dir.trim().to_owned();
    if matches!(config.quiet_hours, Some(value) if !value.is_null()) {
        return Err(ConfigError::QuietHoursUnspecified);
    }
    config.quiet_hours = None;

    let search = search_text(&config.search_hotkey)?;
    let panel = match config.panel_hotkey.as_deref() {
        None => None,
        Some(text) if text.trim().is_empty() => return Err(ConfigError::InvalidHotkey),
        Some(text) => Some(parse_hotkey(text)?),
    };
    if panel == Some(search) {
        return Err(ConfigError::SameHotkey);
    }
    config.search_hotkey = search.canonical();
    config.panel_hotkey = panel.map(Hotkey::canonical);

    check_range(
        config.process_snooze_days,
        MIN_DEFER_DAYS,
        MAX_DEFER_DAYS,
        SNOOZE_RANGE,
    )?;
    check_range(
        config.pomodoro_work_minutes,
        MIN_WORK_MINUTES,
        MAX_WORK_MINUTES,
        WORK_RANGE,
    )?;
    check_range(
        config.pomodoro_break_minutes,
        MIN_BREAK_MINUTES,
        MAX_BREAK_MINUTES,
        BREAK_RANGE,
    )?;
    check_range(
        config.pomodoro_long_break_minutes,
        MIN_BREAK_MINUTES,
        MAX_BREAK_MINUTES,
        LONG_BREAK_RANGE,
    )?;
    Ok(config)
}

fn search_text(text: &str) -> Result<Hotkey, ConfigError> {
    if text.trim().is_empty() {
        return Err(ConfigError::EmptySearchHotkey);
    }
    parse_hotkey(text)
}

fn check_range(value: u32, min: u32, max: u32, message: &'static str) -> Result<(), ConfigError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(ConfigError::OutOfRange { message })
    }
}
