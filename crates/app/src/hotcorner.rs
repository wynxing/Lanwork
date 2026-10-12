//! 热角轮询。判定在 `lanwork_core::shell::hotcorner`。
//!
//! 角不是关闭时，单独线程每 50ms 读一次光标。关闭时停在条件变量上，不再读光标。
//! 不安装鼠标钩子，不调用 `timeBeginPeriod`。搜索条热键不在这里注册。
//! 状态只在变化时记一条日志。打开面板走 `panel::show`，前台是否成功由面板记。

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lanwork_core::config::HotCorner;
use lanwork_core::panel::hot_corner_title;
use lanwork_core::shell::hotcorner::{
    CORNER_EDGE_PX, CornerWatch, Foreground, Observation, PxRect, WatchPhase, hits_corner,
};
use lanwork_core::storage::Store;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowRect,
};

use crate::panel;

/// 架构定的轮询间隔。停留是否满 350ms 用 `Instant`，不靠拍数。
const POLL: Duration = Duration::from_millis(50);

struct Slot {
    pending: Option<HotCorner>,
    runtime: Option<Runtime>,
}

struct Runtime {
    control: Arc<Control>,
    thread: JoinHandle<()>,
}

struct Control {
    state: Mutex<State>,
    cv: Condvar,
}

struct State {
    corner: HotCorner,
    stop: bool,
}

enum Phase {
    Stop,
    Off,
    Run(HotCorner),
}

struct ReadWindow {
    class_name: String,
    rect: Option<PxRect>,
}

struct ComInit {
    uninit: bool,
}

impl ComInit {
    fn new() -> Self {
        // SAFETY: 轮询线程自己的多线程套间，保留参数为空。失败时仍靠窗口矩形判断全屏。
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self { uninit: hr.is_ok() }
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: 与本线程上成功的 CoInitializeEx 配对，线程即将退出。
            unsafe { CoUninitialize() };
        }
    }
}

static SLOT: Mutex<Slot> = Mutex::new(Slot {
    pending: None,
    runtime: None,
});

fn lock_slot() -> MutexGuard<'static, Slot> {
    SLOT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 按当前配置启动轮询线程。已经在跑时只更新热角。
///
/// `store` 用来记状态变化。测试可以不传。已经在跑时忽略后来的 `store`。
pub(crate) fn start(corner: HotCorner, store: Option<Store>) -> Result<(), String> {
    let mut slot = lock_slot();
    let corner = slot.pending.unwrap_or(corner);
    slot.pending = Some(corner);
    if let Some(runtime) = &slot.runtime {
        runtime.control.set_corner(corner);
        return Ok(());
    }
    let control = Arc::new(Control::new(corner));
    let thread_control = Arc::clone(&control);
    let thread = std::thread::Builder::new()
        .name("lanwork-hotcorner".into())
        .spawn(move || poll_loop(thread_control, store))
        .map_err(|err| format!("热角线程没有启动: {err}"))?;
    slot.runtime = Some(Runtime { control, thread });
    Ok(())
}

/// 立刻改用新的热角。设置页写完 `hotCorner` 后调用。
///
/// `Off` 停止轮询，搜索条热键不受影响。还没 [`start`] 时先记下，启动时用这份。
/// 可以在界面线程上调用。
pub(crate) fn apply(corner: HotCorner) {
    let mut slot = lock_slot();
    slot.pending = Some(corner);
    if let Some(runtime) = &slot.runtime {
        runtime.control.set_corner(corner);
    }
}

/// 停掉轮询线程。可以重复调用。
pub(crate) fn shutdown() {
    let runtime = {
        let mut slot = lock_slot();
        slot.runtime.take()
    };
    if let Some(runtime) = runtime {
        runtime.control.stop();
        let joined = runtime.thread.join();
        if !std::thread::panicking() {
            joined.expect("热角线程不应 panic");
        }
    }
}

