//! 计时前后记一笔机器负载。CPU 高或者有别的编译进程时，调用方应重跑计时。

use serde::Serialize;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::GetSystemTimes;

use crate::host::{FileIdxError, filetime_u64};

const SKEW_BUSY_PERCENT: f64 = 70.0;

#[derive(Debug, Clone, Serialize)]
pub struct LoadSnapshot {
    pub interval_ms: u64,
    pub busy_percent: f64,
    pub rustc: u32,
    pub link: u32,
    pub cl: u32,
    pub cargo: u32,
    pub skewed: bool,
    pub reason: String,
}

pub fn sample_load(interval_ms: u64) -> Result<LoadSnapshot, FileIdxError> {
    let start = system_times()?;
    std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    let end = system_times()?;
    let counts = process_counts()?;
    let busy = busy_percent(start, end);
    let compilers = counts.rustc + counts.link + counts.cl;
    let mut reasons = Vec::new();
    if busy >= SKEW_BUSY_PERCENT {
        reasons.push(format!("系统 CPU {busy:.1}% >= {SKEW_BUSY_PERCENT:.0}%"));
    }
    if compilers > 0 {
        reasons.push(format!(
            "同时有编译进程 rustc={} link={} cl={}",
            counts.rustc, counts.link, counts.cl
        ));
    }
    let skewed = !reasons.is_empty();
    let reason = if skewed {
        reasons.join("；")
    } else {
        "未发现明显干扰".to_string()
    };
    Ok(LoadSnapshot {
        interval_ms,
        busy_percent: busy,
        rustc: counts.rustc,
        link: counts.link,
        cl: counts.cl,
        cargo: counts.cargo,
        skewed,
        reason,
    })
}

struct Times {
    idle: u64,
    kernel: u64,
    user: u64,
}

fn system_times() -> Result<Times, FileIdxError> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: 三个输出指针都指向本函数的局部变量。
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }
        .map_err(|err| FileIdxError::new(format!("GetSystemTimes 失败：{err}")))?;
    Ok(Times {
        idle: filetime_u64(idle.dwHighDateTime, idle.dwLowDateTime),
        kernel: filetime_u64(kernel.dwHighDateTime, kernel.dwLowDateTime),
        user: filetime_u64(user.dwHighDateTime, user.dwLowDateTime),
    })
}

fn busy_percent(start: Times, end: Times) -> f64 {
    let idle = end.idle.saturating_sub(start.idle);
    let kernel = end.kernel.saturating_sub(start.kernel);
    let user = end.user.saturating_sub(start.user);
    let total = kernel.saturating_add(user);
    if total == 0 {
        return 0.0;
    }
    let busy = kernel.saturating_sub(idle).saturating_add(user);
    (busy as f64) * 100.0 / (total as f64)
}

struct Counts {
    rustc: u32,
    link: u32,
    cl: u32,
    cargo: u32,
}

fn process_counts() -> Result<Counts, FileIdxError> {
    // SAFETY: 快照句柄在函数结束前关闭。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|err| FileIdxError::new(format!("枚举进程失败：{err}")))?;
    let mut counts = Counts {
        rustc: 0,
        link: 0,
        cl: 0,
        cargo: 0,
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..PROCESSENTRY32W::default()
    };
    let mut has = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while has {
        let name = exe_name(&entry.szExeFile);
        match name.as_str() {
            "rustc.exe" => counts.rustc += 1,
            "link.exe" => counts.link += 1,
            "cl.exe" => counts.cl += 1,
            "cargo.exe" => counts.cargo += 1,
            _ => {}
        }
        has = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    unsafe { windows::Win32::Foundation::CloseHandle(snapshot) }
        .map_err(|err| FileIdxError::new(format!("关闭进程快照失败：{err}")))?;
    Ok(counts)
}

fn exe_name(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).to_ascii_lowercase()
}
