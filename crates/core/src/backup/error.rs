//! 备份与导入的错误。文字不含文件正文和环境变量的值。

use std::path::PathBuf;

use crate::storage;

/// 可以交给设置页的备份、导出或导入错误。
#[derive(Debug)]
pub enum BackupError {
    Storage(storage::Error),
    /// 不针对某一个包内路径。
    Rejected(&'static str),
    /// 包内路径可以显示。正文不进入 `message`。
    Entry {
        name: String,
        message: &'static str,
    },
    Io {
        path: PathBuf,
        action: &'static str,
    },
}

impl BackupError {
    pub(crate) fn entry(name: &str, message: &'static str) -> Self {
        let mut shown: String = name.chars().take(200).collect();
        if name.chars().count() > 200 {
            shown.push('…');
        }
        Self::Entry {
            name: shown,
            message,
        }
    }

    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>) -> Self {
        Self::Io {
            path: path.into(),
            action,
        }
    }
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(err) => write!(f, "{err}"),
            Self::Rejected(message) => write!(f, "{message}"),
            Self::Entry { name, message } => write!(f, "{message}：{name}"),
            Self::Io { path, action } => write!(f, "{action}：{}", path.display()),
        }
    }
}

impl std::error::Error for BackupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(err) => Some(err),
            _ => None,
        }
    }
}

impl From<storage::Error> for BackupError {
    fn from(err: storage::Error) -> Self {
        Self::Storage(err)
    }
}
