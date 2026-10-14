//! 便签和收纳悬浮窗的 DWM 样式，以及收起后的窗口区域。
//!
//! winit 只在创建窗口时调用一次空区域的 `DwmEnableBlurBehindWindow`。一键回到桌面
//! （外壳抬起 `WorkerW`，不一定给这个窗口发最小化）之后，这次透明会被清掉，渲染器
//! 再把客户区清成不透明的颜色。重设 DWM 属性盖不住这块颜色。收起后的窗口就是把手
//! （3×40 逻辑像素，按 DPI 放大），再用 `SetWindowRgn` 收成圆角，方块的角不属于窗口。
//! 悬停靠窗口自己的 `WM_MOUSEMOVE` / `WM_MOUSELEAVE`，不装全局鼠标钩子。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;

use lanwork_core::storage::Store;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
#[cfg(test)]
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateRoundRectRgn, DeleteObject, GDI_REGION_TYPE, GetWindowRgnBox, NULLREGION,
    RDW_FRAME, RDW_INVALIDATE, RGN_ERROR, RedrawWindow, SetWindowRgn,
};
#[cfg(test)]
use windows::Win32::Graphics::Gdi::{CreateRectRgn, GetWindowRgn, PtInRegion};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, IsIconic,
    IsWindowVisible, PostMessageW, STYLESTRUCT, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
    SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, ShowWindow, WINDOWPLACEMENT, WINDOWPOS, WM_ACTIVATE,
    WM_ACTIVATEAPP, WM_APP, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_EXITSIZEMOVE, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_SETTINGCHANGE, WM_SHOWWINDOW, WM_SIZE, WM_STYLECHANGING, WM_SYSCOMMAND,
    WM_THEMECHANGED, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING, WS_EX_TOOLWINDOW,
};

use slint::winit_030::winit;

use crate::bar_win::{self, hwnd_key};
use crate::frame_policy::{self, FrameAction, FrameNotice, is_iconic_origin};

const FRAME_SUBCLASS_ID: usize = 3;
const WM_REAPPLY_FRAME: u32 = WM_APP + 1;
const WM_DWMCOMPOSITIONCHANGED: u32 = 0x031E;
const SC_MINIMIZE: usize = 0xF020;
/// `SIZE_MINIMIZED`。
const SIZE_MINIMIZED: usize = 1;
const WA_INACTIVE: usize = 0;

/// 客户区在窗口坐标系里的矩形。`SetWindowRgn` 的原点是窗口左上角，不是客户区左上角。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GripRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
struct FrameState {
    collapsed: bool,
    dark: bool,
    /// 已经投了一条补设消息，处理完之前不再投。
    follow_up: bool,
    /// 已经为这一次进入调用过 `TrackMouseEvent`，离开消息到达前不再调。
    tracking_leave: bool,
    /// 上次成功设上的把手区域。展开时是 `None`。
    grip: Option<GripRect>,
}

thread_local! {
    static FRAMES: RefCell<HashMap<isize, FrameState>> = RefCell::new(HashMap::new());
    static APPLY_DEPTH: Cell<u32> = const { Cell::new(0) };
    static LOG: RefCell<Option<Store>> = const { RefCell::new(None) };
}

/// 把收起窗口在 Win+D 上收到的消息写进 `logs/app.log`。探测测试不调用。
pub(crate) fn set_log(store: Store) {
    LOG.with(|cell| *cell.borrow_mut() = Some(store));
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
    FRAMES.with(|frames| {
        frames.borrow_mut().remove(&hwnd_key(hwnd));
    });
}

