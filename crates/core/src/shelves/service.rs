//! 收纳规则和写盘。
//!
//! 内存只在对应写入成功之后更新。变更消息在批次提交时发出，提交发生在内存更新之后。
//! 写盘失败时把已经替换的分组文件写回操作前内容，不提交、不发布 `EntityChanged`，
//! 内存保持操作前状态。回滚再失败时重新读入，让内存和磁盘一致，并返回错误。
//!
//! 这里只通过存储层改 `shelves/<id>.json`。不探测拖入路径的属性，
//! 也不对用户文件做删除、移动或复制。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::storage::{CollectionKind, DocumentId, Store, lock_mutex};
use crate::todos::{TodoCommands, TodoNotice};

use super::error::ShelfError;
use super::model::{
    AddRefsOutcome, EXISTS_TIMEOUT, IncomingRef, LoadedShelf, Shelf, ShelfFile, ShelfRef,
    normalize_name, sort_groups, unsupported,
};
use super::path::normalize_path;

#[derive(Default)]
struct Cache {
    groups: BTreeMap<String, Shelf>,
    skipped: BTreeMap<String, u32>,
    poisoned: bool,
}

struct ExistsCheck {
    timeout: Duration,
    probe: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

struct Inner {
    store: Store,
    cache: Mutex<Cache>,
    write: Mutex<()>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    seq: AtomicU64,
    exists: Mutex<ExistsCheck>,
    #[cfg(test)]
    fail_at: Mutex<Option<usize>>,
}

/// 收纳的进程内服务。克隆共享同一份缓存和写锁。
///
/// 界面应通过 [`super::ShelfCommands`] 调用。
#[derive(Clone)]
pub struct ShelfService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ShelfService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShelfService")
            .field("store", &self.inner.store)
            .finish_non_exhaustive()
    }
}

impl ShelfService {
    /// 从 `shelves/` 加载。损坏的文件由存储层隔离，其余分组照常读入。
    pub fn open(store: Store) -> Result<Self, ShelfError> {
        Self::open_at(store, now_millis)
    }

    /// `now` 提供引用的加入时间。测试用它固定时间戳。
    pub fn open_at<F>(store: Store, now: F) -> Result<Self, ShelfError>
    where
        F: Fn() -> i64 + Send + Sync + 'static,
    {
        let service = Self {
            inner: Arc::new(Inner {
                store,
                cache: Mutex::new(Cache::default()),
                write: Mutex::new(()),
                now: Arc::new(now),
                seq: AtomicU64::new(1),
                exists: Mutex::new(ExistsCheck {
                    timeout: EXISTS_TIMEOUT,
                    probe: Arc::new(probe_filesystem),
                }),
                #[cfg(test)]
                fail_at: Mutex::new(None),
            }),
        };
        service.reload()?;
        let reload: Arc<dyn crate::storage::MemoryReload> = service.inner.clone();
        service.inner.store.watch_memory(&reload);
        drop(reload);
        Ok(service)
    }

    pub(crate) fn check_group_id(&self, id: &str) -> Result<(), ShelfError> {
        self.inner
            .store
            .document_path(&DocumentId::Shelf(id.to_owned()))?;
        Ok(())
    }

    pub(crate) fn check_todo_id(&self, id: &str) -> Result<(), ShelfError> {
        match self
            .inner
            .store
            .document_path(&DocumentId::Todo(id.to_owned()))
        {
            Ok(_) => Ok(()),
            Err(_) => Err(ShelfError::InvalidTodoId { id: id.to_owned() }),
        }
    }

    pub fn create_group(&self, name: &str) -> Result<Shelf, ShelfError> {
        let name = normalize_name(name).ok_or(ShelfError::BlankName)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let order = next_order(self.groups()?.iter().map(|group| group.order))?;
        let id = self.allocate_id()?;
        let shelf = Shelf {
            id,
            name,
            order,
            todo_id: None,
            refs: Vec::new(),
        };
        self.persist_one(&shelf)?;
        Ok(shelf)
    }

    pub fn rename_group(&self, id: &str, name: &str) -> Result<Shelf, ShelfError> {
        let name = normalize_name(name).ok_or(ShelfError::BlankName)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let mut shelf = self.group(id)?;
        if shelf.name == name {
            return Ok(shelf);
        }
        shelf.name = name;
        self.persist_one(&shelf)?;
        Ok(shelf)
    }

