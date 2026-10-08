//! 渲染器与亚克力背景的测量程序。不是产品界面。
//!
//! 四个 Cargo feature 各编一份：`femtovg`、`skia-software`、`skia-opengl`、`software`。
//! Skia 的软件路径和 OpenGL 路径用 Slint 的渲染器名字选定。Slint 1.18.1 没有只链接
//! Skia 软件光栅器的 feature，`skia-software` 仍会链上 Skia 的默认依赖。

mod decision;
mod win;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Serialize;

use decision::{
    BackdropInput, MIN_BACKDROP_BUILD, RenderFailureRead, Rgb, Rgba, choose_backdrop,
    framebuffer_transparency_failed, pixel_near, render_failure_from_screen, solid_rgb,
};
use win::{
    DwmRequest, DwmResult, ReferenceWindow, Watcher, apply_dwm, battery_saver,
    capture_screen_pixel, capture_window, hwnd_from_isize, personalize, windows_build, write_png,
};

slint::include_modules!();

const RENDERER_COUNT: u32 = cfg!(feature = "femtovg") as u32
    + cfg!(feature = "skia-software") as u32
    + cfg!(feature = "skia-opengl") as u32
    + cfg!(feature = "software") as u32;
const _: () = assert!(RENDERER_COUNT == 1, "enable exactly one renderer feature");

const RENDERER_NAME: &str = if cfg!(feature = "femtovg") {
    "femtovg"
} else if cfg!(feature = "skia-software") {
    "skia-software"
} else if cfg!(feature = "skia-opengl") {
    "skia-opengl"
} else {
    "software"
};

const ROW_COUNT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThemeSource {
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug)]
struct Args {
    show_list: bool,
    animate: bool,
    exit_after_secs: Option<u64>,
    shot_dir: Option<PathBuf>,
    status_out: Option<PathBuf>,
    self_test_black: bool,
    theme: ThemeSource,
    reference: bool,
    x: i32,
    y: i32,
}

