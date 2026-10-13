//! 便签和收纳悬浮窗的 DWM 样式，以及收起后的窗口区域。
//!
//! winit 只在创建窗口时调用一次空区域的 `DwmEnableBlurBehindWindow`。一键回到桌面
//! （外壳抬起 `WorkerW`，不一定给这个窗口发最小化）之后，这次透明会被清掉，渲染器
//! 再把客户区清成不透明的颜色。重设 DWM 属性盖不住这块颜色，点击区（约 48×9 逻辑像素）
//! 就变成淡蓝色矩形。收起时另外用 `SetWindowRgn` 把窗口收成把手的圆角矩形，区域外的
//! 像素不属于窗口，透明被清掉也铺不出那块矩形。点击区比把手大出来的一圈用鼠标钩子转给窗口。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use lanwork_core::storage::Store;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
#[cfg(test)]
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DwmGetWindowAttribute,
};
#[cfg(test)]
use windows::Win32::Graphics::Gdi::GetRgnBox;
use windows::Win32::Graphics::Gdi::{
    CreateRectRgn, CreateRoundRectRgn, DeleteObject, GDI_REGION_TYPE, GetWindowRgn, NULLREGION,
    RDW_FRAME, RDW_INVALIDATE, RGN_ERROR, RedrawWindow, ScreenToClient, SetWindowRgn,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GW_HWNDNEXT, GWL_EXSTYLE, GetClientRect, GetTopWindow, GetWindow,
    GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, HHOOK, IsIconic, IsWindowVisible,
    PostMessageW, STYLESTRUCT, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_HIDEWINDOW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos,
    SetWindowsHookExW, ShowWindow, UnhookWindowsHookEx, WH_MOUSE_LL, WINDOWPLACEMENT, WINDOWPOS,
    WM_ACTIVATE, WM_ACTIVATEAPP, WM_APP, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_EXITSIZEMOVE,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_SETTINGCHANGE, WM_SHOWWINDOW,
    WM_SIZE, WM_STYLECHANGING, WM_SYSCOMMAND, WM_THEMECHANGED, WM_WINDOWPOSCHANGED,
    WM_WINDOWPOSCHANGING, WS_EX_TOOLWINDOW, WindowFromPoint,
};

use slint::winit_030::winit;

use crate::bar_win::{self, hwnd_key};
use crate::frame_policy::{self, FrameAction, FrameNotice, GripRect, is_iconic_origin};

const FRAME_SUBCLASS_ID: usize = 3;
const WM_REAPPLY_FRAME: u32 = WM_APP + 1;
/// 点击区余量里的光标进或出。`wParam` 非 0 表示正在里面。
const WM_PAD_HOVER: u32 = WM_APP + 2;
const WM_DWMCOMPOSITIONCHANGED: u32 = 0x031E;
const SC_MINIMIZE: usize = 0xF020;
/// `SIZE_MINIMIZED`。
const SIZE_MINIMIZED: usize = 1;
const WA_INACTIVE: usize = 0;

struct FrameState {
    collapsed: bool,
    dark: bool,
    /// 已经投了一条补设消息，处理完之前不再投。
    follow_up: bool,
    /// 上次成功设上的把手区域。展开时是 `None`。
    grip: Option<GripRect>,
}

type PadHook = fn(isize, bool);

thread_local! {
    static FRAMES: RefCell<HashMap<isize, FrameState>> = RefCell::new(HashMap::new());
    static APPLY_DEPTH: Cell<u32> = const { Cell::new(0) };
    static LOG: RefCell<Option<Store>> = const { RefCell::new(None) };
    static PAD_HOOKS: RefCell<Vec<PadHook>> = const { RefCell::new(Vec::new()) };
}

/// 低级鼠标钩子。没有收起窗口时是 0。
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
/// 当前把点击区余量算成悬停的窗口。没有时是 0。
static PAD_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// 把收起窗口在 Win+D 上收到的消息写进 `logs/app.log`。探测测试不调用。
pub(crate) fn set_log(store: Store) {
    LOG.with(|cell| *cell.borrow_mut() = Some(store));
    // 日志装上之后才允许钩子。若这时已经有收起的窗口，补装一次。
    sync_mouse_hook();
}

/// 点击区余量里的悬停。便签和收纳各登记一次，按窗口句柄自己认。
pub(crate) fn add_pad_hook(hook: PadHook) {
    PAD_HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        if !hooks.contains(&hook) {
            hooks.push(hook);
        }
    });
}

/// 还原、重新显示、主题或 DPI 变化时，Slint 这条事件也要重设样式。
pub(crate) fn wants_refresh(event: &winit::event::WindowEvent) -> bool {
    matches!(
        event,
        winit::event::WindowEvent::Occluded(_)
            | winit::event::WindowEvent::ThemeChanged(_)
            | winit::event::WindowEvent::ScaleFactorChanged { .. }
            | winit::event::WindowEvent::Resized(_)
    )
}

