//! 收纳服务的错误。
//!
//! [`Display`] 只包含状态、标识和路径，不包含文件内容。

use std::fmt;

use crate::storage::Error as StoreError;

/// 收纳命令和服务返回的错误。
///
/// 写盘失败时内存和磁盘都保持操作前状态，也不发布变更消息。
#[derive(Debug)]
pub enum ShelfError {
    /// 分组 id 不是单个合法的文件名分量。
    InvalidId { id: String },
    /// 待办 id 不是单个合法的文件名分量。不检查该待办是否存在。
    InvalidTodoId { id: String },
    /// 名称去掉首尾空白后为空。
    BlankName,
    /// 数据目录里没有这个分组。
    NotFound { id: String },
    /// 文件能解析，但 `schemaVersion` 不是本程序负责的版本。服务不改这个文件。
    UnsupportedSchema { id: String, found: u32 },
    /// 不是本服务接受的绝对路径。
    InvalidPath { path: String },
    /// 排序没有覆盖全部分组，或含有未知 id。
    InvalidOrder,
    /// 多次尝试后仍无法分配不冲突的 id。
    AllocateId,
    /// 已有分组，但这次拖入没有给出目标。服务不猜测该写入哪一组。
    TargetGroupRequired,
    /// 分组已经关联了另一条待办。先解除再关联。
    AlreadyLinked { id: String, todo_id: String },
    /// 分组里没有这条路径。
    RefNotFound { id: String, path: String },
    /// 回滚没有写回去，重新读入也失败。内存已清空，避免继续用和磁盘不一致的分组。
    Unreadable,
    /// 存储层错误。写盘失败时原文件还在，显示文本里带路径。
    Store(StoreError),
}

impl fmt::Display for ShelfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId { id } => write!(f, "收纳标识无效：{id}"),
            Self::InvalidTodoId { id } => write!(f, "待办标识无效：{id}"),
            Self::BlankName => write!(f, "名称为空"),
            Self::NotFound { id } => write!(f, "找不到收纳分组：{id}"),
            Self::UnsupportedSchema { id, found } => {
                write!(f, "收纳 schemaVersion {found} 不受支持：{id}")
            }
            Self::InvalidPath { path } => write!(f, "路径无效：{path}"),
            Self::InvalidOrder => write!(f, "排序无效"),
            Self::AllocateId => write!(f, "无法分配收纳标识"),
            Self::TargetGroupRequired => write!(f, "没有指定收纳分组"),
            Self::AlreadyLinked { id, todo_id } => {
                write!(f, "收纳分组已关联待办：{id}：{todo_id}")
            }
            Self::RefNotFound { id, path } => write!(f, "找不到引用：{id}：{path}"),
            Self::Unreadable => write!(f, "收纳数据无法读回"),
            Self::Store(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ShelfError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<StoreError> for ShelfError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::InvalidId { id } => Self::InvalidId { id },
            other => Self::Store(other),
        }
    }
}
