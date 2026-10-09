//! 快捷方式交给 `ShellExecuteExW`，商店应用用 `shell:AppsFolder\<AUMID>`。
//!
//! 不创建 Lanwork 自己的窗口。`nShow` 用 `SW_SHOWNORMAL` 的数值 1。

use std::path::Path;

#[cfg(test)]
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
#[cfg(test)]
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows::core::w;

use super::comutil::{self, pcwstr};
use crate::apps::LaunchTarget;

const SW_SHOWNORMAL: i32 = 1;
#[cfg(test)]
const SW_HIDE: i32 = 0;

#[derive(Debug)]
pub struct LaunchError {
    pub message: String,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LaunchError {}

pub fn launch(target: &LaunchTarget) -> Result<(), LaunchError> {
    launch_inner(target, SW_SHOWNORMAL, false, Verb::Open)?;
    Ok(())
}

/// 有本地 exe 时用动词 `runas`。商店应用和协议链接返回错误，不调用 Shell。
pub fn launch_elevated(target: &LaunchTarget) -> Result<(), LaunchError> {
    if !crate::apps::supports_run_as_admin(target) {
        return Err(LaunchError {
            message: "这个应用不能以管理员身份运行".to_owned(),
        });
    }
    launch_inner(target, SW_SHOWNORMAL, false, Verb::RunAs)?;
    Ok(())
}

enum Verb {
    Open,
    RunAs,
}

/// 测试用。隐藏窗口并等待进程退出。不要拿它启动图形界面程序。
#[cfg(test)]
pub(crate) fn launch_and_wait(target: &LaunchTarget, timeout_ms: u32) -> Result<u32, LaunchError> {
    let handle = launch_inner(target, SW_HIDE, true, Verb::Open)?;
    let handle = handle.ok_or_else(|| LaunchError {
        message: "启动没有返回进程".to_owned(),
    })?;
    let wait = unsafe { WaitForSingleObject(handle, timeout_ms) };
    if wait != WAIT_OBJECT_0 {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Err(LaunchError {
            message: format!("等待进程超时: {wait:?}"),
        });
    }
    let mut code = 0u32;
    let exit = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe {
        let _ = CloseHandle(handle);
    }
    exit.map_err(|err| LaunchError {
        message: format!("读不到退出码: {err}"),
    })?;
    Ok(code)
}

fn launch_inner(
    target: &LaunchTarget,
    show: i32,
    wait: bool,
    verb: Verb,
) -> Result<Option<windows::Win32::Foundation::HANDLE>, LaunchError> {
    comutil::ensure_com().map_err(|message| LaunchError { message })?;
    let (file, args, directory) = command_line(target);
    let file_wide = comutil::wide_null(&file);
    let args_wide = comutil::wide_null(&args);
    let dir_wide = directory.as_ref().map(|dir| comutil::wide_path(dir));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0);
    // 这里没有消息泵。不带 SEE_MASK_NOASYNC 时，Shell 可能在返回后仍使用这些宽字符串。
    info.fMask = SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC;
    if wait {
        info.fMask |= SEE_MASK_NOCLOSEPROCESS;
    }
    info.lpVerb = match verb {
        Verb::Open => w!("open"),
        Verb::RunAs => w!("runas"),
    };
    info.lpFile = pcwstr(&file_wide);
    info.lpParameters = if args.is_empty() {
        windows::core::PCWSTR::null()
    } else {
        pcwstr(&args_wide)
    };
    info.lpDirectory = match &dir_wide {
        Some(buf) => pcwstr(buf),
        None => windows::core::PCWSTR::null(),
    };
    info.nShow = show;
    // SAFETY: SEE_MASK_NOASYNC 让调用在 Shell 用完字符串之前返回。hwnd 为空，不绑定 Lanwork 的窗口。
    unsafe { ShellExecuteExW(&mut info) }.map_err(|err| LaunchError {
        message: format!("启动失败: {err}"),
    })?;
    if wait {
        Ok(Some(info.hProcess))
    } else {
        Ok(None)
    }
}

fn command_line(target: &LaunchTarget) -> (String, String, Option<&Path>) {
    match target {
        LaunchTarget::Path {
            path,
            args,
            working_directory,
        } => (
            path.to_string_lossy().into_owned(),
            args.clone(),
            working_directory.as_deref(),
        ),
        LaunchTarget::Aumid { aumid } => {
            (format!(r"shell:AppsFolder\{aumid}"), String::new(), None)
        }
        LaunchTarget::Url { url } => (url.clone(), String::new(), None),
    }
}
