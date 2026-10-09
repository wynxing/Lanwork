//! 待办规则和写盘。
//!
//! 内存只在对应写入成功后换成新状态。跨清单移动和切换当前标记按架构的顺序写：
//! 先写目标（带新的 `movedAt` 或 `currentSince`），再写源。两次都成功后才提交批次，
//! 变更消息在提交时发出。后面的写入失败时，把本批次已经替换的文件写回内存里的原内容，
//! 不提交、不发布 `EntityChanged`，内存保持操作前状态。回滚写入再失败时，重新读入并按
//! `movedAt`、`currentSince` 修好，避免留下重复 id 或两条当前标记；重新读入失败则返回该错误。
//!
//! 同一清单里的当前标记切换也分两次写入同一文件，这样中断落在清除旧标记之前时，
//! 加载修复仍能留下 `currentSince` 更新的那条。
//!
//! 命令在 `Store::is_ready` 之后才执行。加载修复持有同一把操作锁，启动过程中不穿插命令。
//! 变更订阅回调里不要再调用本服务，否则会和操作锁死锁。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::CivilDate;
use crate::storage::{CollectionKind, DocumentId, Store, is_supported_schema, lock_mutex};

use super::error::{PendingTopic, TodoError};
use super::model::{
    ClockTime, ListKind, NewTodo, TodoItem, TodoList, TodoSource, is_http_source_url,
    normalize_required,
};
use super::repair::{repair_current_since, repair_moved_at};
use super::schedule::{
    DeferError, NextOccurrence, ReminderInstant, deferred_due, next_occurrence, next_reminder,
    overdue_count,
};

/// 永久删除完成后给收纳服务的领域事件。不是 `EntityChanged`。
///
/// #9 第 9 项写进产品规格之前，服务不会发出这条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoNotice {
    Purged { item_id: String },
}

/// 按清单定位到的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTodo {
    pub list_id: String,
    pub list_name: String,
    pub item: TodoItem,
}

/// 搜索索引可以收录的未完成、不在回收站中的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoIndexEntry {
    pub list_id: String,
    pub item_id: String,
    pub title: String,
    pub current: bool,
    pub due: Option<CivilDate>,
    pub source: Option<TodoSource>,
}

struct State {
    loaded: bool,
    lists: Vec<TodoList>,
}

struct NoticeBus {
    senders: Mutex<Vec<Sender<TodoNotice>>>,
}

/// 测试里让批次的某一次写入或回滚失败，而不改存储层。
#[cfg(test)]
struct WriteFault {
    /// 批次里 0 起始的那一次写入在落盘前失败。
    fail_at: Option<usize>,
    fail_rollback: bool,
    before_reread: Option<Box<dyn FnOnce() + Send>>,
}

#[cfg(test)]
impl WriteFault {
    fn none() -> Self {
        Self {
            fail_at: None,
            fail_rollback: false,
            before_reread: None,
        }
    }
}

impl NoticeBus {
    fn subscribe(&self) -> Receiver<TodoNotice> {
        let (sender, receiver) = mpsc::channel();
        lock_mutex(&self.senders).push(sender);
        receiver
    }

    /// 永久删除实现后，在写入完成时调用。
    #[allow(dead_code)]
    fn publish(&self, notice: TodoNotice) {
        let mut senders = lock_mutex(&self.senders);
        senders.retain(|sender| sender.send(notice.clone()).is_ok());
    }
}

impl crate::storage::MemoryReload for Service {
    fn reload_memory(&self) -> Result<(), String> {
        let loaded = lock_mutex(&self.state).loaded;
        if !loaded {
            return Ok(());
        }
        self.load().map_err(|err| err.to_string())
    }
}

pub(crate) struct Service {
    store: Store,
    op: Mutex<()>,
    state: Mutex<State>,
    notices: NoticeBus,
    #[cfg(test)]
    fault: Mutex<WriteFault>,
}

impl Service {
    pub(crate) fn open(store: Store) -> Arc<Self> {
        let service = Arc::new(Self {
            store,
            op: Mutex::new(()),
            state: Mutex::new(State {
                loaded: false,
                lists: Vec::new(),
            }),
            notices: NoticeBus {
                senders: Mutex::new(Vec::new()),
            },
            #[cfg(test)]
            fault: Mutex::new(WriteFault::none()),
        });
        let reload: Arc<dyn crate::storage::MemoryReload> = service.clone();
        service.store.watch_memory(&reload);
        drop(reload);
        service
    }

    #[cfg(test)]
    pub(crate) fn set_write_fault(
        &self,
        fail_at: Option<usize>,
        fail_rollback: bool,
        before_reread: Option<Box<dyn FnOnce() + Send>>,
    ) {
        *lock_mutex(&self.fault) = WriteFault {
            fail_at,
            fail_rollback,
            before_reread,
        };
    }

    pub(crate) fn store(&self) -> Store {
        self.store.clone()
    }

    pub(crate) fn load(&self) -> Result<(), TodoError> {
        let _op = lock_mutex(&self.op);
        let lists = self.read_disk()?;
        self.set_lists(lists);
        Ok(())
    }

    pub(crate) fn repair_moved(&self) -> Result<(), TodoError> {
        let _op = lock_mutex(&self.op);
        let mut lists = self.read_disk()?;
        let changed = repair_moved_at(&mut lists);
        self.persist_changed(lists, &changed)
    }

