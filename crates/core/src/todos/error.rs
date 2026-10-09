//! 待办命令返回的错误。
//!
//! 文字只包含状态。不包含修复步骤，也不包含条目正文以外的说明。

use std::fmt;

use crate::storage::Error as StorageError;

/// 产品规格还没写明、因此服务拒绝猜测的点。
///
/// 每月重复的 31 日已经落到小月月末。这里只留下日不是 31、目标月却没有这一天的情况。
/// 这些分支不落盘。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTopic {
    /// 每月重复的日不是 31，而目标月没有这一天。
    MonthlyMissingDay,
}

/// 待办命令的失败。
#[derive(Debug)]
pub enum TodoError {
    Storage(StorageError),
    /// 还没完成启动加载，不能读写待办。
    NotLoaded,
    BlankTitle,
    BlankName,
    ListNotFound {
        id: String,
    },
    ItemNotFound {
        id: String,
    },
    InvalidDefer,
    RejectedSource,
    InvalidSource,
    InvalidReminder,
    InvalidOrder,
    DateOverflow,
    CannotDeleteInbox,
    ListNotEmpty,
    SameList,
    AlreadyComplete,
    AlreadyInTrash,
    NotInTrash,
    NotInboxActive,
    NoSource,
    UnsupportedSchema {
        found: u32,
    },
    IdMismatch {
        id: String,
    },
    DuplicateId {
        id: String,
    },
    PendingSpec(PendingTopic),
}

impl TodoError {
    pub fn pending_topic(&self) -> Option<PendingTopic> {
        match self {
            Self::PendingSpec(topic) => Some(*topic),
            _ => None,
        }
    }
}

impl fmt::Display for TodoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(err) => write!(f, "{err}"),
            Self::NotLoaded => write!(f, "待办尚未加载"),
            Self::BlankTitle => write!(f, "标题为空"),
            Self::BlankName => write!(f, "名称为空"),
            Self::ListNotFound { id } => write!(f, "找不到清单：{id}"),
            Self::ItemNotFound { id } => write!(f, "找不到待办：{id}"),
            Self::InvalidDefer => write!(f, "顺延天数无效"),
            Self::RejectedSource => write!(f, "来源协议被拒绝"),
            Self::InvalidSource => write!(f, "来源无效"),
            Self::InvalidReminder => write!(f, "提醒时间无效"),
            Self::InvalidOrder => write!(f, "排序无效"),
            Self::DateOverflow => write!(f, "日期超出范围"),
            Self::CannotDeleteInbox => write!(f, "不能删除收件箱"),
            Self::ListNotEmpty => write!(f, "清单里还有待办"),
            Self::SameList => write!(f, "目标清单相同"),
            Self::AlreadyComplete => write!(f, "待办已经完成"),
            Self::AlreadyInTrash => write!(f, "待办已经在回收站"),
            Self::NotInTrash => write!(f, "待办不在回收站"),
            Self::NotInboxActive => write!(f, "不是收件箱中的未完成待办"),
            Self::NoSource => write!(f, "没有来源"),
            Self::UnsupportedSchema { found } => {
                write!(f, "schemaVersion {found} 不受支持")
            }
            Self::IdMismatch { id } => write!(f, "清单标识与文件名不一致：{id}"),
            Self::DuplicateId { id } => write!(f, "标识重复：{id}"),
            Self::PendingSpec(PendingTopic::MonthlyMissingDay) => {
                write!(f, "每月这一天在目标月不存在，尚未规定落到哪一天")
            }
        }
    }
}

impl std::error::Error for TodoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StorageError> for TodoError {
    fn from(err: StorageError) -> Self {
        Self::Storage(err)
    }
}
