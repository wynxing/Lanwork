//! Windows 上的探测窗口。每一项都先等别的进程在前台并且静默，再置前。

use std::ffi::c_void;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use lanwork_core::shell::{
    NudgeWait, VirtualScreen, ZeroMove, absolute_to_pixel, nudge_wait, tick_after, zero_move,
};
use lanwork_foreground::{ColdPoll, ColdWatch, ProbeMethod};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    LASTINPUTINFO, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_MOVE_NOCOALESCE,
    MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GA_ROOT,
    GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, MSG, PM_REMOVE,
    PeekMessageW, PostQuitMessage, RegisterClassW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPI_GETFOREGROUNDLOCKTIMEOUT, SW_RESTORE, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetForegroundWindow, ShowWindow, SystemParametersInfoW,
    TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WM_DESTROY, WM_QUIT, WNDCLASSW,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::PCWSTR;

const CLASS_NAME: &str = "LanworkForegroundProbe";

pub(crate) fn run(methods: &[ProbeMethod]) -> ExitCode {
    let window = match ProbeWindow::open() {
        Ok(window) => window,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    // SAFETY: 没有参数。
    let pid = unsafe { GetCurrentProcessId() };
    println!("本进程 PID {pid}");
    println!("探测窗口类名 {CLASS_NAME}");
    match foreground_lock_timeout_ms() {
        Some(timeout) => println!("前台锁超时 SPI_GETFOREGROUNDLOCKTIMEOUT = {timeout} ms"),
        None => println!("读不到 SPI_GETFOREGROUNDLOCKTIMEOUT"),
    }
    println!(
        "请点击别的程序，不要点标题为「Lanwork 前台探测」的窗口，也不要在这个控制台里按键。点完后停住约 2 秒。"
    );
    for (index, method) in methods.iter().enumerate() {
        println!();
        println!(
            "[{}/{}] {} — {}",
            index + 1,
            methods.len(),
            method.name(),
            method.summary()
        );
        match method {
            ProbeMethod::Alt => {
                println!("这一项会向前台程序送 Alt 抬起，可能打开菜单或打断正在进行的中文输入。");
            }
            ProbeMethod::Attach => {
                println!(
                    "附上输入队列期间不抽消息。若和前台线程互相等待，这一项会卡住，请结束进程。"
                );
            }
            _ => {}
        }
        let foreign = match wait_until_cold(pid) {
            Ok(foreign) => foreign,
            Err(Stopped::Quit) => {
                println!("探测窗口已关闭，剩下的项没跑。");
                return ExitCode::from(1);
            }
        };
        println!(
            "等到：前台 PID {} 类 {}「{}」，已静默 2 秒。",
            foreign.pid, foreign.class_name, foreign.title
        );
        let before = cursor();
        let attempt = attempt(*method, window.hwnd);
        let after = cursor();
        let immediate = is_foreground(window.hwnd);
        let immediate_foreground = foreground_label();
        std::thread::sleep(Duration::from_millis(200));
        let later = is_foreground(window.hwnd);
        println!(
            "{}：{}调用返回时本窗口在前台 {}，200ms 后 {}。光标 {}。当时前台 {}。",
            method.name(),
            attempt.note,
            yes(immediate),
            yes(later),
            cursor_delta(before, after),
            immediate_foreground
        );
        if let Some((dx, dy)) = attempt.restore {
            if send_relative(dx, dy) {
                println!("绝对坐标留下的光标偏移已按相对移动移回。");
            } else {
                println!("想把光标移回，SendInput 没有送出。");
            }
        }
        let gave_back = unsafe { SetForegroundWindow(foreign.hwnd) }.as_bool();
        println!(
            "已请求把前台还给「{}」，API 返回 {}。",
            foreign.title,
            yes(gave_back)
        );
    }
    println!();
    println!("各项都试过了。这是这一次桌面会话的结果，不是验收记录。");
    ExitCode::SUCCESS
}

struct Attempt {
    note: String,
    restore: Option<(i32, i32)>,
}

fn attempt(method: ProbeMethod, hwnd: HWND) -> Attempt {
    match method {
        ProbeMethod::Plain => Attempt {
            note: format!("SetForegroundWindow 返回 {}。", yes(activate(hwnd))),
            restore: None,
        },
        ProbeMethod::RelativeImmediate => Attempt {
            note: format!(
                "{}SetForegroundWindow 返回 {}。",
                inject_note(send_relative_zero()),
                yes(activate(hwnd))
            ),
            restore: None,
        },
        ProbeMethod::RelativeWait => {
            let waited = inject_and_wait(send_relative_zero);
            Attempt {
                note: format!(
                    "{}SetForegroundWindow 返回 {}。",
                    waited.note(),
                    yes(activate(hwnd))
                ),
                restore: None,
            }
        }
        ProbeMethod::AbsoluteWait => absolute_wait(hwnd),
        ProbeMethod::Alt => Attempt {
            note: format!(
                "{}SetForegroundWindow 返回 {}。",
                inject_note(send_alt_keyup()),
                yes(activate(hwnd))
            ),
            restore: None,
        },
        ProbeMethod::Attach => Attempt {
            note: format!(
                "SetForegroundWindow 返回 {}。",
                yes(attach_and_activate(hwnd))
            ),
            restore: None,
        },
    }
}

fn absolute_wait(hwnd: HWND) -> Attempt {
    let Some((x, y)) = cursor() else {
        return Attempt {
            note: "读不到光标，没有注入，也没有置前。".to_owned(),
            restore: None,
        };
    };
    let screen = virtual_screen();
    let ZeroMove::Absolute { x: nx, y: ny } = zero_move(x, y, screen) else {
        return Attempt {
            note: "绝对坐标回投不是同一像素，没有注入，也没有置前。".to_owned(),
            restore: None,
        };
    };
    if absolute_to_pixel(nx, screen.x, screen.width) != Some(x)
        || absolute_to_pixel(ny, screen.y, screen.height) != Some(y)
    {
        return Attempt {
            note: "绝对坐标回投核对失败，没有注入，也没有置前。".to_owned(),
            restore: None,
        };
    }
    let waited = inject_and_wait(|| send_absolute(nx, ny));
    let restore = cursor().and_then(|(ax, ay)| small_restore(ax - x, ay - y));
    Attempt {
        note: format!(
            "{}SetForegroundWindow 返回 {}。",
            waited.note(),
            yes(activate(hwnd))
        ),
        restore,
    }
}

struct Waited {
    sent: bool,
    advanced: bool,
    elapsed_ms: u64,
}

impl Waited {
    fn note(&self) -> String {
        if !self.sent {
            return "SendInput 没有送出。".to_owned();
        }
        if self.advanced {
            format!("已注入，最后输入时刻推进了，等了 {} ms。", self.elapsed_ms)
        } else {
            format!(
                "已注入，等到 {} ms，最后输入时刻没有推进。",
                self.elapsed_ms
            )
        }
    }
}

fn inject_note(sent: bool) -> String {
    if sent {
        "已注入，没有等待。".to_owned()
    } else {
        "SendInput 没有送出。".to_owned()
    }
}

fn inject_and_wait(send: impl FnOnce() -> bool) -> Waited {
    let before = last_input_tick();
    if !send() {
        return Waited {
            sent: false,
            advanced: false,
            elapsed_ms: 0,
        };
    }
    let start = Instant::now();
    let mut settled = false;
    loop {
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let advanced = before.is_some_and(|previous| {
            last_input_tick().is_some_and(|current| tick_after(previous, current))
        });
        match nudge_wait(elapsed, advanced, settled) {
            NudgeWait::Done => {
                return Waited {
                    sent: true,
                    advanced,
                    elapsed_ms: elapsed,
                };
            }
            NudgeWait::Sleep { ms } => std::thread::sleep(Duration::from_millis(ms)),
            NudgeWait::Settle { ms } => {
                settled = true;
                std::thread::sleep(Duration::from_millis(ms));
            }
        }
    }
}

fn small_restore(dx: i32, dy: i32) -> Option<(i32, i32)> {
    if dx == 0 && dy == 0 {
        return None;
    }
    ((-2..=2).contains(&dx) && (-2..=2).contains(&dy)).then_some((-dx, -dy))
}

struct Foreign {
    hwnd: HWND,
    pid: u32,
    class_name: String,
    title: String,
}

enum Stopped {
    Quit,
}

fn wait_until_cold(our_pid: u32) -> Result<Foreign, Stopped> {
    let started = last_input_tick();
    if started.is_none() {
        println!("读不到最后输入时刻。这一项只等别的程序在前台并静默 2 秒。");
    }
    let origin = Instant::now();
    let mut watch = ColdWatch::new(started);
    let mut hinted = origin;
    loop {
        if pump() {
            return Err(Stopped::Quit);
        }
        if !watch.arming() && hinted.elapsed() >= Duration::from_secs(5) {
            println!("仍在等：请点击别的程序，然后停住鼠标和键盘约 2 秒。不要点本探测窗口。");
            hinted = Instant::now();
        }
        let foreground = unsafe { GetForegroundWindow() };
        let foreign = !foreground.0.is_null() && window_pid(foreground) != our_pid;
        let now_ms = u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        if watch.poll(now_ms, foreign, hwnd_token(foreground), last_input_tick()) == ColdPoll::Ready
        {
            return Ok(Foreign {
                hwnd: foreground,
                pid: window_pid(foreground),
                class_name: class_name(foreground),
                title: window_title(foreground),
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn hwnd_token(hwnd: HWND) -> u64 {
    u64::try_from(hwnd.0.addr()).unwrap_or(0)
}

fn foreground_label() -> String {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return "（没有）".to_owned();
    }
    format!(
        "PID {} 类 {}「{}」",
        window_pid(hwnd),
        class_name(hwnd),
        window_title(hwnd)
    )
}

fn cursor_delta(before: Option<(i32, i32)>, after: Option<(i32, i32)>) -> String {
    match (before, after) {
        (Some((x0, y0)), Some((x1, y1))) => format!("位移 {},{}", x1 - x0, y1 - y0),
        _ => "读不到".to_owned(),
    }
}

fn yes(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

struct ProbeWindow {
    hwnd: HWND,
    class_name: Vec<u16>,
    instance: HINSTANCE,
}

impl ProbeWindow {
    fn open() -> Result<Self, String> {
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|err| format!("取模块失败：{err}"))?;
        let instance = HINSTANCE(module.0);
        let class_name = wide(CLASS_NAME);
        let title = wide("Lanwork 前台探测（不要点这个窗口）");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: pcwstr(&class_name),
            hbrBackground: HBRUSH(std::ptr::with_exposed_provenance_mut::<c_void>(
                usize::try_from(COLOR_WINDOW.0 + 1).unwrap_or(0),
            )),
            ..WNDCLASSW::default()
        };
        // SAFETY: 类名在 ProbeWindow 里活到反注册。过程地址是静态函数。
        // SAFETY: 类名在本函数的局部变量里活过注册和创建。过程地址是静态函数。
        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            return Err("探测窗口类注册失败".to_owned());
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                pcwstr(&class_name),
                pcwstr(&title),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                80,
                80,
                560,
                220,
                None,
                None,
                Some(instance),
                None,
            )
        };
        let hwnd = match hwnd {
            Ok(hwnd) => hwnd,
            Err(err) => {
                unsafe {
                    let _ = UnregisterClassW(pcwstr(&class_name), Some(instance));
                }
                return Err(format!("探测窗口创建失败：{err}"));
            }
        };
        Ok(Self {
            hwnd,
            class_name,
            instance,
        })
    }
}

impl Drop for ProbeWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = UnregisterClassW(pcwstr(&self.class_name), Some(self.instance));
        }
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn pump() -> bool {
    loop {
        let mut msg = MSG::default();
        // SAFETY: msg 指向局部变量。hwnd 为空表示本线程的全部窗口。
        if !unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            return false;
        }
        if msg.message == WM_QUIT {
            return true;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn activate(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd).as_bool()
    }
}

fn attach_and_activate(hwnd: HWND) -> bool {
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_thread = unsafe { GetWindowThreadProcessId(foreground, None) };
    let current = unsafe { GetCurrentThreadId() };
    if foreground.0.is_null() || foreground_thread == 0 || foreground_thread == current {
        return activate(hwnd);
    }
    // SAFETY: 只在这次调用里附上，下面无论激活结果如何都解开。这段时间不抽消息。
    let attached = unsafe { AttachThreadInput(current, foreground_thread, true) }.as_bool();
    let ok = activate(hwnd);
    if attached {
        unsafe {
            let _ = AttachThreadInput(current, foreground_thread, false);
        }
    }
    ok
}

fn is_foreground(hwnd: HWND) -> bool {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground == hwnd {
        return true;
    }
    if foreground.0.is_null() || hwnd.0.is_null() {
        return false;
    }
    (unsafe { GetAncestor(foreground, GA_ROOT) }) == hwnd
}

fn send_relative_zero() -> bool {
    send_mouse(0, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_MOVE_NOCOALESCE)
}

fn send_relative(dx: i32, dy: i32) -> bool {
    send_mouse(dx, dy, MOUSEEVENTF_MOVE)
}

fn send_absolute(x: i32, y: i32) -> bool {
    send_mouse(
        x,
        y,
        MOUSEEVENTF_MOVE
            | MOUSEEVENTF_ABSOLUTE
            | MOUSEEVENTF_VIRTUALDESK
            | MOUSEEVENTF_MOVE_NOCOALESCE,
    )
}

fn send_alt_keyup() -> bool {
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
    send_input(&input)
}

fn send_mouse(
    dx: i32,
    dy: i32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> bool {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_input(&input)
}

fn send_input(input: &INPUT) -> bool {
    let sent = unsafe {
        SendInput(
            std::slice::from_ref(input),
            i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(0),
        )
    };
    sent == 1
}

fn last_input_tick() -> Option<u32> {
    let mut info = LASTINPUTINFO {
        cbSize: u32::try_from(std::mem::size_of::<LASTINPUTINFO>()).ok()?,
        dwTime: 0,
    };
    unsafe { GetLastInputInfo(&mut info) }
        .as_bool()
        .then_some(info.dwTime)
}

fn foreground_lock_timeout_ms() -> Option<u32> {
    let mut timeout = 0u32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETFOREGROUNDLOCKTIMEOUT,
            0,
            Some(std::ptr::from_mut(&mut timeout).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()
    .map(|()| timeout)
}

fn virtual_screen() -> VirtualScreen {
    unsafe {
        VirtualScreen {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn cursor() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let count = unsafe { GetClassNameW(hwnd, &mut buf) };
    if count <= 0 {
        return "(读不到类名)".to_owned();
    }
    String::from_utf16_lossy(&buf[..usize::try_from(count).unwrap_or(0)])
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let count = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if count <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..usize::try_from(count).unwrap_or(0)])
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pcwstr(buf: &[u16]) -> PCWSTR {
    PCWSTR::from_raw(buf.as_ptr())
}