    pub(crate) fn repair_current(&self) -> Result<(), TodoError> {
        let _op = lock_mutex(&self.op);
        let mut lists = self.read_disk()?;
        let changed = repair_current_since(&mut lists);
        self.persist_changed(lists, &changed)
    }

    pub(crate) fn subscribe_notices(&self) -> Receiver<TodoNotice> {
        self.notices.subscribe()
    }

    pub(crate) fn lists(&self) -> Result<Vec<TodoList>, TodoError> {
        self.ready_op(|| {
            let mut lists = self.lists_vec()?;
            sort_lists(&mut lists);
            Ok(lists)
        })
    }

    pub(crate) fn item(&self, id: &str) -> Result<StoredTodo, TodoError> {
        self.ready_op(|| {
            let lists = self.lists_vec()?;
            let (list_index, item_index) = find_item(&lists, id).ok_or_else(|| missing_item(id))?;
            Ok(StoredTodo {
                list_id: lists[list_index].id.clone(),
                list_name: lists[list_index].name.clone(),
                item: lists[list_index].items[item_index].clone(),
            })
        })
    }

    pub(crate) fn index_snapshot(&self) -> Result<Vec<TodoIndexEntry>, TodoError> {
        self.ready_op(|| {
            let mut lists = self.lists_vec()?;
            sort_lists(&mut lists);
            let mut entries = Vec::new();
            for list in lists {
                for item in list.items {
                    if item.completed || item.in_trash() {
                        continue;
                    }
                    entries.push(TodoIndexEntry {
                        list_id: list.id.clone(),
                        item_id: item.id,
                        title: item.title,
                        current: item.current,
                        due: item.due,
                        source: item.source,
                    });
                }
            }
            Ok(entries)
        })
    }

    pub(crate) fn overdue(&self, today: CivilDate) -> Result<u32, TodoError> {
        self.ready_op(|| {
            let lists = self.lists_vec()?;
            Ok(overdue_count(
                lists.iter().flat_map(|list| list.items.iter()),
                today,
            ))
        })
    }

    pub(crate) fn reminder_for(
        &self,
        item_id: &str,
        today: CivilDate,
        minute_of_day: u16,
    ) -> Result<Option<ReminderInstant>, TodoError> {
        self.ready_op(|| {
            let lists = self.lists_vec()?;
            let (list_index, item_index) =
                find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
            Ok(next_reminder(
                &lists[list_index].items[item_index],
                today,
                minute_of_day,
            ))
        })
    }

    pub(crate) fn next_inbox_incomplete_after(
        &self,
        after_id: &str,
    ) -> Result<Option<String>, TodoError> {
        self.ready_op(|| {
            let lists = self.lists_vec()?;
            let mut ids: Vec<&str> = lists
                .iter()
                .filter(|list| list.kind == ListKind::Inbox)
                .flat_map(|list| list.items.iter())
                .filter(|item| !item.completed && !item.in_trash())
                .map(|item| item.id.as_str())
                .filter(|id| *id > after_id)
                .collect();
            ids.sort_unstable();
            Ok(ids.first().map(|id| (*id).to_owned()))
        })
    }

    pub(crate) fn create_list(&self, name: &str) -> Result<String, TodoError> {
        self.ready_op(|| self.create_list_locked(name))
    }

