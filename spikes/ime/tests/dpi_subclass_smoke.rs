//! Windows 上启动 spike，确认日志里出现「DPI 子类已装上」。
//!
//! `SLINT_BACKEND=software` 仍走 winit 窗口，只是不用 OpenGL。人工复测不要设这个变量。

#![cfg(windows)]

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn smoke_log_reports_subclass_installed() {
    let log_path = std::env::temp_dir().join("lanwork-ime-spike.log");
    let _ = std::fs::remove_file(&log_path);

    let mut child = Command::new(env!("CARGO_BIN_EXE_ime"))
        .env("LANWORK_IME_SPIKE_SMOKE", "1")
        .env("SLINT_BACKEND", "software")
        .spawn()
        .expect("启动 ime");

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("查询子进程") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            panic!("ime 在 20 秒内没有退出");
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(status.success(), "退出码 {status}，日志：\n{log}");
    assert!(
        log.contains("DPI 子类已装上"),
        "日志里没有装上子类：\n{log}"
    );
    assert!(!log.contains("DPI 子类未装上"), "子类安装失败：\n{log}");
}
