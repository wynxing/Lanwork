//! 待办薄命令。
//!
//! 界面只调用这里。命令校验输入并返回明确错误，规则和写盘在 [`super::service`]。
//! 不打开窗口，也不调用 Win32。`open_source` 只返回允许的 URL，由外壳用系统浏览器打开。
//!
//! 处理模式不另存数据。只有命令返回成功，界面才把光标移到
//! [`TodoCommands::next_inbox_incomplete_after`]；失败时停在当前条。
//! 顺延天数由调用方传入设置值，缺省用 [`super::schedule::DEFAULT_DEFER_DAYS`]。本服务不读配置文件。

use std::sync::Arc;

use crate::CivilDate;
use crate::storage::{BootHooks, BootReport, FnRepair, Store};

use super::error::TodoError;
use super::model::{
    ClockTime, NewTodo, SourceKind, TodoList, TodoSource, is_http_source_url, normalize_required,
};
use super::schedule::{MAX_DEFER_DAYS, MIN_DEFER_DAYS, ReminderInstant};
use super::service::{Service, StoredTodo, TodoIndexEntry, TodoNotice};

/// 界面使用的待办命令。克隆后仍是同一份数据和同一把操作锁。
#[derive(Clone)]
pub struct TodoCommands {
    service: Arc<Service>,
}

impl std::fmt::Debug for TodoCommands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodoCommands").finish_non_exhaustive()
    }
}

impl TodoCommands {
    #[must_use]
    pub fn open(store: Store) -> Self {
        Self {
            service: Service::open(store),
        }
    }

    #[must_use]
    pub fn store(&self) -> Store {
        self.service.store()
    }

    /// 注册加载，以及 `movedAt`、`currentSince` 两条加载修复。
    ///
    /// 已有的加载钩子会先执行。修复追加在现有修复之后。
    pub fn register_boot_hooks(&self, hooks: &mut BootHooks) {
        let load_service = Arc::clone(&self.service);
        let previous = hooks.load.take();
        hooks.load = Some(Box::new(move |store| {
            if let Some(previous) = previous {
                previous(store)?;
            }
            load_service.load().map_err(|err| err.to_string())
        }));
        let moved = Arc::clone(&self.service);
        hooks
            .repairs
            .push(Box::new(FnRepair::new("movedAt", move |store: &Store| {
                let _ = store;
                moved.repair_moved().map_err(|err| err.to_string())
            })));
        let current = Arc::clone(&self.service);
        hooks.repairs.push(Box::new(FnRepair::new(
            "currentSince",
            move |store: &Store| {
                let _ = store;
                current.repair_current().map_err(|err| err.to_string())
            },
        )));
    }

    /// 注册本服务的钩子并完成存储启动。启动成功后才能调用其他命令。
    pub fn boot(&self) -> Result<BootReport, TodoError> {
        let mut hooks = BootHooks::new();
        self.register_boot_hooks(&mut hooks);
        self.service.store().boot(hooks).map_err(TodoError::from)
    }

    pub fn subscribe_notices(&self) -> std::sync::mpsc::Receiver<TodoNotice> {
        self.service.subscribe_notices()
    }

    pub fn lists(&self) -> Result<Vec<TodoList>, TodoError> {
        self.service.lists()
    }

    pub fn item(&self, id: &str) -> Result<StoredTodo, TodoError> {
        self.service.item(id)
    }

    /// 未完成且不在回收站中的条目。搜索索引在加载修复之后读这份快照。
    pub fn index_snapshot(&self) -> Result<Vec<TodoIndexEntry>, TodoError> {
        self.service.index_snapshot()
    }

    pub fn overdue_count(&self, today: CivilDate) -> Result<u32, TodoError> {
        self.service.overdue(today)
    }

    pub fn next_reminder(
        &self,
        item_id: &str,
        today: CivilDate,
        minute_of_day: u16,
    ) -> Result<Option<ReminderInstant>, TodoError> {
        if minute_of_day > 24 * 60 - 1 {
            return Err(TodoError::InvalidReminder);
        }
        self.service.reminder_for(item_id, today, minute_of_day)
    }

    /// 收件箱里 id 排在 `after_id` 之后的下一条未完成、未软删除条目。
    pub fn next_inbox_incomplete_after(&self, after_id: &str) -> Result<Option<String>, TodoError> {
        self.service.next_inbox_incomplete_after(after_id)
    }

    pub fn create_list(&self, name: &str) -> Result<String, TodoError> {
        if normalize_required(name).is_none() {
            return Err(TodoError::BlankName);
        }
        self.service.create_list(name)
    }

    pub fn rename_list(&self, id: &str, name: &str) -> Result<(), TodoError> {
        if normalize_required(name).is_none() {
            return Err(TodoError::BlankName);
        }
        self.service.rename_list(id, name)
    }

    pub fn delete_list(&self, id: &str) -> Result<(), TodoError> {
        self.service.delete_list(id)
    }