    pub(crate) fn rename_list(&self, id: &str, name: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.rename_list_locked(id, name))
    }

    pub(crate) fn delete_list(&self, id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.delete_list_locked(id))
    }

    pub(crate) fn reorder_lists(&self, ids: &[String]) -> Result<(), TodoError> {
        self.ready_op(|| self.reorder_lists_locked(ids))
    }

    pub(crate) fn ensure_inbox(&self) -> Result<String, TodoError> {
        self.ready_op(|| self.ensure_inbox_locked())
    }

    pub(crate) fn create_item(&self, list_id: &str, draft: NewTodo) -> Result<String, TodoError> {
        self.ready_op(|| self.create_item_locked(list_id, draft))
    }

    pub(crate) fn rename_item(&self, item_id: &str, title: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.rename_item_locked(item_id, title))
    }

    pub(crate) fn reorder_items(&self, list_id: &str, ids: &[String]) -> Result<(), TodoError> {
        self.ready_op(|| self.reorder_items_locked(list_id, ids))
    }

    pub(crate) fn set_due(&self, item_id: &str, due: Option<CivilDate>) -> Result<(), TodoError> {
        self.ready_op(|| self.set_due_locked(item_id, due))
    }

    pub(crate) fn set_reminder(
        &self,
        item_id: &str,
        remind_at: Option<ClockTime>,
    ) -> Result<(), TodoError> {
        self.ready_op(|| self.set_reminder_locked(item_id, remind_at))
    }

    pub(crate) fn set_recurrence(
        &self,
        item_id: &str,
        recurrence: Option<super::model::Recurrence>,
    ) -> Result<(), TodoError> {
        self.ready_op(|| self.set_recurrence_locked(item_id, recurrence))
    }

    pub(crate) fn set_source(&self, item_id: &str, source: TodoSource) -> Result<(), TodoError> {
        self.ready_op(|| self.set_source_locked(item_id, source))
    }

    pub(crate) fn clear_source(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.clear_source_locked(item_id))
    }

    pub(crate) fn complete_item(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.complete_locked(item_id))
    }

    pub(crate) fn soft_delete(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.soft_delete_locked(item_id))
    }

    pub(crate) fn restore(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.restore_locked(item_id))
    }

    pub(crate) fn purge(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.purge_locked(item_id))
    }

    pub(crate) fn source_url(&self, item_id: &str) -> Result<String, TodoError> {
        self.ready_op(|| self.source_url_locked(item_id))
    }

    pub(crate) fn set_current(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.set_current_locked(item_id))
    }

    pub(crate) fn clear_current(&self) -> Result<(), TodoError> {
        self.ready_op(|| self.clear_current_locked())
    }

    pub(crate) fn process_today(&self, item_id: &str, today: CivilDate) -> Result<(), TodoError> {
        self.ready_op(|| self.process_today_locked(item_id, today))
    }

    pub(crate) fn process_defer(
        &self,
        item_id: &str,
        days: u32,
        today: CivilDate,
    ) -> Result<(), TodoError> {
        self.ready_op(|| self.process_defer_locked(item_id, days, today))
    }

    pub(crate) fn process_move(
        &self,
        item_id: &str,
        target_list_id: &str,
    ) -> Result<(), TodoError> {
        self.ready_op(|| self.process_move_locked(item_id, target_list_id))
    }

    pub(crate) fn process_complete(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.process_complete_locked(item_id))
    }

    pub(crate) fn process_soft_delete(&self, item_id: &str) -> Result<(), TodoError> {
        self.ready_op(|| self.process_soft_delete_locked(item_id))
    }

    fn ready_op<T>(&self, body: impl FnOnce() -> Result<T, TodoError>) -> Result<T, TodoError> {
        self.ensure_ready()?;
        let _op = lock_mutex(&self.op);
        body()
    }

    fn ensure_ready(&self) -> Result<(), TodoError> {
        let loaded = lock_mutex(&self.state).loaded;
        if loaded && self.store.is_ready() {
            Ok(())
        } else {
            Err(TodoError::NotLoaded)
        }
    }

    fn create_list_locked(&self, name: &str) -> Result<String, TodoError> {
        let name = normalize_required(name).ok_or(TodoError::BlankName)?;
        let mut lists = self.lists_vec()?;
        let order = next_order(lists.iter().map(|list| list.order))?;
        let id = fresh_unique_id('l', |candidate| {
            lists.iter().any(|list| list.id == candidate)
        })?;
        let list = TodoList::new(id.clone(), name, ListKind::Normal, order);
        lists.push(list.clone());
        self.persist(lists, vec![list])?;
        Ok(id)
    }

    fn rename_list_locked(&self, id: &str, name: &str) -> Result<(), TodoError> {
        let name = normalize_required(name).ok_or(TodoError::BlankName)?;
        let mut lists = self.lists_vec()?;
        let list_index = find_list(&lists, id).ok_or_else(|| missing_list(id))?;
        lists[list_index].name = name;
        let changed = lists[list_index].clone();
        self.persist(lists, vec![changed])
    }

    fn delete_list_locked(&self, id: &str) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let list_index = find_list(&lists, id).ok_or_else(|| missing_list(id))?;
        if lists[list_index].kind == ListKind::Inbox {
            return Err(TodoError::CannotDeleteInbox);
        }
        if !lists[list_index].items.is_empty() {
            return Err(TodoError::ListNotEmpty);
        }
        let previous = lists.clone();
        lists.remove(list_index);
        self.set_lists(lists);
        match self.store.remove(&DocumentId::Todo(id.to_owned())) {
            Ok(_) => Ok(()),
            Err(err) => {
                self.set_lists(previous);
                Err(err.into())
            }
        }
    }

    fn reorder_lists_locked(&self, ids: &[String]) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        let mut ordered = reorder_by_ids(lists, ids, |list| list.id.as_str())?;
        for (index, list) in ordered.iter_mut().enumerate() {
            list.order = index_as_order(index);
        }
        let writes = ordered.clone();
        self.persist(ordered, writes)
    }

    fn ensure_inbox_locked(&self) -> Result<String, TodoError> {
        let mut lists = self.lists_vec()?;
        if let Some(existing) = lists
            .iter()
            .filter(|list| list.kind == ListKind::Inbox)
            .min_by(|left, right| left.id.cmp(&right.id))
        {
            return Ok(existing.id.clone());
        }
        let id = allocate_inbox_id(&lists)?;
        let order = next_order(lists.iter().map(|list| list.order))?;
        let list = TodoList::new(id.clone(), "收件箱".into(), ListKind::Inbox, order);
        lists.push(list.clone());
        self.persist(lists, vec![list])?;
        Ok(id)
    }

    fn create_item_locked(&self, list_id: &str, draft: NewTodo) -> Result<String, TodoError> {
        let title = normalize_required(&draft.title).ok_or(TodoError::BlankTitle)?;
        if let Some(source) = &draft.source
            && !is_http_source_url(&source.url)
        {
            return Err(TodoError::RejectedSource);
        }
        let mut lists = self.lists_vec()?;
        let list_index = find_list(&lists, list_id).ok_or_else(|| missing_list(list_id))?;
        let order = next_order(lists[list_index].items.iter().map(|item| item.order))?;
        let id = fresh_unique_id('i', |candidate| {
            lists
                .iter()
                .any(|list| list.items.iter().any(|item| item.id == candidate))
        })?;
        let mut item = TodoItem::new(id.clone(), title, order);
        item.due = draft.due;
        item.remind_at = draft.remind_at;
        item.recurrence = draft.recurrence;
        item.source = draft.source;
        lists[list_index].items.push(item);
        let changed = lists[list_index].clone();
        self.persist(lists, vec![changed])?;
        Ok(id)
    }

    fn rename_item_locked(&self, item_id: &str, title: &str) -> Result<(), TodoError> {
        let title = normalize_required(title).ok_or(TodoError::BlankTitle)?;
        self.edit_active(item_id, |item| {
            item.title = title;
            Ok(())
        })
    }

    fn reorder_items_locked(&self, list_id: &str, ids: &[String]) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let list_index = find_list(&lists, list_id).ok_or_else(|| missing_list(list_id))?;
        let items = reorder_by_ids(std::mem::take(&mut lists[list_index].items), ids, |item| {
            item.id.as_str()
        })?;
        lists[list_index].items = items;
        renumber(&mut lists[list_index].items);
        let changed = lists[list_index].clone();
        self.persist(lists, vec![changed])
    }

    fn set_due_locked(&self, item_id: &str, due: Option<CivilDate>) -> Result<(), TodoError> {
        self.edit_active(item_id, |item| {
            item.due = due;
            Ok(())
        })
    }

    fn set_reminder_locked(
        &self,
        item_id: &str,
        remind_at: Option<ClockTime>,
    ) -> Result<(), TodoError> {
        self.edit_active(item_id, |item| {
            item.remind_at = remind_at;
            Ok(())
        })
    }

    fn set_recurrence_locked(
        &self,
        item_id: &str,
        recurrence: Option<super::model::Recurrence>,
    ) -> Result<(), TodoError> {
        self.edit_active(item_id, |item| {
            item.recurrence = recurrence;
            Ok(())
        })
    }

    fn set_source_locked(&self, item_id: &str, source: TodoSource) -> Result<(), TodoError> {
        if !is_http_source_url(&source.url) {
            return Err(TodoError::RejectedSource);
        }
        self.edit_active(item_id, |item| {
            item.source = Some(source);
            Ok(())
        })
    }

    fn clear_source_locked(&self, item_id: &str) -> Result<(), TodoError> {
        self.edit_active(item_id, |item| {
            item.source = None;
            Ok(())
        })
    }

    fn complete_locked(&self, item_id: &str) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let (list_index, item_index) =
            find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        if lists[list_index].items[item_index].in_trash() {
            return Err(TodoError::AlreadyInTrash);
        }
        if lists[list_index].items[item_index].completed {
            return Err(TodoError::AlreadyComplete);
        }
        let generated =
            if let Some(recurrence) = lists[list_index].items[item_index].recurrence.clone() {
                match next_occurrence(lists[list_index].items[item_index].due, &recurrence) {
                    NextOccurrence::MonthlyMissingDay => {
                        return Err(TodoError::PendingSpec(PendingTopic::MonthlyMissingDay));
                    }
                    NextOccurrence::OnUntil => {
                        return Err(TodoError::PendingSpec(PendingTopic::CompleteOnUntil));
                    }
                    NextOccurrence::Overflow => return Err(TodoError::DateOverflow),
                    NextOccurrence::Date(due) => Some(due),
                    NextOccurrence::PastUntil | NextOccurrence::MissingDue => None,
                }
            } else {
                None
            };
        lists[list_index].items[item_index].completed = true;
        if let Some(due) = generated {
            let mut next = lists[list_index].items[item_index].clone();
            next.id = fresh_unique_id('i', |candidate| {
                lists
                    .iter()
                    .any(|list| list.items.iter().any(|item| item.id == candidate))
            })?;
            next.completed = false;
            next.due = Some(due);
            next.current = false;
            next.current_since = None;
            next.deleted_at = None;
            next.origin_list_id = None;
            next.moved_at = None;
            lists[list_index].items.insert(item_index + 1, next);
            renumber(&mut lists[list_index].items);
        }
        let changed = lists[list_index].clone();
        self.persist(lists, vec![changed])
    }

    fn soft_delete_locked(&self, item_id: &str) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let (list_index, item_index) =
            find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        if lists[list_index].items[item_index].in_trash() {
            return Err(TodoError::AlreadyInTrash);
        }
        let list_id = lists[list_index].id.clone();
        let item = &mut lists[list_index].items[item_index];
        item.deleted_at = Some(now_ms());
        item.origin_list_id = Some(list_id);
        let changed = lists[list_index].clone();
        self.persist(lists, vec![changed])
    }

    fn restore_locked(&self, item_id: &str) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let (list_index, item_index) =
            find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        if !lists[list_index].items[item_index].in_trash() {
            return Err(TodoError::NotInTrash);
        }
        let origin = lists[list_index].items[item_index]
            .origin_list_id
            .clone()
            .unwrap_or_else(|| lists[list_index].id.clone());
        let Some(origin_index) = find_list(&lists, &origin) else {
            return Err(TodoError::PendingSpec(PendingTopic::RestoreWithoutOrigin));
        };
        if origin_index == list_index {
            lists[list_index].items[item_index].deleted_at = None;
            lists[list_index].items[item_index].origin_list_id = None;
            let changed = lists[list_index].clone();
            return self.persist(lists, vec![changed]);
        }
        let mut item = lists[list_index].items[item_index].clone();
        item.deleted_at = None;
        item.origin_list_id = None;
        item.moved_at = Some(now_ms());
        item.order = next_order(lists[origin_index].items.iter().map(|item| item.order))?;
        lists[origin_index].items.push(item);
        lists[list_index].items.remove(item_index);
        let target = lists[origin_index].clone();
        let source = lists[list_index].clone();
        self.persist(lists, vec![target, source])
    }

    fn purge_locked(&self, item_id: &str) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        Err(TodoError::PendingSpec(PendingTopic::Purge))
    }

    fn source_url_locked(&self, item_id: &str) -> Result<String, TodoError> {
        let lists = self.lists_vec()?;
        let (list_index, item_index) =
            find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        lists[list_index].items[item_index]
            .source
            .as_ref()
            .map(|source| source.url.clone())
            .ok_or(TodoError::NoSource)
    }

    fn set_current_locked(&self, item_id: &str) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        let target = find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        let currents = current_positions(&lists);
        if currents.len() == 1 && currents[0] == target {
            return Ok(());
        }
        let now = now_ms();
        let mut marked = lists;
        marked[target.0].items[target.1].current = true;
        marked[target.0].items[target.1].current_since = Some(now);
        let mut final_lists = marked.clone();
        for (list_index, item_index) in currents {
            if (list_index, item_index) == target {
                continue;
            }
            final_lists[list_index].items[item_index].current = false;
            final_lists[list_index].items[item_index].current_since = None;
        }
        let mut writes = vec![marked[target.0].clone()];
        for list in &final_lists {
            let previous = marked
                .iter()
                .find(|candidate| candidate.id == list.id)
                .expect("清单集合不变");
            if previous != list {
                writes.push(list.clone());
            }
        }
        self.persist(final_lists, writes)
    }

    fn clear_current_locked(&self) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let mut changed_ids = BTreeSet::new();
        for list in &mut lists {
            let mut changed = false;
            for item in &mut list.items {
                if item.current {
                    item.current = false;
                    item.current_since = None;
                    changed = true;
                }
            }
            if changed {
                changed_ids.insert(list.id.clone());
            }
        }
        if changed_ids.is_empty() {
            return Ok(());
        }
        let writes = lists
            .iter()
            .filter(|list| changed_ids.contains(&list.id))
            .cloned()
            .collect();
        self.persist(lists, writes)
    }

    fn process_today_locked(&self, item_id: &str, today: CivilDate) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        require_inbox_active(&lists, item_id)?;
        self.set_due_locked(item_id, Some(today))
    }

    fn process_defer_locked(
        &self,
        item_id: &str,
        days: u32,
        today: CivilDate,
    ) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        let (list_index, item_index) = require_inbox_active(&lists, item_id)?;
        let due =
            deferred_due(lists[list_index].items[item_index].due, today, days).map_err(|err| {
                match err {
                    DeferError::InvalidDays => TodoError::InvalidDefer,
                    DeferError::Overflow => TodoError::DateOverflow,
                }
            })?;
        self.set_due_locked(item_id, Some(due))
    }

    fn process_move_locked(&self, item_id: &str, target_list_id: &str) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        require_inbox_active(&lists, item_id)?;
        self.move_item_locked(item_id, target_list_id)
    }

    fn process_complete_locked(&self, item_id: &str) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        require_inbox_active(&lists, item_id)?;
        self.complete_locked(item_id)
    }

    fn process_soft_delete_locked(&self, item_id: &str) -> Result<(), TodoError> {
        let lists = self.lists_vec()?;
        require_inbox_active(&lists, item_id)?;
        self.soft_delete_locked(item_id)
    }

    fn move_item_locked(&self, item_id: &str, target_id: &str) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let (list_index, item_index) =
            find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        if lists[list_index].items[item_index].in_trash() {
            return Err(TodoError::AlreadyInTrash);
        }
        if lists[list_index].id == target_id {
            return Err(TodoError::SameList);
        }
        let target_index = find_list(&lists, target_id).ok_or_else(|| missing_list(target_id))?;
        let mut item = lists[list_index].items[item_index].clone();
        item.moved_at = Some(now_ms());
        item.order = next_order(lists[target_index].items.iter().map(|item| item.order))?;
        lists[target_index].items.push(item);
        lists[list_index].items.remove(item_index);
        let target = lists[target_index].clone();
        let source = lists[list_index].clone();
        self.persist(lists, vec![target, source])
    }

    fn edit_active(
        &self,
        item_id: &str,
        mutate: impl FnOnce(&mut TodoItem) -> Result<(), TodoError>,
    ) -> Result<(), TodoError> {
        let mut lists = self.lists_vec()?;
        let (list_index, item_index) =
            find_item(&lists, item_id).ok_or_else(|| missing_item(item_id))?;
        if lists[list_index].items[item_index].in_trash() {
            return Err(TodoError::AlreadyInTrash);
        }
        mutate(&mut lists[list_index].items[item_index])?;
        let changed = lists[list_index].clone();
        self.persist(lists, vec![changed])
    }

    fn persist_changed(
        &self,
        lists: Vec<TodoList>,
        changed: &BTreeSet<String>,
    ) -> Result<(), TodoError> {
        let writes = lists
            .iter()
            .filter(|list| changed.contains(&list.id))
            .cloned()
            .collect();
        self.persist(lists, writes)
    }

    fn persist(&self, next: Vec<TodoList>, writes: Vec<TodoList>) -> Result<(), TodoError> {
        if writes.is_empty() {
            self.set_lists(next);
            return Ok(());
        }
        let previous = self.lists_vec()?;
        let mut batch = self.store.begin_batch()?;
        let mut replaced = Vec::new();
        for (index, list) in writes.iter().enumerate() {
            if let Some(err) = self.injected_batch_failure(index) {
                drop(batch);
                return self.finish_failed_write(&previous, &replaced, err);
            }
            match batch.write_json(&DocumentId::Todo(list.id.clone()), list) {
                Ok(_) => replaced.push(list.id.clone()),
                Err(err) => {
                    drop(batch);
                    return self.finish_failed_write(&previous, &replaced, err.into());
                }
            }
        }
        self.set_lists(next);
        batch.commit()?;
        Ok(())
    }

    /// 已经替换过的文件写回操作前内容。回滚批次不提交，因此没有 `EntityChanged`。
    fn finish_failed_write(
        &self,
        previous: &[TodoList],
        replaced: &[String],
        err: TodoError,
    ) -> Result<(), TodoError> {
        if replaced.is_empty() {
            return Err(err);
        }
        match self.write_preimage(previous, replaced) {
            Ok(()) => Err(err),
            Err(_) => self.repair_partial(err),
        }
    }

    fn write_preimage(&self, previous: &[TodoList], replaced: &[String]) -> Result<(), TodoError> {
        if let Some(err) = self.injected_rollback_failure() {
            return Err(err);
        }
        let mut batch = self.store.begin_batch()?;
        for id in replaced {
            let write = if let Some(list) = previous.iter().find(|list| list.id == *id) {
                batch
                    .write_json(&DocumentId::Todo(id.clone()), list)
                    .map(|_| ())
            } else {
                batch.remove(&DocumentId::Todo(id.clone())).map(|_| ())
            };
            if let Err(write_err) = write {
                drop(batch);
                return Err(write_err.into());
            }
        }
        drop(batch);
        Ok(())
    }

    /// 回滚没有写回去。读盘后按启动时的两条规则修到没有重复 id、也没有两条当前标记。
    ///
    /// 修复写回不提交批次：这条命令仍要返回错误，不发布 `EntityChanged`。
    /// 读盘或修复写回再失败时返回那个错误，并卸下内存，避免接着用和磁盘不一致的原内容。
    fn repair_partial(&self, original: TodoError) -> Result<(), TodoError> {
        if let Some(hook) = self.take_before_reread() {
            hook();
        }
        let mut lists = match self.read_disk() {
            Ok(lists) => lists,
            Err(err) => {
                self.mark_unloaded();
                return Err(err);
            }
        };
        let mut changed = repair_moved_at(&mut lists);
        changed.extend(repair_current_since(&mut lists));
        if changed.is_empty() {
            self.set_lists(lists);
            return Err(original);
        }
        let mut batch = match self.store.begin_batch() {
            Ok(batch) => batch,
            Err(err) => {
                self.mark_unloaded();
                return Err(err.into());
            }
        };
        for list in lists.iter().filter(|list| changed.contains(&list.id)) {
            if let Err(err) = batch.write_json(&DocumentId::Todo(list.id.clone()), list) {
                drop(batch);
                self.mark_unloaded();
                return Err(err.into());
            }
        }
        drop(batch);
        self.set_lists(lists);
        Err(original)
    }

    fn mark_unloaded(&self) {
        let mut state = lock_mutex(&self.state);
        state.loaded = false;
        state.lists.clear();
    }

    fn injected_batch_failure(&self, index: usize) -> Option<TodoError> {
        #[cfg(test)]
        {
            if lock_mutex(&self.fault).fail_at == Some(index) {
                return Some(injected_replace_error());
            }
            None
        }
        #[cfg(not(test))]
        {
            let _ = index;
            None
        }
    }

    fn injected_rollback_failure(&self) -> Option<TodoError> {
        #[cfg(test)]
        {
            if lock_mutex(&self.fault).fail_rollback {
                return Some(injected_replace_error());
            }
        }
        #[cfg(not(test))]
        {
            let _ = self;
        }
        None
    }

    fn take_before_reread(&self) -> Option<Box<dyn FnOnce() + Send>> {
        #[cfg(test)]
        {
            return lock_mutex(&self.fault).before_reread.take();
        }
        #[cfg(not(test))]
        {
            let _ = self;
            None
        }
    }

    fn read_disk(&self) -> Result<Vec<TodoList>, TodoError> {
        let loaded = self
            .store
            .read_collection::<TodoList>(CollectionKind::Todos)?;
        let mut lists = Vec::with_capacity(loaded.files.len());
        for file in loaded.files {
            if !is_supported_schema(file.value.schema_version) {
                return Err(TodoError::UnsupportedSchema {
                    found: file.value.schema_version,
                });
            }
            if file.value.id != file.id {
                return Err(TodoError::IdMismatch { id: file.id });
            }
            lists.push(file.value);
        }
        Ok(lists)
    }

    fn lists_vec(&self) -> Result<Vec<TodoList>, TodoError> {
        let state = lock_mutex(&self.state);
        if !state.loaded {
            Err(TodoError::NotLoaded)
        } else {
            Ok(state.lists.clone())
        }
    }

    fn set_lists(&self, lists: Vec<TodoList>) {
        let mut state = lock_mutex(&self.state);
        state.lists = lists;
        state.loaded = true;
    }
}