/// 记下收起状态并立刻重设样式。返回展开时实际用上的背景；收起时是纯色（客户区自己画把手）。
pub(crate) fn sync(hwnd: HWND, collapsed: bool, dark: bool) -> lanwork_core::shell::Backdrop {
    remember(hwnd, collapsed, dark);
    let _ = install(hwnd);
    apply_current(hwnd)
}

/// 前台钩子或 `bring_to_front` 之后调用。没登记过的窗口（搜索条、面板）什么也不做。
pub(crate) fn after_topmost(key: isize) {
    if !known(key) {
        return;
    }
    on_notice(HWND(key as *mut c_void), FrameNotice::TopmostReasserted);
}

/// 关闭前卸掉登记。子类留到 `WM_NCDESTROY` 再摘，避免和正在处理的消息打架。
pub(crate) fn forget(hwnd: HWND) {
    clear_pad_if(hwnd);
    FRAMES.with(|frames| {
        frames.borrow_mut().remove(&hwnd_key(hwnd));
    });
    sync_mouse_hook();
}

fn remember(hwnd: HWND, collapsed: bool, dark: bool) {
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let previous = frames.get(&hwnd_key(hwnd));
        let follow_up = previous.is_some_and(|frame| frame.follow_up);
        let grip = previous.and_then(|frame| frame.grip);
        frames.insert(
            hwnd_key(hwnd),
            FrameState {
                collapsed,
                dark,
                follow_up,
                grip,
            },
        );
    });
}

fn known(key: isize) -> bool {
    FRAMES.with(|frames| frames.borrow().contains_key(&key))
}

fn state(hwnd: HWND) -> Option<(bool, bool)> {
    FRAMES.with(|frames| {
        frames
            .borrow()
            .get(&hwnd_key(hwnd))
            .map(|frame| (frame.collapsed, frame.dark))
    })
}

fn install(hwnd: HWND) -> bool {
    // SAFETY: 在创建窗口的线程上调用。回调不带引用数据，窗口销毁时由回调自己移除。
    unsafe { SetWindowSubclass(hwnd, Some(frame_subclass), FRAME_SUBCLASS_ID, 0) }.as_bool()
}

fn on_notice(hwnd: HWND, notice: FrameNotice) {
    let Some((collapsed, _)) = state(hwnd) else {
        return;
    };
    let action = frame_policy::frame_action(collapsed, notice);
    if action.restore && is_minimized(hwnd) {
        request_follow_up(hwnd);
    }
    if action.reapply {
        apply_current(hwnd);
    }
    if action.reapply_again {
        request_follow_up(hwnd);
    }
}

fn apply_current(hwnd: HWND) -> lanwork_core::shell::Backdrop {
    if APPLY_DEPTH.with(Cell::get) > 0 {
        return current_backdrop(hwnd);
    }
    APPLY_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    let backdrop = apply_current_inner(hwnd);
    APPLY_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    backdrop
}

fn current_backdrop(hwnd: HWND) -> lanwork_core::shell::Backdrop {
    let collapsed = state(hwnd).is_some_and(|(collapsed, _)| collapsed);
    if collapsed {
        lanwork_core::shell::Backdrop::Solid
    } else {
        bar_win::current_backdrop()
    }
}

fn apply_current_inner(hwnd: HWND) -> lanwork_core::shell::Backdrop {
    let Some((collapsed, dark)) = state(hwnd) else {
        return lanwork_core::shell::Backdrop::Solid;
    };
    set_tool_window(
        hwnd,
        frame_policy::frame_action(collapsed, FrameNotice::Shown).tool_window,
    );
    let backdrop = if collapsed {
        bar_win::apply_collapsed_frame(hwnd, dark);
        lanwork_core::shell::Backdrop::Solid
    } else {
        bar_win::apply_dwm(hwnd, bar_win::current_backdrop(), dark)
    };
    // 区域放在 DWM 和扩展样式之后。那些调用会改框架，但裁剪不依赖透明有没有设上。
    sync_shape(hwnd, collapsed);
    if !collapsed {
        clear_pad_if(hwnd);
    }
    sync_mouse_hook();
    // SAFETY: hwnd 是本进程的悬浮窗。只让整窗失效，不在这里同步绘制，避免再进窗口过程。
    unsafe {
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_FRAME);
    }
    backdrop
}

