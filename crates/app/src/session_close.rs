//! 事件循环的隐藏目标窗口也要响应外部关闭。
//!
//! `taskkill` 不带 `/F` 时给每个顶层窗口投递 `WM_CLOSE`，包括 winit 的
//! 「Winit Thread Event Target」。那个窗口的默认过程会把它销毁，事件循环却不走托盘「退出」。
//! 子类接住关闭、注销和关机，交给 [`crate::host::request_quit`]，不销毁这个窗口。

use std::time::Duration;

use lanwork_core::shell::{WM_QUERYENDSESSION, external_close_requests_quit};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, GetClassNameW, WM_NCDESTROY};
use windows::core::BOOL;

const EVENT_TARGET_CLASS: &str = "Winit Thread Event Target";
const SUBCLASS_ID: usize = 5;

/// 事件循环起来之后调用。目标窗口还没建好时，下一轮再试一次。
pub(crate) fn install() {
    if !attach() {
        slint::Timer::single_shot(Duration::ZERO, || {
            let _ = attach();
        });
    }
}

fn attach() -> bool {
    let mut found = Vec::new();
    // SAFETY: 回调只在这次枚举期间、同一线程上使用 `found`。
    let ok = unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect),
            LPARAM(&mut found as *mut Vec<HWND> as isize),
        )
    };
    if !ok.as_bool() || found.is_empty() {
        return false;
    }
    for hwnd in found {
        // SAFETY: 句柄是本线程的隐藏目标窗口。回调不带引用数据，销毁时自己摘掉。
        let _ = unsafe { SetWindowSubclass(hwnd, Some(on_session), SUBCLASS_ID, 0) };
    }
    true
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    if class_name(hwnd) == EVENT_TARGET_CLASS {
        // SAFETY: `data` 是 `attach` 里仍活着的 `Vec<HWND>`。
        let found = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        found.push(hwnd);
    }
    true.into()
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: 缓冲区是本函数的局部变量，长度传给系统。
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    let len = usize::try_from(len).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

unsafe extern "system" fn on_session(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if external_close_requests_quit(message, wparam.0) {
        // 已经在界面线程上。直接退出，不要再投一圈，否则工具窗口可能先把 `open` 写成假。
        crate::host::request_quit();
        return if message == WM_QUERYENDSESSION {
            LRESULT(1)
        } else {
            LRESULT(0)
        };
    }
    if message == WM_NCDESTROY {
        // SAFETY: 窗口正在销毁，移除本模块装上的子类。
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(on_session), SUBCLASS_ID);
        }
    }
    // SAFETY: 其余消息原样交给下一层。关闭和注销不交给默认过程，以免把目标窗口销毁。
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
