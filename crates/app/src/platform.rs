//! 热键和系统主题消息。
//!
//! winit 不把 `WM_HOTKEY` 交出来。`WM_SETTINGCHANGE` 只发给顶层窗口，不发给
//! `HWND_MESSAGE`。所以单独开一个线程，创建一个不显示的重叠窗口，在这个线程上
//! `RegisterHotKey`，并在这里等第二个实例的信号量。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use lanwork_core::config::{ConfigCommands, Hotkey};
use lanwork_core::dispatch::{Monotonic, SystemClock};
use lanwork_core::shell::{
    BindError, HotkeyPort, RegisteredHotkey, apply_hotkeys, is_immersive_color_set, resume_hotkeys,
    save_search_hotkey, suspend_hotkeys,
};
use windows::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WAIT_FAILED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, PostMessageW, PostQuitMessage, QS_ALLINPUT,
    RegisterClassW, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WM_HOTKEY, WM_QUIT,
    WM_SETTINGCHANGE, WM_USER, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::PCWSTR;

use crate::instance::Primary;
use crate::winutil::{from_wide, pcwstr, wide};

const WM_JOB: u32 = WM_USER + 1;
const WM_STOP: u32 = WM_USER + 2;
const CLASS_NAME: &str = "Lanwork.Shell";

/// 在平台线程上执行的一次热键操作。`RegisterHotKey` 属于创建窗口的线程，所以只能在那里调用。
type Job = Box<dyn FnOnce(&mut WinPort, &mut Vec<RegisteredHotkey>) + Send>;

/// 平台线程应答的最长等待。线程已经退出时，队列里的任务不会再有人执行。
const JOB_TIMEOUT: Duration = Duration::from_secs(5);

/// 设置页正在录入热键。已注册的热键此时已卸掉，迟到的 `WM_HOTKEY` 也不处理。
static RECORDING: AtomicBool = AtomicBool::new(false);

struct PortState {
    hwnd: HWND,
    current: Vec<RegisteredHotkey>,
}

thread_local! {
    static PORT: RefCell<Option<PortState>> = const { RefCell::new(None) };
}

fn job_queue() -> &'static Mutex<VecDeque<Job>> {
    static QUEUE: OnceLock<Mutex<VecDeque<Job>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

type ThemeCallback = Mutex<Option<Box<dyn Fn() + Send>>>;