fn set_tool_window(hwnd: HWND, collapsed: bool) {
    // SAFETY: hwnd 是本进程的窗口，只读写扩展样式位。
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let tool = WS_EX_TOOLWINDOW.0 as isize;
    let next = if collapsed {
        style | tool
    } else {
        style & !tool
    };
    if next == style {
        return;
    }
    // SAFETY: 同上。SWP_FRAMECHANGED 让系统按新样式重算，不移动、不激活、不改层次。
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
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

fn request_follow_up(hwnd: HWND) {
    let mut post = false;
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let Some(frame) = frames.get_mut(&hwnd_key(hwnd)) else {
            return;
        };
        if frame.follow_up {
            return;
        }
        frame.follow_up = true;
        post = true;
    });
    if !post {
        return;
    }
    // SAFETY: hwnd 是本进程的窗口。这条消息由本模块的子类处理，不进入 winit。
    let posted = unsafe { PostMessageW(Some(hwnd), WM_REAPPLY_FRAME, WPARAM(0), LPARAM(0)) };
    if posted.is_err() {
        FRAMES.with(|frames| {
            if let Some(frame) = frames.borrow_mut().get_mut(&hwnd_key(hwnd)) {
                frame.follow_up = false;
            }
        });
    }
}

fn finish_follow_up(hwnd: HWND) {
    let collapsed = state(hwnd).is_some_and(|(collapsed, _)| collapsed);
    if collapsed && is_minimized(hwnd) {
        restore_without_activating(hwnd);
    } else if collapsed && !is_visible(hwnd) {
        // SAFETY: 收起的窗口被藏起来时再显示，不激活，避免打断一键回到桌面。
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    }
    apply_current(hwnd);
    FRAMES.with(|frames| {
        if let Some(frame) = frames.borrow_mut().get_mut(&hwnd_key(hwnd)) {
            frame.follow_up = false;
        }
    });
}

fn is_minimized(hwnd: HWND) -> bool {
    // SAFETY: hwnd 是本进程的窗口。
    unsafe { IsIconic(hwnd) }.as_bool()
}

