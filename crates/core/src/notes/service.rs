//! 便签服务。业务规则在这里：修订号、标签、置顶顺序、软删除。
//!
//! 内存里的副本只在写盘成功之后、发布变更之前更新。写盘失败或修订号冲突都不改磁盘，也不改这份副本。
//! 订阅者读到 [`crate::storage::EntityChanged`] 时，[`NoteService::list`] 已是新内容。
//! 那个回调里不要再进入本服务的写入：写入锁还被这次保存持有。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::storage::{ChangeMeta, CollectionKind, DocumentId, Store, lock_mutex};

use super::error::NoteError;
use super::model::{
    Note, NoteFile, NoteInput, SkipReason, TimestampMillis, normalize_query_tag, normalize_tags,
    sort_for_list,
};

#[derive(Default)]
struct Cache {
    notes: BTreeMap<String, Note>,
    skipped: BTreeMap<String, SkipReason>,
}

struct Inner {
    store: Store,
    cache: Mutex<Cache>,
    write: Mutex<()>,
    now: Arc<dyn Fn() -> TimestampMillis + Send + Sync>,
    seq: AtomicU64,
}

/// 便签的进程内服务。克隆共享同一份缓存和写锁。
///
/// 一个数据目录在进程里只打开一个服务。界面应通过 [`super::NoteCommands`] 调用。
#[derive(Clone)]
pub struct NoteService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for NoteService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteService")
            .field("store", &self.inner.store)
            .finish_non_exhaustive()
    }
}

impl NoteService {
    /// 从 `notes/` 加载。损坏的文件由存储层隔离，其余便签照常读入。
    pub fn open(store: Store) -> Result<Self, NoteError> {
        Self::open_at(store, || TimestampMillis::from_system(SystemTime::now()))
    }

    /// `now` 提供写入用的时间。测试用它固定时间戳。
    pub fn open_at<F>(store: Store, now: F) -> Result<Self, NoteError>
    where
        F: Fn() -> TimestampMillis + Send + Sync + 'static,
    {
        let now: Arc<dyn Fn() -> TimestampMillis + Send + Sync> = Arc::new(now);
        let service = Self {
            inner: Arc::new(Inner {
                store,
                cache: Mutex::new(Cache::default()),
                write: Mutex::new(()),
                now,
                seq: AtomicU64::new(1),
            }),
        };
        service.reload_cache()?;
        Ok(service)
    }

    pub(crate) fn check_id(&self, id: &str) -> Result<(), NoteError> {
        self.inner
            .store
            .document_path(&DocumentId::Note(id.to_owned()))?;
        Ok(())
    }

    /// 新建一篇。`revision` 从 1 开始，创建时间与更新时间相同。
    pub fn create(&self, input: &NoteInput) -> Result<Note, NoteError> {
        let _write = lock_mutex(&self.inner.write);
        let id = self.allocate_id()?;
        let now = (self.inner.now)();
        let note = Note {
            id,
            title: input.title.clone(),
            body: input.body.clone(),
            tags: normalize_tags(&input.tags),
            pinned: input.pinned,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            revision: 1,
        };
        self.persist(note)
    }

    /// 用加载时的 `revision` 保存标题、正文、标签和置顶。
    ///
    /// 不修改 `deleted_at` 和 `created_at`。成功后 `revision` 加一。
    /// 与磁盘不一致时返回 [`NoteError::Conflict`]，不写盘。
    pub fn save(&self, id: &str, revision: u64, input: &NoteInput) -> Result<Note, NoteError> {
        let tags = normalize_tags(&input.tags);
        let title = input.title.clone();
        let body = input.body.clone();
        let pinned = input.pinned;
        self.mutate(id, revision, move |current, _now| {
            let mut next = current.clone();
            next.title = title;
            next.body = body;
            next.tags = tags;
            next.pinned = pinned;
            Ok(next)
        })
    }

    /// 只改置顶。其他字段保持不变，`revision` 仍加一。
    pub fn set_pinned(&self, id: &str, revision: u64, pinned: bool) -> Result<Note, NoteError> {
        self.mutate(id, revision, move |current, _now| {
            let mut next = current.clone();
            next.pinned = pinned;
            Ok(next)
        })
    }

    /// 替换标签列表。规则与保存时相同。
    pub fn set_tags(&self, id: &str, revision: u64, tags: &[String]) -> Result<Note, NoteError> {
        let tags = normalize_tags(tags);
        self.mutate(id, revision, move |current, _now| {
            let mut next = current.clone();
            next.tags = tags;
            Ok(next)
        })
    }

    /// 软删除：写入 `deletedAt`，文件留在 `notes/`。
    ///
    /// 界面入口以及删除后是否对用户显示为回收站，等 product.md 写入 #9 第 10 项。
    /// 搜索规格排除了回收站中的便签，因此 [`Self::list`] 和 [`Self::filter_by_tag`] 不返回它们。
    /// 关闭悬浮窗不是这个动作。
    pub fn soft_delete(&self, id: &str, revision: u64) -> Result<Note, NoteError> {
        self.mutate(id, revision, |current, now| {
            if current.deleted_at.is_some() {
                return Err(NoteError::AlreadyDeleted {
                    id: current.id.clone(),
                });
            }
            let mut next = current.clone();
            next.deleted_at = Some(now);
            Ok(next)
        })
    }

