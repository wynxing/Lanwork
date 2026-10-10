//! 搜索条用到的 Win32：窗口句柄、光标所在显示器、DWM 背景、系统版本和节电。

use std::ffi::c_void;

use lanwork_core::shell::{Backdrop, BackdropInput, WorkArea, choose_backdrop};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWMWINDOWATTRIBUTE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetCursorPos, GetWindowLongPtrW, GetWindowRect, STYLESTRUCT, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, WM_NCDESTROY, WM_STYLECHANGING, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
};

use crate::registry::read_transparency_enabled;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> i32;
}

pub(crate) fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    let owned = window.window_handle();
    let handle = HasWindowHandle::window_handle(&owned).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// 光标所在显示器的工作区和有效 DPI。
pub(crate) fn cursor_monitor() -> Option<(WorkArea, u32)> {
    let mut point = POINT::default();
    // SAFETY: 输出指针指向本函数的局部变量。
    unsafe { GetCursorPos(&mut point) }.ok()?;
    // SAFETY: 只按坐标查询显示器，返回的句柄不需要释放。
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).ok()?,
        ..Default::default()
    };
    // SAFETY: cbSize 已设为结构体大小，指针指向局部变量。
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let mut dpi_x = 0u32;
    let mut dpi_y = 0u32;
    // SAFETY: 两个输出指针都指向局部变量。
    let dpi = match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
    {
        Ok(()) if dpi_x > 0 => dpi_x,
        _ => lanwork_core::shell::BASE_DPI,
    };
    let work = info.rcWork;
    Some((
        WorkArea {
            left: work.left,
            top: work.top,
            right: work.right,
            bottom: work.bottom,
        },
        dpi,
    ))
}

pub(crate) fn window_origin(hwnd: HWND) -> Option<(i32, i32)> {
    let mut rect = RECT::default();
    // SAFETY: hwnd 是本进程的窗口，输出指针指向局部变量。
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some((rect.left, rect.top))
}

pub(crate) fn move_window(hwnd: HWND, x: i32, y: i32) {
    // SAFETY: 只改位置，不改大小、层次和激活状态。
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 系统菜单和最小化、最大化框。winit 每次改可见性都按自己的标志重写 `GWL_STYLE`，
/// 总带 `WS_SYSMENU`；边框扩展到整个客户区后，DWM 会在右上角画出这三个按钮。
const CAPTION_BUTTONS: u32 = WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0;
const CAPTION_SUBCLASS_ID: usize = 1;

/// 去掉标题栏按钮，并在之后每次改样式时继续去掉。保留 `WS_CAPTION`，DWM 仍画阴影和圆角。
pub(crate) fn remove_caption_buttons(hwnd: HWND) -> bool {
    // SAFETY: 在创建窗口的线程上调用。回调是本模块的函数，不带引用数据；窗口销毁时由回调自己移除。
    let installed =
        unsafe { SetWindowSubclass(hwnd, Some(strip_caption_buttons), CAPTION_SUBCLASS_ID, 0) }
            .as_bool();
    // SAFETY: hwnd 是本进程的窗口，只读写样式位。
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
    let stripped = style & !(CAPTION_BUTTONS as isize);
    if stripped != style {
        // SAFETY: 同上。SWP_FRAMECHANGED 让系统按新样式重算边框，不移动、不激活。
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, stripped);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    installed
}

unsafe extern "system" fn strip_caption_buttons(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_STYLECHANGING && wparam.0 as i32 == GWL_STYLE.0 && lparam.0 != 0 {
        // SAFETY: WM_STYLECHANGING 的 lParam 指向系统传来的 STYLESTRUCT，处理期间可写。
        let change = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
        change.styleNew &= !CAPTION_BUTTONS;
    } else if message == WM_NCDESTROY {
        // SAFETY: 窗口正在销毁，移除本模块装上的子类。之后的消息直接交给下一层。
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(strip_caption_buttons), CAPTION_SUBCLASS_ID);
        }
    }
    // SAFETY: 参数原样交给子类链的下一层。
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

pub(crate) fn bring_to_front(hwnd: HWND) {
    // SAFETY: 本进程刚收到热键消息，系统允许它把自己的窗口放到前台。失败时窗口仍显示。
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
}

fn windows_build() -> Option<u32> {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: u32::try_from(std::mem::size_of::<OSVERSIONINFOW>()).ok()?,
        ..Default::default()
    };
    // SAFETY: dwOSVersionInfoSize 已设为结构体大小，指针指向局部变量。
    let status = unsafe { RtlGetVersion(&mut info) };
    (status >= 0).then_some(info.dwBuildNumber)
}

fn battery_saver() -> bool {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: 输出指针指向局部变量。
    match unsafe { GetSystemPowerStatus(&mut status) } {
        Ok(()) => status.SystemStatusFlag & 1 != 0,
        Err(_) => true,
    }
}

/// 按当前系统状态选背景。读不到节电状态时按节电处理，用纯色。
pub(crate) fn current_backdrop() -> Backdrop {
    choose_backdrop(BackdropInput {
        build: windows_build(),
        transparency_enabled: read_transparency_enabled(),
        battery_saver: battery_saver(),
    })
}

/// 圆角、暗色标题区和背景类型。返回实际可用的背景：亚克力属性没有设上时是纯色。
pub(crate) fn apply_dwm(hwnd: HWND, wanted: Backdrop, dark: bool) -> Backdrop {
    let corner = DWMWCP_ROUND;
    let _ = set_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner);
    let dark_value: u32 = u32::from(dark);
    let _ = set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_value);
    // 纯色时也扩展边框：DWM 照样画阴影和圆角描边，客户区由 Slint 画满纯色。
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // SAFETY: hwnd 是本进程的窗口，margins 是局部变量。
    let extended = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) }.is_ok();
    if wanted == Backdrop::Acrylic && extended {
        let backdrop: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW;
        if set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop) {
            return Backdrop::Acrylic;
        }
    }
    if windows_build().is_some_and(|build| build >= lanwork_core::shell::MIN_BACKDROP_BUILD) {
        let none: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_NONE;
        let _ = set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &none);
    }
    Backdrop::Solid
}

fn set_attribute<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) -> bool {
    let Ok(size) = u32::try_from(std::mem::size_of::<T>()) else {
        return false;
    };
    // SAFETY: value 指向一个大小为 size 的有效值，调用期间不被修改。
    unsafe { DwmSetWindowAttribute(hwnd, attribute, std::ptr::from_ref(value).cast(), size) }
        .is_ok()
}
