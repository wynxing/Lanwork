//! 薄命令。先校验标识和空名称，再调用 [`super::ShelfService`] 里的规则。
//!
//! 收纳页以后走这一层。这里不注册拖放，也不访问用户文件。

use crate::storage::Store;
use crate::todos::TodoCommands;

use super::error::ShelfError;
use super::model::{AddRefsOutcome, IncomingRef, IncomingWeb, Shelf};
use super::service::ShelfService;
use crate::notes::{FloatDock, FloatGeometry, FloatStackSlot};

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

    /// 把网址加入分组。规则见 [`ShelfService::add_web_refs`]。
    pub fn add_web_refs(
        &self,
        group_id: Option<&str>,
        incoming: &[IncomingWeb],
    ) -> Result<AddRefsOutcome, ShelfError> {
        if let Some(id) = group_id {
            self.service.check_group_id(id)?;
        }
        for item in incoming {
            if item.url.is_empty() || item.url.contains('\0') {
                return Err(ShelfError::InvalidUrl {
                    url: item.url.clone(),
                });
            }
        }
        self.service.add_web_refs(group_id, incoming)
    }

    /// 把引用从一组挪到另一组。规则见 [`ShelfService::move_refs`]。
    pub fn move_refs(
        &self,
        from_id: &str,
        to_id: &str,
        paths: &[String],
        urls: &[String],
    ) -> Result<super::model::MoveRefsOutcome, ShelfError> {
        self.service.check_group_id(from_id)?;
        self.service.check_group_id(to_id)?;
        for path in paths {
            if path.is_empty() {
                return Err(ShelfError::InvalidPath {
                    path: path.to_owned(),
                });
            }
        }
        for url in urls {
            if url.is_empty() {
                return Err(ShelfError::InvalidUrl { url: url.clone() });
            }
        }
        self.service.move_refs(from_id, to_id, paths, urls)
    }

    /// 从分组去掉一条网页引用。
    pub fn remove_web_ref(&self, id: &str, url: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        if url.is_empty() {
            return Err(ShelfError::InvalidUrl {
                url: url.to_owned(),
            });
        }
        self.service.remove_web_ref(id, url)
    }

    pub fn link_todo(&self, id: &str, todo_id: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        self.service.check_todo_id(todo_id)?;
        self.service.link_todo(id, todo_id)
    }

    /// 记下这个分组的悬浮窗的展开位置、大小和贴边。
    pub fn set_float(
        &self,
        id: &str,
        geometry: FloatGeometry,
        dock: Option<FloatDock>,
    ) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        self.service.set_float(id, geometry, dock)
    }

    /// 记下这个分组属于哪一叠。见 [`ShelfService::set_stack`]。
    pub fn set_stack(&self, id: &str, slot: Option<FloatStackSlot>) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        self.service.set_stack(id, slot)
    }

    /// 解除这个分组对一条待办的关联，其余关联不动。
    pub fn unlink_todo(&self, id: &str, todo_id: &str) -> Result<Shelf, ShelfError> {
        self.service.check_group_id(id)?;
        self.service.check_todo_id(todo_id)?;
        self.service.unlink_todo(id, todo_id)
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
