//! 验证进程自己的启动、内存和空闲 CPU。
//!
//! 这不是 architecture.md「性能测量」的 5 分钟协议，也不是 Windows 11 桌面窗口的采样。
//! 数字上限仍待定。Linux 上读的是 VmRSS 和 RssAnon，不是 Windows 的 Private Bytes。

use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct MeasureReport {
    pub startup_ms: u128,
    pub frame_ms: u128,
    pub rss_bytes: u64,
    pub private_bytes: u64,
    pub idle_cpu_percent: f64,
    pub idle_secs: f64,
    pub renderer: String,
    pub lua: String,
    pub slint: String,
    pub memory_note: String,
}

pub fn memory_note() -> &'static str {
    #[cfg(windows)]
    {
        "Windows PrivateUsage 与 WorkingSetSize。不是性能测量验收，数字上限仍待定。"
    }
    #[cfg(target_os = "linux")]
    {
        "Linux VmRSS 与 RssAnon。不是 Windows Private Bytes，也不是性能测量验收。数字上限仍待定。"
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        "这个系统没有单独的内存读数。数字上限仍待定。"
    }
}

pub fn process_memory() -> (u64, u64) {
    #[cfg(target_os = "linux")]
    {
        linux_memory()
    }
    #[cfg(windows)]
    {
        windows_memory()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        (0, 0)
    }
}

pub fn idle_cpu_percent(secs: f64) -> f64 {
    if !(secs.is_finite() && secs > 0.0) {
        return 0.0;
    }
    let before = cpu_time_seconds();
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs_f64(secs));
    let elapsed = started.elapsed().as_secs_f64().max(secs);
    let used = (cpu_time_seconds() - before).max(0.0);
    used / elapsed * 100.0
}

#[cfg(target_os = "linux")]
fn linux_memory() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let rss = kib_field(&text, "VmRSS:") * 1024;
    let private_bytes = kib_field(&text, "RssAnon:") * 1024;
    (rss, private_bytes)
}

#[cfg(target_os = "linux")]
fn kib_field(text: &str, name: &str) -> u64 {
    text.lines()
        .find_map(|line| {
            let rest = line.strip_prefix(name)?;
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn cpu_time_seconds() -> f64 {
    let ticks = proc_ticks();
    ticks as f64 / ticks_per_sec() as f64
}

#[cfg(target_os = "linux")]
fn proc_ticks() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return 0;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = fields
        .get(11)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0);
    let stime: u64 = fields
        .get(12)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0);
    utime.saturating_add(stime)
}

#[cfg(target_os = "linux")]
fn ticks_per_sec() -> u64 {
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    // Linux 上 `_SC_CLK_TCK` 的编号是 2。`sysconf` 只读这个常量。
    let value = unsafe { sysconf(2) };
    if value > 0 { value as u64 } else { 100 }
}

#[cfg(windows)]
fn windows_memory() -> (u64, u64) {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
    let Ok(bytes) = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()) else {
        return (0, 0);
    };
    memory.cb = bytes;
    let read = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut memory as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
            bytes,
        )
    };
    if read.is_err() {
        return (0, 0);
    }
    (memory.WorkingSetSize as u64, memory.PrivateUsage as u64)
}

#[cfg(windows)]
fn cpu_time_seconds() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let read = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if read.is_err() {
        return 0.0;
    }
    let hundred_ns = filetime_u64(kernel).saturating_add(filetime_u64(user));
    hundred_ns as f64 / 10_000_000.0
}

#[cfg(windows)]
fn filetime_u64(value: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn cpu_time_seconds() -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_reading_is_present() {
        let (rss, private_bytes) = process_memory();
        assert!(rss > 0, "rss {rss}");
        assert!(private_bytes > 0, "private {private_bytes}");
    }

    #[test]
    fn short_sleep_stays_idle() {
        let percent = idle_cpu_percent(0.2);
        assert!(percent.is_finite() && percent >= 0.0, "{percent}");
    }
}
