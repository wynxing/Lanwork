//! 配置命令的错误。
//!
//! [`Display`] 只包含状态和范围。不包含文件内容、环境变量或修复步骤。

use std::fmt;

use crate::storage::Error as StoreError;
use crate::storage::SCHEMA_VERSION;

/// 配置读写和校验失败。
#[derive(Debug)]
pub enum ConfigError {
    /// 热键字符串无法识别。不回显原文。
    InvalidHotkey,
    /// 搜索条热键与面板热键相同。
    SameHotkey,
    /// 搜索条热键是空的。面板热键可以空，搜索条热键不行。
    EmptySearchHotkey,
    /// 数值不在产品规格写明的范围内。`message` 是可以显示的整句。
    OutOfRange { message: &'static str },
    /// 安静时段的起止格式产品规格没有写。非空值不保存。
    QuietHoursUnspecified,
    /// JSON 能解析，但 `schemaVersion` 不是本程序负责的版本。不改文件。
    UnsupportedSchema { found: u32 },
    /// 存储层错误。写盘失败时原文件还在。
    Store(StoreError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHotkey => write!(f, "热键无效"),
            Self::SameHotkey => write!(f, "搜索条热键与面板热键相同"),
            Self::EmptySearchHotkey => write!(f, "搜索条热键不能为空"),
            Self::OutOfRange { message } => write!(f, "{message}"),
            Self::QuietHoursUnspecified => write!(f, "安静时段的格式尚未确定"),
            Self::UnsupportedSchema { found } => {
                write!(f, "schemaVersion {found} 不受支持，当前为 {SCHEMA_VERSION}")
            }
            Self::Store(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StoreError> for ConfigError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}
