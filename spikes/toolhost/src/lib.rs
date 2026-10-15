//! 自定义工具宿主的技术验证。不进 `lanwork` 的依赖，也没有产品界面。
//!
//! 窗口在 Windows 11 桌面上长什么样，不在自动测试里。内存和空闲 CPU 的数字上限仍待定。

mod guide;
mod host;
mod http;
mod id;
mod manifest;
mod measure;
mod secret;
mod shell;

pub use guide::{pomodoro, quota, write_example};
pub use host::{Durations, Options, Session, TodoItem};
pub use http::{RustlsTransport, install_ring};
pub use manifest::{Origin, materialize};
pub use measure::{MeasureReport, idle_cpu_percent, memory_note, process_memory};
pub use secret::{MemorySecretStore, SecretStore, spike_target_name};
pub use shell::{attach_for_test, click, green_pixels, platform_lock, refresh, run_window};

#[cfg(test)]
mod tests;