fn current_positions(lists: &[TodoList]) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    for (list_index, list) in lists.iter().enumerate() {
        for (item_index, item) in list.items.iter().enumerate() {
            if item.current {
                positions.push((list_index, item_index));
            }
        }
    }
    positions
}

fn require_inbox_active(lists: &[TodoList], item_id: &str) -> Result<(usize, usize), TodoError> {
    let (list_index, item_index) =
        find_item(lists, item_id).ok_or_else(|| missing_item(item_id))?;
    let list = &lists[list_index];
    let item = &list.items[item_index];
    if list.kind != ListKind::Inbox || item.completed || item.in_trash() {
        return Err(TodoError::NotInboxActive);
    }
    Ok((list_index, item_index))
}

fn find_list(lists: &[TodoList], id: &str) -> Option<usize> {
    lists.iter().position(|list| list.id == id)
}

fn find_item(lists: &[TodoList], id: &str) -> Option<(usize, usize)> {
    for (list_index, list) in lists.iter().enumerate() {
        if let Some(item_index) = list.items.iter().position(|item| item.id == id) {
            return Some((list_index, item_index));
        }
    }
    None
}

fn missing_list(id: &str) -> TodoError {
    TodoError::ListNotFound { id: id.to_owned() }
}

fn missing_item(id: &str) -> TodoError {
    TodoError::ItemNotFound { id: id.to_owned() }
}