    /// 清除 `deletedAt`。便签回到 [`Self::list`]。
    pub fn restore(&self, id: &str, revision: u64) -> Result<Note, NoteError> {
        self.mutate(id, revision, |current, _now| {
            if current.deleted_at.is_none() {
                return Err(NoteError::NotDeleted {
                    id: current.id.clone(),
                });
            }
            let mut next = current.clone();
            next.deleted_at = None;
            Ok(next)
        })
    }

    pub fn get(&self, id: &str) -> Result<Note, NoteError> {
        self.check_id(id)?;
        let cache = lock_mutex(&self.inner.cache);
        if let Some(reason) = cache.skipped.get(id) {
            return Err(reason.error(id));
        }
        cache
            .notes
            .get(id)
            .cloned()
            .ok_or_else(|| NoteError::NotFound { id: id.to_owned() })
    }

    /// 未软删除的便签。置顶在前。
    pub fn list(&self) -> Vec<Note> {
        self.collect(|note| !note.is_deleted())
    }

    /// 已软删除的便签。顺序与 [`Self::list`] 相同。界面是否展示等 #9 第 10 项。
    pub fn deleted(&self) -> Vec<Note> {
        self.collect(Note::is_deleted)
    }

    /// 按规范化之后的整段标签筛选未软删除的便签。不是子串搜索。
    ///
    /// 查询标签会去掉首尾空白。只有空白时返回空列表。
    pub fn filter_by_tag(&self, tag: &str) -> Vec<Note> {
        let Some(tag) = normalize_query_tag(tag) else {
            return Vec::new();
        };
        self.collect(|note| !note.is_deleted() && note.tags.iter().any(|existing| existing == tag))
    }

    fn collect(&self, include: impl Fn(&Note) -> bool) -> Vec<Note> {
        let cache = lock_mutex(&self.inner.cache);
        let mut notes: Vec<_> = cache
            .notes
            .values()
            .filter(|note| include(note))
            .cloned()
            .collect();
        sort_for_list(&mut notes);
        notes
    }

    fn mutate(
        &self,
        id: &str,
        expected: u64,
        edit: impl FnOnce(&Note, TimestampMillis) -> Result<Note, NoteError>,
    ) -> Result<Note, NoteError> {
        self.check_id(id)?;
        let _write = lock_mutex(&self.inner.write);
        let current = {
            let cache = lock_mutex(&self.inner.cache);
            if let Some(reason) = cache.skipped.get(id) {
                return Err(reason.error(id));
            }
            cache
                .notes
                .get(id)
                .cloned()
                .ok_or_else(|| NoteError::NotFound { id: id.to_owned() })?
        };
        if current.revision != expected {
            return Err(NoteError::Conflict {
                id: id.to_owned(),
                expected,
                actual: current.revision,
            });
        }
        let now = (self.inner.now)();
        let mut next = edit(&current, now)?;
        next.id = current.id;
        next.created_at = current.created_at;
        next.revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| NoteError::RevisionOverflow { id: id.to_owned() })?;
        next.updated_at = now;
        self.persist(next)
    }

    fn persist(&self, note: Note) -> Result<Note, NoteError> {
        let file = NoteFile::from_note(&note);
        let doc = DocumentId::Note(note.id.clone());
        let revision = note.revision;
        self.inner.store.write_with_before_publish(
            &doc,
            &file,
            ChangeMeta {
                revision: Some(revision),
            },
            || {
                let mut cache = lock_mutex(&self.inner.cache);
                cache.skipped.remove(&note.id);
                cache.notes.insert(note.id.clone(), note.clone());
            },
        )?;
        Ok(note)
    }

    fn allocate_id(&self) -> Result<String, NoteError> {
        for _ in 0..32 {
            let seq = self.inner.seq.fetch_add(1, Ordering::Relaxed);
            let millis = (self.inner.now)().as_millis();
            let id = format!("n{millis:x}{seq:x}");
            let taken = {
                let cache = lock_mutex(&self.inner.cache);
                cache.notes.contains_key(&id) || cache.skipped.contains_key(&id)
            };
            if taken {
                continue;
            }
            self.check_id(&id)?;
            return Ok(id);
        }
        Err(NoteError::AllocateId)
    }

    fn reload_cache(&self) -> Result<(), NoteError> {
        let _write = lock_mutex(&self.inner.write);
        let loaded = self
            .inner
            .store
            .read_collection::<NoteFile>(CollectionKind::Notes)?;
        let mut cache = Cache::default();
        for file in loaded.files {
            match file.value.into_note(file.id.clone()) {
                Ok(note) => {
                    cache.notes.insert(note.id.clone(), note);
                }
                Err(reason) => {
                    cache.skipped.insert(file.id, reason);
                }
            }
        }
        *lock_mutex(&self.inner.cache) = cache;
        Ok(())
    }
}
