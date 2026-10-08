use std::path::PathBuf;

/// 可以显示给触发该动作的界面的错误。
///
/// [`Display`] 只包含状态和路径，不包含文件内容、修复步骤或环境变量的值。
#[derive(Debug)]
pub enum Error {
    Io {
        path: PathBuf,
        action: IoAction,
        source: std::io::Error,
    },
    /// 编码失败。不携带待写入的正文。
    Encode,
    Quarantined {
        path: PathBuf,
        quarantine: Option<PathBuf>,
    },
    InvalidId {
        id: String,
    },
    MayDolist {
        path: PathBuf,
    },
    Bootstrap {
        path: PathBuf,
        message: &'static str,
    },
    MissingEnv {
        name: &'static str,
    },
    MissingSchema,
    InvalidSchema,
    UnsupportedSchema {
        found: u32,
        supported: u32,
    },
    Startup(StartupError),
    /// 启动未完成，不能建索引、处理通知点击，也不能在失败后继续写入。
    NotReady,
    ImportPending {
        message: &'static str,
    },
    Batch {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoAction {
    Read,
    Write,
    Replace,
    CreateDir,
    Remove,
    Rename,
}

#[derive(Debug)]
pub enum StartupError {
    ImportRecoveryMissing { backup: PathBuf },
    ImportRecovery { message: String },
    ImportStillPending,
    PendingInvalid,
    Load { message: String },
    Repair { name: String, message: String },
}

impl Error {
    pub(crate) fn io(action: IoAction, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            action,
            source,
        }
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Io { path, .. }
            | Self::Quarantined { path, .. }
            | Self::MayDolist { path }
            | Self::Bootstrap { path, .. } => Some(path),
            Self::Startup(StartupError::ImportRecoveryMissing { backup }) => Some(backup),
            _ => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, action, .. } => write!(f, "{}：{}", action.label(), path.display()),
            Self::Encode => write!(f, "无法编码 JSON"),
            Self::Quarantined { path, .. } => write!(f, "文件已损坏：{}", path.display()),
            Self::InvalidId { id } => write!(f, "标识无效：{id}"),
            Self::MayDolist { path } => write!(f, "不能使用该数据目录：{}", path.display()),
            Self::Bootstrap { path, message } => {
                write!(f, "引导文件错误：{message}：{}", path.display())
            }
            Self::MissingEnv { name } => write!(f, "缺少环境变量：{name}"),
            Self::MissingSchema => write!(f, "缺少 schemaVersion"),
            Self::InvalidSchema => write!(f, "schemaVersion 无效"),
            Self::UnsupportedSchema { found, supported } => {
                write!(f, "schemaVersion {found} 不受支持，当前为 {supported}")
            }
            Self::Startup(err) => write!(f, "{err}"),
            Self::NotReady => write!(f, "启动未完成"),
            Self::ImportPending { message } => write!(f, "{message}"),
            Self::Batch { message } => write!(f, "{message}"),
        }
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ImportRecoveryMissing { backup } => {
                write!(f, "导入未完成，无法恢复：{}", backup.display())
            }
            Self::ImportRecovery { message } => write!(f, "导入未完成：{message}"),
            Self::ImportStillPending => write!(f, "导入未完成"),
            Self::PendingInvalid => write!(f, "导入未完成"),
            Self::Load { message } => write!(f, "加载失败：{message}"),
            Self::Repair { name, message } => write!(f, "加载修复失败：{name}：{message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Startup(err) => Some(err),
            _ => None,
        }
    }
}

impl std::error::Error for StartupError {}

impl IoAction {
    fn label(self) -> &'static str {
        match self {
            Self::Read => "读取失败",
            Self::Write | Self::Replace => "写入失败",
            Self::CreateDir => "无法创建目录",
            Self::Remove => "删除失败",
            Self::Rename => "重命名失败",
        }
    }
}
