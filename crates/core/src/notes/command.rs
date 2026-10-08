//! 薄命令。先校验标识，再调用 [`super::NoteService`] 里的规则。
//!
//! 便签页、悬浮窗和快速收集以后都走这一层。这里不解析搜索框，也不做去抖。

use crate::storage::Store;

use super::error::NoteError;
use super::model::{Note, NoteInput, TimestampMillis};
use super::service::NoteService;

/// 界面调用的便签命令。克隆与对应的 [`NoteService`] 共享状态。
#[derive(Clone, Debug)]
pub struct NoteCommands {
    service: NoteService,
}

impl NoteCommands {
    pub fn open(store: Store) -> Result<Self, NoteError> {
        Ok(Self {
            service: NoteService::open(store)?,
        })
    }

    pub fn open_at<F>(store: Store, now: F) -> Result<Self, NoteError>
    where
        F: Fn() -> TimestampMillis + Send + Sync + 'static,
    {
        Ok(Self {
            service: NoteService::open_at(store, now)?,
        })
    }

    pub fn service(&self) -> &NoteService {
        &self.service
    }

    pub fn create(&self, input: &NoteInput) -> Result<Note, NoteError> {
        self.service.create(input)
    }

    pub fn save(&self, id: &str, revision: u64, input: &NoteInput) -> Result<Note, NoteError> {
        self.service.check_id(id)?;
        self.service.save(id, revision, input)
    }

    pub fn set_pinned(&self, id: &str, revision: u64, pinned: bool) -> Result<Note, NoteError> {
        self.service.check_id(id)?;
        self.service.set_pinned(id, revision, pinned)
    }

    pub fn set_tags(&self, id: &str, revision: u64, tags: &[String]) -> Result<Note, NoteError> {
        self.service.check_id(id)?;
        self.service.set_tags(id, revision, tags)
    }

    /// 软删除。不提供永久删除，界面入口等 #9 第 10 项。
    pub fn soft_delete(&self, id: &str, revision: u64) -> Result<Note, NoteError> {
        self.service.check_id(id)?;
        self.service.soft_delete(id, revision)
    }

    pub fn restore(&self, id: &str, revision: u64) -> Result<Note, NoteError> {
        self.service.check_id(id)?;
        self.service.restore(id, revision)
    }

    pub fn get(&self, id: &str) -> Result<Note, NoteError> {
        self.service.check_id(id)?;
        self.service.get(id)
    }

    pub fn list(&self) -> Vec<Note> {
        self.service.list()
    }

    pub fn deleted(&self) -> Vec<Note> {
        self.service.deleted()
    }

    pub fn filter_by_tag(&self, tag: &str) -> Vec<Note> {
        self.service.filter_by_tag(tag)
    }
}
