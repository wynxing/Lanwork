//! Windows 上的两种热角检测。
//!
//! 方案 A：`WH_MOUSE_LL` 在独立线程里。回调只比较已发布的矩形并 `PostMessageW`。
//! 停留计时在窗口线程上，用 `SetTimer`，不在回调里计时。
//! 方案 B：按 50ms 或 100ms `GetCursorPos`，间隔用 `thread::sleep`。
//!
//! 不调用 `timeBeginPeriod`，不写注册表。`cargo test` 不进入这些入口。

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicIsize, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, Ordering,
};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HWND, LPARAM, LRESULT, POINT,
    RECT, TRUE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetDC, GetDeviceCaps, GetMonitorInfoW, HDC, HMONITOR, LOGPIXELSX,
    LOGPIXELSY, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
    ReleaseDC,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_DWORD, REG_SZ, RRF_RT_ANY, RegGetValueW,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, Sleep,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowRect,
    GetWindowThreadProcessId, HHOOK, HWND_MESSAGE, KillTimer, MONITORINFOF_PRIMARY, MSG,
    MSLLHOOKSTRUCT, PM_NOREMOVE, PM_REMOVE, PeekMessageW, PostMessageW, PostThreadMessageW,
    RegisterClassW, SM_CXVIRTUALSCREEN, SM_CYCAPTION, SM_CYFRAME, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, SetForegroundWindow, SetTimer,
    SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL,
    WINDOW_EX_STYLE, WM_APP, WM_QUIT, WM_TIMER, WNDCLASSW, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW,
};
use windows::core::{BOOL, PCWSTR, PWSTR, w};

use crate::{
    Corner, Dwell, Monitor, MonitorPick, PxRect, aim_point, in_corner, outside_point,
    point_in_hot_corner, select_monitor,
};

const WM_APP_POINT: u32 = WM_APP + 1;
const WM_APP_BLOCK: u32 = WM_APP + 2;
const WM_APP_STOP: u32 = WM_APP + 5;
const WM_APP_SYNC: u32 = WM_APP + 6;
const WM_APP_REHOOK: u32 = WM_APP + 10;
const WM_APP_UNHOOK: u32 = WM_APP + 11;
const WM_APP_HOOK_BLOCK: u32 = WM_APP + 12;
const TIMER_DWELL: usize = 1;
const TIMER_STOP: usize = 2;
const CLASS_ALREADY_EXISTS: u32 = 1410;

static TIMING_HWND: AtomicIsize = AtomicIsize::new(0);
static LOGIC: AtomicPtr<Logic> = AtomicPtr::new(ptr::null_mut());
static HOOK_TID: AtomicU32 = AtomicU32::new(0);
static GEN: AtomicU32 = AtomicU32::new(0);
static RECT_LEFT: AtomicI32 = AtomicI32::new(0);
static RECT_TOP: AtomicI32 = AtomicI32::new(0);
static RECT_RIGHT: AtomicI32 = AtomicI32::new(0);
static RECT_BOTTOM: AtomicI32 = AtomicI32::new(0);
static CORNER_PX: AtomicU32 = AtomicU32::new(0);
static CORNER_TAG: AtomicU8 = AtomicU8::new(0);
static CFG_CORNER: AtomicU8 = AtomicU8::new(0);
static CFG_PX: AtomicU32 = AtomicU32::new(0);
static CFG_PICK: AtomicU8 = AtomicU8::new(0);
static CFG_GEN: AtomicU32 = AtomicU32::new(0);
static HOOK_HITS: AtomicU64 = AtomicU64::new(0);
static POST_FAIL: AtomicU64 = AtomicU64::new(0);
static SLOW_MS: AtomicU32 = AtomicU32::new(0);
static BLOCK_ENTERED: AtomicBool = AtomicBool::new(false);
static BLOCK_DONE: AtomicBool = AtomicBool::new(false);
static REHOOK_DONE: AtomicBool = AtomicBool::new(false);
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
static TRIGGER_US: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static REHOOK_RESULT: Mutex<String> = Mutex::new(String::new());
static EVENTS: Mutex<Option<File>> = Mutex::new(None);
static ORIGIN: OnceLock<Instant> = OnceLock::new();
static VIRTUAL: OnceLock<VirtualScreen> = OnceLock::new();

struct Logic {
    dwell: Dwell,
    dwell_for: Duration,
    monitors: Vec<Monitor>,
    pick: MonitorPick,
    corner: Corner,
    corner_px: u32,
    cfg_gen: u32,
    hwnd: HWND,
    timer_on: bool,
    timer_fail: u64,
    disagree: u64,
    last_x: i32,
    last_y: i32,
}

struct Sampler {
    dwell: Dwell,
    dwell_for: Duration,
    seen: u32,
}

