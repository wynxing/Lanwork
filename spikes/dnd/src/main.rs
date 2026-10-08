//! 外部拖放技术验证。
//!
//! `--self-test` 跑不需要资源管理器的检查。`--slint` 和 `--ole` 留给人工拖放。

mod logic;
mod ole;
mod samples;

use std::cell::RefCell;
use std::io::Write;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, LogicalPosition, ModelRc, SharedString, VecModel};

use ole::{
    DropRegistration, SharedDrop, ShelfDrop, apartment, com_pointer, data_object_for_paths,
    drag_metrics, drag_out, effect_name, ensure_ole, handle_snapshot, os_version,
    paths_from_data_object, probe_drop_registration, registered_drop_pointer, replace_drop_target,
    wide_len,
};
use samples::Samples;

slint::include_modules!();

fn main() -> ExitCode {
    let mode = match parse_mode() {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    match mode {
        Mode::ProbeNoOle => match run_probe_no_ole() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("dnd: {error}");
                ExitCode::from(1)
            }
        },
        Mode::SelfTest => {
            if let Err(error) = run_self_test() {
                eprintln!("dnd: {error}");
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Mode::Slint | Mode::Ole => match run_interactive(mode) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("dnd: {error}");
                ExitCode::from(1)
            }
        },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    SelfTest,
    ProbeNoOle,
    Slint,
    Ole,
}

fn parse_mode() -> Result<Mode, String> {
    let mut args = std::env::args().skip(1);
    let Some(arg) = args.next() else {
        return Err(
            "usage: cargo run -p dnd -- --self-test | --probe-no-ole | --slint | --ole".to_string(),
        );
    };
    if args.next().is_some() {
        return Err("dnd takes one argument".to_string());
    }
    match arg.as_str() {
        "--self-test" => Ok(Mode::SelfTest),
        "--probe-no-ole" => Ok(Mode::ProbeNoOle),
        "--slint" => Ok(Mode::Slint),
        "--ole" => Ok(Mode::Ole),
        other => Err(format!("unknown argument {other}")),
    }
}

fn line(message: &str) {
    println!("dnd: {message}");
    let _ = std::io::stdout().flush();
}

/// The window log is a fixed-height text block. Keep the tail so a new drag
/// line is visible instead of staying clipped under the first lines.
fn visible_log(full: &str) -> String {
    const MAX_LINES: usize = 12;
    let lines: Vec<&str> = full.lines().collect();
    if lines.len() <= MAX_LINES {
        full.to_string()
    } else {
        lines[lines.len() - MAX_LINES..].join("\n")
    }
}

fn show_log(shared: &SharedDrop, ui: &MainWindow, message: &str) {
    line(message);
    shared.append_log(message);
    let full = shared.log.lock().expect("log").clone();
    ui.set_log(visible_log(&full).into());
}

fn run_self_test() -> Result<(), String> {
    line(&format!("os={}", os_version()));
    line(&format!(
        "computer={}",
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string())
    ));
    line("slint=1.18.1");
    line("build=debug");
    line("backend=winit");
    line(&format!("before-ole {}", apartment()));
    let child =
        std::process::Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
            .arg("--probe-no-ole")
            .output()
            .map_err(|error| format!("spawn --probe-no-ole: {error}"))?;
    line(&format!("probe-no-ole-exit={}", child.status));
    for raw in String::from_utf8_lossy(&child.stdout).lines() {
        line(&format!("probe-no-ole: {raw}"));
    }
    for raw in String::from_utf8_lossy(&child.stderr).lines() {
        line(&format!("probe-no-ole-err: {raw}"));
    }
    if !child.status.success() {
        return Err("probe-no-ole failed".to_string());
    }
    ensure_ole()?;
    line(&format!("after-ole {}", apartment()));
    let (cx, cy) = drag_metrics();
    line(&format!("SM_CXDRAG={cx} SM_CYDRAG={cy}"));
    let samples = Samples::create()?;
    line(&format!("sample-dir={}", samples.dir.display()));
    line(&format!(
        "long-wide-len={} path={}",
        wide_len(&samples.long_file),
        samples.long_file.display()
    ));
    check_shell_roundtrip(&samples)?;
    check_synthetic_hdrop(&samples)?;
    let before = handle_snapshot();
    line(&format!("com-loop-before {before}"));
    for _ in 0..100 {
        let data = data_object_for_paths(std::slice::from_ref(&samples.readme))?;
        let _ = paths_from_data_object(&data)?;
        drop(data);
    }
    let after = handle_snapshot();
    line(&format!("com-loop-after {after}"));
    let source: windows::Win32::System::Ole::IDropSource =
        ole::ShelfSource { force_cancel: true }.into();
    let cancel = unsafe {
        source.QueryContinueDrag(
            false,
            windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS(
                windows::Win32::System::SystemServices::MK_LBUTTON.0,
            ),
        )
    };
    let still = samples.readme.is_file();
    line(&format!(
        "query-continue-force-cancel code={:#x} file-remains={still}",
        cancel.0
    ));
    line(
        "do-drag-drop-not-called reason=DoDragDrop waits for a keyboard or mouse change before the first QueryContinueDrag",
    );
    if cancel.0 != ole::DRAG_CANCEL_CODE {
        return Err(format!("expected DRAGDROP_S_CANCEL, got {:#x}", cancel.0));
    }
    if !still {
        return Err("sample file missing before the window test".to_string());
    }
    run_window(Mode::SelfTest, samples)
}

