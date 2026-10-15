//! 工具悬浮窗的屏幕验证。宿主、沙箱和安全测试在 `lanwork-toolhost`。
//!
//! 这一扇窗口不进产品界面。Windows 11 桌面上它看起来怎么样，不在自动测试里。
//! 内存和空闲 CPU 的数字上限仍待定。

mod measure;
mod shell;

pub use lanwork_toolhost::{
    Durations, MemorySecretStore, Options, Origin, Session, TodoItem, write_example,
};
pub use measure::{MeasureReport, idle_cpu_percent, memory_note, process_memory};
pub use shell::{attach_for_test, click, green_pixels, platform_lock, refresh, run_window};

#[cfg(test)]
mod tests;