struct VirtualScreen {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

struct Env {
    monitors: Vec<Monitor>,
    dpi: Vec<Value>,
    virtual_screen: VirtualScreen,
    hooks_timeout: Value,
    foreground: Value,
    windows_build: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemeKind {
    Hook,
    Poll,
}

pub struct RunOpts {
    pub scheme: SchemeKind,
    pub interval: Option<Duration>,
    pub corner: Corner,
    pub corner_px: u32,
    pub pick: MonitorPick,
    pub dwell: Duration,
    pub duration: Duration,
    pub events: Option<std::path::PathBuf>,
}

pub fn probe(out: &Path) -> Result<(), String> {
    write_json(out, &collect_env()?.to_json())
}

pub fn run(opts: &RunOpts) -> Result<(), String> {
    validate_run(opts)?;
    let env = collect_env()?;
    remember_screen(&env);
    if let Some(path) = &opts.events {
        let file = File::create(path).map_err(|err| format!("打不开事件文件：{err}"))?;
        *EVENTS.lock().map_err(|err| err.to_string())? = Some(file);
    }
    let primary = primary_monitor(&env.monitors)?;
    configure(opts.corner, opts.corner_px, opts.pick, primary.rect);
    let scheme = match opts.scheme {
        SchemeKind::Hook => "hook",
        SchemeKind::Poll => "poll",
    };
    println!(
        "ready pid={} scheme={scheme} interval_ms={:?} corner={:?} corner_px={} monitor={:?} dwell_ms={}",
        std::process::id(),
        opts.interval.map(|interval| interval.as_millis()),
        opts.corner,
        opts.corner_px,
        opts.pick,
        opts.dwell.as_millis()
    );
    let _ = io::stdout().flush();
    match opts.scheme {
        SchemeKind::Hook => run_hook(opts, &env),
        SchemeKind::Poll => {
            run_poll(opts, &env.monitors);
            Ok(())
        }
    }
}

pub fn self_test(corner_px: u32, out: &Path) -> Result<(), String> {
    if corner_px == 0 {
        return Err("corner_px 为 0 时角区域是空的".to_string());
    }
    eprintln!("hotcorner self-test 会短时间移动光标，结束时放回原处");
    let env = collect_env()?;
    remember_screen(&env);
    let _cursor = CursorGuard::capture()?;
    let dwell = Duration::from_millis(350);
    let mut report = json!({
        "corner_px": corner_px,
        "dwell_ms": 350,
        "corner_px_note": "待定。产品规格没有写热角的像素边长。",
        "startup_note": "待定。开始时视为在角外。第一拍已经在角内会开始计时。",
        "gap_note": "待定。点不在任何显示器内时，光标所在显示器模式退回主显示器。",
        "off_note": "待定。关闭时检测仍在跑，但角测试恒为假，所以不触发。规格只要求不打开面板。",
        "env": env.to_json(),
    });
    let result = (|| -> Result<(), String> {
        report["poll_50"] = poll_suite(&env, corner_px, dwell, Duration::from_millis(50))?;
        report["poll_100"] = poll_suite(&env, corner_px, dwell, Duration::from_millis(100))?;
        report["hook"] = hook_suite(&env, corner_px, dwell)?;
        Ok(())
    })();
    write_json(out, &report)?;
    result
}

fn validate_run(opts: &RunOpts) -> Result<(), String> {
    if opts.corner_px == 0 {
        return Err("--corner-px 必须大于 0。边长不是产品规格。".to_string());
    }
    if opts.dwell.is_zero() {
        return Err("--dwell-ms 必须大于 0。产品规格的停留是 350ms。".to_string());
    }
    if opts.duration.is_zero() {
        return Err("--duration-secs 必须大于 0，到时退出并卸下钩子。".to_string());
    }
    match opts.scheme {
        SchemeKind::Hook if opts.interval.is_some() => Err("方案 A 不用 --interval-ms".to_string()),
        SchemeKind::Poll => match opts.interval {
            Some(interval)
                if interval == Duration::from_millis(50)
                    || interval == Duration::from_millis(100) =>
            {
                Ok(())
            }
            _ => Err("方案 B 的 --interval-ms 只接受 50 或 100。".to_string()),
        },
        SchemeKind::Hook => Ok(()),
    }
}

fn poll_suite(
    env: &Env,
    corner_px: u32,
    dwell: Duration,
    interval: Duration,
) -> Result<Value, String> {
    let primary = primary_monitor(&env.monitors)?;
    configure(
        Corner::TopRight,
        corner_px,
        MonitorPick::Primary,
        primary.rect,
    );
    let stop = AtomicBool::new(false);
    let monitors = env.monitors.clone();
    thread::scope(|scope| {
        scope.spawn(|| poll_until(&stop, &monitors, dwell, interval));
        let mut body = functional_body(&primary.rect, corner_px);
        body["other_monitors"] = other_monitors(&env.monitors, corner_px);
        body["drag_fast"] = drag_observed(&primary, corner_px, "fast");
        body["drag_hold"] = drag_observed(&primary, corner_px, "hold");
        stop.store(true, Ordering::Release);
        Ok(json!({
            "scheme": "poll",
            "interval_ms": interval.as_millis(),
            "body": body,
        }))
    })
}

fn hook_suite(env: &Env, corner_px: u32, dwell: Duration) -> Result<Value, String> {
    let primary = primary_monitor(&env.monitors)?;
    configure(
        Corner::TopRight,
        corner_px,
        MonitorPick::Primary,
        primary.rect,
    );
    let hwnd = create_timing_window()?;
    let mut logic = Logic {
        dwell: Dwell::new(dwell),
        dwell_for: dwell,
        monitors: env.monitors.clone(),
        pick: MonitorPick::Primary,
        corner: Corner::TopRight,
        corner_px,
        cfg_gen: CFG_GEN.load(Ordering::Acquire),
        hwnd,
        timer_on: false,
        timer_fail: 0,
        disagree: 0,
        last_x: 0,
        last_y: 0,
    };
    LOGIC.store(&mut logic, Ordering::Release);
    set_hwnd(hwnd);
    let hook = HookThread::start()?;
    let hwnd_bits = hwnd_bits(hwnd);
    let hook_tid = hook.tid;
    let monitors = env.monitors.clone();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _stop = StopGuard(hwnd_bits);
        let mut body = functional_body(&primary.rect, corner_px);
        body["other_monitors"] = other_monitors(&monitors, corner_px);
        body["drag_fast"] = drag_observed(&primary, corner_px, "fast");
        body["drag_hold"] = drag_observed(&primary, corner_px, "hold");
        body["block"] = block_measurements(hwnd_bits, hook_tid);
        body["removal"] = removal_and_fallback(&monitors, &primary, corner_px, dwell, hook_tid);
        let _ = tx.send(body);
    });
    pump_until_stop();
    LOGIC.store(ptr::null_mut(), Ordering::Release);
    let disagree = logic.disagree;
    let timer_fail = logic.timer_fail;
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| json!({"error": "工作线程没有返回"}));
    let panicked = worker.join().is_err();
    drop(hook);
    clear_hwnd();
    let _ = unsafe { DestroyWindow(hwnd) };
    Ok(json!({
        "scheme": "hook",
        "disagree": disagree,
        "timer_fail": timer_fail,
        "hook_hits": HOOK_HITS.load(Ordering::Acquire),
        "post_fail": POST_FAIL.load(Ordering::Acquire),
        "worker_panicked": panicked,
        "body": body,
    }))
}