struct State {
    args: Args,
    render_failed: bool,
    inconclusive: bool,
    graphics_api: String,
    shot_seq: u32,
    shots: Vec<String>,
    ready_for_shots: bool,
    subclass_installed: bool,
    dwm: DwmResult,
    snapshot: Option<[u8; 4]>,
    screen: Option<[u8; 3]>,
    outside: Option<[u8; 3]>,
    self_test_detected: Option<bool>,
    self_test_solid: Option<bool>,
    self_test_stage: u8,
    confirming: bool,
    reference_settled: bool,
    last_label: String,
    personalize_error: Option<String>,
    battery_error: Option<String>,
    transparency_enabled: bool,
    apps_use_light_theme: bool,
    battery_saver: bool,
    os_major: u32,
    os_minor: u32,
    build: u32,
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("lanwork-render-spike: {err}");
            eprintln!("{}", usage());
            std::process::exit(2);
        }
    };
    if let Err(err) = run(args) {
        eprintln!("lanwork-render-spike: {err}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(RENDERER_NAME.into())
        .select()
        .map_err(|err| format!("选择渲染器 {RENDERER_NAME} 失败：{err}"))?;

    let ui = Panel::new().map_err(|err| format!("创建窗口失败：{err}"))?;
    ui.set_show_list(args.show_list);
    ui.set_rows(rows());
    ui.set_inject_black(args.self_test_black);
    ui.window()
        .set_position(slint::PhysicalPosition::new(args.x, args.y));

    let graphics_api = Arc::new(Mutex::new(String::from("unknown")));
    let frames = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let api_slot = Arc::clone(&graphics_api);
    let frame_slot = Arc::clone(&frames);
    ui.window()
        .set_rendering_notifier(move |state, api| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                frame_slot.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if let Ok(mut slot) = api_slot.lock() {
                *slot = format!("{api:?}");
            }
        })
        .map_err(|err| format!("设置渲染回调失败：{err}"))?;

    let build = windows_build()?;
    let state = Arc::new(Mutex::new(State {
        args: args.clone(),
        render_failed: false,
        inconclusive: false,
        graphics_api: String::from("unknown"),
        shot_seq: 0,
        shots: Vec::new(),
        ready_for_shots: false,
        subclass_installed: false,
        dwm: DwmResult {
            backdrop: 1,
            readback: None,
            corner: 1,
            dark: 1,
            extend: 1,
        },
        snapshot: None,
        screen: None,
        outside: None,
        self_test_detected: None,
        self_test_solid: None,
        self_test_stage: 0,
        confirming: false,
        reference_settled: !args.reference,
        last_label: String::new(),
        personalize_error: None,
        battery_error: None,
        transparency_enabled: true,
        apps_use_light_theme: true,
        battery_saver: false,
        os_major: build.major,
        os_minor: build.minor,
        build: build.build,
    }));

    ui.on_request_close(move || {
        let _ = slint::quit_event_loop();
    });

    let reference = if args.reference {
        Some(ReferenceWindow::new(args.x, args.y, 520, 680)?)
    } else {
        None
    };

    ui.show().map_err(|err| format!("显示窗口失败：{err}"))?;
    println!(
        "ready renderer={RENDERER_NAME} pid={} scene={}",
        std::process::id(),
        if args.show_list { "b" } else { "a" }
    );

    let watcher = Rc::new(RefCell::new(None));
    schedule_boot(
        ui.as_weak(),
        Arc::clone(&state),
        Arc::clone(&frames),
        Arc::clone(&graphics_api),
        Rc::clone(&watcher),
        reference,
        0,
    );

    let scroll = slint::Timer::default();
    if args.animate && args.show_list {
        let weak = ui.as_weak();
        scroll.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(32),
            move || {
                if let Some(ui) = weak.upgrade() {
                    let next = (ui.get_scroll_y() + 6.0) % 6_800.0;
                    ui.set_scroll_y(next);
                }
            },
        );
    }
    if let Some(secs) = args.exit_after_secs {
        slint::Timer::single_shot(Duration::from_secs(secs), || {
            let _ = slint::quit_event_loop();
        });
    }

    ui.run().map_err(|err| format!("事件循环结束：{err}"))?;
    watcher.borrow_mut().take();
    Ok(())
}

fn schedule_boot(
    weak: slint::Weak<Panel>,
    state: Arc<Mutex<State>>,
    frames: Arc<std::sync::atomic::AtomicU32>,
    graphics_api: Arc<Mutex<String>>,
    watcher: Rc<RefCell<Option<Watcher>>>,
    reference: Option<ReferenceWindow>,
    attempt: u32,
) {
    slint::Timer::single_shot(Duration::from_millis(200), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if let Ok(api) = graphics_api.lock()
            && let Ok(mut state) = state.lock()
        {
            state.graphics_api.clone_from(&api);
        }
        let hwnd = window_hwnd(&ui);
        let painted = frames.load(std::sync::atomic::Ordering::Relaxed) >= 2;
        if (hwnd.is_none() || !painted) && attempt < 25 {
            schedule_boot(
                weak,
                state,
                frames,
                graphics_api,
                watcher,
                reference,
                attempt + 1,
            );
            return;
        }
        let Some(hwnd) = hwnd else {
            eprintln!("lanwork-render-spike: 窗口句柄不可用");
            let _ = slint::quit_event_loop();
            return;
        };
        if let Some(reference) = reference.as_ref()
            && let Err(err) = reference.place_behind(hwnd)
        {
            eprintln!("lanwork-render-spike: 参考窗：{err}");
        }
        if watcher.borrow().is_none() {
            let weak_watch = weak.clone();
            let state_watch = Arc::clone(&state);
            match Watcher::start(hwnd, move || {
                let weak_watch = weak_watch.clone();
                let state_watch = Arc::clone(&state_watch);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak_watch.upgrade() else {
                        return;
                    };
                    let ready = state_watch
                        .lock()
                        .map(|state| state.ready_for_shots)
                        .unwrap_or(false);
                    refresh(&ui, &state_watch, ready);
                });
            }) {
                Ok(started) => {
                    if let Ok(mut state) = state.lock() {
                        state.subclass_installed = started.subclass_installed();
                    }
                    *watcher.borrow_mut() = Some(started);
                }
                Err(err) => {
                    eprintln!("lanwork-render-spike: 变更监听失败：{err}");
                    let _ = slint::quit_event_loop();
                    return;
                }
            }
        }
        refresh(&ui, &state, false);
        schedule_detect(weak, state, reference, 0);
    });
}