fn check_synthetic_hdrop(samples: &Samples) -> Result<(), String> {
    let hdrop = ole::hdrop_from_paths(&[
        samples.shortcut.clone(),
        samples.long_file.clone(),
        samples.folder.clone(),
    ])?;
    let paths = ole::paths_from_hdrop(hdrop);
    unsafe { windows::Win32::UI::Shell::DragFinish(hdrop) };
    line(&format!(
        "synthetic-hdrop {}",
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    if paths.len() != 3 || paths[0] != samples.shortcut || paths[1] != samples.long_file {
        return Err(format!(
            "synthetic HDROP roundtrip changed paths: {paths:?}"
        ));
    }
    if wide_len(&paths[1]) <= 260 {
        return Err("synthetic long path is not over 260 UTF-16 units".to_string());
    }
    Ok(())
}

fn check_shell_roundtrip(samples: &Samples) -> Result<(), String> {
    let short = [
        samples.readme.clone(),
        samples.folder.clone(),
        samples.shortcut.clone(),
    ];
    match data_object_for_paths(&short).and_then(|data| paths_from_data_object(&data)) {
        Ok(paths) => {
            line(&format!(
                "shell-roundtrip-short {}",
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
            if paths.len() != 3 || paths[2] != samples.shortcut || paths[2] == samples.readme {
                return Err(format!("shortcut path was not preserved: {paths:?}"));
            }
        }
        Err(error) => return Err(format!("short shell roundtrip: {error}")),
    }
    let paths = data_object_for_paths(std::slice::from_ref(&samples.long_file))
        .and_then(|data| paths_from_data_object(&data))?;
    let same = paths.first().is_some_and(|path| path == &samples.long_file);
    let wide = paths.first().map(|path| wide_len(path)).unwrap_or(0);
    line(&format!(
        "shell-roundtrip-long wide={wide} same={same} path={}",
        paths
            .first()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    ));
    if !same || wide <= 260 {
        return Err(format!(
            "shell long path was not preserved: wide={wide} paths={paths:?}"
        ));
    }
    Ok(())
}

fn run_probe_no_ole() -> Result<(), String> {
    line(&format!("probe-before-window {}", apartment()));
    let ui = MainWindow::new().map_err(|error| error.to_string())?;
    line(&format!("probe-after-new {}", apartment()));
    let failed = Rc::new(AtomicBool::new(false));
    let failed_timer = failed.clone();
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(400), move || {
        let Some(ui) = weak.upgrade() else {
            failed_timer.store(true, Ordering::Release);
            let _ = slint::quit_event_loop();
            return;
        };
        line(&format!("probe-inside-loop {}", apartment()));
        match hwnd_of(ui.window()) {
            Ok(hwnd) => {
                line(&format!("probe-hwnd={hwnd:?}"));
                let shared = Arc::new(SharedDrop::new());
                let probe: windows::Win32::System::Ole::IDropTarget = ShelfDrop::new(shared).into();
                let status = probe_drop_registration(hwnd, &probe);
                line(&format!(
                    "probe-registration={}",
                    registration_name(&status)
                ));
            }
            Err(error) => {
                line(&format!("probe-hwnd-error={error}"));
                failed_timer.store(true, Ordering::Release);
            }
        }
        let _ = slint::quit_event_loop();
    });
    ui.run().map_err(|error| error.to_string())?;
    if failed.load(Ordering::Acquire) {
        return Err("probe-no-ole window step failed".to_string());
    }
    Ok(())
}

fn run_interactive(mode: Mode) -> Result<(), String> {
    line(&format!("os={}", os_version()));
    line(&format!("before-ole {}", apartment()));
    ensure_ole()?;
    line(&format!("after-ole {}", apartment()));
    let (cx, cy) = drag_metrics();
    line(&format!("SM_CXDRAG={cx} SM_CYDRAG={cy}"));
    line(&handle_snapshot());
    let samples = Samples::create()?;
    line(&format!("sample-dir={}", samples.dir.display()));
    line(&format!(
        "long-wide-len={} path={}",
        wide_len(&samples.long_file),
        samples.long_file.display()
    ));
    run_window(mode, samples)
}

fn run_window(mode: Mode, samples: Samples) -> Result<(), String> {
    let ui = MainWindow::new().map_err(|error| error.to_string())?;
    let graphics = Arc::new(Mutex::new(String::from("not-yet")));
    let graphics_for_notifier = graphics.clone();
    ui.window()
        .set_rendering_notifier(move |state, api| {
            if matches!(state, slint::RenderingState::RenderingSetup) {
                *graphics_for_notifier.lock().expect("graphics") = format!("{api:?}");
            }
        })
        .map_err(|error| error.to_string())?;

    let readme = samples.readme.clone();
    let rows = samples.rows();
    let row_text: Vec<SharedString> = rows
        .iter()
        .map(|path| path.display().to_string().into())
        .collect();
    ui.set_rows(ModelRc::new(VecModel::from(row_text)));
    ui.set_status(
        format!(
            "样本目录 {}\n拖放阈值 SM_CXDRAG / SM_CYDRAG 见控制台。OLE 拖入只认红色矩形。",
            samples.dir.display()
        )
        .into(),
    );

    let api = ui.global::<Api>();
    api.on_file_transfer(move || {
        let mut transfer = slint::DataTransfer::default();
        transfer.set_file_paths([readme.clone()]);
        transfer
    });
    api.on_accepts(|data| data.has_file_paths());
    api.on_describe(|data| match data.file_paths() {
        Ok(paths) => paths
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .into(),
        Err(error) => SharedString::from(error.to_string()),
    });

    let shared = Arc::new(SharedDrop::new());
    let target: windows::Win32::System::Ole::IDropTarget = ShelfDrop::new(shared.clone()).into();
    let installed = Arc::new(AtomicBool::new(false));
    let press = Rc::new(std::cell::Cell::new(None::<Press>));
    let drag_starts = Rc::new(RefCell::new(Vec::<(i32, String)>::new()));
    wire_ole_drag(&ui, &rows, press, mode, shared.clone(), drag_starts.clone());

    let weak = ui.as_weak();
    let shared_timer = shared.clone();
    let target_timer = target.clone();
    let installed_timer = installed.clone();
    let graphics_timer = graphics.clone();
    let samples_timer = SamplesHold {
        readme: samples.readme.clone(),
        shortcut: samples.shortcut.clone(),
    };
    let starts_timer = drag_starts.clone();
    let mode_timer = mode;
    let attempts = Rc::new(std::cell::Cell::new(0u32));
    let failed = Arc::new(AtomicBool::new(false));
    let attempts_timer = attempts.clone();
    let failed_timer = failed.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(200),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            publish_zone(&ui, &shared_timer);
            let full = shared_timer.log.lock().expect("log").clone();
            let visible = visible_log(&full);
            if ui.get_log().as_str() != visible {
                ui.set_log(visible.into());
            }
            if mode_timer != Mode::Slint && !installed_timer.load(Ordering::Acquire) {
                match hwnd_of(ui.window()) {
                    Ok(hwnd) => match install_ole(hwnd, &target_timer, &shared_timer) {
                        Ok(()) => installed_timer.store(true, Ordering::Release),
                        Err(error) => line(&format!("install-error={error}")),
                    },
                    Err(error) => line(&format!("hwnd-error={error}")),
                }
            }
            if mode_timer != Mode::SelfTest {
                return;
            }
            let ready = mode_timer == Mode::Slint || installed_timer.load(Ordering::Acquire);
            if !ready {
                let attempt = attempts_timer.get() + 1;
                attempts_timer.set(attempt);
                if attempt > 25 {
                    line("window-self-test-error=drop target was not installed");
                    failed_timer.store(true, Ordering::Release);
                    let _ = slint::quit_event_loop();
                }
                return;
            }
            match self_test_window(
                &ui,
                &shared_timer,
                &graphics_timer,
                &samples_timer,
                &starts_timer,
            ) {
                Ok(()) => {
                    let _ = slint::quit_event_loop();
                }
                Err(error) => {
                    let attempt = attempts_timer.get() + 1;
                    attempts_timer.set(attempt);
                    line(&format!("window-self-test-retry={attempt} {error}"));
                    if error.contains("did not receive")
                        || error.contains("ole-row")
                        || attempt > 25
                    {
                        failed_timer.store(true, Ordering::Release);
                        let _ = slint::quit_event_loop();
                    }
                }
            }
        },
    );

    ui.run().map_err(|error| error.to_string())?;
    drop(timer);
    if failed.load(Ordering::Acquire) {
        return Err("window self-test failed".to_string());
    }
    Ok(())
}

struct SamplesHold {
    readme: std::path::PathBuf,
    shortcut: std::path::PathBuf,
}

struct Press {
    index: i32,
    x: f32,
    y: f32,
}

fn wire_ole_drag(
    ui: &MainWindow,
    rows: &[std::path::PathBuf],
    press: Rc<std::cell::Cell<Option<Press>>>,
    mode: Mode,
    shared: Arc<SharedDrop>,
    drag_starts: Rc<RefCell<Vec<(i32, String)>>>,
) {
    let api = ui.global::<Api>();
    let rows_press: Vec<_> = rows.to_vec();
    let press_down = press.clone();
    api.on_ole_press(move |index, x, y| {
        if mode == Mode::Slint {
            line("ole-press ignored in --slint");
            return;
        }
        press_down.set(Some(Press { index, x, y }));
    });
    let press_move = press.clone();
    let rows_move = rows_press.clone();
    let ui_move = ui.as_weak();
    let shared_move = shared;
    api.on_ole_move(move |index, x, y| {
        if mode == Mode::Slint {
            return;
        }
        let Some(start) = press_move.take() else {
            return;
        };
        if start.index != index {
            press_move.set(Some(start));
            return;
        }
        let Some(ui) = ui_move.upgrade() else {
            return;
        };
        let scale = ui.window().scale_factor();
        let (cx, cy) = drag_metrics();
        let dx = ((x - start.x) * scale).abs();
        let dy = ((y - start.y) * scale).abs();
        if !logic::passed_drag_threshold(dx, dy, cx, cy) {
            press_move.set(Some(start));
            return;
        }
        let Some(path) = rows_move.get(index as usize) else {
            // The row is gone, but the TouchArea may still own the grab.
            release_pointer_grab(&ui, x, y);
            return;
        };
        let path_text = path.display().to_string();
        show_log(
            &shared_move,
            &ui,
            &format!("ole-drag-start index={index} path={path_text}"),
        );
        show_log(&shared_move, &ui, &handle_snapshot());
        if mode == Mode::SelfTest {
            drag_starts.borrow_mut().push((index, path_text));
            show_log(&shared_move, &ui, "ole-drag-end code=self-test effect=none");
        } else {
            match drag_out(std::slice::from_ref(path), false) {
                Ok(outcome) => {
                    show_log(
                        &shared_move,
                        &ui,
                        &format!(
                            "ole-drag-end code={:#x} effect={} {}",
                            outcome.code,
                            effect_name(outcome.effect),
                            handle_snapshot()
                        ),
                    );
                }
                Err(error) => {
                    show_log(&shared_move, &ui, &format!("ole-drag-error={error}"));
                }
            }
        }
        // DoDragDrop runs a modal loop and consumes the mouse-up, so this
        // TouchArea would stay pressed and grab the next press. Release after
        // every outcome, including the self-test path that does not call it.
        release_pointer_grab(&ui, x, y);
    });
    let press_up = press;
    api.on_ole_release(move || {
        press_up.set(None);
    });
}

fn release_pointer_grab(ui: &MainWindow, x: f32, y: f32) {
    let window = ui.window();
    let position = LogicalPosition::new(x, y);
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
    window.dispatch_event(slint::platform::WindowEvent::PointerExited);
}

fn publish_zone(ui: &MainWindow, shared: &SharedDrop) {
    let scale = ui.window().scale_factor();
    *shared.zone.lock().expect("zone") = logic::Zone {
        x: ui.get_ole_x(),
        y: ui.get_ole_y(),
        w: ui.get_ole_w(),
        h: ui.get_ole_h(),
        scale,
    };
    if let Ok(hwnd) = hwnd_of(ui.window()) {
        shared.hwnd.store(hwnd.0 as isize, Ordering::Release);
    }
}

fn install_ole(
    hwnd: windows::Win32::Foundation::HWND,
    target: &windows::Win32::System::Ole::IDropTarget,
    shared: &SharedDrop,
) -> Result<(), String> {
    let before = probe_drop_registration(hwnd, target);
    line(&format!(
        "registration-before={}",
        registration_name(&before)
    ));
    // probe_drop_registration revokes when it was the one to register.
    // AlreadyRegistered means winit or someone else owns it.
    let how = replace_drop_target(hwnd, target)?;
    let ours = com_pointer(target);
    let prop = registered_drop_pointer(hwnd);
    let prop_text = prop
        .map(|value| format!("{value:#x}"))
        .unwrap_or_else(|| "none".to_string());
    line(&format!(
        "registration-how={how} our-pointer={ours:#x} prop={prop_text}"
    ));
    let again = probe_drop_registration(hwnd, target);
    // The second probe registers a different object if the slot is empty, and
    // reports AlreadyRegistered if ours is still installed. If it replaced ours,
    // put ours back.
    if !matches!(again, DropRegistration::AlreadyRegistered) {
        let _ = replace_drop_target(hwnd, target)?;
        return Err(format!(
            "drop target did not stick: {}",
            registration_name(&again)
        ));
    }
    let _ = shared;
    line("registration-stuck=already-registered");
    Ok(())
}

fn registration_name(status: &DropRegistration) -> String {
    match status {
        DropRegistration::RegisteredByUs => "empty-slot".to_string(),
        DropRegistration::AlreadyRegistered => "already-registered".to_string(),
        DropRegistration::NotRegistered => "not-registered".to_string(),
        DropRegistration::Error(error) => format!("error:{error}"),
    }
}

fn self_test_window(
    ui: &MainWindow,
    shared: &SharedDrop,
    graphics: &Mutex<String>,
    samples: &SamplesHold,
    drag_starts: &RefCell<Vec<(i32, String)>>,
) -> Result<(), String> {
    let renderer = graphics.lock().expect("graphics").clone();
    if renderer == "not-yet" {
        return Err("renderer not reported yet".to_string());
    }
    line(&format!("graphics={renderer}"));
    line(&format!("inside-loop {}", apartment()));
    let zone = *shared.zone.lock().expect("zone");
    line(&format!(
        "ole-zone x={} y={} w={} h={} scale={}",
        zone.x, zone.y, zone.w, zone.h, zone.scale
    ));
    if zone.w <= 0.0 || zone.h <= 0.0 {
        return Err("ole zone has no size".to_string());
    }
    exercise_slint_drag(ui)?;
    exercise_ole_row_grab(ui, drag_starts)?;
    line(&format!(
        "sample-readme={} sample-shortcut={}",
        samples.readme.display(),
        samples.shortcut.display()
    ));
    Ok(())
}

fn exercise_ole_row_grab(
    ui: &MainWindow,
    drag_starts: &RefCell<Vec<(i32, String)>>,
) -> Result<(), String> {
    let row_h = ui.get_ole_row_h();
    if ui.get_ole_list_y() <= 0.0 || row_h <= 0.0 {
        return Err("ole-row list has no geometry".to_string());
    }
    drag_starts.borrow_mut().clear();
    let x = ui.get_ole_list_x() + 24.0;
    let row_y = |index: f32| ui.get_ole_list_y() + row_h * index + row_h / 2.0;
    // Row 3 is long.txt. A drag from it used to leave the TouchArea grabbed,
    // so the next press on row 1 (folder) still dragged long.txt.
    dispatch_press_move(ui, x, row_y(3.0));
    dispatch_press_move(ui, x, row_y(1.0));
    let starts = drag_starts.borrow().clone();
    line(&format!("ole-row-grab {starts:?}"));
    let log = ui.get_log().to_string();
    if starts.len() != 2 {
        return Err(format!(
            "ole-row expected two drags, got {starts:?} log={log}"
        ));
    }
    let first_ok = starts[0].0 == 3 && starts[0].1.contains("long.txt");
    let second_ok = starts[1].0 == 1
        && (starts[1].1.ends_with("\\folder") || starts[1].1.ends_with("/folder"))
        && !starts[1].1.contains("long.txt");
    if !first_ok || !second_ok {
        return Err(format!(
            "ole-row second drag did not follow the pressed row: {starts:?}"
        ));
    }
    if !log.contains("index=3") || !log.contains("index=1") || !log.contains("\\folder") {
        return Err(format!("ole-row window log was not refreshed: {log}"));
    }
    Ok(())
}

fn dispatch_press_move(ui: &MainWindow, x: f32, y: f32) {
    let window = ui.window();
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed {
        position: LogicalPosition::new(x, y),
        button: slint::platform::PointerEventButton::Left,
    });
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved {
        position: LogicalPosition::new(x + 24.0, y),
    });
}