fn functional_body(rect: &PxRect, corner_px: u32) -> Value {
    let corners = [
        Corner::TopRight,
        Corner::TopLeft,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];
    let stay = corners
        .into_iter()
        .map(|corner| {
            let repeats = if corner == Corner::TopRight { 3 } else { 1 };
            json!({
                "corner": format!("{corner:?}"),
                "trials": (0..repeats)
                    .map(|index| stay_once(rect, corner, corner_px, index, MonitorPick::Primary))
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "stay": stay,
        "leave_340": leave_trial(rect, corner_px),
        "rearm": rearm_trial(rect, corner_px),
        "off": off_trial(rect, corner_px),
    })
}

fn stay_once(
    rect: &PxRect,
    corner: Corner,
    corner_px: u32,
    index: usize,
    pick: MonitorPick,
) -> Value {
    configure(corner, corner_px, pick, *rect);
    poke_sync();
    let Some(outside) = outside_point(*rect, corner, corner_px) else {
        return json!({"index": index, "result": "未测", "reason": "找不到角外的点"});
    };
    let Some(target) = aim_point(*rect, corner, corner_px) else {
        return json!({"index": index, "result": "未测", "reason": "这个角没有坐标"});
    };
    if let Err(err) = move_cursor(outside.0, outside.1) {
        return json!({"index": index, "result": "未测", "reason": err});
    }
    thread::sleep(Duration::from_millis(200));
    let before = trigger_len();
    let move_started = stamp_us();
    if let Err(err) = move_cursor_inside(*rect, corner, corner_px, target) {
        return json!({"index": index, "result": "未测", "reason": err});
    }
    let arrived = stamp_us();
    let found = wait_count(before + 1, Duration::from_millis(1500));
    let _ = move_cursor(outside.0, outside.1);
    let triggers = triggers_since(before, move_started);
    let arrived_delta = arrived.saturating_sub(move_started);
    json!({
        "index": index,
        "result": if found && triggers.len() == 1 { "通过" } else { "失败" },
        "target": [target.0, target.1],
        "move_to_trigger_us": triggers.first().copied(),
        "arrived_to_trigger_us": triggers.first().map(|us| us.saturating_sub(arrived_delta)),
        "triggers_us_from_move": triggers,
    })
}

fn leave_trial(rect: &PxRect, corner_px: u32) -> Value {
    let corner = Corner::TopRight;
    configure(corner, corner_px, MonitorPick::Primary, *rect);
    poke_sync();
    let Some(outside) = outside_point(*rect, corner, corner_px) else {
        return json!({"result": "未测", "reason": "找不到角外的点"});
    };
    let Some(target) = aim_point(*rect, corner, corner_px) else {
        return json!({"result": "未测", "reason": "没有坐标"});
    };
    if let Err(err) = move_cursor(outside.0, outside.1) {
        return json!({"result": "未测", "reason": err});
    }
    thread::sleep(Duration::from_millis(200));
    let before = trigger_len();
    let started = Instant::now();
    if let Err(err) = move_cursor_inside(*rect, corner, corner_px, target) {
        return json!({"result": "未测", "reason": err});
    }
    while started.elapsed() < Duration::from_millis(340) {
        thread::sleep(Duration::from_millis(1));
    }
    if let Err(err) = move_cursor(outside.0, outside.1) {
        return json!({"result": "未测", "reason": err});
    }
    let inside_ms = started.elapsed().as_millis();
    thread::sleep(Duration::from_millis(450));
    let triggers = trigger_len().saturating_sub(before);
    let result = if inside_ms >= 350 {
        "未测"
    } else if triggers == 0 {
        "通过"
    } else {
        "失败"
    };
    json!({
        "result": result,
        "inside_ms": inside_ms,
        "triggers": triggers,
        "note": "inside_ms 含移出角的时间。达到或超过 350 时，这次不能当作 340ms 边界。",
    })
}

fn rearm_trial(rect: &PxRect, corner_px: u32) -> Value {
    let corner = Corner::TopRight;
    configure(corner, corner_px, MonitorPick::Primary, *rect);
    poke_sync();
    let Some(outside) = outside_point(*rect, corner, corner_px) else {
        return json!({"result": "未测", "reason": "找不到角外的点"});
    };
    let Some(target) = aim_point(*rect, corner, corner_px) else {
        return json!({"result": "未测", "reason": "没有坐标"});
    };
    if let Err(err) = move_cursor(outside.0, outside.1) {
        return json!({"result": "未测", "reason": err});
    }
    thread::sleep(Duration::from_millis(200));
    let first_base = trigger_len();
    let first_at = stamp_us();
    if let Err(err) = move_cursor_inside(*rect, corner, corner_px, target) {
        return json!({"result": "未测", "reason": err});
    }
    let first_ok = wait_count(first_base + 1, Duration::from_millis(1500));
    thread::sleep(Duration::from_millis(250));
    let after_hold = trigger_len();
    if let Err(err) = move_cursor(outside.0, outside.1) {
        return json!({"result": "未测", "reason": err});
    }
    thread::sleep(Duration::from_millis(200));
    let second_base = trigger_len();
    let second_at = stamp_us();
    if let Err(err) = move_cursor_inside(*rect, corner, corner_px, target) {
        return json!({"result": "未测", "reason": err});
    }
    let second_ok = wait_count(second_base + 1, Duration::from_millis(1500));
    let _ = move_cursor(outside.0, outside.1);
    let extra = after_hold.saturating_sub(first_base + usize::from(first_ok));
    let result = if first_ok && second_ok && extra == 0 {
        "通过"
    } else {
        "失败"
    };
    json!({
        "result": result,
        "first_trigger_us": triggers_since(first_base, first_at).first().copied(),
        "second_trigger_us": triggers_since(second_base, second_at).first().copied(),
        "extra_while_held": extra,
    })
}

fn off_trial(rect: &PxRect, corner_px: u32) -> Value {
    configure(Corner::Off, corner_px, MonitorPick::Primary, *rect);
    poke_sync();
    let Some(outside) = outside_point(*rect, Corner::TopRight, corner_px) else {
        return json!({"result": "未测", "reason": "找不到角外的点"});
    };
    let Some(target) = aim_point(*rect, Corner::TopRight, corner_px) else {
        return json!({"result": "未测", "reason": "没有坐标"});
    };
    if let Err(err) = move_cursor(outside.0, outside.1) {
        return json!({"result": "未测", "reason": err});
    }
    thread::sleep(Duration::from_millis(200));
    let before = trigger_len();
    if let Err(err) = move_cursor_inside(*rect, Corner::TopRight, corner_px, target) {
        configure(Corner::TopRight, corner_px, MonitorPick::Primary, *rect);
        return json!({"result": "未测", "reason": err});
    }
    thread::sleep(Duration::from_millis(500));
    let _ = move_cursor(outside.0, outside.1);
    let triggers = trigger_len().saturating_sub(before);
    configure(Corner::TopRight, corner_px, MonitorPick::Primary, *rect);
    poke_sync();
    json!({
        "result": if triggers == 0 { "通过" } else { "失败" },
        "triggers": triggers,
        "held_ms": 500,
    })
}

fn other_monitors(monitors: &[Monitor], corner_px: u32) -> Value {
    if monitors.len() < 2 {
        return json!({
            "result": "未测",
            "monitor_count": monitors.len(),
            "reason": "这次只枚举到一块显示器，看不到两种模式的行为差。实现都在。待定。",
        });
    }
    let Some(secondary) = monitors.iter().find(|monitor| !monitor.primary).copied() else {
        return json!({"result": "未测", "reason": "没有非主显示器"});
    };
    let cursor_mode = stay_once(
        &secondary.rect,
        Corner::TopRight,
        corner_px,
        0,
        MonitorPick::Cursor,
    );
    // stay_once 把模式设回主显示器。下面改成主显示器模式，再把光标放进副屏的角。
    configure(
        Corner::TopRight,
        corner_px,
        MonitorPick::Primary,
        primary_rect(monitors),
    );
    poke_sync();
    let ignored = if let Some(target) = aim_point(secondary.rect, Corner::TopRight, corner_px) {
        let before = trigger_len();
        let moved = move_cursor_inside(secondary.rect, Corner::TopRight, corner_px, target);
        thread::sleep(Duration::from_millis(800));
        let triggers = trigger_len().saturating_sub(before);
        json!({
            "move_error": moved.as_ref().err().map(ToString::to_string),
            "triggers": triggers,
            "cursor_entered": moved.is_ok(),
        })
    } else {
        json!({"result": "未测"})
    };
    json!({
        "result": "已记录",
        "monitor_count": monitors.len(),
        "cursor_mode_on_secondary": cursor_mode,
        "primary_mode_on_secondary_corner": ignored,
        "note": "待定。产品规格没有选定多显示器行为。",
    })
}

fn block_measurements(hwnd_bits: isize, hook_tid: u32) -> Value {
    let baseline = input_latency();
    let main_blocked = blocked_latency(|| {
        let _ = post_window(hwnd_bits, WM_APP_BLOCK, 400);
    });
    let hook_blocked = blocked_latency(|| {
        let _ = post_thread_param(hook_tid, WM_APP_HOOK_BLOCK, 400);
    });
    json!({
        "baseline": baseline,
        "main_thread_blocked": main_blocked,
        "hook_thread_blocked": hook_blocked,
        "hook_installed_after": HOOK_INSTALLED.load(Ordering::Acquire),
        "note": "returned_before_sleep_finished 表示 SendInput 返回时对方还在 Sleep。不另设卡顿阈值。",
    })
}

fn blocked_latency(request: impl FnOnce()) -> Value {
    BLOCK_ENTERED.store(false, Ordering::Release);
    BLOCK_DONE.store(false, Ordering::Release);
    request();
    if !wait_flag(&BLOCK_ENTERED, Duration::from_millis(1000)) {
        return json!({"result": "未测", "reason": "阻塞没有开始"});
    }
    let sample = input_latency();
    let returned_before_sleep_finished = sample
        .get("block_done_at_send_return")
        .and_then(Value::as_bool)
        .map(|done| !done);
    let _ = wait_flag(&BLOCK_DONE, Duration::from_millis(2000));
    json!({
        "returned_before_sleep_finished": returned_before_sleep_finished,
        "sample": sample,
    })
}

fn input_latency() -> Value {
    let start = match cursor_pos() {
        Ok(point) => point,
        Err(err) => return json!({"result": "未测", "reason": err}),
    };
    let dx = if start.x > 80 { -40 } else { 40 };
    let t0 = Instant::now();
    let sent = send_relative(dx, 0);
    let send_ms = t0.elapsed().as_millis();
    // 必须在还原光标之前读。还原还会再发一次 SendInput。
    let block_done_at_send_return = BLOCK_DONE.load(Ordering::Acquire);
    let end = cursor_pos().ok();
    let moved = end.is_some_and(|point| point.x != start.x || point.y != start.y);
    let _ = move_cursor(start.x, start.y);
    json!({
        "send_ms": send_ms,
        "send_error": sent.err(),
        "moved": moved,
        "block_done_at_send_return": block_done_at_send_return,
        "from": [start.x, start.y],
        "to": end.map(|point| [point.x, point.y]),
    })
}

fn removal_and_fallback(
    monitors: &[Monitor],
    primary: &Monitor,
    corner_px: u32,
    dwell: Duration,
    hook_tid: u32,
) -> Value {
    let installed_at_start = HOOK_INSTALLED.load(Ordering::Acquire);
    let restored = if installed_at_start {
        None
    } else {
        Some(rehook_now(hook_tid))
    };
    let before_hits = HOOK_HITS.load(Ordering::Acquire);
    SLOW_MS.store(1000, Ordering::Release);
    let slow_started = Instant::now();
    let slow_send = send_relative(3, 0);
    let slow_send_ms = slow_started.elapsed().as_millis();
    let hits_after_slow = HOOK_HITS.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(150));
    let still_called = hook_sees_move();
    REHOOK_DONE.store(false, Ordering::Release);
    if let Ok(mut text) = REHOOK_RESULT.lock() {
        text.clear();
    }
    let posted = post_thread(hook_tid, WM_APP_REHOOK);
    let rehook_finished = wait_flag(&REHOOK_DONE, Duration::from_millis(1000));
    let rehook_result = REHOOK_RESULT
        .lock()
        .map(|text| text.clone())
        .unwrap_or_else(|err| err.to_string());
    let sees_after = if rehook_finished {
        hook_sees_move()
    } else {
        false
    };
    let stay_after = if sees_after {
        stay_once(
            &primary.rect,
            Corner::TopRight,
            corner_px,
            0,
            MonitorPick::Primary,
        )
    } else {
        json!({"result": "未测", "reason": "重新安装后钩子没有收到移动"})
    };
    let _ = post_thread(hook_tid, WM_APP_UNHOOK);
    let unhooked = wait_until(
        || !HOOK_INSTALLED.load(Ordering::Acquire),
        Duration::from_millis(500),
    );
    let fallback = fallback_poll(monitors, primary, corner_px, dwell);
    json!({
        "hook_installed_at_start": installed_at_start,
        "restored_before_slow": restored,
        "slow_ms_requested": 1000,
        "slow_send_ms": slow_send_ms,
        "slow_send_error": slow_send.err(),
        "hits_before": before_hits,
        "hits_after_slow": hits_after_slow,
        "hook_called_after_slow": still_called,
        "detected_removal": !still_called,
        "rehook_posted": posted.err(),
        "rehook_finished": rehook_finished,
        "rehook_result": rehook_result,
        "hook_called_after_rehook": sees_after,
        "stay_after_rehook": stay_after,
        "unhooked": unhooked,
        "fallback_poll": fallback,
    })
}

fn fallback_poll(
    monitors: &[Monitor],
    primary: &Monitor,
    corner_px: u32,
    dwell: Duration,
) -> Value {
    let stop = AtomicBool::new(false);
    let monitors = monitors.to_vec();
    thread::scope(|scope| {
        scope.spawn(|| poll_until(&stop, &monitors, dwell, Duration::from_millis(50)));
        let stay = stay_once(
            &primary.rect,
            Corner::TopRight,
            corner_px,
            0,
            MonitorPick::Primary,
        );
        stop.store(true, Ordering::Release);
        json!({"interval_ms": 50, "stay": stay})
    })
}

fn rehook_now(hook_tid: u32) -> Value {
    REHOOK_DONE.store(false, Ordering::Release);
    if let Ok(mut text) = REHOOK_RESULT.lock() {
        text.clear();
    }
    let posted = post_thread(hook_tid, WM_APP_REHOOK);
    let finished = wait_flag(&REHOOK_DONE, Duration::from_millis(1000));
    let result = REHOOK_RESULT
        .lock()
        .map(|text| text.clone())
        .unwrap_or_else(|err| err.to_string());
    json!({
        "posted_error": posted.err(),
        "finished": finished,
        "result": result,
        "installed": HOOK_INSTALLED.load(Ordering::Acquire),
    })
}

fn hook_sees_move() -> bool {
    let before = HOOK_HITS.load(Ordering::Acquire);
    let _ = send_relative(1, 0);
    wait_until(
        || HOOK_HITS.load(Ordering::Acquire) > before,
        Duration::from_millis(500),
    )
}

fn drag_observed(primary: &Monitor, corner_px: u32, mode: &str) -> Value {
    configure(
        Corner::TopRight,
        corner_px,
        MonitorPick::Primary,
        primary.rect,
    );
    poke_sync();
    let Some(outside) = outside_point(primary.rect, Corner::TopRight, corner_px) else {
        return json!({"mode": mode, "result": "未测", "reason": "找不到角外的点"});
    };
    let Some(corner) = aim_point(primary.rect, Corner::TopRight, corner_px) else {
        return json!({"mode": mode, "result": "未测", "reason": "没有坐标"});
    };
    let hwnd = match create_drag_window(outside.0, outside.1) {
        Ok(hwnd) => hwnd,
        Err(err) => return json!({"mode": mode, "result": "未测", "reason": err}),
    };
    let before_rect = window_rect(hwnd);
    let title = title_point(hwnd);
    let title_error = title.as_ref().err().cloned();
    let before = trigger_len();
    let started = Instant::now();
    let input_error = if let Ok((x, y)) = title {
        thread::scope(|scope| {
            let handle = scope.spawn(|| drag_inputs(x, y, corner, mode));
            while !handle.is_finished() && started.elapsed() < Duration::from_secs(3) {
                pump_one();
            }
            match handle.join() {
                Ok(Ok(())) => None,
                Ok(Err(err)) => Some(err),
                Err(_) => Some("拖动线程 panic".to_string()),
            }
        })
    } else {
        None
    };
    let elapsed_ms = started.elapsed().as_millis();
    let after_rect = window_rect(hwnd);
    let triggers = trigger_len().saturating_sub(before);
    let _ = unsafe { DestroyWindow(hwnd) };
    let moved = match (&before_rect, &after_rect) {
        (Ok(before), Ok(after)) => before != after,
        _ => false,
    };
    json!({
        "mode": mode,
        "window_ex": "WS_EX_TOPMOST",
        "result": if moved { "已记录" } else { "未测" },
        "reason": if moved { "窗口矩形变了" } else { "窗口矩形没变，不能算拖动了窗口" },
        "elapsed_ms": elapsed_ms,
        "triggers": triggers,
        "before": before_rect.ok(),
        "after": after_rect.ok(),
        "title_error": title_error,
        "input_error": input_error,
    })
}

fn drag_inputs(title_x: i32, title_y: i32, corner: (i32, i32), mode: &str) -> Result<(), String> {
    move_cursor(title_x, title_y)?;
    thread::sleep(Duration::from_millis(40));
    send_button(true)?;
    move_cursor(corner.0, corner.1)?;
    if mode == "hold" {
        thread::sleep(Duration::from_millis(500));
    } else {
        thread::sleep(Duration::from_millis(40));
    }
    move_cursor(corner.0.saturating_sub(140), corner.1.saturating_add(140))?;
    send_button(false)?;
    Ok(())
}

fn poll_until(stop: &AtomicBool, monitors: &[Monitor], dwell: Duration, interval: Duration) {
    let mut sampler = Sampler {
        dwell: Dwell::new(dwell),
        dwell_for: dwell,
        seen: CFG_GEN.load(Ordering::Acquire),
    };
    while !stop.load(Ordering::Acquire) {
        let tick = Instant::now();
        sample_poll(monitors, &mut sampler);
        while tick.elapsed() < interval {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let rest = interval.saturating_sub(tick.elapsed());
            thread::sleep(Duration::from_millis(5).min(rest));
        }
    }
}

fn run_poll(opts: &RunOpts, monitors: &[Monitor]) {
    let mut sampler = Sampler {
        dwell: Dwell::new(opts.dwell),
        dwell_for: opts.dwell,
        seen: CFG_GEN.load(Ordering::Acquire),
    };
    let interval = opts.interval.unwrap_or(Duration::from_millis(50));
    let start = Instant::now();
    while start.elapsed() < opts.duration {
        let tick = Instant::now();
        sample_poll(monitors, &mut sampler);
        let remain = opts.duration.saturating_sub(start.elapsed());
        let slice = interval.min(remain);
        let elapsed = tick.elapsed();
        if slice > elapsed {
            thread::sleep(slice - elapsed);
        }
    }
}

fn sample_poll(monitors: &[Monitor], sampler: &mut Sampler) {
    let (corner, px, pick) = sync_sampler(sampler);
    let point = match cursor_pos() {
        Ok(point) => point,
        Err(_) => return,
    };
    let inside = point_in_hot_corner(monitors, pick, corner, px, point.x, point.y);
    if sampler.dwell.sample(inside, Instant::now()) {
        note_trigger(point.x, point.y);
    }
}

fn sync_sampler(sampler: &mut Sampler) -> (Corner, u32, MonitorPick) {
    let generation = CFG_GEN.load(Ordering::Acquire);
    let corner = Corner::from_u8(CFG_CORNER.load(Ordering::Acquire));
    let px = CFG_PX.load(Ordering::Acquire);
    let pick = pick_from_tag(CFG_PICK.load(Ordering::Acquire));
    if generation != sampler.seen {
        sampler.dwell = Dwell::new(sampler.dwell_for);
        sampler.seen = generation;
    }
    (corner, px, pick)
}

fn run_hook(opts: &RunOpts, env: &Env) -> Result<(), String> {
    let hwnd = create_timing_window()?;
    let mut logic = Logic {
        dwell: Dwell::new(opts.dwell),
        dwell_for: opts.dwell,
        monitors: env.monitors.clone(),
        pick: opts.pick,
        corner: opts.corner,
        corner_px: opts.corner_px,
        cfg_gen: CFG_GEN.load(Ordering::Acquire),
        hwnd,
        timer_on: false,
        timer_fail: 0,
        disagree: 0,
        last_x: 0,
        last_y: 0,
    };
    LOGIC.store(&mut logic, Ordering::Release);
    set_hwnd(hwnd);
    let hook = HookThread::start()?;
    let ms = u32::try_from(opts.duration.as_millis())
        .unwrap_or(u32::MAX)
        .max(1);
    let timer = unsafe { SetTimer(Some(hwnd), TIMER_STOP, ms, None) };
    if timer == 0 {
        let err = unsafe { GetLastError() };
        drop(hook);
        clear_hwnd();
        LOGIC.store(ptr::null_mut(), Ordering::Release);
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(format!("SetTimer 失败：{err:?}"));
    }
    pump_until_stop();
    let _ = unsafe { KillTimer(Some(hwnd), TIMER_STOP) };
    drop(hook);
    clear_hwnd();
    LOGIC.store(ptr::null_mut(), Ordering::Release);
    let _ = unsafe { DestroyWindow(hwnd) };
    Ok(())
}

fn pump_until_stop() {
    loop {
        let mut msg = MSG::default();
        let continued = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if !continued.as_bool() {
            break;
        }
        if msg.message == WM_QUIT || msg.message == WM_APP_STOP {
            break;
        }
        if msg.message == WM_TIMER && msg.wParam.0 == TIMER_STOP {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn pump_one() {
    let mut msg = MSG::default();
    let pending = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) };
    if pending.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    } else {
        thread::sleep(Duration::from_millis(5));
    }
}

struct HookThread {
    tid: u32,
    handle: Option<thread::JoinHandle<()>>,
}

impl HookThread {
    fn start() -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || hook_thread_main(tx));
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(tid)) => Ok(Self {
                tid,
                handle: Some(handle),
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => Err("钩子线程没有在 2 秒内就绪".to_string()),
        }
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        if self.tid != 0 {
            let _ = post_thread(self.tid, WM_QUIT);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        HOOK_TID.store(0, Ordering::Release);
        HOOK_INSTALLED.store(false, Ordering::Release);
    }
}

