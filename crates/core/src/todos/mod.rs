//! 待办服务。
//!
//! 清单、条目、收件箱、周期、回收站、当前标记和处理模式都在这里。
//! 界面调用 [`command::TodoCommands`]。搜索索引在 [`TodoCommands::boot`] 成功之后
//! 再读 [`TodoCommands::index_snapshot`]，这样只看得到加载修复之后的数据。
//!
//! 每月重复的 31 日在没有 31 日的月份落到该月最后一天。在重复截止日当天完成不再生成下一次。
//! 永久删除只作用于回收站中的条目。进入回收站满 30 天后，[`command::TodoCommands::purge_expired`]
//! 以及启动加载会清除它们。删除前把 id 写入 [`PURGE_PENDING_FILE`]。
//! 原清单已删除时，恢复写入系统收件箱。
//! 每月重复的日不是 31、目标月却没有这一天时，仍返回 [`TodoError::PendingSpec`] 且不落盘。

mod command;
mod error;
mod model;
mod purge_record;
mod repair;
mod schedule;
mod service;

pub use command::TodoCommands;
pub use error::{PendingTopic, TodoError};
pub use model::{
    ClockTime, ListKind, NewTodo, Recurrence, RecurrenceRule, SourceKind, TodoItem, TodoList,
    TodoSource, is_http_source_url,
};
pub use purge_record::PURGE_PENDING_FILE;
pub use schedule::{
    DEFAULT_DEFER_DAYS, DeferError, MAX_DEFER_DAYS, MIN_DEFER_DAYS, NextOccurrence,
    ReminderInstant, TRASH_RETENTION_MS, deferred_due, is_overdue, next_occurrence, next_reminder,
    overdue_count, trash_expired,
};
pub use service::{StoredTodo, TodoIndexEntry, TodoNotice};