impl Control {
    fn new(corner: HotCorner) -> Self {
        Self {
            state: Mutex::new(State {
                corner,
                stop: false,
            }),
            cv: Condvar::new(),
        }
    }

    fn set_corner(&self, corner: HotCorner) {
        let mut state = self.lock();
        state.corner = corner;
        self.cv.notify_one();
    }

    fn stop(&self) {
        let mut state = self.lock();
        state.stop = true;
        self.cv.notify_one();
    }

    fn phase(&self) -> Phase {
        let state = self.lock();
        if state.stop {
            Phase::Stop
        } else if state.corner == HotCorner::Off {
            Phase::Off
        } else {
            Phase::Run(state.corner)
        }
    }

    /// 等到不再是关闭，或收到停止。返回 false 表示要退出。
    fn wait_while_off(&self) -> bool {
        let mut state = self.lock();
        loop {
            if state.stop {
                return false;
            }
            if state.corner != HotCorner::Off {
                return true;
            }
            state = self.cv.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn should_wait(&self, seen: HotCorner) -> bool {
        let state = self.lock();
        !state.stop && state.corner == seen
    }

    fn wait_interval(&self, dur: Duration) {
        let state = self.lock();
        let _ = self
            .cv
            .wait_timeout(state, dur)
            .unwrap_or_else(PoisonError::into_inner);
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn poll_loop(control: Arc<Control>, store: Option<Store>) {
    let _com = ComInit::new();
    let mut watch = CornerWatch::new(HotCorner::Off);
    let mut seen = Seen {
        corner: HotCorner::Off,
        phase: WatchPhase::Idle,
    };
    loop {
        match control.phase() {
            Phase::Stop => break,
            Phase::Off => {
                // 关掉时清掉未完成的停留。不读光标。关闭时不会要求打开。
                watch.poll(HotCorner::Off, None, Instant::now());
                log_watch(&store, &mut seen, &watch, false, true);
                if !control.wait_while_off() {
                    break;
                }
            }
            Phase::Run(corner) => {
                let open = observe(corner, &mut watch);
                // 这一拍读数期间若已改成别的角或关掉，就不要按旧角打开。
                let still = control.should_wait(corner);
                log_watch(&store, &mut seen, &watch, open, still);
                if open && still {
                    request_open(&store);
                }
                if still {
                    control.wait_interval(POLL);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Seen {
    corner: HotCorner,
    phase: WatchPhase,
}

/// 相对上一拍多出来的日志。没有变化时为空，所以停留过程中不会每 50ms 记一条。
fn note_logs(
    seen: &mut Seen,
    corner: HotCorner,
    phase: WatchPhase,
    open: bool,
    still: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    if corner != seen.corner {
        lines.push(if corner == HotCorner::Off {
            "热角：关闭".to_owned()
        } else {
            format!("热角：{}", hot_corner_title(corner))
        });
    }
    if phase != seen.phase {
        let line = match phase {
            WatchPhase::Dwelling => Some("热角：进入，开始停留"),
            WatchPhase::Disarmed => Some(if open && still {
                "热角：停留到点，请求打开面板"
            } else if open {
                "热角：停留到点，角已改变，不打开"
            } else {
                "热角：停留到点，全屏抑制，不打开"
            }),
            WatchPhase::Idle => {
                if corner == HotCorner::Off {
                    None
                } else {
                    Some("热角：离开，重新武装")
                }
            }
        };
        if let Some(line) = line {
            lines.push(line.to_owned());
        }
    } else if corner != seen.corner && phase == WatchPhase::Dwelling {
        // 换角时清掉旧停留；新角里这一拍又开始计时。上面的相位没变，这里补一条。
        lines.push("热角：进入，开始停留".to_owned());
    }
    seen.corner = corner;
    seen.phase = phase;
    lines
}

fn log_watch(store: &Option<Store>, seen: &mut Seen, watch: &CornerWatch, open: bool, still: bool) {
    for line in note_logs(seen, watch.corner(), watch.phase(), open, still) {
        if let Some(store) = store {
            store.log_info(&line);
        }
    }
}

fn observe(corner: HotCorner, watch: &mut CornerWatch) -> bool {
    let now = Instant::now();
    let monitors = monitor_bounds();
    let Some(cursor) = cursor_pos() else {
        return watch.poll(corner, None, now);
    };
    if monitors.is_empty() {
        return watch.poll(corner, None, now);
    }
    if !hits_corner(&monitors, corner, CORNER_EDGE_PX, cursor.0, cursor.1) {
        let observation = Observation {
            cursor,
            monitors: &monitors,
            foreground: None,
            exclusive_d3d: false,
        };
        return watch.poll(corner, Some(&observation), now);
    }
    let owned = read_foreground();
    let exclusive_d3d = exclusive_d3d();
    let foreground = owned.as_ref().map(|item| Foreground {
        class_name: item.class_name.as_str(),
        rect: item.rect,
    });
    let observation = Observation {
        cursor,
        monitors: &monitors,
        foreground,
        exclusive_d3d,
    };
    watch.poll(corner, Some(&observation), now)
}

fn request_open(store: &Option<Store>) {
    if let Err(err) = slint::invoke_from_event_loop(|| panel::show(None))
        && let Some(store) = store
    {
        store.log_warn(&format!("热角：打开面板没有排进界面线程：{err}"));
    }
}

fn cursor_pos() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: 输出指针指向局部变量。
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}

fn read_foreground() -> Option<ReadWindow> {
    // SAFETY: 没有参数。没有前台窗口时句柄为空。
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    Some(ReadWindow {
        class_name: class_name(hwnd),
        rect: window_rect(hwnd),
    })
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: 缓冲区是本函数的局部变量，长度就是数组长度。
    let copied = unsafe { GetClassNameW(hwnd, &mut buf) };
    if copied <= 0 {
        return String::new();
    }
    let len = usize::try_from(copied).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn window_rect(hwnd: HWND) -> Option<PxRect> {
    let mut rect = RECT::default();
    // SAFETY: hwnd 是刚取到的前台窗口，输出指针指向局部变量。
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(PxRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    })
}

/// `QUNS_RUNNING_D3D_FULL_SCREEN`。失败、或是别的通知状态时返回 false。
///
/// `QUNS_BUSY` 同时表示全屏应用和演示设置，也不指出哪一块显示器，这里不用。
/// 演示设置是否抑制，规格没有写，待定。
fn exclusive_d3d() -> bool {
    // SAFETY: 输出由返回值给出，不保留指针。
    let Ok(state) = (unsafe { SHQueryUserNotificationState() }) else {
        return false;
    };
    state == QUNS_RUNNING_D3D_FULL_SCREEN
}

fn monitor_bounds() -> Vec<PxRect> {
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
            // SAFETY: data 是下面 EnumDisplayMonitors 调用传入的 `Vec<PxRect>` 的地址，
            // 回调只在那次调用返回之前、在同一线程上执行。
            let list = unsafe { &mut *(data.0 as *mut Vec<PxRect>) };
            let bounds = info.rcMonitor;
            list.push(PxRect {
                left: bounds.left,
                top: bounds.top,
                right: bounds.right,
                bottom: bounds.bottom,
            });
        }
        true.into()
    }

    let mut list = Vec::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    struct StopGuard;

    impl Drop for StopGuard {
        fn drop(&mut self) {
            shutdown();
        }
    }

    #[test]
    fn off_parks_the_poller_and_shutdown_joins_it() {
        let guard = StopGuard;
        start(HotCorner::Off, None).expect("热角线程应能启动");
        apply(HotCorner::BottomLeft);
        std::thread::sleep(Duration::from_millis(80));
        apply(HotCorner::Off);
        assert!(
            lock_slot().runtime.is_some(),
            "关闭后线程还在，只是不再轮询"
        );
        drop(guard);
        assert!(lock_slot().runtime.is_none());
    }

    fn fresh() -> Seen {
        Seen {
            corner: HotCorner::Off,
            phase: WatchPhase::Idle,
        }
    }

    #[test]
    fn transition_logs_only_when_the_phase_changes() {
        let mut seen = fresh();
        assert_eq!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Idle,
                false,
                true
            ),
            vec!["热角：右上".to_owned()]
        );
        assert!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Idle,
                false,
                true
            )
            .is_empty()
        );
        assert_eq!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Dwelling,
                false,
                true
            ),
            vec!["热角：进入，开始停留".to_owned()]
        );
        assert!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Dwelling,
                false,
                true
            )
            .is_empty()
        );
        assert_eq!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Disarmed,
                true,
                true
            ),
            vec!["热角：停留到点，请求打开面板".to_owned()]
        );
        assert!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Disarmed,
                false,
                true
            )
            .is_empty()
        );
        assert_eq!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Idle,
                false,
                true
            ),
            vec!["热角：离开，重新武装".to_owned()]
        );
    }

    #[test]
    fn suppress_and_corner_change_have_their_own_lines() {
        let mut seen = fresh();
        let _ = note_logs(
            &mut seen,
            HotCorner::TopRight,
            WatchPhase::Dwelling,
            false,
            true,
        );
        assert_eq!(
            note_logs(
                &mut seen,
                HotCorner::TopRight,
                WatchPhase::Disarmed,
                false,
                true
            ),
            vec!["热角：停留到点，全屏抑制，不打开".to_owned()]
        );

        let mut dwelling = fresh();
        let _ = note_logs(
            &mut dwelling,
            HotCorner::TopRight,
            WatchPhase::Dwelling,
            false,
            true,
        );
        assert_eq!(
            note_logs(
                &mut dwelling,
                HotCorner::TopRight,
                WatchPhase::Disarmed,
                true,
                false
            ),
            vec!["热角：停留到点，角已改变，不打开".to_owned()]
        );

        let mut switched = fresh();
        let _ = note_logs(
            &mut switched,
            HotCorner::TopRight,
            WatchPhase::Dwelling,
            false,
            true,
        );
        assert_eq!(
            note_logs(&mut switched, HotCorner::Off, WatchPhase::Idle, false, true),
            vec!["热角：关闭".to_owned()]
        );

        let mut still_inside = fresh();
        let _ = note_logs(
            &mut still_inside,
            HotCorner::TopRight,
            WatchPhase::Dwelling,
            false,
            true,
        );
        assert_eq!(
            note_logs(
                &mut still_inside,
                HotCorner::TopLeft,
                WatchPhase::Dwelling,
                false,
                true
            ),
            vec!["热角：左上".to_owned(), "热角：进入，开始停留".to_owned(),]
        );
    }

    #[test]
    fn a_real_dwell_logs_each_transition_once() {
        use lanwork_core::shell::hotcorner::{Observation, PxRect};

        let mut watch = CornerWatch::new(HotCorner::Off);
        let mut seen = fresh();
        let t0 = Instant::now();
        let monitors = [PxRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }];
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut step = |cursor: (i32, i32), now| {
            let open = watch.poll(
                HotCorner::TopRight,
                Some(&Observation {
                    cursor,
                    monitors: &monitors,
                    foreground: None,
                    exclusive_d3d: false,
                }),
                now,
            );
            note_logs(&mut seen, watch.corner(), watch.phase(), open, true)
        };

        assert_eq!(step((100, 100), t0), vec!["热角：右上".to_owned()]);
        assert_eq!(step((1919, 0), t0), vec!["热角：进入，开始停留".to_owned()]);
        assert!(step((1919, 0), at(100)).is_empty());
        assert_eq!(
            step((1919, 0), at(350)),
            vec!["热角：停留到点，请求打开面板".to_owned()]
        );
        assert!(step((1919, 0), at(700)).is_empty());
        assert_eq!(
            step((100, 100), at(800)),
            vec!["热角：离开，重新武装".to_owned()]
        );
    }
}