    /// 删除分组数据文件。不改用户文件。
    pub fn delete_group(&self, id: &str) -> Result<(), ShelfError> {
        self.check_group_id(id)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        self.ensure_editable(id)?;
        if self.lookup(id)?.is_none() {
            return Err(ShelfError::NotFound { id: id.to_owned() });
        }
        let mut batch = self.inner.store.begin_batch()?;
        batch.remove(&DocumentId::Shelf(id.to_owned()))?;
        self.cache_remove(id);
        batch.commit()?;
        Ok(())
    }

    pub fn reorder_groups(&self, ids: &[String]) -> Result<(), ShelfError> {
        for id in ids {
            self.check_group_id(id)?;
        }
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let groups = self.groups()?;
        let mut ordered = reorder(groups, ids)?;
        let mut changed = Vec::new();
        for (index, group) in ordered.iter_mut().enumerate() {
            let order = i64::try_from(index).unwrap_or(i64::MAX);
            if group.order != order {
                group.order = order;
                changed.push(group.clone());
            }
        }
        self.persist(&changed)?;
        Ok(())
    }

    /// 把路径加入分组。
    ///
    /// `group_id` 为空且当前没有分组时，创建名为「收纳」的分组，和引用同一次写入。
    /// 同一分组内规范化路径相同则不新增。任何失败都不留下空分组。
    pub fn add_refs(
        &self,
        group_id: Option<&str>,
        incoming: &[IncomingRef],
    ) -> Result<AddRefsOutcome, ShelfError> {
        if let Some(id) = group_id {
            self.check_group_id(id)?;
        }
        let prepared = prepare_incoming(incoming)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let groups = self.groups()?;
        let shelf = match group_id {
            Some(id) => self.group(id)?,
            None if groups.is_empty() => {
                if prepared.is_empty() {
                    return Ok(AddRefsOutcome {
                        group: None,
                        added: 0,
                    });
                }
                let id = self.allocate_id()?;
                Shelf {
                    id,
                    name: super::model::DEFAULT_GROUP_NAME.to_owned(),
                    order: next_order(groups.iter().map(|group| group.order))?,
                    todo_id: None,
                    refs: Vec::new(),
                }
            }
            None => return Err(ShelfError::TargetGroupRequired),
        };
        let mut next = shelf.clone();
        let added_at = (self.inner.now)();
        let mut added = 0usize;
        for item in prepared {
            if next
                .refs
                .iter()
                .any(|existing| existing.path_key() == item.key)
            {
                continue;
            }
            next.refs.push(ShelfRef {
                path: item.stored,
                name: item.name,
                folder: item.folder,
                added_at,
            });
            added += 1;
        }
        if added == 0 {
            let group = if group_id.is_some() {
                Some(shelf)
            } else {
                None
            };
            return Ok(AddRefsOutcome { group, added: 0 });
        }
        self.persist_one(&next)?;
        Ok(AddRefsOutcome {
            group: Some(next),
            added,
        })
    }

    /// 从分组去掉一条引用。不改用户文件。
    pub fn remove_ref(&self, id: &str, path: &str) -> Result<Shelf, ShelfError> {
        self.check_group_id(id)?;
        let normalized = normalize_path(path)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let mut shelf = self.group(id)?;
        let before = shelf.refs.len();
        shelf.refs.retain(|item| item.path_key() != normalized.key);
        if shelf.refs.len() == before {
            return Err(ShelfError::RefNotFound {
                id: id.to_owned(),
                path: path.to_owned(),
            });
        }
        self.persist_one(&shelf)?;
        Ok(shelf)
    }

    /// 关联一条待办。已关联另一条时拒绝，不替换。
    ///
    /// 只校验待办 id 的格式。该 id 是否已有待办，产品规格没有写，这里不检查。
    pub fn link_todo(&self, id: &str, todo_id: &str) -> Result<Shelf, ShelfError> {
        self.check_group_id(id)?;
        self.check_todo_id(todo_id)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let mut shelf = self.group(id)?;
        if shelf.todo_id.as_deref() == Some(todo_id) {
            return Ok(shelf);
        }
        if let Some(existing) = shelf.todo_id.clone() {
            return Err(ShelfError::AlreadyLinked {
                id: id.to_owned(),
                todo_id: existing,
            });
        }
        shelf.todo_id = Some(todo_id.to_owned());
        self.persist_one(&shelf)?;
        Ok(shelf)
    }

