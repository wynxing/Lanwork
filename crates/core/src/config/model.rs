//! `config.json`。
//!
//! 字段按架构文档「数据」列出的配置项。名字用 camelCase。
//! 新字段可选并有默认值。缺少 `schemaVersion` 时按 1 读取。
//!
//! 规格没有写默认值的项用下面的缺省，并在架构文档里标明。它们不是产品规则。
//! 安静时段见 [`super::quiet`]。数据目录字段只是记录，不参与目录解析。

use serde::{Deserialize, Serialize};

use super::quiet::QuietHours;
use crate::search::WebSearchEngine;
use crate::storage::{SCHEMA_VERSION, is_supported_schema};
use crate::todos::{DEFAULT_DEFER_DAYS, MAX_DEFER_DAYS, MIN_DEFER_DAYS};

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

use super::error::ConfigError;
use super::hotkey::{Hotkey, default_search_hotkey, parse_hotkey};

pub const SNOOZE_DAYS_ERROR: &str = "处理模式顺延天数只接受 1 至 30";

fn default_snooze_days() -> u32 {
    DEFAULT_DEFER_DAYS
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

/// 搜索条上次拖动结束时的左上角。
///
/// 没有这个字段表示还没记住，显示时用默认位置。`x`、`y` 怎么解释，产品规格没有写，
/// 见架构文档「搜索条」的待定项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchBarOrigin {
    pub x: i32,
    pub y: i32,
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
    /// 每天固定的一段。不写表示没有安静时段。见 [`super::quiet`]。
    #[serde(
        rename = "quietHours",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub quiet_hours: Option<QuietHours>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(rename = "launchAtStartup", default)]
    pub launch_at_startup: bool,
    #[serde(rename = "processSnoozeDays", default = "default_snooze_days")]
    pub process_snooze_days: u32,
    /// 网页搜索结果用的搜索引擎：`google`、`bing` 或 `baidu`。缺省 `google`。
    #[serde(rename = "webSearchEngine", default)]
    pub web_search_engine: WebSearchEngine,
    /// 搜索条左上角。没有这个字段时不记住，显示用默认位置。
    #[serde(
        rename = "searchBarOrigin",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub search_bar_origin: Option<SearchBarOrigin>,
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
            process_snooze_days: DEFAULT_DEFER_DAYS,
            web_search_engine: WebSearchEngine::Google,
            search_bar_origin: None,
        }
    }
}

impl Config {
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
    if let Some(hours) = config.quiet_hours
        && hours.start.minute_of_day() == hours.end.minute_of_day()
    {
        return Err(ConfigError::QuietHoursSame);
    }

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
        SNOOZE_DAYS_ERROR,
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
