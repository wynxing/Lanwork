//! 搜索条用到的 Win32：窗口句柄、光标所在显示器、DWM 背景、系统版本和节电。

use std::cell::Cell;
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
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VK_MENU,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GA_ROOT, GWL_STYLE, GetAncestor, GetCursorPos, GetForegroundWindow,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    STYLESTRUCT, SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_STYLECHANGING, WS_MAXIMIZEBOX,
    WS_MINIMIZEBOX, WS_SYSMENU,
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

/// 一块显示器的读数。`bounds` 是 `rcMonitor`，`work` 是 `rcWork`，物理像素。
pub(crate) struct MonitorReading {
    pub bounds: WorkArea,
    pub work: WorkArea,
    pub dpi: u32,
}

/// 所有显示器。枚举失败时返回空表。
pub(crate) fn monitors() -> Vec<MonitorReading> {
    // SAFETY: 系统只在下面 EnumDisplayMonitors 调用期间、同一线程上调用它，data 是该调用传入的指针。
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> windows::core::BOOL {
        let mut info = MONITORINFO {
            cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: cbSize 已设为结构体大小，指针指向局部变量。
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let mut dpi_x = 0u32;
            let mut dpi_y = 0u32;
            // SAFETY: 两个输出指针都指向局部变量。
            let dpi = match unsafe {
                GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)
            } {
                Ok(()) if dpi_x > 0 => dpi_x,
                _ => lanwork_core::shell::BASE_DPI,
            };
            // SAFETY: data 是下面 EnumDisplayMonitors 调用传入的 `Vec<MonitorReading>` 的地址，
            // 回调只在那次调用返回之前、在同一线程上执行。
            let list = unsafe { &mut *(data.0 as *mut Vec<MonitorReading>) };
            list.push(MonitorReading {
                bounds: area_of(info.rcMonitor),
                work: area_of(info.rcWork),
                dpi,
            });
        }
        true.into()
    }

    let mut list: Vec<MonitorReading> = Vec::new();
    // SAFETY: 回调只借用 `list`，枚举在本调用内同步结束，`list` 在此期间不会被移动。
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(std::ptr::from_mut(&mut list) as isize),
        )
    };
    list
}

fn area_of(rect: RECT) -> WorkArea {
    WorkArea {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// 所有显示器的工作区，物理像素，虚拟屏幕坐标。枚举失败时返回空表。
pub(crate) fn work_areas() -> Vec<WorkArea> {
    // SAFETY: 系统只在下面 EnumDisplayMonitors 调用期间、同一线程上调用它，data 是该调用传入的指针。
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> windows::core::BOOL {
        let mut info = MONITORINFO {
            cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: cbSize 已设为结构体大小，指针指向局部变量。
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            // SAFETY: data 是下面 EnumDisplayMonitors 调用传入的 `Vec<WorkArea>` 的地址，
            // 回调只在那次调用返回之前、在同一线程上执行。
            let areas = unsafe { &mut *(data.0 as *mut Vec<WorkArea>) };
            let work = info.rcWork;
            areas.push(WorkArea {
                left: work.left,
                top: work.top,
                right: work.right,
                bottom: work.bottom,
            });
        }
        true.into()
    }

    let mut areas: Vec<WorkArea> = Vec::new();
    // SAFETY: 回调只借用 `areas`，枚举在本调用内同步结束，`areas` 在此期间不会被移动。
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(std::ptr::from_mut(&mut areas) as isize),
        )
    };
    areas
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
const MOVE_SUBCLASS_ID: usize = 2;

thread_local! {
    static MOVE_LOOP: Cell<Option<fn(bool)>> = const { Cell::new(None) };
}

/// 拖动循环开始（`true`）或结束（`false`）时调用。只由搜索条登记。
pub(crate) fn set_move_loop_hook(hook: fn(bool)) {
    MOVE_LOOP.with(|cell| cell.set(Some(hook)));
}

/// 观察这个窗口的 `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE`。和标题栏子类互不替换。
pub(crate) fn watch_move_loop(hwnd: HWND) -> bool {
    // SAFETY: 在创建窗口的线程上调用。回调不带引用数据，窗口销毁时由回调自己移除。
    unsafe { SetWindowSubclass(hwnd, Some(observe_move_loop), MOVE_SUBCLASS_ID, 0) }.as_bool()
}

unsafe extern "system" fn observe_move_loop(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_ENTERSIZEMOVE || message == WM_EXITSIZEMOVE {
        let entering = message == WM_ENTERSIZEMOVE;
        MOVE_LOOP.with(|cell| {
            if let Some(hook) = cell.get() {
                hook(entering);
            }
        });
    } else if message == WM_NCDESTROY {
        // SAFETY: 窗口正在销毁，移除本模块装上的子类。之后的消息直接交给下一层。
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(observe_move_loop), MOVE_SUBCLASS_ID);
        }
    }
    // SAFETY: 参数原样交给子类链的下一层。
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

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