fn is_visible(hwnd: HWND) -> bool {
    // SAFETY: hwnd 是本进程的窗口。
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

fn restore_without_activating(hwnd: HWND) {
    let mut place = WINDOWPLACEMENT {
        length: u32::try_from(std::mem::size_of::<WINDOWPLACEMENT>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: hwnd 是本进程的窗口，place 的 length 已设为结构体大小。
    if unsafe { GetWindowPlacement(hwnd, &mut place) }.is_ok() {
        place.showCmd = u32::try_from(SW_SHOWNOACTIVATE.0).unwrap_or(4);
        // SAFETY: 同上，place 仍指向这块局部变量。
        let _ = unsafe { SetWindowPlacement(hwnd, &place) };
    } else {
        // SAFETY: 还原失败时至少按不激活的方式显示。
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    }
}

fn sync_shape(hwnd: HWND, collapsed: bool) {
    if collapsed {
        let Some(grip) = grip_for(hwnd) else {
            return;
        };
        let remembered = FRAMES.with(|frames| {
            frames
                .borrow()
                .get(&hwnd_key(hwnd))
                .and_then(|frame| frame.grip)
        });
        // 同一块把手还在时不重复设，避免每次置顶都写一条日志。
        // 区域被系统清掉时再设一次，并记下来，方便对照 Win+D。
        if remembered == Some(grip) && has_region(hwnd) {
            return;
        }
        let repairing = remembered == Some(grip);
        if set_grip_region(hwnd, grip) {
            remember_grip(hwnd, Some(grip));
            log_line(&format!(
                "收起窗口区域{} hwnd={} grip={},{},{}x{}",
                if repairing {
                    "被清掉后已重设"
                } else {
                    "设为把手"
                },
                hwnd_key(hwnd),
                grip.x,
                grip.y,
                grip.width,
                grip.height
            ));
        } else {
            log_line(&format!("收起窗口区域没有设上 hwnd={}", hwnd_key(hwnd)));
        }
    } else if has_region(hwnd) || grip_remembered(hwnd) {
        clear_region(hwnd);
        remember_grip(hwnd, None);
        log_line(&format!("悬浮窗区域已去掉 hwnd={}", hwnd_key(hwnd)));
    }
}

fn grip_remembered(hwnd: HWND) -> bool {
    FRAMES.with(|frames| {
        frames
            .borrow()
            .get(&hwnd_key(hwnd))
            .is_some_and(|frame| frame.grip.is_some())
    })
}

fn grip_for(hwnd: HWND) -> Option<GripRect> {
    let mut client = RECT::default();
    // SAFETY: hwnd 是本进程的窗口，输出指向局部变量。
    unsafe { GetClientRect(hwnd, &mut client) }.ok()?;
    let width = u32::try_from(client.right.max(0)).ok()?;
    let height = u32::try_from(client.bottom.max(0)).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    // SAFETY: hwnd 是本进程的窗口。读不到 DPI 时按 96，和几何模块的缺省一致。
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    Some(frame_policy::collapsed_grip(width, height, dpi))
}

fn remember_grip(hwnd: HWND, grip: Option<GripRect>) {
    FRAMES.with(|frames| {
        if let Some(frame) = frames.borrow_mut().get_mut(&hwnd_key(hwnd)) {
            frame.grip = grip;
        }
    });
}

fn set_grip_region(hwnd: HWND, grip: GripRect) -> bool {
    let right = grip.x.saturating_add_unsigned(grip.width);
    let bottom = grip.y.saturating_add_unsigned(grip.height);
    let thick = i32::try_from(grip.width.min(grip.height))
        .unwrap_or(1)
        .max(1);
    // SAFETY: 圆角椭圆的直径等于把手厚度，两端是半圆。区域在设成功之后由系统接管。
    let region = unsafe { CreateRoundRectRgn(grip.x, grip.y, right, bottom, thick, thick) };
    if region.is_invalid() {
        return false;
    }
    // SAFETY: hwnd 是本进程的窗口。成功时系统取得 region 的所有权，失败时由这里释放。
    // 非 0 才算设上。失败时区域仍归调用方。
    let set = unsafe { SetWindowRgn(hwnd, Some(region), true) };
    if set == 0 {
        let _ = unsafe { DeleteObject(region.into()) };
        return false;
    }
    true
}

fn clear_region(hwnd: HWND) {
    // SAFETY: 空区域把窗口恢复成矩形。空句柄不需要释放。
    let _ = unsafe { SetWindowRgn(hwnd, None, true) };
}

fn has_region(hwnd: HWND) -> bool {
    let probe = unsafe { CreateRectRgn(0, 0, 0, 0) };
    if probe.is_invalid() {
        return false;
    }
    // SAFETY: probe 是刚创建的区域，调用后由这里释放。
    let kind = unsafe { GetWindowRgn(hwnd, probe) };
    let _ = unsafe { DeleteObject(probe.into()) };
    region_present(kind)
}

#[cfg(test)]
fn region_is_grip(hwnd: HWND, grip: GripRect) -> bool {
    let probe = unsafe { CreateRectRgn(0, 0, 0, 0) };
    if probe.is_invalid() {
        return false;
    }
    // SAFETY: probe 指向本函数创建的区域。
    let kind = unsafe { GetWindowRgn(hwnd, probe) };
    let mut bounds = RECT::default();
    let boxed = unsafe { GetRgnBox(probe, &mut bounds) };
    let _ = unsafe { DeleteObject(probe.into()) };
    // 圆角区域一般是 COMPLEXREGION。外接矩形应等于把手；类型只要不是空或失败即可。
    let right = grip.x.saturating_add_unsigned(grip.width);
    let bottom = grip.y.saturating_add_unsigned(grip.height);
    region_present(kind)
        && region_present(boxed)
        && (bounds.left - grip.x).abs() <= 1
        && (bounds.top - grip.y).abs() <= 1
        && (bounds.right - right).abs() <= 1
        && (bounds.bottom - bottom).abs() <= 1
}

fn region_present(kind: GDI_REGION_TYPE) -> bool {
    kind != RGN_ERROR && kind != NULLREGION
}

fn sync_mouse_hook() {
    // 探测测试不挂全局钩子。正式进程在 `set_log` 之后才会收起窗口。
    let logging = LOG.with(|cell| cell.borrow().is_some());
    let need =
        logging && FRAMES.with(|frames| frames.borrow().values().any(|frame| frame.collapsed));
    if need {
        install_mouse_hook();
    } else {
        clear_pad_window();
        uninstall_mouse_hook();
    }
}

fn install_mouse_hook() {
    if MOUSE_HOOK.load(Ordering::Acquire) != 0 {
        return;
    }
    // SAFETY: 钩子过程不带捕获，卸掉之前一直有效。模块句柄是本进程。
    let module = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }.ok();
    let installed = unsafe {
        SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_hook),
            module.map(|module| module.into()),
            0,
        )
    };
    match installed {
        Ok(hook) => {
            MOUSE_HOOK.store(hook.0 as isize, Ordering::Release);
        }
        Err(err) => log_line(&format!("收起点击区的鼠标钩子没有装上：{err}")),
    }
}

fn uninstall_mouse_hook() {
    let raw = MOUSE_HOOK.swap(0, Ordering::AcqRel);
    if raw == 0 {
        return;
    }
    // SAFETY: raw 是本模块装上的钩子，只卸一次。
    let _ = unsafe { UnhookWindowsHookEx(HHOOK(raw as *mut c_void)) };
}

fn clear_pad_if(hwnd: HWND) {
    let key = hwnd_key(hwnd);
    if PAD_WINDOW.load(Ordering::Acquire) == key {
        PAD_WINDOW.store(0, Ordering::Release);
        post_pad(hwnd, false);
    }
}

fn clear_pad_window() {
    let raw = PAD_WINDOW.swap(0, Ordering::AcqRel);
    if raw != 0 {
        post_pad(HWND(raw as *mut c_void), false);
    }
}

fn post_pad(hwnd: HWND, inside: bool) {
    let _ = unsafe {
        PostMessageW(
            Some(hwnd),
            WM_PAD_HOVER,
            WPARAM(usize::from(inside)),
            LPARAM(0),
        )
    };
}