fn reorder_by_ids<T>(
    mut items: Vec<T>,
    ids: &[String],
    id_of: impl Fn(&T) -> &str,
) -> Result<Vec<T>, TodoError> {
    if items.len() != ids.len() {
        return Err(TodoError::InvalidOrder);
    }
    let mut ordered = Vec::with_capacity(items.len());
    for id in ids {
        let Some(pos) = items.iter().position(|item| id_of(item) == id) else {
            return Err(TodoError::InvalidOrder);
        };
        ordered.push(items.remove(pos));
    }
    Ok(ordered)
}

fn renumber(items: &mut [TodoItem]) {
    for (index, item) in items.iter_mut().enumerate() {
        item.order = index_as_order(index);
    }
}

fn index_as_order(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

fn next_order(values: impl Iterator<Item = i64>) -> Result<i64, TodoError> {
    values.max().map_or(Ok(0), |max| {
        max.checked_add(1).ok_or(TodoError::InvalidOrder)
    })
}

fn sort_lists(lists: &mut [TodoList]) {
    for list in lists.iter_mut() {
        list.items.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.id.cmp(&right.id))
        });
    }
    lists.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn allocate_inbox_id(lists: &[TodoList]) -> Result<String, TodoError> {
    let mut candidate = "inbox".to_owned();
    let mut suffix = 2u32;
    loop {
        if lists.iter().all(|list| list.id != candidate) {
            return Ok(candidate);
        }
        candidate = format!("inbox{suffix}");
        suffix = suffix
            .checked_add(1)
            .ok_or_else(|| TodoError::DuplicateId {
                id: candidate.clone(),
            })?;
    }
}

