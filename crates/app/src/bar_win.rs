//! 搜索条用到的 Win32：窗口句柄、光标所在显示器、DWM 背景、系统版本和节电。

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use lanwork_core::shell::{
    Backdrop, BackdropInput, NudgeWait, WorkArea, choose_backdrop, nudge_wait, tick_after,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMNCRP_DISABLED, DWMNCRP_USEWINDOWSTYLE, DWMSBT_NONE,
    DWMSBT_TRANSIENTWINDOW, DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT, DWMWA_COLOR_NONE,
    DWMWA_NCRENDERING_POLICY, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWINDOWATTRIBUTE,
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::Accessibility::SetWinEventHook;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, INPUT, INPUT_0, INPUT_MOUSE, LASTINPUTINFO, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EVENT_SYSTEM_FOREGROUND, GA_ROOT, GWL_STYLE, GetAncestor, GetCursorPos,
    GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, IsIconic, IsWindowVisible,
    STYLESTRUCT, SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WINEVENT_OUTOFCONTEXT, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_STYLECHANGING,
    WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
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

type MoveHook = fn(isize, bool, i32, i32, u32, u32);

thread_local! {
    static MOVE_HOOKS: RefCell<Vec<MoveHook>> = const { RefCell::new(Vec::new()) };
    /// 正在移动或改大小的窗口。进入时加上，结束时去掉。悬浮窗用它挡住拖动循环里的失焦。
    static SIZING: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    /// 移动刚结束。拖动循环带出来的失焦会晚到，这两轮里先不让贴边窗口收回。
    static BLUR_HOLD: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    static TOPMOST: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    static FOREGROUND_HOOK: Cell<bool> = const { Cell::new(false) };
}

/// 拖动或改大小开始、结束时调用。`entering` 为真是开始。坐标和大小是这一刻的窗口矩形。
/// 搜索条和每个悬浮窗各登记一次，回调里按窗口句柄自己认。
pub(crate) fn add_move_hook(hook: MoveHook) {
    MOVE_HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        if !hooks.contains(&hook) {
            hooks.push(hook);
        }
    });
}

pub(crate) fn hwnd_key(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

pub(crate) fn size_move_blocks_blur(key: isize) -> bool {
    SIZING.with(|list| list.borrow().contains(&key))
        || BLUR_HOLD.with(|list| list.borrow().contains(&key))
}

pub(crate) fn clear_blur_hold(key: isize) {
    BLUR_HOLD.with(|list| list.borrow_mut().retain(|item| *item != key));
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
        let mut rect = RECT::default();
        // SAFETY: hwnd 是本进程的窗口，输出指针指向局部变量。
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
            let key = hwnd_key(hwnd);
            if entering {
                SIZING.with(|list| set_flag(list, key, true));
            } else {
                // 先挡住失焦，再标成已经结束，中间没有空隙。
                BLUR_HOLD.with(|list| set_flag(list, key, true));
                SIZING.with(|list| set_flag(list, key, false));
            }
            let width = u32::try_from(rect.right.saturating_sub(rect.left)).unwrap_or(0);
            let height = u32::try_from(rect.bottom.saturating_sub(rect.top)).unwrap_or(0);
            let hooks = MOVE_HOOKS.with(|hooks| hooks.borrow().clone());
            for hook in hooks {
                hook(key, entering, rect.left, rect.top, width, height);
            }
        }
    } else if message == WM_NCDESTROY {
        // SAFETY: 窗口正在销毁，移除本模块装上的子类。之后的消息直接交给下一层。
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(observe_move_loop), MOVE_SUBCLASS_ID);
        }
    }
    // SAFETY: 参数原样交给子类链的下一层。
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn set_flag(list: &RefCell<Vec<isize>>, key: isize, on: bool) {
    let mut list = list.borrow_mut();
    if on {
        if !list.contains(&key) {
            list.push(key);
        }
    } else {
        list.retain(|item| *item != key);
    }
}

/// 改位置和大小，并保持在最上层。不激活窗口。
pub(crate) fn place_topmost(hwnd: HWND, x: i32, y: i32, width: u32, height: u32) {
    let width = i32::try_from(width).unwrap_or(i32::MAX);
    let height = i32::try_from(height).unwrap_or(i32::MAX);
    // SAFETY: hwnd 是本进程的悬浮窗。HWND_TOPMOST 只改层次，SWP_NOACTIVATE 不抢走焦点。
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE,
        );
    }
}