fn any_collapsed() -> bool {
    FRAMES.with(|frames| {
        frames
            .try_borrow()
            .ok()
            .is_some_and(|frames| frames.values().any(|frame| frame.collapsed))
    })
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let message = wparam.0 as u32;
    let tracked = message == WM_MOUSEMOVE || message == WM_LBUTTONDOWN || message == WM_LBUTTONUP;
    if tracked && lparam.0 != 0 && any_collapsed() {
        // SAFETY: WH_MOUSE_LL 的 lParam 指向 MSLLHOOKSTRUCT，在钩子返回前有效。
        let info = unsafe {
            &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::MSLLHOOKSTRUCT)
        };
        if let Some(hwnd) = padding_target(info.pt) {
            forward_pointer(hwnd, message, info.pt);
            set_pad_target(hwnd, true);
            return LRESULT(1);
        }
        set_pad_target(HWND::default(), false);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn set_pad_target(hwnd: HWND, inside: bool) {
    let key = if inside { hwnd_key(hwnd) } else { 0 };
    let previous = PAD_WINDOW.swap(key, Ordering::AcqRel);
    if previous == key {
        return;
    }
    if previous != 0 {
        post_pad(HWND(previous as *mut c_void), false);
    }
    if key != 0 {
        post_pad(hwnd, true);
    }
}

fn padding_target(screen: POINT) -> Option<HWND> {
    let hwnd = FRAMES.with(|frames| {
        let Ok(frames) = frames.try_borrow() else {
            return None;
        };
        let mut found: Option<HWND> = None;
        for (key, frame) in frames.iter() {
            if !frame.collapsed || bar_win::size_move_blocks_blur(*key) {
                continue;
            }
            let hwnd = HWND(*key as *mut c_void);
            if !is_visible(hwnd) || is_minimized(hwnd) {
                continue;
            }
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
                continue;
            }
            if screen.x < rect.left
                || screen.y < rect.top
                || screen.x >= rect.right
                || screen.y >= rect.bottom
            {
                continue;
            }
            let mut local = screen;
            if !unsafe { ScreenToClient(hwnd, &mut local) }.as_bool() {
                continue;
            }
            let mut client = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
                continue;
            }
            let width = u32::try_from(client.right.max(0)).unwrap_or(0);
            let height = u32::try_from(client.bottom.max(0)).unwrap_or(0);
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
            if !frame_policy::point_in_padding(width, height, dpi, local.x, local.y) {
                continue;
            }
            found = match found {
                Some(previous) if is_above(previous, hwnd) => Some(previous),
                _ => Some(hwnd),
            };
        }
        found
    })?;
    let under = unsafe { WindowFromPoint(screen) };
    if under == hwnd {
        return None;
    }
    is_above(hwnd, under).then_some(hwnd)
}

fn pack_point(x: i32, y: i32) -> i32 {
    // 低 16 位是客户区坐标，和 `lParam` 的惯例一样，负数要保留原码。
    let x = x.cast_unsigned() & 0xffff;
    let y = y.cast_unsigned() & 0xffff;
    i32::from_ne_bytes((y << 16 | x).to_ne_bytes())
}

fn is_above(upper: HWND, lower: HWND) -> bool {
    if upper == lower || upper.0.is_null() {
        return false;
    }
    if lower.0.is_null() {
        return true;
    }
    // SAFETY: 只读顶层窗口的层次，不保留句柄。
    let mut cursor = match unsafe { GetTopWindow(None) } {
        Ok(hwnd) => hwnd,
        Err(_) => return false,
    };
    for _ in 0..512 {
        if cursor.0.is_null() {
            return false;
        }
        if cursor == upper {
            return true;
        }
        if cursor == lower {
            return false;
        }
        cursor = match unsafe { GetWindow(cursor, GW_HWNDNEXT) } {
            Ok(next) => next,
            Err(_) => return false,
        };
    }
    false
}

fn forward_pointer(hwnd: HWND, message: u32, screen: POINT) {
    let mut local = screen;
    if !unsafe { ScreenToClient(hwnd, &mut local) }.as_bool() {
        return;
    }
    let packed = pack_point(local.x, local.y);
    let button = if message == WM_LBUTTONUP { 0 } else { 1 };
    let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(button), LPARAM(packed as isize)) };
}

fn log_line(message: &str) {
    LOG.with(|cell| {
        if let Some(store) = cell.borrow().as_ref() {
            store.log_info(message);
        }
    });
}

fn note_shell_message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) {
    let Some(summary) = shell_message(message, wparam, lparam) else {
        return;
    };
    log_line(&format!(
        "收起悬浮窗消息 hwnd={} {summary} visible={} iconic={}",
        hwnd_key(hwnd),
        u8::from(is_visible(hwnd)),
        u8::from(is_minimized(hwnd))
    ));
}

