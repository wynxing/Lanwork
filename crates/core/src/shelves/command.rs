//! 薄命令。先校验标识和空名称，再调用 [`super::ShelfService`] 里的规则。
//!
//! 收纳页以后走这一层。这里不注册拖放，也不访问用户文件。

use crate::storage::Store;
use crate::todos::TodoCommands;

use super::error::ShelfError;
use super::model::{AddRefsOutcome, IncomingRef, Shelf};
use super::service::ShelfService;

/// 界面调用的收纳命令。克隆与对应的 [`ShelfService`] 共享状态。
#[derive(Clone, Debug)]
pub struct ShelfCommands {
    service: ShelfService,
}

impl ShelfCommands {
    pub fn open(store: Store) -> Result<Self, ShelfError> {
        Ok(Self {
            service: ShelfService::open(store)?,
        })
    }

    pub fn open_at<F>(store: Store, now: F) -> Result<Self, ShelfError>
    where
        F: Fn() -> i64 + Send + Sync + 'static,
    {
        Ok(Self {
            service: ShelfService::open_at(store, now)?,
        })
    }

    pub fn service(&self) -> &ShelfService {
        &self.service
    }

    pub fn create_group(&self, name: &str) -> Result<Shelf, ShelfError> {
        if super::model::normalize_name(name).is_none() {
            return Err(ShelfError::BlankName);
        }
        self.service.create_group(name)
    }

    pub fn rename_group(&self, id: &str, name: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        if super::model::normalize_name(name).is_none() {
            return Err(ShelfError::BlankName);
        }
        self.service.rename_group(id, name)
    }

    pub fn delete_group(&self, id: &str) -> Result<(), ShelfError> {
        self.service.check_group_id(id)?;
        self.service.delete_group(id)
    }

    pub fn reorder_groups(&self, ids: &[String]) -> Result<(), ShelfError> {
        for id in ids {
            self.service.check_group_id(id)?;
        }
        self.service.reorder_groups(ids)
    }

    pub fn add_refs(
        &self,
        group_id: Option<&str>,
        incoming: &[IncomingRef],
    ) -> Result<AddRefsOutcome, ShelfError> {
        if let Some(id) = group_id {
            self.service.check_group_id(id)?;
        }
        for item in incoming {
            if item.path.is_empty() || item.path.contains('\0') {
                return Err(ShelfError::InvalidPath {
                    path: item.path.clone(),
                });
            }
        }
        self.service.add_refs(group_id, incoming)
    }

    pub fn remove_ref(&self, id: &str, path: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        if path.is_empty() {
            return Err(ShelfError::InvalidPath {
                path: path.to_owned(),
            });
        }
        self.service.remove_ref(id, path)
    }

    pub fn link_todo(&self, id: &str, todo_id: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        self.service.check_todo_id(todo_id)?;
        self.service.link_todo(id, todo_id)
    }

    pub fn unlink_todo(&self, id: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        self.service.unlink_todo(id)
    }

    pub fn unlink_todo_everywhere(&self, todo_id: &str) -> Result<Vec<String>, ShelfError> {
        self.service.check_todo_id(todo_id)?;
        self.service.unlink_todo_everywhere(todo_id)
    }

    pub fn apply_todo_notice(&self, notice: &crate::todos::TodoNotice) -> Result<(), ShelfError> {
        self.service.apply_todo_notice(notice)
    }

    pub fn watch_todo_notices(&self, todos: &TodoCommands) {
        self.service.watch_todo_notices(todos);
    }

    pub fn get(&self, id: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        self.service.get(id)
    }

    pub fn list(&self) -> Result<Vec<Shelf>, ShelfError> {
        self.service.list()
    }

    pub fn check_exists(&self, paths: &[String]) -> Vec<bool> {
        self.service.check_exists(paths)
    }
}