fn hook_thread_main(tx: mpsc::Sender<Result<u32, String>>) {
    let mut boot = MSG::default();
    let _ = unsafe { PeekMessageW(&mut boot, None, 0, 0, PM_NOREMOVE) };
    let mut hook = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) } {
        Ok(hook) => hook,
        Err(err) => {
            let _ = tx.send(Err(format!("SetWindowsHookExW 失败：{err}")));
            return;
        }
    };
    HOOK_INSTALLED.store(true, Ordering::Release);
    let tid = unsafe { GetCurrentThreadId() };
    HOOK_TID.store(tid, Ordering::Release);
    let _ = tx.send(Ok(tid));
    loop {
        let mut msg = MSG::default();
        let continued = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if !continued.as_bool() || msg.message == WM_QUIT {
            break;
        }
        match msg.message {
            WM_APP_REHOOK => rehook(&mut hook),
            WM_APP_UNHOOK => unhook(&mut hook),
            WM_APP_HOOK_BLOCK => {
                BLOCK_ENTERED.store(true, Ordering::Release);
                unsafe { Sleep(msg.wParam.0 as u32) };
                BLOCK_DONE.store(true, Ordering::Release);
            }
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }
    unhook(&mut hook);
}

fn rehook(hook: &mut HHOOK) {
    unhook(hook);
    match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) } {
        Ok(next) => {
            *hook = next;
            HOOK_INSTALLED.store(true, Ordering::Release);
            if let Ok(mut text) = REHOOK_RESULT.lock() {
                *text = "ok".to_string();
            }
        }
        Err(err) => {
            HOOK_INSTALLED.store(false, Ordering::Release);
            if let Ok(mut text) = REHOOK_RESULT.lock() {
                *text = err.to_string();
            }
        }
    }
    REHOOK_DONE.store(true, Ordering::Release);
}