fn schedule_detect(
    weak: slint::Weak<Panel>,
    state: Arc<Mutex<State>>,
    reference: Option<ReferenceWindow>,
    delay_ms: u64,
) {
    slint::Timer::single_shot(Duration::from_millis(delay_ms.max(1)), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let waiting_for_reference = if let Some(window) = reference.as_ref()
            && let Some(hwnd) = window_hwnd(&ui)
        {
            let _ = window.place_behind(hwnd);
            !state
                .lock()
                .map(|state| state.reference_settled)
                .unwrap_or(true)
        } else {
            false
        };
        if waiting_for_reference {
            if let Ok(mut guard) = state.lock() {
                guard.reference_settled = true;
            }
            schedule_detect(weak, state, reference, 400);
            return;
        }
        let self_test = state
            .lock()
            .map(|state| state.args.self_test_black)
            .unwrap_or(false);
        let show_list = state
            .lock()
            .map(|state| state.args.show_list)
            .unwrap_or(false);
        if self_test {
            run_self_test(&ui, &state, &weak);
            return;
        }
        if show_list {
            if let Ok(mut guard) = state.lock() {
                guard.ready_for_shots = true;
            }
            refresh(&ui, &state, true);
            return;
        }
        let failed = match observe(&ui, &state) {
            Ok(failed) => failed,
            Err(err) => {
                eprintln!("lanwork-render-spike: 采样失败：{err}");
                if let Ok(mut state) = state.lock() {
                    state.inconclusive = true;
                    state.ready_for_shots = true;
                }
                refresh(&ui, &state, true);
                return;
            }
        };
        let confirming = state.lock().map(|state| state.confirming).unwrap_or(false);
        if failed && !confirming {
            if let Ok(mut state) = state.lock() {
                state.confirming = true;
            }
            schedule_detect(weak, state, reference, 400);
            return;
        }
        if failed && let Ok(mut guard) = state.lock() {
            guard.render_failed = true;
        }
        if let Ok(mut state) = state.lock() {
            state.ready_for_shots = true;
        }
        refresh(&ui, &state, true);
    });
}

fn run_self_test(ui: &Panel, state: &Arc<Mutex<State>>, weak: &slint::Weak<Panel>) {
    let stage = state.lock().map(|state| state.self_test_stage).unwrap_or(0);
    if stage == 0 {
        let _ = observe(ui, state);
        if let Ok(mut guard) = state.lock() {
            guard.self_test_detected = Some(detection_failed_from(&guard));
            guard.self_test_stage = 1;
            guard.render_failed = true;
        }
        ui.set_inject_black(false);
        refresh(ui, state, true);
        let weak = weak.clone();
        let state = Arc::clone(state);
        slint::Timer::single_shot(Duration::from_millis(400), move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            run_self_test(&ui, &state, &weak);
        });
        return;
    }
    let _ = observe(ui, state);
    let solid_ok = solid_matches(state);
    if let Ok(mut state) = state.lock() {
        state.self_test_solid = Some(solid_ok);
        state.ready_for_shots = true;
    }
    refresh(ui, state, true);
    let _ = slint::quit_event_loop();
}