fn fresh_unique_id(prefix: char, taken: impl Fn(&str) -> bool) -> Result<String, TodoError> {
    for _ in 0..8 {
        let id = fresh_id(prefix);
        if !taken(&id) {
            return Ok(id);
        }
    }
    Err(TodoError::DuplicateId {
        id: fresh_id(prefix),
    })
}

fn fresh_id(prefix: char) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}{:x}-{:x}", now_ms(), seq)
}

#[cfg(test)]
fn injected_replace_error() -> TodoError {
    TodoError::Storage(crate::storage::Error::io(
        crate::storage::IoAction::Replace,
        "fault",
        std::io::Error::other("injected"),
    ))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::storage::{BootHooks, DocumentId, Store, StorePaths};
    use crate::todos::{NewTodo, TodoList};

    use super::{Service, TodoNotice};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("lanwork-todo-rollback-{nanos}-{seq}"));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct Ready {
        _temp: TempDir,
        store: Store,
        service: Arc<Service>,
    }

    impl Ready {
        fn new() -> Self {
            let temp = TempDir::new();
            let paths = StorePaths {
                data_dir: temp.path.join("data"),
                cache_dir: temp.path.join("cache"),
                user_profile: temp.path.join("profile"),
                local_app_data: temp.path.join("local"),
            };
            let store = Store::open(paths).unwrap();
            let service = Service::open(store.clone());
            let loading = Arc::clone(&service);
            let mut hooks = BootHooks::new();
            hooks.load = Some(Box::new(move |_| {
                loading.load().map_err(|err| err.to_string())
            }));
            store.boot(hooks).unwrap();
            Self {
                _temp: temp,
                store,
                service,
            }
        }
    }

    fn draft(title: &str) -> NewTodo {
        NewTodo {
            title: title.into(),
            due: None,
            remind_at: None,
            recurrence: None,
            source: None,
        }
    }

    fn file_bytes(store: &Store, id: &str) -> Vec<u8> {
        let path = store.document_path(&DocumentId::Todo(id.into())).unwrap();
        std::fs::read(path).unwrap()
    }

    fn disk_list(store: &Store, id: &str) -> TodoList {
        store
            .read_json(&DocumentId::Todo(id.into()))
            .unwrap()
            .unwrap()
    }

    fn parent_dir(path: &Path) -> PathBuf {
        path.parent().unwrap().to_path_buf()
    }

    #[test]
    fn purged_notice_names_the_item() {
        let notice = TodoNotice::Purged {
            item_id: "a".into(),
        };
        assert_eq!(
            notice,
            TodoNotice::Purged {
                item_id: "a".into()
            }
        );
    }

    #[test]
    fn same_list_second_write_rolls_back_to_the_preimage() {
        let ready = Ready::new();
        let list_id = ready.service.create_list("工作").unwrap();
        let first = ready.service.create_item(&list_id, draft("一")).unwrap();
        let second = ready.service.create_item(&list_id, draft("二")).unwrap();
        ready.service.set_current(&first).unwrap();
        let before = file_bytes(&ready.store, &list_id);
        let rx = ready.store.subscribe();
        ready.service.set_write_fault(Some(1), false, None);
        let err = ready.service.set_current(&second).unwrap_err();
        assert!(err.to_string().contains("写入失败"), "{err}");
        assert!(ready.service.item(&first).unwrap().item.current);
        assert!(!ready.service.item(&second).unwrap().item.current);
        assert!(rx.try_recv().is_err());
        assert_eq!(file_bytes(&ready.store, &list_id), before);
    }

    #[test]
    fn failed_rollback_repairs_a_duplicated_move() {
        let ready = Ready::new();
        let inbox = ready.service.ensure_inbox().unwrap();
        let target = ready.service.create_list("工作").unwrap();
        let item_id = ready.service.create_item(&inbox, draft("搬走")).unwrap();
        let rx = ready.store.subscribe();
        ready.service.set_write_fault(Some(1), true, None);
        let err = ready.service.process_move(&item_id, &target).unwrap_err();
        assert!(err.to_string().contains("写入失败"), "{err}");
        let stored = ready.service.item(&item_id).unwrap();
        assert_eq!(stored.list_id, target);
        let lists = ready.service.lists().unwrap();
        let copies = lists
            .iter()
            .filter(|list| list.items.iter().any(|item| item.id == item_id))
            .count();
        assert_eq!(copies, 1);
        assert!(
            disk_list(&ready.store, &inbox)
                .items
                .iter()
                .all(|item| item.id != item_id)
        );
        assert_eq!(disk_list(&ready.store, &target).items[0].id, item_id);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failed_rollback_repairs_two_current_flags() {
        let ready = Ready::new();
        let first_list = ready.service.create_list("甲").unwrap();
        let second_list = ready.service.create_list("乙").unwrap();
        let first = ready
            .service
            .create_item(&first_list, draft("旧当前"))
            .unwrap();
        let second = ready
            .service
            .create_item(&second_list, draft("新当前"))
            .unwrap();
        ready.service.set_current(&first).unwrap();
        let rx = ready.store.subscribe();
        ready.service.set_write_fault(Some(1), true, None);
        let err = ready.service.set_current(&second).unwrap_err();
        assert!(err.to_string().contains("写入失败"), "{err}");
        assert!(rx.try_recv().is_err());
        assert!(!ready.service.item(&first).unwrap().item.current);
        assert!(ready.service.item(&second).unwrap().item.current);
        let currents = ready
            .service
            .lists()
            .unwrap()
            .iter()
            .flat_map(|list| list.items.iter())
            .filter(|item| item.current)
            .count();
        assert_eq!(currents, 1);
        assert!(!disk_list(&ready.store, &first_list).items[0].current);
        assert!(disk_list(&ready.store, &second_list).items[0].current);
    }

    #[test]
    fn failed_reread_after_rollback_is_not_ignored() {
        let ready = Ready::new();
        let inbox = ready.service.ensure_inbox().unwrap();
        let target = ready.service.create_list("工作").unwrap();
        let item_id = ready.service.create_item(&inbox, draft("搬走")).unwrap();
        let dir = parent_dir(
            &ready
                .store
                .document_path(&DocumentId::Todo(inbox.clone()))
                .unwrap(),
        );
        ready.service.set_write_fault(
            Some(1),
            true,
            Some(Box::new(move || {
                std::fs::remove_dir_all(&dir).unwrap();
                std::fs::write(&dir, b"not-a-directory").unwrap();
            })),
        );
        let err = ready.service.process_move(&item_id, &target).unwrap_err();
        assert!(err.to_string().contains("读取失败"), "{err}");
        assert!(ready.service.lists().is_err());
    }
}