fn shell_message(message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<String> {
    Some(match message {
        WM_SIZE => format!("WM_SIZE wparam={:#x}", wparam.0),
        WM_SHOWWINDOW => format!("WM_SHOWWINDOW show={}", wparam.0),
        WM_ACTIVATE => format!("WM_ACTIVATE active={}", u8::from(wparam.0 != WA_INACTIVE)),
        WM_ACTIVATEAPP => format!("WM_ACTIVATEAPP active={}", wparam.0),
        WM_SYSCOMMAND => {
            let command = wparam.0 & 0xFFF0;
            if command != SC_MINIMIZE && command != 0xF120 && command != 0xF030 {
                return None;
            }
            format!("WM_SYSCOMMAND cmd={command:#x}")
        }
        WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED if lparam.0 != 0 => {
            let pos = unsafe { &*(lparam.0 as *const WINDOWPOS) };
            let hide = pos.flags.contains(SWP_HIDEWINDOW);
            let iconic = is_iconic_origin(pos.x, pos.y) && !pos.flags.contains(SWP_NOMOVE);
            if !hide && !iconic {
                return None;
            }
            let name = if message == WM_WINDOWPOSCHANGING {
                "WM_WINDOWPOSCHANGING"
            } else {
                "WM_WINDOWPOSCHANGED"
            };
            format!(
                "{name} hide={hide} iconic={iconic} flags={:#x}",
                pos.flags.0
            )
        }
        WM_THEMECHANGED => "WM_THEMECHANGED".to_owned(),
        WM_DWMCOMPOSITIONCHANGED => "WM_DWMCOMPOSITIONCHANGED".to_owned(),
        WM_SETTINGCHANGE => format!("WM_SETTINGCHANGE wparam={:#x}", wparam.0),
        WM_DPICHANGED => format!("WM_DPICHANGED wparam={:#x}", wparam.0),
        WM_DISPLAYCHANGE => "WM_DISPLAYCHANGE".to_owned(),
        _ => return None,
    })
}

fn block_pos_change(pos: &mut WINDOWPOS, action: FrameAction) {
    if !action.block_hide {
        return;
    }
    pos.flags &= !SWP_HIDEWINDOW;
    if is_iconic_origin(pos.x, pos.y) && !pos.flags.contains(SWP_NOMOVE) {
        pos.flags |= SWP_NOMOVE | SWP_NOSIZE;
    }
}

unsafe extern "system" fn frame_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        forget(hwnd);
        // SAFETY: 窗口正在销毁，移除本模块装上的子类。之后的消息直接交给下一层。
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(frame_subclass), FRAME_SUBCLASS_ID);
        }
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }

    let Some((collapsed, _)) = state(hwnd) else {
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    };

    if collapsed {
        note_shell_message(hwnd, message, wparam, lparam);
    }

    if message == WM_PAD_HOVER {
        let inside = wparam.0 != 0;
        let key = hwnd_key(hwnd);
        let hooks = PAD_HOOKS.with(|hooks| hooks.borrow().clone());
        for hook in hooks {
            hook(key, inside);
        }
        return LRESULT(0);
    }

    if message == WM_STYLECHANGING && wparam.0 as i32 == GWL_EXSTYLE.0 && lparam.0 != 0 && collapsed
    {
        // SAFETY: WM_STYLECHANGING 的 lParam 指向系统传来的 STYLESTRUCT，处理期间可写。
        let change = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
        change.styleNew |= WS_EX_TOOLWINDOW.0;
    }

    if message == WM_WINDOWPOSCHANGING && lparam.0 != 0 && collapsed {
        // SAFETY: WM_WINDOWPOSCHANGING 的 lParam 指向系统传来的 WINDOWPOS，处理期间可写。
        let pos = unsafe { &mut *(lparam.0 as *mut WINDOWPOS) };
        let notice = FrameNotice::PosChanging {
            hide: pos.flags.contains(SWP_HIDEWINDOW),
            iconic: is_iconic_origin(pos.x, pos.y) && !pos.flags.contains(SWP_NOMOVE),
        };
        let action = frame_policy::frame_action(true, notice);
        block_pos_change(pos, action);
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        // 后面的窗口过程可能又把隐藏标志加回来。返回前再清一次，系统用的是这份结构。
        block_pos_change(pos, action);
        if action.reapply_again {
            request_follow_up(hwnd);
        }
        return result;
    }

    if message == WM_SYSCOMMAND && collapsed && (wparam.0 & 0xFFF0) == SC_MINIMIZE {
        let action = frame_policy::frame_action(true, FrameNotice::SysMinimize);
        if action.block_hide {
            return LRESULT(0);
        }
    }

    if message == WM_REAPPLY_FRAME {
        finish_follow_up(hwnd);
        return LRESULT(0);
    }

    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if let Some(notice) = notice_from(message, wparam) {
        on_notice(hwnd, notice);
    }
    result
}