fn detection_failed(state: &Arc<Mutex<State>>) -> Option<bool> {
    let state = state.lock().ok()?;
    Some(detection_failed_from(&state))
}

fn detection_failed_from(state: &State) -> bool {
    let snapshot_failed = state
        .snapshot
        .is_some_and(|pixel| framebuffer_transparency_failed(rgba_from_array(pixel)));
    let screen_failed = match (state.screen, state.outside) {
        (Some(inside), Some(outside)) => {
            render_failure_from_screen(&[rgb_from_array(inside)], rgb_from_array(outside))
                == RenderFailureRead::Failed
        }
        _ => false,
    };
    snapshot_failed || screen_failed
}

fn solid_matches(state: &Arc<Mutex<State>>) -> bool {
    let Ok(state) = state.lock() else {
        return false;
    };
    let Some(screen) = state.screen else {
        return false;
    };
    let dark = theme_is_dark(&state.args, state.apps_use_light_theme);
    pixel_near(rgb_from_array(screen), solid_rgb(dark), 16)
}

fn observe(ui: &Panel, state: &Arc<Mutex<State>>) -> Result<bool, String> {
    let snapshot = snapshot_rgba(ui);
    let screen = window_hwnd(ui).and_then(|hwnd| capture_window(hwnd).ok());
    let outside = window_hwnd(ui).and_then(|hwnd| outside_pixel(hwnd).ok());
    let snapshot_pixel = snapshot.as_ref().ok().copied();
    let screen_pixel = screen.as_ref().and_then(|capture| {
        capture
            .pixel(capture.width / 2, capture.height * 62 / 100)
            .map(rgb_array)
    });
    let outside_pixel = outside.map(rgb_array);
    if let Ok(mut state) = state.lock() {
        state.snapshot = snapshot_pixel;
        state.screen = screen_pixel;
        state.outside = outside_pixel;
        if snapshot.is_err() && screen_pixel.is_none() {
            state.inconclusive = true;
        }
    }
    Ok(detection_failed(state).unwrap_or(false))
}

fn refresh(ui: &Panel, state: &Arc<Mutex<State>>, take_shot: bool) {
    let Ok(mut guard) = state.lock() else {
        return;
    };
    match personalize() {
        Ok(value) => {
            guard.transparency_enabled = value.transparency_enabled;
            guard.apps_use_light_theme = value.apps_use_light_theme;
            guard.personalize_error = None;
        }
        Err(err) => guard.personalize_error = Some(err),
    }
    match battery_saver() {
        Ok(value) => {
            guard.battery_saver = value;
            guard.battery_error = None;
        }
        Err(err) => guard.battery_error = Some(err),
    }
    let dark = theme_is_dark(&guard.args, guard.apps_use_light_theme);
    let decision = choose_backdrop(BackdropInput {
        build: guard.build,
        transparency_enabled: guard.transparency_enabled,
        battery_saver: guard.battery_saver,
        render_failed: guard.render_failed,
    });
    let color = solid_rgb(dark);
    ui.set_dark(dark);
    ui.set_use_solid(!decision.use_acrylic);
    ui.set_solid_color(slint::Color::from_rgb_u8(color.r, color.g, color.b));
    if let Some(hwnd) = window_hwnd(ui) {
        guard.dwm = apply_dwm(
            hwnd,
            DwmRequest {
                apply_backdrop: guard.build >= MIN_BACKDROP_BUILD,
                acrylic: decision.use_acrylic,
                dark,
            },
        );
    }
    let label = decision_label(&decision);
    let shot_dir = guard.args.shot_dir.clone();
    // 第一次 refresh 发生在窗口尚未准备截图时。只在真正要截的那一次记下标签，
    // 否则准备好之后标签没变，场景 A/B 的首张图会被跳过。
    let shot_path = if take_shot && guard.ready_for_shots && guard.last_label != label {
        let path = shot_dir.map(|dir| {
            guard.shot_seq += 1;
            dir.join(format!("{:02}-{label}.png", guard.shot_seq))
        });
        guard.last_label = label.to_string();
        path
    } else {
        None
    };
    let status = status_from(&guard, label);
    let status_path = guard.args.status_out.clone();
    drop(guard);
    if let Some(path) = shot_path
        && let Some(hwnd) = window_hwnd(ui)
    {
        match capture_window(hwnd).and_then(|capture| write_png(&capture, &path)) {
            Ok(()) => {
                if let Ok(mut state) = state.lock() {
                    state.shots.push(path.display().to_string());
                }
            }
            Err(err) => eprintln!("lanwork-render-spike: 截图失败：{err}"),
        }
    }
    if let Some(path) = status_path {
        if let Ok(state) = state.lock() {
            let status = status_from(&state, label);
            if let Err(err) = write_status(&path, &status) {
                eprintln!("lanwork-render-spike: 状态文件：{err}");
            }
        }
    } else if let Err(err) = write_status_stdout(&status) {
        eprintln!("lanwork-render-spike: 状态：{err}");
    }
}