fn exercise_slint_drag(ui: &MainWindow) -> Result<(), String> {
    ui.set_request_move(false);
    ui.set_slint_got_drop(false);
    ui.set_slint_dropped("".into());
    ui.set_slint_finished(false);
    if ui.get_slint_drag_w() <= 0.0 || ui.get_slint_drop_w() <= 0.0 {
        return Err("slint drag areas have no size".to_string());
    }
    let from = center(
        ui.get_slint_drag_x(),
        ui.get_slint_drag_y(),
        ui.get_slint_drag_w(),
        ui.get_slint_drag_h(),
    );
    let to = center(
        ui.get_slint_drop_x(),
        ui.get_slint_drop_y(),
        ui.get_slint_drop_w(),
        ui.get_slint_drop_h(),
    );
    line(&format!(
        "slint-geom drag=({}, {}, {}, {}) drop=({}, {}, {}, {}) from=({}, {}) to=({}, {})",
        ui.get_slint_drag_x(),
        ui.get_slint_drag_y(),
        ui.get_slint_drag_w(),
        ui.get_slint_drag_h(),
        ui.get_slint_drop_x(),
        ui.get_slint_drop_y(),
        ui.get_slint_drop_w(),
        ui.get_slint_drop_h(),
        from.0,
        from.1,
        to.0,
        to.1
    ));
    dispatch_drag(ui, from, to);
    line(&format!(
        "slint-in-window got={} finished={} dragging={} action={:?} text={}",
        ui.get_slint_got_drop(),
        ui.get_slint_finished(),
        ui.get_slint_dragging(),
        ui.get_slint_finished_action(),
        ui.get_slint_dropped()
    ));
    if !ui.get_slint_got_drop() {
        return Err("in-window DropArea did not receive the file path".to_string());
    }
    if !ui.get_slint_dropped().contains("readme.txt") {
        return Err(format!(
            "in-window drop text was {}",
            ui.get_slint_dropped()
        ));
    }
    ui.set_slint_got_drop(false);
    ui.set_slint_dropped("".into());
    ui.set_slint_finished(false);
    ui.set_request_move(true);
    dispatch_drag(ui, from, to);
    line(&format!(
        "slint-reject-move got={} finished={} action={:?}",
        ui.get_slint_got_drop(),
        ui.get_slint_finished(),
        ui.get_slint_finished_action()
    ));
    if ui.get_slint_got_drop() {
        return Err("DropArea accepted a move the source disallows".to_string());
    }
    Ok(())
}