    pub fn reorder_lists(&self, ids: &[String]) -> Result<(), TodoError> {
        self.service.reorder_lists(ids)
    }

    /// 第一次需要收件箱时创建，之后返回已有的那份。
    pub fn ensure_inbox(&self) -> Result<String, TodoError> {
        self.service.ensure_inbox()
    }

    pub fn create_item(&self, list_id: &str, draft: NewTodo) -> Result<String, TodoError> {
        if normalize_required(&draft.title).is_none() {
            return Err(TodoError::BlankTitle);
        }
        if let Some(source) = &draft.source
            && !is_http_source_url(&source.url)
        {
            return Err(TodoError::RejectedSource);
        }
        self.service.create_item(list_id, draft)
    }

    pub fn rename_item(&self, item_id: &str, title: &str) -> Result<(), TodoError> {
        if normalize_required(title).is_none() {
            return Err(TodoError::BlankTitle);
        }
        self.service.rename_item(item_id, title)
    }

    pub fn reorder_items(&self, list_id: &str, ids: &[String]) -> Result<(), TodoError> {
        self.service.reorder_items(list_id, ids)
    }

    pub fn set_due(&self, item_id: &str, due: Option<CivilDate>) -> Result<(), TodoError> {
        self.service.set_due(item_id, due)
    }

    pub fn set_reminder(&self, item_id: &str, hour: u8, minute: u8) -> Result<(), TodoError> {
        let time = ClockTime::try_new(hour, minute).ok_or(TodoError::InvalidReminder)?;
        self.service.set_reminder(item_id, Some(time))
    }

    pub fn clear_reminder(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.set_reminder(item_id, None)
    }

    pub fn set_recurrence(
        &self,
        item_id: &str,
        recurrence: Option<super::model::Recurrence>,
    ) -> Result<(), TodoError> {
        self.service.set_recurrence(item_id, recurrence)
    }

    pub fn set_source(
        &self,
        item_id: &str,
        kind: SourceKind,
        url: &str,
        repo: &str,
        number: u64,
    ) -> Result<(), TodoError> {
        let source = TodoSource::try_new(kind, url, repo, number)?;
        self.service.set_source(item_id, source)
    }

    pub fn clear_source(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.clear_source(item_id)
    }

    /// 仓库字符串按原文比较。已完成和回收站里的条目也断开，条目本身保留。
    pub fn clear_sources_for_repo(&self, repo: &str) -> Result<(), TodoError> {
        self.service.clear_sources_for_repo(repo)
    }

    pub fn complete_item(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.complete_item(item_id)
    }

    pub fn soft_delete(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.soft_delete(item_id)
    }

    pub fn restore(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.restore(item_id)
    }

    /// 永久删除回收站中的一条。不在回收站时不改数据，也不发出 [`TodoNotice::Purged`]。
    pub fn purge(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.purge(item_id)
    }

    /// 清除 `now_ms` 看来已经在回收站满 30 天的条目。启动加载也会用当时的时间做一次。
    pub fn purge_expired(&self, now_ms: i64) -> Result<Vec<String>, TodoError> {
        self.service.purge_expired(now_ms)
    }

    /// 清除记录里尚未确认的 id。收纳用来补解除，不给界面调用。
    pub(crate) fn pending_purges(&self) -> Result<Vec<String>, TodoError> {
        self.service.pending_purges()
    }

    /// 这些 id 的收纳关联已经解除。从清除记录去掉。
    pub(crate) fn ack_purges(&self, ids: &[String]) -> Result<(), TodoError> {
        self.service.ack_purges(ids)
    }

    /// 返回 `http` / `https` 来源 URL。其他协议拒绝。
    pub fn open_source(&self, item_id: &str) -> Result<String, TodoError> {
        let url = self.service.source_url(item_id)?;
        if !is_http_source_url(&url) {
            return Err(TodoError::RejectedSource);
        }
        Ok(url)
    }

    pub fn set_current(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.set_current(item_id)
    }

    pub fn clear_current(&self) -> Result<(), TodoError> {
        self.service.clear_current()
    }

    pub fn process_today(&self, item_id: &str, today: CivilDate) -> Result<(), TodoError> {
        self.service.process_today(item_id, today)
    }

    pub fn process_defer(
        &self,
        item_id: &str,
        days: u32,
        today: CivilDate,
    ) -> Result<(), TodoError> {
        if !(MIN_DEFER_DAYS..=MAX_DEFER_DAYS).contains(&days) {
            return Err(TodoError::InvalidDefer);
        }
        self.service.process_defer(item_id, days, today)
    }

    pub fn process_move(&self, item_id: &str, target_list_id: &str) -> Result<(), TodoError> {
        self.service.process_move(item_id, target_list_id)
    }

    pub fn process_complete(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.process_complete(item_id)
    }

    pub fn process_soft_delete(&self, item_id: &str) -> Result<(), TodoError> {
        self.service.process_soft_delete(item_id)
    }
}