fn notice_from(message: u32, wparam: WPARAM) -> Option<FrameNotice> {
    Some(match message {
        WM_SIZE if wparam.0 == SIZE_MINIMIZED => FrameNotice::SizeMinimized,
        WM_SIZE => FrameNotice::SizeRestored,
        WM_SHOWWINDOW if wparam.0 == 0 => FrameNotice::Hidden,
        WM_SHOWWINDOW => FrameNotice::Shown,
        WM_THEMECHANGED | WM_DWMCOMPOSITIONCHANGED | WM_SETTINGCHANGE => {
            FrameNotice::ThemeOrComposition
        }
        WM_DPICHANGED => FrameNotice::DpiChanged,
        WM_ACTIVATE => FrameNotice::Activated,
        WM_EXITSIZEMOVE => FrameNotice::SizeMoveEnded,
        _ => return None,
    })
}

/// 给探测和测试读回的 DWM 属性。读失败的字段是 `None`。
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameReadback {
    pub corner: Option<i32>,
    pub border: Option<u32>,
    pub backdrop: Option<u32>,
    pub nc_policy: Option<i32>,
    pub tool_window: bool,
}

#[cfg(test)]
fn read_frame(hwnd: HWND) -> FrameReadback {
    FrameReadback {
        corner: read_i32(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE),
        border: read_u32(hwnd, DWMWA_BORDER_COLOR),
        backdrop: read_u32(hwnd, DWMWA_SYSTEMBACKDROP_TYPE),
        nc_policy: read_i32(hwnd, DWMWA_NCRENDERING_POLICY),
        tool_window: (unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
            & WS_EX_TOOLWINDOW.0 as isize)
            != 0,
    }
}

#[cfg(test)]
fn read_i32(
    hwnd: HWND,
    attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE,
) -> Option<i32> {
    let mut value = 0i32;
    let size = u32::try_from(std::mem::size_of_val(&value)).ok()?;
    // SAFETY: value 指向局部变量，大小与 size 一致。
    unsafe { DwmGetWindowAttribute(hwnd, attribute, std::ptr::from_mut(&mut value).cast(), size) }
        .ok()
        .map(|()| value)
}

