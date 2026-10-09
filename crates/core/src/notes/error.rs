//! 便签服务的错误。
//!
//! [`Display`] 只包含状态和标识，不包含正文，因此可以进入日志。

use std::fmt;

use crate::storage::Error as StoreError;

/// 便签命令和服务返回的错误。
///
/// 写盘失败与 [`Self::Conflict`] 是两种错误。两种情况下调用方手里的正文都还在，
/// 服务也不会改掉上一次成功保存的内容。
#[derive(Debug)]
pub enum NoteError {
    /// id 不是单个合法的文件名分量。
    InvalidId { id: String },
    /// 数据目录里没有这篇便签。
    NotFound { id: String },
    /// 调用方带来的 `revision` 与磁盘不一致。服务不覆盖。
    ///
    /// 冲突之后由用户选择保留哪一版。服务不覆盖。关闭和退出时的未保存正文由界面处理。
    Conflict {
        id: String,
        expected: u64,
        actual: u64,
    },
    /// 已经软删除，再次软删除不会写盘。
    AlreadyDeleted { id: String },
    /// 没有软删除，不能恢复，也不能永久删除。
    NotDeleted { id: String },
    /// 文件能解析，但 `schemaVersion` 不是本程序负责的版本。服务不改这个文件。
    UnsupportedSchema { id: String, found: u32 },
    /// `revision` 不能再加一。
    RevisionOverflow { id: String },
    /// 多次尝试后仍无法分配不冲突的 id。
    AllocateId,
    /// 存储层错误。写盘失败时原文件还在，显示文本里带路径。
    Store(StoreError),
}

impl fmt::Display for NoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId { id } => write!(f, "便签标识无效：{id}"),
            Self::NotFound { id } => write!(f, "找不到便签：{id}"),
            Self::Conflict { id, .. } => write!(f, "便签保存冲突：{id}"),
            Self::AlreadyDeleted { id } => write!(f, "便签已在回收站：{id}"),
            Self::NotDeleted { id } => write!(f, "便签不在回收站：{id}"),
            Self::UnsupportedSchema { id, found } => {
                write!(f, "便签 schemaVersion {found} 不受支持：{id}")
            }
            Self::RevisionOverflow { id } => write!(f, "便签修订号已满：{id}"),
            Self::AllocateId => write!(f, "无法分配便签标识"),
            Self::Store(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for NoteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StoreError> for NoteError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::InvalidId { id } => Self::InvalidId { id },
            other => Self::Store(other),
        }
    }
}