    pub fn unlink_todo(&self, id: &str) -> Result<Shelf, ShelfError> {
        self.check_group_id(id)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let mut shelf = self.group(id)?;
        if shelf.todo_id.is_none() {
            return Ok(shelf);
        }
        shelf.todo_id = None;
        self.persist_one(&shelf)?;
        Ok(shelf)
    }

    /// 按待办 id 解除所有分组上的关联。没有关联时不写盘。
    pub fn unlink_todo_everywhere(&self, todo_id: &str) -> Result<Vec<String>, ShelfError> {
        self.check_todo_id(todo_id)?;
        let _write = lock_mutex(&self.inner.write);
        self.ensure_usable()?;
        let changed: Vec<Shelf> = self
            .groups()?
            .into_iter()
            .filter(|group| group.todo_id.as_deref() == Some(todo_id))
            .map(|mut group| {
                group.todo_id = None;
                group
            })
            .collect();
        let ids = changed.iter().map(|group| group.id.clone()).collect();
        self.persist(&changed)?;
        Ok(ids)
    }

    /// 收到待办永久删除事件后解除关联。
    ///
    /// 目前唯一的事件是 [`TodoNotice::Purged`]。待办服务在永久删除写进产品规格之前不会发出它。
    pub fn apply_todo_notice(&self, notice: &TodoNotice) -> Result<(), ShelfError> {
        match notice {
            TodoNotice::Purged { item_id } => {
                self.unlink_todo_everywhere(item_id)?;
                Ok(())
            }
        }
    }

    /// 订阅待办服务的领域事件。收到永久删除后解除对应关联。
    ///
    /// 监听在后台线程进行，直到待办服务的订阅端关闭。重复调用会再订一份。
    pub fn watch_todo_notices(&self, todos: &TodoCommands) {
        self.watch_notices(todos.subscribe_notices());
    }

    pub(crate) fn watch_notices(&self, notices: Receiver<TodoNotice>) {
        let service = self.clone();
        thread::Builder::new()
            .name("lanwork-shelf-todo-notice".to_owned())
            .spawn(move || {
                while let Ok(notice) = notices.recv() {
                    let _ = service.apply_todo_notice(&notice);
                }
            })
            .expect("spawn shelf notice thread");
    }

    pub fn get(&self, id: &str) -> Result<Shelf, ShelfError> {
        self.check_group_id(id)?;
        self.ensure_usable()?;
        self.group(id)
    }

    pub fn list(&self) -> Result<Vec<Shelf>, ShelfError> {
        self.ensure_usable()?;
        let mut groups = self.groups()?;
        sort_groups(&mut groups);
        Ok(groups)
    }

    /// 在后台线程检查路径是否存在。
    ///
    /// 每条路径单独计时。超时、查询失败或路径无法规范化时，该条为 false。
    /// 结果与输入顺序一致。不修改用户文件。
    pub fn check_exists(&self, paths: &[String]) -> Vec<bool> {
        let check = lock_mutex(&self.inner.exists);
        let timeout = check.timeout;
        let probe = Arc::clone(&check.probe);
        drop(check);
        let mut workers = Vec::with_capacity(paths.len());
        for path in paths {
            let probe = Arc::clone(&probe);
            let path = path.clone();
            workers.push(thread::spawn(move || path_exists(&path, &probe, timeout)));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap_or(false))
            .collect()
    }

    fn ensure_usable(&self) -> Result<(), ShelfError> {
        if lock_mutex(&self.inner.cache).poisoned {
            Err(ShelfError::Unreadable)
        } else {
            Ok(())
        }
    }

    fn poison(&self) {
        let mut cache = lock_mutex(&self.inner.cache);
        cache.groups.clear();
        cache.skipped.clear();
        cache.poisoned = true;
    }

    fn group(&self, id: &str) -> Result<Shelf, ShelfError> {
        self.ensure_editable(id)?;
        self.lookup(id)?
            .ok_or_else(|| ShelfError::NotFound { id: id.to_owned() })
    }

