//! 自定义工具的宿主。没有界面。
//!
//! 沙箱、清单和权限服务在这里。悬浮窗的软件渲染验证仍在 `spikes/toolhost`。
//! 技术验证「工具宿主」还没有通过：Windows 11 桌面上的窗口、空闲采样和凭据管理器界面未测。
//! 没有通过记录之前，不从这里画产品界面。
//!
//! 这个 crate 不打开数据目录，也不读写 MayDolist。待办、安静时段和密钥库由调用方注入。
//! 内存和 CPU 的数字上限仍待定。

mod guide;
mod host;
mod http;
mod id;
mod manifest;
mod pattern;
mod secret;

pub mod probe;

pub use guide::{pomodoro, quota, write_example};
pub use host::{Durations, Notice, Notification, Options, Session, TodoItem, WidgetViewKind};
pub use http::{
    HttpResponse, HttpTransport, Outgoing, RustlsTransport, TransportError, install_ring,
};
pub use manifest::{Manifest, Origin, materialize};
pub use secret::{MemorySecretStore, SecretStore, spike_target_name};

#[cfg(windows)]
pub use secret::WindowsCredentialStore;

#[cfg(test)]
mod tests;
