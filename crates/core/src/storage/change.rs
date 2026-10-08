//! 进程内变更消息。窗口和索引在应用层订阅；本模块不投递 Win32 窗口消息。

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};

use super::lock_mutex;

/// 写盘成功后的通知。`revision` 只在便签保存时有值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityChanged {
    pub kind: EntityKind,
    pub id: String,
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Config,
    Todo,
    Note,
    Shelf,
    GithubWatchlist,
    GithubCache,
}

/// 随一次成功写入带上的附加信息。存储层不解释 revision 是否与正文一致。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeMeta {
    pub revision: Option<u64>,
}

#[derive(Debug)]
pub struct ChangeBus {
    subscribers: Mutex<Vec<Sender<EntityChanged>>>,
}

impl ChangeBus {
    pub(crate) fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub fn subscribe(&self) -> Receiver<EntityChanged> {
        let (sender, receiver) = mpsc::channel();
        lock_mutex(&self.subscribers).push(sender);
        receiver
    }

    pub(crate) fn publish(&self, event: EntityChanged) {
        let mut subscribers = lock_mutex(&self.subscribers);
        subscribers.retain(|sender| sender.send(event.clone()).is_ok());
    }
}

impl Default for ChangeBus {
    fn default() -> Self {
        Self::new()
    }
}