fn unhook(hook: &mut HHOOK) {
    if hook.is_invalid() {
        return;
    }
    let current = std::mem::take(hook);
    let _ = unsafe { UnhookWindowsHookEx(current) };
    HOOK_INSTALLED.store(false, Ordering::Release);
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let slow = SLOW_MS.swap(0, Ordering::AcqRel);
        HOOK_HITS.fetch_add(1, Ordering::Release);
        if slow > 0 {
            // 只在移除测试里置位。平时回调不睡眠。
            unsafe { Sleep(slow) };
        }
        if let Some(hwnd) = timing_hwnd() {
            let inside = read_hit(info.pt.x, info.pt.y);
            let packed = ((inside as usize) << 63) | (info.pt.x as u32 as usize);
            if unsafe {
                PostMessageW(
                    Some(hwnd),
                    WM_APP_POINT,
                    WPARAM(packed),
                    LPARAM(info.pt.y as u32 as isize),
                )
            }
            .is_err()
            {
                POST_FAIL.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn timing_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_POINT => {
            if let Some(logic) = logic_mut() {
                on_point(logic, wparam, lparam);
            }
            LRESULT(0)
        }
        WM_APP_SYNC => {
            if let Some(logic) = logic_mut() {
                sync_logic(logic);
                let _ = logic.dwell.sample(false, Instant::now());
                kill_dwell_timer(logic);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_DWELL
                && let Some(logic) = logic_mut()
            {
                on_timer(logic);
            }
            LRESULT(0)
        }
        WM_APP_BLOCK => {
            BLOCK_ENTERED.store(true, Ordering::Release);
            unsafe { Sleep(wparam.0 as u32) };
            BLOCK_DONE.store(true, Ordering::Release);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn default_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn on_point(logic: &mut Logic, wparam: WPARAM, lparam: LPARAM) {
    sync_logic(logic);
    let callback_inside = (wparam.0 >> 63) != 0;
    let x = wparam.0 as u32 as i32;
    let y = lparam.0 as u32 as i32;
    logic.last_x = x;
    logic.last_y = y;
    let actual = point_in_hot_corner(
        &logic.monitors,
        logic.pick,
        logic.corner,
        logic.corner_px,
        x,
        y,
    );
    if callback_inside != actual {
        logic.disagree = logic.disagree.saturating_add(1);
    }
    if let Some(monitor) = select_monitor(&logic.monitors, logic.pick, x, y) {
        publish_rect(monitor.rect, logic.corner, logic.corner_px);
    }
    apply_inside(logic, actual, x, y);
}

fn on_timer(logic: &mut Logic) {
    sync_logic(logic);
    let inside = point_in_hot_corner(
        &logic.monitors,
        logic.pick,
        logic.corner,
        logic.corner_px,
        logic.last_x,
        logic.last_y,
    );
    if !inside {
        kill_dwell_timer(logic);
        let _ = logic.dwell.sample(false, Instant::now());
        return;
    }
    apply_inside(logic, true, logic.last_x, logic.last_y);
}

fn sync_logic(logic: &mut Logic) {
    let generation = CFG_GEN.load(Ordering::Acquire);
    if generation == logic.cfg_gen {
        return;
    }
    logic.corner = Corner::from_u8(CFG_CORNER.load(Ordering::Acquire));
    logic.corner_px = CFG_PX.load(Ordering::Acquire);
    logic.pick = pick_from_tag(CFG_PICK.load(Ordering::Acquire));
    logic.dwell = Dwell::new(logic.dwell_for);
    logic.cfg_gen = generation;
    kill_dwell_timer(logic);
}

fn apply_inside(logic: &mut Logic, inside: bool, x: i32, y: i32) {
    let now = Instant::now();
    if logic.dwell.sample(inside, now) {
        note_trigger(x, y);
        kill_dwell_timer(logic);
        return;
    }
    if let Some(remain) = logic.dwell.remaining(now) {
        let ms = u32::try_from(remain.as_millis()).unwrap_or(u32::MAX).max(1);
        arm_dwell_timer(logic, ms);
    } else {
        kill_dwell_timer(logic);
    }
}

fn arm_dwell_timer(logic: &mut Logic, ms: u32) {
    let id = unsafe { SetTimer(Some(logic.hwnd), TIMER_DWELL, ms, None) };
    if id == 0 {
        logic.timer_fail = logic.timer_fail.saturating_add(1);
        logic.timer_on = false;
    } else {
        logic.timer_on = true;
    }
}

fn kill_dwell_timer(logic: &mut Logic) {
    if logic.timer_on {
        let _ = unsafe { KillTimer(Some(logic.hwnd), TIMER_DWELL) };
        logic.timer_on = false;
    }
}

fn note_trigger(x: i32, y: i32) {
    let us = stamp_us();
    if let Ok(mut log) = TRIGGER_US.lock() {
        log.push(us);
    }
    if let Ok(mut events) = EVENTS.lock()
        && let Some(file) = events.as_mut()
    {
        let _ = writeln!(file, "{{\"us\":{us},\"x\":{x},\"y\":{y}}}");
        let _ = file.flush();
    }
}

fn configure(corner: Corner, px: u32, pick: MonitorPick, rect: PxRect) {
    CFG_CORNER.store(corner as u8, Ordering::Release);
    CFG_PX.store(px, Ordering::Release);
    CFG_PICK.store(pick_tag(pick), Ordering::Release);
    publish_rect(rect, corner, px);
    CFG_GEN.fetch_add(1, Ordering::Release);
}

fn pick_tag(pick: MonitorPick) -> u8 {
    match pick {
        MonitorPick::Primary => 0,
        MonitorPick::Cursor => 1,
    }
}

fn pick_from_tag(tag: u8) -> MonitorPick {
    if tag == 1 {
        MonitorPick::Cursor
    } else {
        MonitorPick::Primary
    }
}

fn publish_rect(rect: PxRect, corner: Corner, corner_px: u32) {
    let generation = GEN.load(Ordering::Relaxed);
    GEN.store(generation.wrapping_add(1), Ordering::Release);
    RECT_LEFT.store(rect.left, Ordering::Relaxed);
    RECT_TOP.store(rect.top, Ordering::Relaxed);
    RECT_RIGHT.store(rect.right, Ordering::Relaxed);
    RECT_BOTTOM.store(rect.bottom, Ordering::Relaxed);
    CORNER_PX.store(corner_px, Ordering::Relaxed);
    CORNER_TAG.store(corner as u8, Ordering::Relaxed);
    GEN.store(generation.wrapping_add(2), Ordering::Release);
}

fn read_hit(x: i32, y: i32) -> bool {
    for _ in 0..3 {
        let first = GEN.load(Ordering::Acquire);
        if first & 1 != 0 {
            continue;
        }
        let rect = PxRect {
            left: RECT_LEFT.load(Ordering::Relaxed),
            top: RECT_TOP.load(Ordering::Relaxed),
            right: RECT_RIGHT.load(Ordering::Relaxed),
            bottom: RECT_BOTTOM.load(Ordering::Relaxed),
        };
        let corner_px = CORNER_PX.load(Ordering::Relaxed);
        let corner = Corner::from_u8(CORNER_TAG.load(Ordering::Relaxed));
        let second = GEN.load(Ordering::Acquire);
        if first == second {
            return in_corner(rect, corner, corner_px, x, y);
        }
    }
    false
}

fn logic_mut() -> Option<&'static mut Logic> {
    let pointer = LOGIC.load(Ordering::Acquire);
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { &mut *pointer })
    }
}

fn set_hwnd(hwnd: HWND) {
    TIMING_HWND.store(hwnd_bits(hwnd), Ordering::Release);
}

fn clear_hwnd() {
    TIMING_HWND.store(0, Ordering::Release);
}

fn timing_hwnd() -> Option<HWND> {
    let raw = TIMING_HWND.load(Ordering::Acquire);
    if raw == 0 { None } else { Some(hwnd_of(raw)) }
}

fn hwnd_bits(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

fn hwnd_of(bits: isize) -> HWND {
    HWND(bits as *mut _)
}

fn poke_sync() {
    if let Some(hwnd) = timing_hwnd() {
        let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_SYNC, WPARAM(0), LPARAM(0)) };
        thread::sleep(Duration::from_millis(40));
    }
}

fn create_timing_window() -> Result<HWND, String> {
    register_class(w!("LanworkHotcornerTiming"), Some(timing_wndproc))?;
    let instance = module_instance()?;
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("LanworkHotcornerTiming"),
            w!("hotcorner-timing"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|err| format!("创建计时窗口失败：{err}"))
}

fn create_drag_window(x: i32, y: i32) -> Result<HWND, String> {
    register_class(w!("LanworkHotcornerDrag"), Some(default_wndproc))?;
    let instance = module_instance()?;
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            w!("LanworkHotcornerDrag"),
            w!("hotcorner-spike"),
            WS_OVERLAPPEDWINDOW,
            x - 120,
            y - 70,
            240,
            140,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|err| format!("创建拖动窗口失败：{err}"))?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(hwnd)
}

