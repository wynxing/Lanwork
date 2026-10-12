//! 设置页录入热键时拦截面板窗口的键盘消息。
//!
//! 录入期间按下的键不能交给 Slint：Alt 组合键会激活窗口菜单，Esc 会收起面板。
//! 所以在面板窗口上装一个子类，在 winit 之前看到 `WM_KEYDOWN`、`WM_SYSKEYDOWN` 和字符消息，
//! 交给 [`KeyGate`] 决定吞掉还是放行。已注册的全局热键由平台线程在录入前卸掉，见 `platform`。

use std::cell::RefCell;

use lanwork_core::shell::{KeyEvent, KeyGate, KeyMods, Recorded, Route};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_CHAR, WM_DEADCHAR, WM_KEYDOWN, WM_KEYUP, WM_NCDESTROY, WM_SYSCHAR, WM_SYSDEADCHAR,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const SUBCLASS_ID: usize = 3;
/// `WM_KEYDOWN` 的 lParam 第 30 位：这个键在这条消息之前已经按下。
const REPEAT_BIT: isize = 1 << 30;

struct State {
    gate: KeyGate,
    sink: Option<fn(Recorded)>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State { gate: KeyGate::new(), sink: None })
    };
}

/// 在窗口上装键盘子类。`sink` 在录入结束时调用，在界面线程上、窗口过程里，所以应当只排队。
pub(crate) fn install(hwnd: HWND, sink: fn(Recorded)) -> bool {
    STATE.with(|cell| cell.borrow_mut().sink = Some(sink));
    // SAFETY: 在创建窗口的线程上调用。回调是本模块的函数，不带引用数据；窗口销毁时由回调自己移除。
    unsafe { SetWindowSubclass(hwnd, Some(capture_keys), SUBCLASS_ID, 0) }.as_bool()
}

/// 开始录入。已经在录入时返回 false。
pub(crate) fn begin() -> bool {
    STATE.with(|cell| cell.borrow_mut().gate.begin())
}

/// 放弃录入，并忘掉还在等待抬起的键。
pub(crate) fn reset() {
    STATE.with(|cell| cell.borrow_mut().gate.reset());
}

fn pressed(key: VIRTUAL_KEY) -> bool {
    // SAFETY: 只读当前线程消息队列的按键状态。
    let state = unsafe { GetKeyState(i32::from(key.0)) };
    state.cast_unsigned() & 0x8000 != 0
}

fn current_mods() -> KeyMods {
    KeyMods {
        control: pressed(VK_CONTROL),
        alt: pressed(VK_MENU),
        shift: pressed(VK_SHIFT),
        win: pressed(VK_LWIN) || pressed(VK_RWIN),
    }
}

fn key_event(message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<KeyEvent> {
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyEvent::Down {
            vk: u32::try_from(wparam.0).unwrap_or(0),
            mods: current_mods(),
            repeat: lparam.0 & REPEAT_BIT != 0,
        }),
        WM_KEYUP | WM_SYSKEYUP => Some(KeyEvent::Up {
            vk: u32::try_from(wparam.0).unwrap_or(0),
        }),
        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => Some(KeyEvent::Char),
        _ => None,
    }
}

unsafe extern "system" fn capture_keys(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        // SAFETY: 窗口正在销毁，移除本模块装上的子类。之后的消息直接交给下一层。
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(capture_keys), SUBCLASS_ID);
        }
    } else if let Some(event) = key_event(message, wparam, lparam) {
        let (route, sink) = STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            (state.gate.route(event), state.sink)
        });
        match route {
            Route::Pass => {}
            Route::Swallow => return LRESULT(0),
            Route::Recorded(recorded) => {
                if let Some(sink) = sink {
                    sink(recorded);
                }
                return LRESULT(0);
            }
        }
    }
    // SAFETY: 参数原样交给子类链的下一层。
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