fn theme_callback() -> &'static ThemeCallback {
    static CALLBACK: OnceLock<ThemeCallback> = OnceLock::new();
    CALLBACK.get_or_init(|| Mutex::new(None))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct Platform {
    hwnd: isize,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// 可以在别的线程上请求重新注册热键。窗口仍由平台线程拥有。
#[derive(Clone, Copy)]
pub(crate) struct HotkeyControl {
    hwnd: isize,
}

impl Platform {
    pub(crate) fn start(primary: Primary) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("lanwork-shell".into())
            .spawn(move || thread_main(primary, stop_thread, ready_tx))
            .map_err(|err| format!("平台线程没有启动: {err}"))?;
        let hwnd = match ready_rx.recv() {
            Ok(Ok(hwnd)) => hwnd,
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            Err(_) => {
                let _ = thread.join();
                return Err("平台线程已退出".to_owned());
            }
        };
        Ok(Self {
            hwnd,
            stop,
            thread: Some(thread),
        })
    }

    pub(crate) fn control(&self) -> HotkeyControl {
        HotkeyControl { hwnd: self.hwnd }
    }

    pub(crate) fn set_theme_callback(&self, callback: Box<dyn Fn() + Send>) {
        *lock(theme_callback()) = Some(callback);
    }

    pub(crate) fn shutdown(mut self) {
        self.stop.store(true, Ordering::Release);
        self.post(WM_STOP);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn post(&self, message: u32) {
        self.control().post(message);
    }
}

impl HotkeyControl {
    pub(crate) fn rebind(self, next: Vec<RegisteredHotkey>) -> Result<(), BindError> {
        self.run(move |port, current| {
            let result = apply_hotkeys(port, current, &next);
            if result.is_ok() {
                *current = next;
            }
            result
        })
        .unwrap_or(Err(BindError::Occupied { id: 0 }))
    }

    /// 开始录入：卸掉已注册的热键，之后按下它们不会再被系统截走。
    pub(crate) fn suspend(self) -> Result<(), String> {
        self.run(|port, current| {
            suspend_hotkeys(port, current);
            RECORDING.store(true, Ordering::Release);
        })
        .ok_or_else(|| no_reply().to_owned())
    }

    /// 结束录入：先把卸掉的热键注册回去，再按需保存搜索条热键。
    ///
    /// 注册回去时有一条被别的程序占用，就不再保存，并把「当前已注册」记成空，
    /// 这样下一次保存会把整组重新注册。`text` 为空表示只恢复，不保存。
    pub(crate) fn resume_and_save(
        self,
        commands: ConfigCommands,
        text: Option<String>,
    ) -> Result<(), String> {
        self.run(move |port, current| {
            let resumed = resume_hotkeys(port, current);
            RECORDING.store(false, Ordering::Release);
            if let Err(err) = resumed {
                current.clear();
                return Err(err.to_string());
            }
            let Some(text) = text else {
                return Ok(());
            };
            match save_search_hotkey(port, &commands, current, &text) {
                Ok(next) => {
                    *current = next;
                    Ok(())
                }
                Err(err) => Err(err.to_string()),
            }
        })
        .unwrap_or_else(|| Err(no_reply().to_owned()))
    }

    /// 把一次操作交给平台线程执行并等结果。线程没有应答时返回 `None`。
    fn run<T: Send + 'static>(
        self,
        job: impl FnOnce(&mut WinPort, &mut Vec<RegisteredHotkey>) -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        lock(job_queue()).push_back(Box::new(move |port, current| {
            let _ = tx.send(job(port, current));
        }));
        self.post(WM_JOB);
        rx.recv_timeout(JOB_TIMEOUT).ok()
    }

    fn post(self, message: u32) {
        let hwnd = HWND(self.hwnd as *mut std::ffi::c_void);
        // SAFETY: 窗口在平台线程退出前一直存在。失败表示线程已经结束。
        unsafe {
            let _ = PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.post(WM_STOP);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn no_reply() -> &'static str {
    "热键服务没有应答"
}

struct WinPort {
    hwnd: HWND,
}

impl HotkeyPort for WinPort {
    fn register(&mut self, id: i32, hotkey: &Hotkey) -> bool {
        let mut modifiers = MOD_NOREPEAT;
        if hotkey.control {
            modifiers |= MOD_CONTROL;
        }
        if hotkey.alt {
            modifiers |= MOD_ALT;
        }
        if hotkey.shift {
            modifiers |= MOD_SHIFT;
        }
        if hotkey.win {
            modifiers |= MOD_WIN;
        }
        // SAFETY: 窗口属于本线程。id 是 1 或 2。
        unsafe { RegisterHotKey(Some(self.hwnd), id, modifiers, hotkey.virtual_key()) }.is_ok()
    }

    fn unregister(&mut self, id: i32) {
        // SAFETY: 与 register 同一窗口。未注册时失败可以忽略。
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), id);
        }
    }
}

fn thread_main(primary: Primary, stop: Arc<AtomicBool>, ready: Sender<Result<isize, String>>) {
    let created = match create_window() {
        Ok(created) => created,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    PORT.with(|slot| {
        *slot.borrow_mut() = Some(PortState {
            hwnd: created.hwnd,
            current: Vec::new(),
        });
    });
    if ready.send(Ok(created.hwnd.0 as isize)).is_err() {
        cleanup(&created);
        return;
    }
    let semaphore = primary.semaphore();
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        // SAFETY: 信号量由 primary 持有到本函数结束。
        let wake =
            unsafe { MsgWaitForMultipleObjects(Some(&[semaphore]), false, u32::MAX, QS_ALLINPUT) };
        if wake == WAIT_OBJECT_0 {
            if stop.load(Ordering::Acquire) {
                break;
            }
            signal_second_instance();
            continue;
        }
        if wake.0 == WAIT_OBJECT_0.0 + 1 && pump() {
            break;
        }
        if wake == WAIT_FAILED {
            break;
        }
    }
    cleanup(&created);
    drop(primary);
}