    fn ensure_editable(&self, id: &str) -> Result<(), ShelfError> {
        let cache = lock_mutex(&self.inner.cache);
        if let Some(found) = cache.skipped.get(id) {
            return Err(unsupported(id, *found));
        }
        Ok(())
    }

    fn lookup(&self, id: &str) -> Result<Option<Shelf>, ShelfError> {
        let cache = lock_mutex(&self.inner.cache);
        Ok(cache.groups.get(id).cloned())
    }

    fn groups(&self) -> Result<Vec<Shelf>, ShelfError> {
        let cache = lock_mutex(&self.inner.cache);
        Ok(cache.groups.values().cloned().collect())
    }

    fn persist_one(&self, shelf: &Shelf) -> Result<(), ShelfError> {
        self.persist(std::slice::from_ref(shelf))
    }

    fn persist(&self, shelves: &[Shelf]) -> Result<(), ShelfError> {
        if shelves.is_empty() {
            return Ok(());
        }
        let preimages = {
            let cache = lock_mutex(&self.inner.cache);
            shelves
                .iter()
                .map(|shelf| cache.groups.get(&shelf.id).cloned())
                .collect::<Vec<_>>()
        };
        let fail_at = self.take_fail_at();
        let mut batch = self.inner.store.begin_batch()?;
        let mut replaced = Vec::new();
        for (index, shelf) in shelves.iter().enumerate() {
            if fail_at == Some(index) {
                drop(batch);
                return self.finish_failed(&preimages, &replaced, injected_error());
            }
            let file = ShelfFile::from_shelf(shelf);
            match batch.write_json(&DocumentId::Shelf(shelf.id.clone()), &file) {
                Ok(_) => replaced.push(shelf.id.clone()),
                Err(err) => {
                    drop(batch);
                    return self.finish_failed(&preimages, &replaced, err.into());
                }
            }
        }
        self.cache_upsert(shelves);
        batch.commit()?;
        Ok(())
    }

    fn finish_failed(
        &self,
        preimages: &[Option<Shelf>],
        replaced: &[String],
        err: ShelfError,
    ) -> Result<(), ShelfError> {
        if replaced.is_empty() {
            return Err(err);
        }
        match self.restore(preimages, replaced) {
            Ok(()) => Err(err),
            Err(_) => match self.reload() {
                Ok(()) => Err(err),
                Err(reload_err) => {
                    self.poison();
                    Err(reload_err)
                }
            },
        }
    }

    /// 把本批次已经替换的数据文件写回操作前内容。不提交，因此不发布变更。
    fn restore(&self, preimages: &[Option<Shelf>], replaced: &[String]) -> Result<(), ShelfError> {
        let mut batch = self.inner.store.begin_batch()?;
        for id in replaced {
            let previous = preimages.iter().find_map(|shelf| {
                shelf
                    .as_ref()
                    .filter(|shelf| shelf.id == *id)
                    .map(ShelfFile::from_shelf)
            });
            let write = if let Some(file) = previous {
                batch
                    .write_json(&DocumentId::Shelf(id.clone()), &file)
                    .map(|_| ())
            } else {
                batch.remove(&DocumentId::Shelf(id.clone())).map(|_| ())
            };
            if let Err(err) = write {
                drop(batch);
                return Err(err.into());
            }
        }
        drop(batch);
        Ok(())
    }

    fn cache_upsert(&self, shelves: &[Shelf]) {
        let mut cache = lock_mutex(&self.inner.cache);
        for shelf in shelves {
            cache.skipped.remove(&shelf.id);
            cache.groups.insert(shelf.id.clone(), shelf.clone());
        }
    }

    fn cache_remove(&self, id: &str) {
        let mut cache = lock_mutex(&self.inner.cache);
        cache.groups.remove(id);
        cache.skipped.remove(id);
    }

    fn reload(&self) -> Result<(), ShelfError> {
        reload_inner(&self.inner)
    }

    fn allocate_id(&self) -> Result<String, ShelfError> {
        for _ in 0..32 {
            let seq = self.inner.seq.fetch_add(1, Ordering::Relaxed);
            let millis = (self.inner.now)();
            let id = format!("s{millis:x}{seq:x}");
            let taken = {
                let cache = lock_mutex(&self.inner.cache);
                cache.groups.contains_key(&id) || cache.skipped.contains_key(&id)
            };
            if taken {
                continue;
            }
            self.check_group_id(&id)?;
            return Ok(id);
        }
        Err(ShelfError::AllocateId)
    }