fn status_from(state: &State, label: &str) -> Status {
    Status {
        renderer: RENDERER_NAME,
        scene: if state.args.show_list { "b" } else { "a" },
        pid: std::process::id(),
        os_major: state.os_major,
        os_minor: state.os_minor,
        build: state.build,
        label: label.to_string(),
        transparency_enabled: state.transparency_enabled,
        battery_saver: state.battery_saver,
        apps_use_light_theme: state.apps_use_light_theme,
        theme_override: match state.args.theme {
            ThemeSource::System => "system",
            ThemeSource::Light => "light",
            ThemeSource::Dark => "dark",
        },
        use_acrylic: label == "acrylic",
        api_unavailable: state.build < MIN_BACKDROP_BUILD,
        transparency_off: !state.transparency_enabled,
        render_failed: state.render_failed,
        render_failure_inconclusive: state.inconclusive,
        backdrop_hresult: state.dwm.backdrop,
        backdrop_readback: state.dwm.readback,
        corner_hresult: state.dwm.corner,
        dark_hresult: state.dwm.dark,
        extend_frame_hresult: state.dwm.extend,
        subclass_installed: state.subclass_installed,
        graphics_api: state.graphics_api.clone(),
        snapshot_rgba: state.snapshot,
        screen_rgb: state.screen,
        outside_rgb: state.outside,
        self_test_black: state.args.self_test_black,
        self_test_detected: state.self_test_detected,
        self_test_solid: state.self_test_solid,
        personalize_error: state.personalize_error.clone(),
        battery_error: state.battery_error.clone(),
        shots: state.shots.clone(),
    }
}

fn decision_label(decision: &decision::BackdropDecision) -> &'static str {
    if decision.api_unavailable {
        "api-unavailable"
    } else if decision.transparency_off {
        "transparency-off"
    } else if decision.battery_saver {
        "battery-saver"
    } else if decision.render_failed {
        "render-failed"
    } else if decision.use_acrylic {
        "acrylic"
    } else {
        "solid"
    }
}

fn theme_is_dark(args: &Args, apps_use_light_theme: bool) -> bool {
    match args.theme {
        ThemeSource::Dark => true,
        ThemeSource::Light => false,
        ThemeSource::System => !apps_use_light_theme,
    }
}

fn window_hwnd(ui: &Panel) -> Option<windows::Win32::Foundation::HWND> {
    let owned = ui.window().window_handle();
    let handle = HasWindowHandle::window_handle(&owned).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(hwnd_from_isize(win32.hwnd.get())),
        _ => None,
    }
}