struct CreatedWindow {
    hwnd: HWND,
    class: Vec<u16>,
    instance: HINSTANCE,
}

fn create_window() -> Result<CreatedWindow, String> {
    // SAFETY: 取当前进程模块。空名字表示本 exe。
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map_err(|err| format!("平台窗口创建失败: {err}"))?;
    let instance = HINSTANCE(module.0);
    let class = wide(CLASS_NAME);
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: pcwstr(&class),
        ..WNDCLASSW::default()
    };
    // SAFETY: 类名在 CreatedWindow 里活到反注册。过程地址是静态函数。
    let atom = unsafe { RegisterClassW(&window_class) };
    if atom == 0 {
        // SAFETY: 紧挨着失败的 RegisterClassW，读取这次失败的错误码。
        let err = unsafe { GetLastError() };
        return Err(format!("平台窗口类注册失败: {err:?}"));
    }
    // SAFETY: 不带 WS_VISIBLE，创建后不显示。父窗口为空，才能收到设置变更广播。
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            pcwstr(&class),
            pcwstr(&class),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|err| format!("平台窗口创建失败: {err}"))?;
    Ok(CreatedWindow {
        hwnd,
        class,
        instance,
    })
}

fn cleanup(created: &CreatedWindow) {
    PORT.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            let mut port = WinPort { hwnd: state.hwnd };
            for item in state.current.drain(..) {
                port.unregister(item.id);
            }
        }
        *slot.borrow_mut() = None;
    });
    // SAFETY: 窗口和类都是本线程创建的。
    unsafe {
        let _ = DestroyWindow(created.hwnd);
        let _ = UnregisterClassW(pcwstr(&created.class), Some(created.instance));
    }
}

fn pump() -> bool {
    let mut message = MSG::default();
    loop {
        // SAFETY: message 是本函数的局部变量。
        let has_message = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) };
        if !has_message.as_bool() {
            return false;
        }
        if message.message == WM_QUIT {
            return true;
        }
        // SAFETY: message 已由上面的 PeekMessageW 填好。这两次调用只使用这条 MSG。
        unsafe {
            let _ = TranslateMessage(&message);
            let _ = DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_HOTKEY => {
            signal_hotkey(wparam.0 as i32);
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            if is_immersive_color_set(lparam_text(lparam).as_deref()) {
                signal_theme();
            }
            LRESULT(0)
        }
        WM_JOB => {
            apply_rebind();
            LRESULT(0)
        }
        WM_STOP => {
            // SAFETY: 在这个窗口过程所在的线程上投递 WM_QUIT。
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: 未处理的消息交给系统默认过程。hwnd 是这次回调收到的窗口。
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

fn lparam_text(lparam: LPARAM) -> Option<String> {
    let ptr = lparam.0 as *const u16;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: 设置变更的 lParam 要么是空，要么是以 0 结尾的宽字符串。长度有上限。
    let mut units = Vec::new();
    for index in 0..256 {
        let unit = unsafe { *ptr.add(index) };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    Some(from_wide(&units))
}

fn signal_hotkey(id: i32) {
    if RECORDING.load(Ordering::Acquire) {
        return;
    }
    // 热召回的起点是收到热键消息的这一刻，不是界面线程开始处理的时刻。
    let received_ns = SystemClock.now_ns();
    let _ = slint::invoke_from_event_loop(move || crate::host::on_hotkey(id, received_ns));
}

fn signal_second_instance() {
    let _ = slint::invoke_from_event_loop(crate::host::on_second_instance);
}

fn signal_theme() {
    let _ = slint::invoke_from_event_loop(|| {
        let callback = lock(theme_callback());
        if let Some(callback) = callback.as_ref() {
            callback();
        }
    });
}

fn apply_rebind() {
    let Some(job) = lock(job_queue()).pop_front() else {
        return;
    };
    PORT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return;
        };
        let mut port = WinPort { hwnd: state.hwnd };
        job(&mut port, &mut state.current);
    });
}