fn register_class(
    name: PCWSTR,
    proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
) -> Result<(), String> {
    let instance = module_instance()?;
    let class = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: Default::default(),
        hCursor: Default::default(),
        hbrBackground: Default::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: name,
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let err = unsafe { GetLastError() };
        if err.0 == CLASS_ALREADY_EXISTS {
            return Ok(());
        }
        return Err(format!("RegisterClassW 失败：{err:?}"));
    }
    Ok(())
}

fn module_instance() -> Result<HINSTANCE, String> {
    let module =
        unsafe { GetModuleHandleW(None) }.map_err(|err| format!("GetModuleHandleW 失败：{err}"))?;
    Ok(HINSTANCE(module.0))
}

fn title_point(hwnd: HWND) -> Result<(i32, i32), String> {
    let rect = window_rect(hwnd)?;
    let frame = unsafe { GetSystemMetrics(SM_CYFRAME) };
    let caption = unsafe { GetSystemMetrics(SM_CYCAPTION) };
    Ok((
        rect[0] + (rect[2] - rect[0]) / 2,
        rect[1] + frame + caption / 2,
    ))
}

fn window_rect(hwnd: HWND) -> Result<[i32; 4], String> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|err| format!("GetWindowRect 失败：{err}"))?;
    Ok([rect.left, rect.top, rect.right, rect.bottom])
}