fn keep_topmost(key: isize) {
    let hwnd = HWND(key as *mut c_void);
    // SAFETY: 句柄是登记过的本进程悬浮窗。不移动、不改大小、不激活。
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 这个悬浮窗要一直留在最上层。别的程序（包括全屏）到前台时再置一次。
/// 前台如果就是某个悬浮窗，不动它们之间的先后。
pub(crate) fn watch_topmost(hwnd: HWND) -> bool {
    let key = hwnd_key(hwnd);
    TOPMOST.with(|list| set_flag(list, key, true));
    keep_topmost(key);
    ensure_foreground_hook()
}

pub(crate) fn unwatch_topmost(hwnd: HWND) {
    let key = hwnd_key(hwnd);
    TOPMOST.with(|list| set_flag(list, key, false));
    SIZING.with(|list| set_flag(list, key, false));
    BLUR_HOLD.with(|list| set_flag(list, key, false));
}

fn ensure_foreground_hook() -> bool {
    if FOREGROUND_HOOK.with(Cell::get) {
        return true;
    }
    // SAFETY: 回调不带引用数据，进程结束前不卸。OUTOFCONTEXT 在装钩子的线程上回调。
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    // 这个句柄没有 Drop，不调用 UnhookWinEvent，进程结束前钩子一直留着。
    let installed = !hook.is_invalid();
    if installed {
        FOREGROUND_HOOK.with(|cell| cell.set(true));
    }
    installed
}

unsafe extern "system" fn on_foreground(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let key = hwnd_key(hwnd);
    let ours = TOPMOST.with(|list| list.borrow().contains(&key));
    if ours {
        return;
    }
    let keys = TOPMOST.with(|list| list.borrow().clone());
    for key in keys {
        keep_topmost(key);
    }
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

/// 这次置前走到了哪一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForegroundTake {
    /// 本来就是前台，或第一次 `SetForegroundWindow` 就成功。没有注入输入。
    Plain,
    /// 注入一次鼠标移动并等它被取走之后才到前台。
    AfterInput,
    Failed,
}

/// 把本进程的窗口放到前台并激活。
///
/// 热角、托盘菜单和别的程序里按下的热键，都不算「用户正在操作这个窗口」。
/// 前台锁会拒绝单独的 `SetForegroundWindow`：调用进程不是前台，也不是最近一次输入的接收者。
/// 窗口可以已经可见，但落在别的窗口后面。收到 `WM_HOTKEY` 的进程被允许置前，所以热键这一步经常就够。
///
/// 还不是前台时，本进程 `SendInput` 一次相对位移为 0 的鼠标移动，等原始输入线程把它取走，
/// 再 `SetForegroundWindow`。这样「本进程收到了最后一次输入」才成立。`SendInput` 返回时事件
/// 只是进了输入流，立刻置前会和输入线程竞争。不送任何按键：单独的 Alt 抬起不是文档里的
/// 「用户按下 Alt」，还会以 `WM_SYSKEYUP` 进到当前前台程序的菜单或正在进行的中文输入。
/// 不调用 `AttachThreadInput`：置前会同步把激活消息发给前台线程，两边的输入队列附在一起时
/// 可能互相等待。相对位移 0 不会移动光标；系统若把它丢掉，等待结束后再试一次置前，仍然失败
/// 就由调用方收起。空句柄直接失败，不调用窗口 API。
pub(crate) fn bring_to_front(hwnd: HWND) -> ForegroundTake {
    if hwnd.0.is_null() {
        return ForegroundTake::Failed;
    }
    if is_foreground(hwnd) {
        return ForegroundTake::Plain;
    }
    activate(hwnd);
    if is_foreground(hwnd) {
        return ForegroundTake::Plain;
    }
    let before = last_input_tick();
    if !send_relative_zero() {
        return ForegroundTake::Failed;
    }
    wait_for_nudge(before);
    activate(hwnd);
    if is_foreground(hwnd) {
        ForegroundTake::AfterInput
    } else {
        ForegroundTake::Failed
    }
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

fn last_input_tick() -> Option<u32> {
    let mut info = LASTINPUTINFO {
        cbSize: u32::try_from(std::mem::size_of::<LASTINPUTINFO>()).ok()?,
        dwTime: 0,
    };
    // SAFETY: cbSize 已设为结构体大小，指针指向局部变量。
    unsafe { GetLastInputInfo(&mut info) }
        .as_bool()
        .then_some(info.dwTime)
}

/// 相对位移 0，并要求不要和别的移动合并。没有按下、抬起或滚轮。
fn send_relative_zero() -> bool {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_MOVE_NOCOALESCE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: 输入是本函数的局部变量，调用期间有效。只送一次鼠标移动。
    let sent = unsafe {
        SendInput(
            &[input],
            i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(0),
        )
    };
    sent == 1
}

/// 等到注入被取走，或等到上限。不抽消息：调用方还借着面板或搜索条的状态。
fn wait_for_nudge(before: Option<u32>) {
    let start = std::time::Instant::now();
    let mut settled = false;
    loop {
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let advanced = before.is_some_and(|previous| {
            last_input_tick().is_some_and(|current| tick_after(previous, current))
        });
        match nudge_wait(elapsed, advanced, settled) {
            NudgeWait::Done => break,
            NudgeWait::Sleep { ms } => std::thread::sleep(std::time::Duration::from_millis(ms)),
            NudgeWait::Settle { ms } => {
                settled = true;
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
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
    let border = DWMWA_COLOR_DEFAULT;
    let _ = set_attribute(hwnd, DWMWA_BORDER_COLOR, &border);
    let policy = DWMNCRP_USEWINDOWSTYLE;
    let _ = set_attribute(hwnd, DWMWA_NCRENDERING_POLICY, &policy);
    // 纯色时也扩展边框：DWM 照样画阴影和圆角描边，客户区由 Slint 画满纯色。
    let extended = extend_frame(hwnd);
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

/// 收起后的窗口只画把手。关掉圆角、边框、非客户区阴影和亚克力，透明像素才能透出后面的画面。
pub(crate) fn apply_collapsed_frame(hwnd: HWND, dark: bool) {
    let corner = DWMWCP_DONOTROUND;
    let _ = set_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner);
    let dark_value: u32 = u32::from(dark);
    let _ = set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_value);
    let border = DWMWA_COLOR_NONE;
    let _ = set_attribute(hwnd, DWMWA_BORDER_COLOR, &border);
    let policy = DWMNCRP_DISABLED;
    let _ = set_attribute(hwnd, DWMWA_NCRENDERING_POLICY, &policy);
    let _ = extend_frame(hwnd);
    if windows_build().is_some_and(|build| build >= lanwork_core::shell::MIN_BACKDROP_BUILD) {
        let none: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_NONE;
        let _ = set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &none);
    }
}

fn extend_frame(hwnd: HWND) -> bool {
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // SAFETY: hwnd 是本进程的窗口，margins 是局部变量。
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) }.is_ok()
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
        assert!(matches!(
            bring_to_front(HWND::default()),
            ForegroundTake::Failed
        ));
    }
}