fn remember(hwnd: HWND, collapsed: bool, dark: bool) {
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let previous = frames.get(&hwnd_key(hwnd)).copied();
        frames.insert(
            hwnd_key(hwnd),
            FrameState {
                collapsed,
                dark,
                follow_up: previous.is_some_and(|frame| frame.follow_up),
                tracking_leave: previous
                    .is_some_and(|frame| frame.tracking_leave && frame.collapsed && collapsed),
                grip: previous.and_then(|frame| frame.grip),
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
        bar_win::apply_float_backdrop(hwnd, bar_win::current_backdrop(), dark)
    };
    // 区域放在 DWM 和扩展样式之后。扩展客户区会让客户区原点相对窗口原点挪开，
    // 裁剪要用挪开之后的窗口坐标。裁剪不依赖透明有没有设上。
    sync_shape(hwnd, collapsed);
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
        // 同一块把手还在、外接矩形也对得上时不重复设，避免每次置顶都写一条日志。
        // 只问「有没有区域」不够：DWM 重置会留下另一块区域，读回来却不是把手。
        if remembered == Some(grip) && region_matches(hwnd, grip) {
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
    let mut window = RECT::default();
    // SAFETY: hwnd 是本进程的窗口，输出指向局部变量。
    unsafe { GetWindowRect(hwnd, &mut window) }.ok()?;
    let mut origin = POINT { x: 0, y: 0 };
    // SAFETY: 把客户区原点换成屏幕坐标。失败时设不出和读回一致的窗口区域。
    if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
        return None;
    }
    let mut client = RECT::default();
    // SAFETY: hwnd 是本进程的窗口，输出指向局部变量。
    unsafe { GetClientRect(hwnd, &mut client) }.ok()?;
    let width = u32::try_from(client.right.max(0)).ok()?;
    let height = u32::try_from(client.bottom.max(0)).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(GripRect {
        x: origin.x.saturating_sub(window.left),
        y: origin.y.saturating_sub(window.top),
        width,
        height,
    })
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
    // `CreateRoundRectRgn` 的右、下是开区间，调用时再各少一像素。
    // 直接传入把手的右、下时，40×3 读回来是 (1,0,38,2)，底边和右端被裁掉。
    // 右、下各加 1，区域才盖住看得见的把手。圆角仍会吃掉方框的角。
    // SAFETY: 圆角椭圆的直径等于把手厚度，两端是半圆。区域在设成功之后由系统接管。
    let region = unsafe {
        CreateRoundRectRgn(
            grip.x,
            grip.y,
            right.saturating_add(1),
            bottom.saturating_add(1),
            thick,
            thick,
        )
    };
    if region.is_invalid() {
        return false;
    }
    // SAFETY: hwnd 是本进程的窗口。成功时系统取得 region 的所有权，失败时由这里释放。
    // 非 0 才算设上。`HRGN` 是复制句柄，离开作用域不会释放。
    // 不在这里重绘：`SetWindowRgn` 的重绘会同步发出 `WM_WINDOWPOSCHANGING`，
    // 调用方已经在设完样式之后统一 `RedrawWindow`。
    let set = unsafe { SetWindowRgn(hwnd, Some(region), false) };
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
    region_box(hwnd).is_some()
}

fn region_matches(hwnd: HWND, grip: GripRect) -> bool {
    region_box(hwnd).is_some_and(|bounds| region_covers_grip(bounds, grip))
}

fn region_box(hwnd: HWND) -> Option<RECT> {
    let mut bounds = RECT::default();
    // SAFETY: bounds 指向局部变量。外接矩形用窗口坐标，不经过一块空的目标区域。
    let kind = unsafe { GetWindowRgnBox(hwnd, &mut bounds) };
    region_present(kind).then_some(bounds)
}

fn region_covers_grip(bounds: RECT, grip: GripRect) -> bool {
    let right = grip.x.saturating_add_unsigned(grip.width);
    let bottom = grip.y.saturating_add_unsigned(grip.height);
    // 圆角只该吃掉方框的角，每一边最多缩进 1 像素。缩得更多就把把手裁掉。
    // GDI 有时会比传入的矩形多 1 像素，窗口外的部分不会画出来。
    bounds.left <= grip.x.saturating_add(1)
        && bounds.top <= grip.y.saturating_add(1)
        && bounds.right >= right.saturating_sub(1)
        && bounds.bottom >= bottom.saturating_sub(1)
        && bounds.left >= grip.x.saturating_sub(1)
        && bounds.top >= grip.y.saturating_sub(1)
        && bounds.right <= right.saturating_add(1)
        && bounds.bottom <= bottom.saturating_add(1)
}

fn region_present(kind: GDI_REGION_TYPE) -> bool {
    kind != RGN_ERROR && kind != NULLREGION
}

fn note_pointer(hwnd: HWND, message: u32) {
    if message == WM_MOUSEMOVE {
        let armed = FRAMES.with(|frames| {
            frames
                .borrow()
                .get(&hwnd_key(hwnd))
                .is_some_and(|frame| frame.tracking_leave)
        });
        if !armed {
            arm_mouse_leave(hwnd);
        }
    } else if message == WM_MOUSELEAVE {
        FRAMES.with(|frames| {
            if let Some(frame) = frames.borrow_mut().get_mut(&hwnd_key(hwnd)) {
                frame.tracking_leave = false;
            }
        });
    }
}

fn arm_mouse_leave(hwnd: HWND) {
    let mut event = TRACKMOUSEEVENT {
        cbSize: u32::try_from(std::mem::size_of::<TRACKMOUSEEVENT>()).unwrap_or(0),
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: event 指向局部变量。成功之后系统会在光标离开窗口时投递 `WM_MOUSELEAVE`。
    if unsafe { TrackMouseEvent(&mut event) }.is_ok() {
        FRAMES.with(|frames| {
            if let Some(frame) = frames.borrow_mut().get_mut(&hwnd_key(hwnd)) {
                frame.tracking_leave = true;
            }
        });
    }
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

    if collapsed && (message == WM_MOUSEMOVE || message == WM_MOUSELEAVE) {
        note_pointer(hwnd, message);
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
    use lanwork_core::panel::{HANDLE_LENGTH_LOGICAL, HANDLE_THICKNESS_LOGICAL};
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMSBT_NONE, DWMWA_COLOR_NONE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
        DwmIsCompositionEnabled,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
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
        // 进程还没锁过感知时，让 CreateWindow 和 SetWindowRgn 用同一套物理像素。
        // 已经锁过就沿用现状；断言比较的是同一窗口读回来的客户区，不依赖这次是否成功。
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let width = i32::try_from(HANDLE_LENGTH_LOGICAL).unwrap_or(40);
        let height = i32::try_from(HANDLE_THICKNESS_LOGICAL).unwrap_or(3);
        // SAFETY: 类名指向仍有效的局部缓冲。窗口是无主弹出窗口，测试结束时 Drop 销毁。
        // 尺寸就是横着的把手。收起后的窗口不再比把手大一圈。
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WS_POPUP | WS_VISIBLE,
                40,
                40,
                width,
                height,
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
    fn expanded_float_uses_accent_acrylic_instead_of_the_transient_backdrop() {
        if !composition_enabled() {
            eprintln!("DWM 组合没开，跳过失焦亚克力");
            return;
        }
        let window = create_probe().expect("探测窗口");
        let hwnd = window.0;
        let got =
            bar_win::apply_float_backdrop(hwnd, lanwork_core::shell::Backdrop::Acrylic, false);
        pump(hwnd);
        let frame = read_frame(hwnd);
        assert_eq!(
            got,
            lanwork_core::shell::Backdrop::Acrylic,
            "亚克力应设上，实际 {frame:?}"
        );
        assert_eq!(
            frame.backdrop,
            Some(DWMSBT_NONE.0 as u32),
            "transient 背景会在失焦时变成不透明，展开的悬浮窗不能留着它，实际 {frame:?}"
        );
        assert_eq!(frame.corner, Some(DWMWCP_ROUND.0), "{frame:?}");
        if let Some(state) = bar_win::read_accent_state(hwnd) {
            assert!(
                bar_win::accent_is_persistent_acrylic(state),
                "读回应是失焦后仍在的亚克力，实际 {state}"
            );
        }

        let solid = bar_win::apply_float_backdrop(hwnd, lanwork_core::shell::Backdrop::Solid, true);
        pump(hwnd);
        assert_eq!(solid, lanwork_core::shell::Backdrop::Solid);
        let solid_frame = read_frame(hwnd);
        assert_eq!(
            solid_frame.backdrop,
            Some(DWMSBT_NONE.0 as u32),
            "纯色回退仍是没有系统背景，实际 {solid_frame:?}"
        );
        if let Some(state) = bar_win::read_accent_state(hwnd) {
            assert!(
                !bar_win::accent_is_persistent_acrylic(state),
                "纯色回退应关掉亚克力，读回 {state}"
            );
        }

        sync(hwnd, true, false);
        pump(hwnd);
        let collapsed = read_frame(hwnd);
        assert_eq!(collapsed.corner, Some(DWMWCP_DONOTROUND.0), "{collapsed:?}");
        assert_eq!(
            collapsed.backdrop,
            Some(DWMSBT_NONE.0 as u32),
            "{collapsed:?}"
        );
        if let Some(state) = bar_win::read_accent_state(hwnd) {
            assert!(
                !bar_win::accent_is_persistent_acrylic(state),
                "收起的把手不应留着亚克力，读回 {state}"
            );
        }
        assert_grip_region(hwnd);
    }

    #[test]
    fn dot_row_drops_the_system_backdrop_so_it_stays_clear() {
        if !composition_enabled() {
            eprintln!("DWM 组合没开，跳过页点透明");
            return;
        }
        let window = create_probe().expect("探测窗口");
        let hwnd = window.0;
        let _ = bar_win::apply_float_backdrop(hwnd, lanwork_core::shell::Backdrop::Acrylic, false);
        bar_win::apply_clear_plate(hwnd);
        pump(hwnd);
        let frame = read_frame(hwnd);
        assert_eq!(
            frame.backdrop,
            Some(DWMSBT_NONE.0 as u32),
            "页点不能留着会在失焦时变成不透明的系统背景，实际 {frame:?}"
        );
        if let Some(state) = bar_win::read_accent_state(hwnd) {
            assert!(
                !bar_win::accent_is_persistent_acrylic(state),
                "页点不应铺亚克力，读回 {state}"
            );
        }
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
        let mut window = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut window) }.expect("窗口矩形");
        let grip = grip_for(hwnd).expect("把手在窗口坐标里的矩形");
        assert_eq!(
            (client.right, client.bottom),
            (
                i32::try_from(HANDLE_LENGTH_LOGICAL).unwrap_or(40),
                i32::try_from(HANDLE_THICKNESS_LOGICAL).unwrap_or(3),
            ),
            "探测窗口应是把手大小，客户区 {}x{}，窗口 ({},{},{},{})",
            client.right,
            client.bottom,
            window.left,
            window.top,
            window.right,
            window.bottom
        );
        let mut bounds = RECT::default();
        // SAFETY: bounds 指向局部变量。外接矩形是窗口坐标。
        let kind = unsafe { GetWindowRgnBox(hwnd, &mut bounds) };
        assert!(
            region_present(kind) && region_covers_grip(bounds, grip),
            "窗口区域应盖住把手。kind={kind:?} bounds=({},{},{},{}) grip={grip:?} 客户区 {}x{} 窗口原点偏移 {},{}",
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom,
            client.right,
            client.bottom,
            grip.x,
            grip.y
        );
        // 复制区域时目标不能是空区域：`GetWindowRgn` 填进 `CreateRectRgn(0,0,0,0)` 会返回 `RGN_ERROR`。
        let region = unsafe {
            CreateRectRgn(
                0,
                0,
                client.right.saturating_add(grip.x).max(1),
                client.bottom.saturating_add(grip.y).max(1),
            )
        };
        assert!(!region.is_invalid(), "能建一块用来抄窗口区域的区域");
        // SAFETY: region 是刚创建的，调用后由这里释放。
        let copied = unsafe { GetWindowRgn(hwnd, region) };
        assert!(
            region_present(copied),
            "应能抄出窗口区域 kind={copied:?} bounds=({},{},{},{})",
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom
        );
        let right = grip.x.saturating_add_unsigned(grip.width);
        let bottom = grip.y.saturating_add_unsigned(grip.height);
        let mid_x = grip.x + i32::try_from(grip.width / 2).unwrap_or(0);
        let mid_y = grip.y + i32::try_from(grip.height / 2).unwrap_or(0);
        // 直边中点和两端往里 1 像素都在看得见的把手上。圆角只去掉方框的角。
        let samples = [
            (mid_x, mid_y, "中心"),
            (grip.x.saturating_add(1), mid_y, "左端内侧"),
            (right.saturating_sub(2), mid_y, "右端内侧"),
            (mid_x, grip.y, "上沿"),
            (mid_x, bottom.saturating_sub(1), "下沿"),
        ];
        for (x, y, label) in samples {
            assert!(
                unsafe { PtInRegion(region, x, y) }.as_bool(),
                "把手{label} ({x},{y}) 应在区域内，bounds=({},{},{},{})",
                bounds.left,
                bounds.top,
                bounds.right,
                bounds.bottom
            );
        }
        let _ = unsafe { DeleteObject(region.into()) };
    }
}