/// 把本进程的窗口放到前台并激活。返回结束时它是不是前台窗口。
///
/// 热角、托盘菜单和别的程序里按下的热键，都不算「用户正在操作这个窗口」。
/// 前台锁会拒绝单独的 `SetForegroundWindow`：调用进程不是前台，也不是最近一次输入的接收者。
/// 窗口可以已经可见，但落在别的窗口后面。
///
/// 先直接激活。还不是前台、并且 Alt 没有按着时，用 `SendInput` 送一次 Alt 键抬起，再激活。
/// 键抬起让本线程成为最近一次输入的来源，前台锁的允许条件里有这一条；只送抬起、不送按下，
/// 避免把当前前台程序的菜单点亮。Alt 已经按着时不送，否则会松开用户还按着的键
/// （搜索条热键默认带 Alt）。仍不是前台时，把本线程的输入队列接到前台线程上再激活一次，
/// 随即解开，附上期间不抽消息。`AttachThreadInput` 不作为第一步：它不产生输入，
/// 也不在前台锁的允许条件里，而且附上另一个线程有死锁风险。
///
/// 空句柄直接返回假，不调用窗口 API。
pub(crate) fn bring_to_front(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    if is_foreground(hwnd) {
        return true;
    }
    activate(hwnd);
    if is_foreground(hwnd) {
        return true;
    }
    if !alt_held() {
        send_alt_keyup();
        activate(hwnd);
        if is_foreground(hwnd) {
            return true;
        }
    }
    // 附上之后不再送按键，避免键抬起进了前台线程的输入队列。
    let attached = attach_to_foreground();
    activate(hwnd);
    if let Some(attached) = attached {
        detach_foreground(attached);
    }
    is_foreground(hwnd)
}

fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: 没有参数。没有前台窗口时句柄为空。
    let foreground = unsafe { GetForegroundWindow() };
    if foreground == hwnd {
        return true;
    }
    if foreground.0.is_null() || hwnd.0.is_null() {
        return false;
    }
    // SAFETY: 只读窗口的根，不保留句柄以外的状态。
    (unsafe { GetAncestor(foreground, GA_ROOT) }) == hwnd
}

fn activate(hwnd: HWND) {
    // SAFETY: hwnd 是本进程的窗口。已经可见时不再 ShowWindow，避免多一次失败的激活
    // 把失焦消息排进来。最小化时先还原；还不可见时再显示。然后提到 Z 序顶部并请求前台。
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Alt 此刻是否按着。高位为 1 表示按着。
fn alt_held() -> bool {
    // SAFETY: 只查询按键状态，没有输出指针。
    let state = unsafe { GetAsyncKeyState(i32::from(VK_MENU.0)) };
    state < 0
}

fn send_alt_keyup() {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_MENU,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: 输入是本函数的局部变量，调用期间有效。只送一次键抬起。
    let _ = unsafe {
        SendInput(
            &[input],
            i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(0),
        )
    };
}

struct AttachedForeground {
    current: u32,
    foreground: u32,
}

fn attach_to_foreground() -> Option<AttachedForeground> {
    // SAFETY: 没有参数。没有前台窗口时句柄为空。
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return None;
    }
    // SAFETY: 不需要进程 id。
    let foreground_thread = unsafe { GetWindowThreadProcessId(foreground, None) };
    // SAFETY: 没有参数。
    let current = unsafe { GetCurrentThreadId() };
    if foreground_thread == 0 || foreground_thread == current {
        return None;
    }
    // SAFETY: 只附上输入队列。失败时不记成已附上。
    let attached = (unsafe { AttachThreadInput(current, foreground_thread, true) }).as_bool();
    attached.then_some(AttachedForeground {
        current,
        foreground: foreground_thread,
    })
}

fn detach_foreground(attached: AttachedForeground) {
    // SAFETY: 与上面成功的附上配对，马上解开。
    unsafe {
        let _ = AttachThreadInput(attached.current, attached.foreground, false);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_window_is_not_brought_forward() {
        assert!(!bring_to_front(HWND::default()));
    }
}
