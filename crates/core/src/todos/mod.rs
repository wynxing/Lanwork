//! 待办服务。
//!
//! 清单、条目、收件箱、周期、回收站、当前标记和处理模式都在这里。
//! 界面调用 [`command::TodoCommands`]。搜索索引在 [`TodoCommands::boot`] 成功之后
//! 再读 [`TodoCommands::index_snapshot`]，这样只看得到加载修复之后的数据。
//!
//! 以下行为等 #9 写进产品规格，调用会返回 [`TodoError::PendingSpec`] 且不落盘：
//! 每月重复时目标月没有这一天、在重复截止日当天完成是否再生成、永久删除、
//! 恢复时原清单已经不存在。

mod command;
mod error;
mod model;
mod repair;
mod schedule;
mod service;

pub use command::TodoCommands;
pub use error::{PendingTopic, TodoError};
pub use model::{
    ClockTime, ListKind, NewTodo, Recurrence, RecurrenceRule, SourceKind, TodoItem, TodoList,
    TodoSource, is_http_source_url,
};
pub use schedule::{
    DEFAULT_DEFER_DAYS, DeferError, MAX_DEFER_DAYS, MIN_DEFER_DAYS, NextOccurrence,
    ReminderInstant, deferred_due, is_overdue, next_occurrence, next_reminder, overdue_count,
};
pub use service::{StoredTodo, TodoIndexEntry, TodoNotice};