    fn take_fail_at(&self) -> Option<usize> {
        #[cfg(test)]
        {
            return lock_mutex(&self.inner.fail_at).take();
        }
        #[cfg(not(test))]
        {
            None
        }
    }

    #[cfg(test)]
    fn fail_next_write_at(&self, index: usize) {
        *lock_mutex(&self.inner.fail_at) = Some(index);
    }

    #[cfg(test)]
    fn set_exists_check<F>(&self, timeout: Duration, probe: F)
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        *lock_mutex(&self.inner.exists) = ExistsCheck {
            timeout,
            probe: Arc::new(probe),
        };
    }
}

struct PreparedRef {
    stored: String,
    key: String,
    name: String,
    folder: bool,
}

fn prepare_incoming(incoming: &[IncomingRef]) -> Result<Vec<PreparedRef>, ShelfError> {
    let mut prepared = Vec::new();
    let mut seen = Vec::new();
    for item in incoming {
        let normalized = normalize_path(&item.path)?;
        if seen.iter().any(|key| key == &normalized.key) {
            continue;
        }
        seen.push(normalized.key.clone());
        prepared.push(PreparedRef {
            stored: normalized.stored,
            key: normalized.key,
            name: normalized.name,
            folder: item.folder,
        });
    }
    Ok(prepared)
}

fn path_exists(
    path: &str,
    probe: &Arc<dyn Fn(&str) -> bool + Send + Sync>,
    timeout: Duration,
) -> bool {
    let Ok(normalized) = normalize_path(path) else {
        return false;
    };
    let (sender, receiver) = mpsc::channel();
    let probe = Arc::clone(probe);
    let stored = normalized.stored;
    let _ = thread::Builder::new()
        .name("lanwork-shelf-exists".to_owned())
        .spawn(move || {
            let found = probe(&stored);
            let _ = sender.send(found);
        });
    receiver.recv_timeout(timeout).unwrap_or(false)
}

fn probe_filesystem(path: &str) -> bool {
    std::path::Path::new(path).try_exists().unwrap_or(false)
}

impl ShelfRef {
    fn path_key(&self) -> String {
        super::path::fold_key(&self.path)
    }
}

fn next_order(values: impl Iterator<Item = i64>) -> Result<i64, ShelfError> {
    values.max().map_or(Ok(0), |max| {
        max.checked_add(1).ok_or(ShelfError::InvalidOrder)
    })
}

fn reorder(mut groups: Vec<Shelf>, ids: &[String]) -> Result<Vec<Shelf>, ShelfError> {
    if groups.len() != ids.len() {
        return Err(ShelfError::InvalidOrder);
    }
    let mut ordered = Vec::with_capacity(groups.len());
    for id in ids {
        let Some(pos) = groups.iter().position(|group| group.id == *id) else {
            return Err(ShelfError::InvalidOrder);
        };
        ordered.push(groups.remove(pos));
    }
    Ok(ordered)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

impl crate::storage::MemoryReload for Inner {
    fn reload_memory(&self) -> Result<(), String> {
        reload_inner(self).map_err(|err| err.to_string())
    }
}

fn reload_inner(inner: &Inner) -> Result<(), ShelfError> {
    let loaded = inner
        .store
        .read_collection::<ShelfFile>(CollectionKind::Shelves)?;
    let mut cache = Cache::default();
    for file in loaded.files {
        match file.value.into_loaded(file.id.clone()) {
            LoadedShelf::Group(shelf) => {
                cache.groups.insert(shelf.id.clone(), shelf);
            }
            LoadedShelf::Unsupported { found } => {
                cache.skipped.insert(file.id, found);
            }
        }
    }
    *lock_mutex(&inner.cache) = cache;
    Ok(())
}

fn injected_error() -> ShelfError {
    ShelfError::Store(crate::storage::Error::io(
        crate::storage::IoAction::Replace,
        "fault",
        std::io::Error::other("injected"),
    ))
}

#[cfg(test)]
mod tests;