fn center(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

fn dispatch_drag(ui: &MainWindow, from: (f32, f32), to: (f32, f32)) {
    let window = ui.window();
    // The first move has to stay inside the DragArea. Slint only starts the drag from the
    // item under the pointer, and its in-window threshold is 8 logical pixels.
    let nudged = LogicalPosition::new(from.0, from.1 + 20.0);
    window.dispatch_event(slint::platform::WindowEvent::PointerPressed {
        position: LogicalPosition::new(from.0, from.1),
        button: slint::platform::PointerEventButton::Left,
    });
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: nudged });
    window.dispatch_event(slint::platform::WindowEvent::PointerMoved {
        position: LogicalPosition::new(to.0, to.1),
    });
    window.dispatch_event(slint::platform::WindowEvent::PointerReleased {
        position: LogicalPosition::new(to.0, to.1),
        button: slint::platform::PointerEventButton::Left,
    });
}

fn hwnd_of(window: &slint::Window) -> Result<windows::Win32::Foundation::HWND, String> {
    let owned = window.window_handle();
    let handle = owned.window_handle().map_err(|error| error.to_string())?;
    match handle.as_raw() {
        RawWindowHandle::Win32(raw) => {
            Ok(windows::Win32::Foundation::HWND(raw.hwnd.get() as *mut _))
        }
        other => Err(format!("unexpected window handle {other:?}")),
    }
}