struct CursorGuard(POINT);

impl CursorGuard {
    fn capture() -> Result<Self, String> {
        Ok(Self(cursor_pos()?))
    }
}

impl Drop for CursorGuard {
    fn drop(&mut self) {
        let _ = move_cursor(self.0.x, self.0.y);
    }
}

struct StopGuard(isize);

impl Drop for StopGuard {
    fn drop(&mut self) {
        let _ = post_window(self.0, WM_APP_STOP, 0);
    }
}

fn move_cursor_inside(
    rect: PxRect,
    corner: Corner,
    corner_px: u32,
    target: (i32, i32),
) -> Result<(), String> {
    move_cursor(target.0, target.1)?;
    let point = cursor_pos()?;
    if in_corner(rect, corner, corner_px, point.x, point.y) {
        Ok(())
    } else {
        Err(format!(
            "光标停在 {},{}，不在 {corner:?} 的 {corner_px} 像素角内",
            point.x, point.y
        ))
    }
}

fn move_cursor(x: i32, y: i32) -> Result<(), String> {
    let screen = VIRTUAL.get().ok_or("还没有读到虚拟屏幕")?;
    let (dx, dy) = to_absolute(screen, x, y);
    send_absolute(dx, dy)?;
    let point = cursor_pos()?;
    if point.x != x || point.y != y {
        send_relative(x.saturating_sub(point.x), y.saturating_sub(point.y))?;
    }
    Ok(())
}

fn to_absolute(screen: &VirtualScreen, x: i32, y: i32) -> (i32, i32) {
    let width = i64::from((screen.w.max(1) - 1).max(1));
    let height = i64::from((screen.h.max(1) - 1).max(1));
    let dx = i64::from(x.saturating_sub(screen.x)) * 65535 / width;
    let dy = i64::from(y.saturating_sub(screen.y)) * 65535 / height;
    (
        i32::try_from(dx).unwrap_or(0),
        i32::try_from(dy).unwrap_or(0),
    )
}

