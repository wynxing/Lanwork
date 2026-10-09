//! 进程内变更消息。窗口和索引在应用层订阅；本模块不投递 Win32 窗口消息。

#[cfg(test)]
use std::sync::Arc;
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
    /// 测试在消息已经入队、`publish` 返回之前读内存。正式构建没有这一步。
    #[cfg(test)]
    probe: Mutex<Option<Probe>>,
}

#[cfg(test)]
#[derive(Clone)]
struct Probe(Arc<dyn Fn() + Send + Sync>);

#[cfg(test)]
impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Probe")
    }
}

impl ChangeBus {
    pub(crate) fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            #[cfg(test)]
            probe: Mutex::new(None),
        }
    }

    pub fn subscribe(&self) -> Receiver<EntityChanged> {
        let (sender, receiver) = mpsc::channel();
        lock_mutex(&self.subscribers).push(sender);
        receiver
    }

    #[cfg(test)]
    pub(crate) fn set_probe(&self, probe: Option<Arc<dyn Fn() + Send + Sync>>) {
        *lock_mutex(&self.probe) = probe.map(Probe);
    }

    pub(crate) fn publish(&self, event: EntityChanged) {
        {
            let mut subscribers = lock_mutex(&self.subscribers);
            subscribers.retain(|sender| sender.send(event.clone()).is_ok());
        }
        #[cfg(test)]
        let probe = lock_mutex(&self.probe).clone();
        #[cfg(test)]
        if let Some(probe) = probe {
            probe.0();
        }
    }
}

impl Default for ChangeBus {
    fn default() -> Self {
        Self::new()
    }
}
