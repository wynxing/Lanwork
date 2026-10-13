//! 便签和收纳悬浮窗的 DWM 样式。收起时保持逐像素透明，展开时回到亚克力或纯色。
//!
//! winit 只在创建窗口时调用一次空区域的 `DwmEnableBlurBehindWindow`。一键回到桌面、
//! 最小化后还原、主题或 DPI 变化，以及 `SetWindowPos`，都会把这次设置清掉，DWM 同时
//! 把圆角和系统背景设回来。点击区（约 48×9 逻辑像素）就会变成一块不透明的圆角色块。
//! 这里在那些消息上按当前状态重设，并在消息返回后再补一次。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
#[cfg(test)]
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{RDW_FRAME, RDW_INVALIDATE, RedrawWindow};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowPlacement, IsIconic, IsWindowVisible, PostMessageW,
    STYLESTRUCT, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, ShowWindow,
    WINDOWPLACEMENT, WINDOWPOS, WM_ACTIVATE, WM_APP, WM_DPICHANGED, WM_EXITSIZEMOVE, WM_NCDESTROY,
    WM_SETTINGCHANGE, WM_SHOWWINDOW, WM_SIZE, WM_STYLECHANGING, WM_SYSCOMMAND, WM_THEMECHANGED,
    WM_WINDOWPOSCHANGING, WS_EX_TOOLWINDOW,
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

struct FrameState {
    collapsed: bool,
    dark: bool,
    /// 已经投了一条补设消息，处理完之前不再投。
    follow_up: bool,
}

thread_local! {
    static FRAMES: RefCell<HashMap<isize, FrameState>> = RefCell::new(HashMap::new());
    static APPLY_DEPTH: Cell<u32> = const { Cell::new(0) };
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
        let follow_up = frames
            .get(&hwnd_key(hwnd))
            .is_some_and(|frame| frame.follow_up);
        frames.insert(
            hwnd_key(hwnd),
            FrameState {
                collapsed,
                dark,
                follow_up,
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
            fixed.border,
            Some(DWMWA_COLOR_NONE),
            "收起后应没有边框色，实际 {fixed:?}"
        );
        assert_eq!(
            fixed.backdrop,
            Some(DWMSBT_NONE.0 as u32),
            "收起后应没有系统背景，实际 {fixed:?}"
        );
        assert_eq!(
            fixed.nc_policy,
            Some(DWMNCRP_DISABLED.0),
            "收起后应关掉非客户区，实际 {fixed:?}"
        );
        assert!(fixed.tool_window, "收起后应是工具窗口，实际 {fixed:?}");

        // 展开后再收起，工具窗口位要能去掉再加回来。
        sync(hwnd, false, false);
        assert!(!read_frame(hwnd).tool_window);
        sync(hwnd, true, false);
        assert!(read_frame(hwnd).tool_window);
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
    }
}