fn send_absolute(dx: i32, dy: i32) -> Result<(), String> {
    send_mouse(
        dx,
        dy,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

fn send_relative(dx: i32, dy: i32) -> Result<(), String> {
    send_mouse(dx, dy, MOUSEEVENTF_MOVE)
}

fn send_button(down: bool) -> Result<(), String> {
    send_mouse(
        0,
        0,
        if down {
            MOUSEEVENTF_LEFTDOWN
        } else {
            MOUSEEVENTF_LEFTUP
        },
    )
}

fn send_mouse(
    dx: i32,
    dy: i32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> Result<(), String> {
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
    let sent = unsafe { SendInput(&[input], i32::try_from(size_of::<INPUT>()).unwrap_or(0)) };
    if sent == 1 {
        Ok(())
    } else {
        let err = unsafe { GetLastError() };
        Err(format!("SendInput 返回 {sent}，GetLastError={err:?}"))
    }
}

fn cursor_pos() -> Result<POINT, String> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.map_err(|err| format!("GetCursorPos 失败：{err}"))?;
    Ok(point)
}

fn wait_count(target: usize, timeout: Duration) -> bool {
    wait_until(|| trigger_len() >= target, timeout)
}

fn wait_until(mut pred: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if pred() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    pred()
}

fn wait_flag(flag: &AtomicBool, timeout: Duration) -> bool {
    wait_until(|| flag.load(Ordering::Acquire), timeout)
}

fn trigger_len() -> usize {
    TRIGGER_US.lock().map(|log| log.len()).unwrap_or(0)
}

fn triggers_since(start: usize, t0: u64) -> Vec<u64> {
    TRIGGER_US
        .lock()
        .map(|log| {
            log.iter()
                .skip(start)
                .map(|us| us.saturating_sub(t0))
                .collect()
        })
        .unwrap_or_default()
}

fn stamp_us() -> u64 {
    let origin = *ORIGIN.get_or_init(Instant::now);
    u64::try_from(origin.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn post_window(bits: isize, message: u32, ms: u32) -> Result<(), String> {
    unsafe { PostMessageW(Some(hwnd_of(bits)), message, WPARAM(ms as usize), LPARAM(0)) }
        .map_err(|err| format!("PostMessageW 失败：{err}"))
}

fn post_thread(tid: u32, message: u32) -> Result<(), String> {
    post_thread_param(tid, message, 0)
}

fn post_thread_param(tid: u32, message: u32, ms: u32) -> Result<(), String> {
    unsafe { PostThreadMessageW(tid, message, WPARAM(ms as usize), LPARAM(0)) }
        .map_err(|err| format!("PostThreadMessageW 失败：{err}"))
}

fn remember_screen(env: &Env) {
    let _ = VIRTUAL.set(VirtualScreen {
        x: env.virtual_screen.x,
        y: env.virtual_screen.y,
        w: env.virtual_screen.w,
        h: env.virtual_screen.h,
    });
}

fn collect_env() -> Result<Env, String> {
    let monitors = enum_monitors()?;
    if monitors.is_empty() {
        return Err("EnumDisplayMonitors 没有返回显示器".to_string());
    }
    Ok(Env {
        dpi: monitor_dpi(&monitors),
        hooks_timeout: read_hooks_timeout(),
        foreground: foreground_probe(),
        windows_build: read_windows_build(),
        virtual_screen: VirtualScreen {
            x: unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
            y: unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
            w: unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
            h: unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
        },
        monitors,
    })
}

fn enum_monitors() -> Result<Vec<Monitor>, String> {
    let mut found = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor),
            LPARAM(&mut found as *mut Vec<Monitor> as isize),
        )
    };
    if !ok.as_bool() {
        let err = unsafe { GetLastError() };
        return Err(format!("EnumDisplayMonitors 失败：{err:?}"));
    }
    Ok(found)
}

unsafe extern "system" fn enum_monitor(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
    let mut info = blank_monitor_info();
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        list.push(Monitor {
            rect: PxRect {
                left: info.rcMonitor.left,
                top: info.rcMonitor.top,
                right: info.rcMonitor.right,
                bottom: info.rcMonitor.bottom,
            },
            primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    TRUE
}

fn blank_monitor_info() -> MONITORINFO {
    MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(0),
        rcMonitor: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rcWork: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        dwFlags: 0,
    }
}

fn monitor_dpi(monitors: &[Monitor]) -> Vec<Value> {
    monitors
        .iter()
        .map(|monitor| {
            let handle = unsafe {
                MonitorFromPoint(
                    POINT {
                        x: monitor.rect.left,
                        y: monitor.rect.top,
                    },
                    MONITOR_DEFAULTTONEAREST,
                )
            };
            let mut dpi_x = 0u32;
            let mut dpi_y = 0u32;
            let from_monitor =
                unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
            if from_monitor.is_ok() {
                json!({
                    "rect": rect_json(monitor.rect),
                    "primary": monitor.primary,
                    "dpi_x": dpi_x,
                    "dpi_y": dpi_y,
                    "source": "GetDpiForMonitor",
                })
            } else {
                let screen = screen_dpi();
                json!({
                    "rect": rect_json(monitor.rect),
                    "primary": monitor.primary,
                    "dpi_x": screen.0,
                    "dpi_y": screen.1,
                    "source": "GetDeviceCaps",
                    "GetDpiForMonitor": from_monitor.err().map(|err| err.to_string()),
                })
            }
        })
        .collect()
}

fn screen_dpi() -> (i32, i32) {
    unsafe {
        let dc = GetDC(None);
        let x = GetDeviceCaps(Some(dc), LOGPIXELSX);
        let y = GetDeviceCaps(Some(dc), LOGPIXELSY);
        let _ = ReleaseDC(None, dc);
        (x, y)
    }
}

fn foreground_probe() -> Value {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return json!({"result": "没有前台窗口"});
    }
    let rect = window_rect(hwnd).ok();
    let class = window_class(hwnd);
    let exe = window_exe(hwnd);
    let covers = rect.as_ref().and_then(|rect| {
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut info = blank_monitor_info();
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            Some(
                rect[0] <= info.rcMonitor.left
                    && rect[1] <= info.rcMonitor.top
                    && rect[2] >= info.rcMonitor.right
                    && rect[3] >= info.rcMonitor.bottom,
            )
        } else {
            None
        }
    });
    json!({
        "class": class,
        "exe": exe,
        "rect": rect,
        "covers_monitor": covers,
        "note": "全屏时是否抑制热角，产品规格没有写。这里只探测，不改变触发。待定。",
    })
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let copied = unsafe { GetClassNameW(hwnd, &mut buf) };
    if copied <= 0 {
        return String::new();
    }
    let len = usize::try_from(copied).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn window_exe(hwnd: HWND) -> Value {
    let mut pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return json!({"error": "没有进程 id"});
    }
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => handle,
        Err(err) => return json!({"pid": pid, "error": err.to_string()}),
    };
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let read = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    match read {
        Ok(()) => {
            let n = usize::try_from(len).unwrap_or(0).min(buf.len());
            json!({"pid": pid, "path": String::from_utf16_lossy(&buf[..n])})
        }
        Err(err) => json!({"pid": pid, "error": err.to_string()}),
    }
}

fn read_hooks_timeout() -> Value {
    read_reg_value(
        HKEY_CURRENT_USER,
        w!("Control Panel\\Desktop"),
        w!("LowLevelHooksTimeout"),
    )
}

fn read_windows_build() -> Value {
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    json!({
        "ProductName": read_reg_value(HKEY_LOCAL_MACHINE, key, w!("ProductName")),
        "DisplayVersion": read_reg_value(HKEY_LOCAL_MACHINE, key, w!("DisplayVersion")),
        "CurrentBuild": read_reg_value(HKEY_LOCAL_MACHINE, key, w!("CurrentBuild")),
        "UBR": read_reg_value(HKEY_LOCAL_MACHINE, key, w!("UBR")),
    })
}

fn read_reg_value(key: windows::Win32::System::Registry::HKEY, sub: PCWSTR, name: PCWSTR) -> Value {
    let mut kind = windows::Win32::System::Registry::REG_NONE;
    let mut buf = [0u8; 512];
    let mut size = buf.len() as u32;
    let status = unsafe {
        RegGetValueW(
            key,
            sub,
            name,
            RRF_RT_ANY,
            Some(&mut kind),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.0 == ERROR_FILE_NOT_FOUND.0 {
        return json!({"found": false, "error": "ERROR_FILE_NOT_FOUND"});
    }
    if status.0 != ERROR_SUCCESS.0 {
        return json!({"found": false, "status": status.0});
    }
    if kind == REG_DWORD && size >= 4 {
        let value = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        return json!({"found": true, "type": "REG_DWORD", "value": value});
    }
    if kind == REG_SZ {
        let chars = size as usize;
        let (chunks, _) = buf[..chars.min(buf.len())].as_chunks::<2>();
        let words: Vec<u16> = chunks
            .iter()
            .map(|chunk| u16::from_le_bytes(*chunk))
            .take_while(|unit| *unit != 0)
            .collect();
        return json!({"found": true, "type": "REG_SZ", "value": String::from_utf16_lossy(&words)});
    }
    json!({"found": true, "type": kind.0, "bytes": size})
}

fn rect_json(rect: PxRect) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

fn primary_rect(monitors: &[Monitor]) -> PxRect {
    primary_monitor(monitors)
        .map(|monitor| monitor.rect)
        .unwrap_or(PxRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        })
}

impl Env {
    fn to_json(&self) -> Value {
        json!({
            "monitors": self.monitors.iter().map(|monitor| json!({
                "left": monitor.rect.left,
                "top": monitor.rect.top,
                "right": monitor.rect.right,
                "bottom": monitor.rect.bottom,
                "primary": monitor.primary,
            })).collect::<Vec<_>>(),
            "dpi": self.dpi,
            "virtual_screen": {
                "x": self.virtual_screen.x,
                "y": self.virtual_screen.y,
                "w": self.virtual_screen.w,
                "h": self.virtual_screen.h,
            },
            "low_level_hooks_timeout": self.hooks_timeout,
            "foreground": self.foreground,
            "windows": self.windows_build,
        })
    }
}

fn primary_monitor(monitors: &[Monitor]) -> Result<Monitor, String> {
    monitors
        .iter()
        .find(|monitor| monitor.primary)
        .copied()
        .or_else(|| monitors.first().copied())
        .ok_or_else(|| "没有显示器".to_string())
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建目录失败：{err}"))?;
    }
    let mut file = File::create(path).map_err(|err| format!("写入 {path:?} 失败：{err}"))?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|err| format!("JSON 失败：{err}"))?;
    writeln!(file).map_err(|err| format!("JSON 失败：{err}"))?;
    Ok(())
}