fn snapshot_rgba(ui: &Panel) -> Result<[u8; 4], String> {
    let buffer = ui
        .window()
        .take_snapshot()
        .map_err(|err| format!("take_snapshot 失败：{err}"))?;
    let width = buffer.width();
    let height = buffer.height();
    if width == 0 || height == 0 {
        return Err("快照是空的".to_string());
    }
    let x = width / 2;
    let y = height * 62 / 100;
    let pixel = buffer
        .as_slice()
        .get((y * width + x) as usize)
        .ok_or_else(|| "快照像素越界".to_string())?;
    Ok([pixel.r, pixel.g, pixel.b, pixel.a])
}

fn outside_pixel(hwnd: windows::Win32::Foundation::HWND) -> Result<Rgb, String> {
    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect) }
        .map_err(|err| format!("GetWindowRect 失败：{err}"))?;
    let y = rect.top + (rect.bottom - rect.top) * 62 / 100;
    let x = if rect.left > 16 {
        rect.left - 12
    } else {
        rect.right + 8
    };
    capture_screen_pixel(x, y)
}

fn rows() -> slint::ModelRc<Row> {
    let mut items = Vec::with_capacity(ROW_COUNT);
    for index in 0..ROW_COUNT {
        items.push(Row {
            name: row_name(index).into(),
            icon: icon_color(index),
        });
    }
    slint::ModelRc::new(slint::VecModel::from(items))
}

fn row_name(index: usize) -> String {
    match index {
        0 => "中文字形：测量亚克力渲染器 繁體簡体 ggyy 0OIl1".to_string(),
        1 => "全角ＡＢＣ与半角ABC".to_string(),
        _ => format!("待办 Todo {index:03} 中文 Mixed"),
    }
}

fn icon_color(index: usize) -> slint::Color {
    let (r, g, b) = hsv_to_rgb((index * 47 % 360) as f32, 0.55, 0.85);
    slint::Color::from_rgb_u8(r, g, b)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let chroma = value * saturation;
    let section = (hue / 60.0).rem_euclid(6.0);
    let x = chroma * (1.0 - (section.rem_euclid(2.0) - 1.0).abs());
    let match_value = value - chroma;
    let (r, g, b) = match section as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let channel = |component: f32| ((component + match_value) * 255.0).round() as u8;
    (channel(r), channel(g), channel(b))
}

fn rgb_array(rgb: Rgb) -> [u8; 3] {
    [rgb.r, rgb.g, rgb.b]
}

fn rgb_from_array(rgb: [u8; 3]) -> Rgb {
    Rgb {
        r: rgb[0],
        g: rgb[1],
        b: rgb[2],
    }
}

fn rgba_from_array(pixel: [u8; 4]) -> Rgba {
    Rgba {
        r: pixel[0],
        g: pixel[1],
        b: pixel[2],
        a: pixel[3],
    }
}

fn write_status(path: &std::path::Path, status: &Status) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建状态目录失败：{err}"))?;
    }
    let text = serde_json::to_string_pretty(status).map_err(|err| err.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text).map_err(|err| format!("写状态失败：{err}"))?;
    std::fs::rename(&temporary, path).map_err(|err| format!("替换状态失败：{err}"))?;
    Ok(())
}

fn write_status_stdout(status: &Status) -> Result<(), String> {
    let text = serde_json::to_string(status).map_err(|err| err.to_string())?;
    println!("{text}");
    Ok(())
}