#[cfg(test)]
fn read_u32(
    hwnd: HWND,
    attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE,
) -> Option<u32> {
    let mut value = 0u32;
    let size = u32::try_from(std::mem::size_of_val(&value)).ok()?;
    // SAFETY: value 指向局部变量，大小与 size 一致。
    unsafe { DwmGetWindowAttribute(hwnd, attribute, std::ptr::from_mut(&mut value).cast(), size) }
        .ok()
        .map(|()| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMSBT_NONE, DWMWA_COLOR_NONE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
        DwmIsCompositionEnabled,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        HMENU, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_MINIMIZE, TranslateMessage,
        WNDCLASSW, WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE,
    };
    use windows::core::PCWSTR;

    struct ProbeWindow(HWND);

    impl Drop for ProbeWindow {
        fn drop(&mut self) {
            forget(self.0);
            // SAFETY: 句柄是这个测试刚创建的窗口，测试结束就销毁。
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    fn composition_enabled() -> bool {
        // SAFETY: 没有输出指针，返回值就是组合是否开启。
        unsafe { DwmIsCompositionEnabled() }.is_ok_and(|enabled| enabled.as_bool())
    }

    unsafe extern "system" fn probe_wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    fn create_probe() -> Result<ProbeWindow, String> {
        let class_name: Vec<u16> = "LanworkFrameProbe\0".encode_utf16().collect();
        // SAFETY: 取本进程模块，不保留额外状态。
        let instance = unsafe { GetModuleHandleW(None) }.map_err(|err| err.to_string())?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(probe_wndproc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: class 指向局部变量，类名在函数返回前一直有效；注册成功后类留到进程结束。
        // 类已经注册过时会失败，创建窗口会给出真正的错误。
        let _ = unsafe { RegisterClassW(&class) };
        // SAFETY: 类名指向仍有效的局部缓冲。窗口是无主弹出窗口，测试结束时 Drop 销毁。
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WS_POPUP | WS_VISIBLE,
                40,
                40,
                48,
                9,
                None,
                None::<HMENU>,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|err| err.to_string())?;
        Ok(ProbeWindow(hwnd))
    }

    fn pump(hwnd: HWND) {
        for _ in 0..64 {
            let mut msg = MSG::default();
            // SAFETY: msg 指向局部变量。只取这个探测窗口的消息。
            let found = unsafe { PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE) };
            if !found.as_bool() {
                break;
            }
            // SAFETY: msg 刚从队列取出，仍指向这块局部变量。
            let _ = unsafe { TranslateMessage(&msg) };
            unsafe { DispatchMessageW(&msg) };
        }
    }

    #[test]
    fn collapsed_attributes_come_back_after_a_rounded_acrylic_reset() {
        if !composition_enabled() {
            eprintln!("DWM 组合没开，跳过收起样式读回");
            return;
        }
        let window = create_probe().expect("探测窗口");
        let hwnd = window.0;
        sync(hwnd, true, false);
        let _ = bar_win::apply_dwm(hwnd, lanwork_core::shell::Backdrop::Acrylic, false);
        let dirty = read_frame(hwnd);
        assert_eq!(
            dirty.corner,
            Some(DWMWCP_ROUND.0),
            "模拟重置后应是圆角，实际 {dirty:?}"
        );

        sync(hwnd, true, false);
        pump(hwnd);
        let fixed = read_frame(hwnd);
        assert_eq!(
            fixed.corner,
            Some(DWMWCP_DONOTROUND.0),
            "收起后应关掉圆角，实际 {fixed:?}"
        );
        assert_eq!(
            fixed.backdrop,
            Some(DWMSBT_NONE.0 as u32),
            "收起后应没有系统背景，实际 {fixed:?}"
        );
        // 边框色和非客户区策略可以设，这一版 `DwmGetWindowAttribute` 读不回来。
        // 读得回来时必须是关掉的值。
        if let Some(border) = fixed.border {
            assert_eq!(
                border, DWMWA_COLOR_NONE,
                "收起后应没有边框色，实际 {fixed:?}"
            );
        }
        if let Some(policy) = fixed.nc_policy {
            assert_eq!(
                policy, DWMNCRP_DISABLED.0,
                "收起后应关掉非客户区，实际 {fixed:?}"
            );
        }
        assert!(fixed.tool_window, "收起后应是工具窗口，实际 {fixed:?}");

        // 展开后再收起，工具窗口位要能去掉再加回来。
        sync(hwnd, false, false);
        assert!(!read_frame(hwnd).tool_window);
        sync(hwnd, true, false);
        assert!(read_frame(hwnd).tool_window);
        assert_grip_region(hwnd);
    }

    #[test]
    fn show_desktop_minimize_does_not_leave_a_collapsed_window_iconic() {
        if !composition_enabled() {
            eprintln!("DWM 组合没开，跳过最小化探测");
            return;
        }
        let window = create_probe().expect("探测窗口");
        let hwnd = window.0;
        sync(hwnd, true, false);
        // SAFETY: hwnd 是这个测试的窗口。SW_MINIMIZE 走和一键回到桌面相近的隐藏路径。
        let _ = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
        pump(hwnd);
        assert!(
            !is_minimized(hwnd),
            "收起的窗口在最小化之后仍是图标状态，读回 {:?}",
            read_frame(hwnd)
        );
        let fixed = read_frame(hwnd);
        assert_eq!(fixed.corner, Some(DWMWCP_DONOTROUND.0), "{fixed:?}");
        assert!(fixed.tool_window, "{fixed:?}");
        assert_grip_region(hwnd);
        assert!(is_visible(hwnd), "最小化之后应收起来的窗口还看得见");

        // 模拟外壳先藏起窗口。子类在消息返回后把它再显示出来，区域仍是把手。
        let _ = unsafe { ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE) };
        pump(hwnd);
        assert!(is_visible(hwnd), "藏起来之后应收起来的窗口再显示");
        assert!(!is_minimized(hwnd));
        assert_grip_region(hwnd);

        sync(hwnd, false, false);
        assert!(!has_region(hwnd), "展开后窗口应恢复成矩形，不再裁成把手");
    }

    fn assert_grip_region(hwnd: HWND) {
        let mut client = RECT::default();
        unsafe { GetClientRect(hwnd, &mut client) }.expect("客户区");
        let width = u32::try_from(client.right).unwrap_or(0);
        let height = u32::try_from(client.bottom).unwrap_or(0);
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let grip = frame_policy::collapsed_grip(width, height, dpi);
        assert!(
            region_is_grip(hwnd, grip),
            "窗口区域应是把手 {grip:?}，客户区 {width}x{height} dpi={dpi}"
        );
        let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
        assert!(!region.is_invalid());
        let kind = unsafe { GetWindowRgn(hwnd, region) };
        assert_ne!(kind, RGN_ERROR, "读得到窗口区域");
        let center = POINT {
            x: grip.x + i32::try_from(grip.width / 2).unwrap_or(0),
            y: grip.y + i32::try_from(grip.height / 2).unwrap_or(0),
        };
        assert!(
            unsafe { windows::Win32::Graphics::Gdi::PtInRegion(region, center.x, center.y) }
                .as_bool(),
            "把手中心应在区域内"
        );
        if grip.x > 0 || grip.y > 0 {
            assert!(
                !unsafe { windows::Win32::Graphics::Gdi::PtInRegion(region, 0, 0) }.as_bool(),
                "点击区角落不应在区域内，否则透明失效时会铺成色块"
            );
        }
        let _ = unsafe { DeleteObject(region.into()) };
    }
}