#[derive(Serialize)]
struct Status {
    renderer: &'static str,
    scene: &'static str,
    pid: u32,
    os_major: u32,
    os_minor: u32,
    build: u32,
    label: String,
    transparency_enabled: bool,
    battery_saver: bool,
    apps_use_light_theme: bool,
    theme_override: &'static str,
    use_acrylic: bool,
    api_unavailable: bool,
    transparency_off: bool,
    render_failed: bool,
    render_failure_inconclusive: bool,
    backdrop_hresult: i32,
    backdrop_readback: Option<u32>,
    corner_hresult: i32,
    dark_hresult: i32,
    extend_frame_hresult: i32,
    subclass_installed: bool,
    graphics_api: String,
    snapshot_rgba: Option<[u8; 4]>,
    screen_rgb: Option<[u8; 3]>,
    outside_rgb: Option<[u8; 3]>,
    self_test_black: bool,
    self_test_detected: Option<bool>,
    self_test_solid: Option<bool>,
    personalize_error: Option<String>,
    battery_error: Option<String>,
    shots: Vec<String>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        show_list: false,
        animate: false,
        exit_after_secs: None,
        shot_dir: None,
        status_out: None,
        self_test_black: false,
        theme: ThemeSource::System,
        reference: false,
        x: 80,
        y: 60,
    };
    let mut items = args.into_iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--help" | "-h" => return Err("帮助".to_string()),
            "--scene" => match items.next().as_deref() {
                Some("a") => parsed.show_list = false,
                Some("b") => parsed.show_list = true,
                other => return Err(format!("--scene 需要 a 或 b，得到 {other:?}")),
            },
            "--animate" => parsed.animate = true,
            "--exit-after-secs" => {
                let value = items
                    .next()
                    .ok_or_else(|| "--exit-after-secs 需要秒数".to_string())?;
                parsed.exit_after_secs =
                    Some(value.parse().map_err(|_| format!("秒数无效：{value}"))?);
            }
            "--shot-dir" => {
                parsed.shot_dir = Some(PathBuf::from(
                    items
                        .next()
                        .ok_or_else(|| "--shot-dir 需要目录".to_string())?,
                ));
            }
            "--status-out" => {
                parsed.status_out = Some(PathBuf::from(
                    items
                        .next()
                        .ok_or_else(|| "--status-out 需要路径".to_string())?,
                ));
            }
            "--self-test-black" => parsed.self_test_black = true,
            "--theme" => {
                parsed.theme = match items.next().as_deref() {
                    Some("system") => ThemeSource::System,
                    Some("light") => ThemeSource::Light,
                    Some("dark") => ThemeSource::Dark,
                    other => {
                        return Err(format!(
                            "--theme 需要 system、light 或 dark，得到 {other:?}"
                        ));
                    }
                };
            }
            "--reference" => parsed.reference = true,
            "--x" => {
                parsed.x = items
                    .next()
                    .ok_or_else(|| "--x 需要坐标".to_string())?
                    .parse()
                    .map_err(|_| "--x 不是整数".to_string())?;
            }
            "--y" => {
                parsed.y = items
                    .next()
                    .ok_or_else(|| "--y 需要坐标".to_string())?
                    .parse()
                    .map_err(|_| "--y 不是整数".to_string())?;
            }
            other => return Err(format!("未知参数：{other}")),
        }
    }
    Ok(parsed)
}

fn usage() -> &'static str {
    "用法：lanwork-render-spike [--scene a|b] [--animate] [--exit-after-secs N] \
[--shot-dir DIR] [--status-out FILE] [--self-test-black] [--theme system|light|dark] \
[--reference] [--x N] [--y N]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scene_and_theme() {
        let args = parse_args(
            ["--scene", "b", "--animate", "--theme", "dark", "--x", "12"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(args.show_list);
        assert!(args.animate);
        assert_eq!(args.theme, ThemeSource::Dark);
        assert_eq!(args.x, 12);
    }

    #[test]
    fn rejects_unknown_scene() {
        let error = parse_args(["--scene", "c"].into_iter().map(str::to_string)).unwrap_err();
        assert!(error.contains("scene"));
    }

    #[test]
    fn windows_build_is_readable() {
        let build = windows_build().unwrap();
        assert!(build.major >= 10);
        assert!(build.build > 0);
    }

    #[test]
    fn personalize_key_is_readable() {
        let value = personalize().unwrap();
        let _ = (value.transparency_enabled, value.apps_use_light_theme);
    }
}
