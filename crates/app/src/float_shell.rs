//! 工具悬浮窗。外壳负责置顶、拖动、贴边和记忆，内容由调用方放进槽里。
//!
//! 应用页的「悬浮窗」走 [`open_tool`]，只打开窗口，不跑清单，也不开始计时。
//! 隐藏命令 `lanwork --float-shell-demo` 仍打开一个带「示例」的窗口。
//! 记录在数据目录的 `tool-windows/<id>.json`。不进叠页，也不进导出白名单。
//! 启用开关关掉的工具，即使记录里 `open` 为真，启动时也不再打开。

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use lanwork_core::panel::{
    EnabledBytes, PxBox, ScanSkip, ShellBytes, ShellRecord, read_enabled, read_record,
    record_relative, scan_records, shell_default_origin, shell_logical_size, should_reopen,
    write_record,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::storage::Store;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SetWindowPos,
};

use crate::FloatShell;
use crate::bar_win;
use crate::float_dock::{self, Motion};

/// 调试窗口的 id。不是已安装的工具。
const DEMO_ID: &str = "lanwork.float-shell-demo";
const DEMO_MARKER: &str = "示例";

struct ShellWindow {
    id: String,
    ui: FloatShell,
    session: float_dock::Session,
    timer: slint::Timer,
    hwnd_ready: bool,
    origin: Option<(i32, i32)>,
    pinned: Option<bool>,
    closed: bool,
    writable: bool,
}

struct Shells {
    store: Store,
    dark: bool,
    windows: HashMap<String, ShellWindow>,
}

thread_local! {
    static SHELLS: RefCell<Option<Shells>> = const { RefCell::new(None) };
}

pub(crate) fn install(store: Store) {
    bar_win::add_move_hook(on_move);
    SHELLS.with(|cell| {
        *cell.borrow_mut() = Some(Shells {
            store,
            dark: false,
            windows: HashMap::new(),
        });
    });
}

pub(crate) fn apply_theme(dark: bool) {
    with_shells(move |shells| {
        shells.dark = dark;
        for window in shells.windows.values_mut() {
            window.ui.set_dark(dark);
            if !window.closed {
                window.refresh_backdrop();
            }
        }
    });
}

/// 下一轮事件循环再打开 `open` 为真、并且启用开关开着的窗口。坏记录跳过，不改文件。
pub(crate) fn schedule_restore() {
    slint::Timer::single_shot(Duration::ZERO, restore_open);
}

/// 下一轮事件循环打开调试外壳。已经开着时只把它显示到前面。
pub(crate) fn schedule_demo() {
    slint::Timer::single_shot(Duration::ZERO, || {
        if let Err(message) = open(DEMO_ID, None) {
            with_shells(move |shells| {
                shells
                    .store
                    .log_warn(&format!("调试外壳没有打开：{message}"));
            });
        }
    });
}

/// 打开这个工具的悬浮窗。已经开着时只把焦点给它。不设置示例文字，也不开始计时。
pub(crate) fn open_tool(tool_id: &str) -> Result<(), String> {
    open(tool_id, None)
}

/// 关掉这个工具的悬浮窗。没开着时没有动作。位置和大小仍按外壳自己的规则记下。
pub(crate) fn close_tool(tool_id: &str) {
    let tool_id = tool_id.to_owned();
    with_window(&tool_id, ShellWindow::request_close);
}

pub(crate) fn save_for_quit() {
    SHELLS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(shells) = slot.as_mut() else {
            return;
        };
        let store = shells.store.clone();
        for window in shells.windows.values() {
            if !window.closed {
                window.persist(&store);
            }
        }
    });
}

fn restore_open() {
    let ids = SHELLS.with(|cell| {
        let Ok(slot) = cell.try_borrow() else {
            return Vec::new();
        };
        let Some(shells) = slot.as_ref() else {
            return Vec::new();
        };
        match scan_records(&shells.store.data_dir()) {
            Ok((stored, skipped)) => {
                for skip in &skipped {
                    shells.store.log_warn(&skip_line(skip));
                }
                let disabled = disabled_ids(&shells.store);
                stored
                    .into_iter()
                    .filter(|item| should_reopen(item.record.open, !disabled.contains(&item.id)))
                    .map(|item| item.id)
                    .collect()
            }
            Err(err) => {
                shells
                    .store
                    .log_warn(&format!("工具悬浮窗记录没有读到：{err}"));
                Vec::new()
            }
        }
    });
    for id in ids {
        if let Err(message) = open(&id, None) {
            with_shells(move |shells| {
                shells
                    .store
                    .log_warn(&format!("工具悬浮窗没有重新打开：{id}：{message}"));
            });
        }
    }
}

fn open(tool_id: &str, declared: Option<(u32, u32)>) -> Result<(), String> {
    let tool_id = tool_id.to_owned();
    SHELLS.with(|cell| {
        let mut slot = cell
            .try_borrow_mut()
            .map_err(|_| "外壳正在处理，请再试一次".to_owned())?;
        let shells = slot.as_mut().ok_or_else(|| "外壳还没有装上".to_owned())?;
        shells.open(&tool_id, declared)
    })
}

fn with_shells(action: impl FnOnce(&mut Shells) + 'static) {
    let deferred = SHELLS.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => {
            if let Some(shells) = slot.as_mut() {
                action(shells);
            }
            None
        }
        Err(_) => Some(action),
    });
    if let Some(action) = deferred {
        slint::Timer::single_shot(Duration::ZERO, move || with_shells(action));
    }
}

fn with_window(id: &str, action: impl FnOnce(&mut ShellWindow, &Store) + 'static) {
    let id = id.to_owned();
    with_shells(move |shells| {
        let store = shells.store.clone();
        if let Some(window) = shells.windows.get_mut(&id) {
            action(window, &store);
        }
    });
}

fn disabled_ids(store: &lanwork_core::storage::Store) -> BTreeSet<String> {
    match read_enabled(&store.data_dir()) {
        Ok(EnabledBytes::Known(set)) => set,
        Ok(EnabledBytes::Corrupt) => {
            store.log_warn("启用状态读不了");
            BTreeSet::new()
        }
        Ok(EnabledBytes::Unsupported) => {
            store.log_warn("启用状态的 schemaVersion 不受支持");
            BTreeSet::new()
        }
        Err(err) => {
            store.log_warn(&format!("启用状态没有读到：{err}"));
            BTreeSet::new()
        }
    }
}

fn skip_line(skip: &ScanSkip) -> String {
    match skip {
        ScanSkip::Unreadable(id) => format!("工具悬浮窗记录读不到：{id}"),
        ScanSkip::Corrupt(id) => format!("工具悬浮窗记录读不了：{id}"),
        ScanSkip::Unsupported(id) => format!("工具悬浮窗记录的 schemaVersion 不受支持：{id}"),
    }
}

fn rgb(value: u32) -> slint::Color {
    let [_, r, g, b] = value.to_be_bytes();
    slint::Color::from_rgb_u8(r, g, b)
}

fn primary_work() -> (WorkArea, u32) {
    if let Some(reading) = bar_win::primary_reading() {
        return (reading.work, reading.dpi);
    }
    bar_win::cursor_monitor().unwrap_or((
        WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        },
        lanwork_core::shell::BASE_DPI,
    ))
}

impl Shells {
    fn open(&mut self, tool_id: &str, declared: Option<(u32, u32)>) -> Result<(), String> {
        if record_relative(tool_id).is_none() {
            return Err("工具 id 不能作为文件名".to_owned());
        }
        if let Some(window) = self.windows.get_mut(tool_id) {
            if !window.closed {
                window.raise();
                return Ok(());
            }
            self.windows.remove(tool_id);
        }
        let (record, writable) = match read_record(&self.store.data_dir(), tool_id) {
            Ok(ShellBytes::Record(record)) => (record, true),
            Ok(ShellBytes::UnsupportedSchema) => {
                return Err("外壳记录的 schemaVersion 不受支持".to_owned());
            }
            Ok(ShellBytes::Corrupt) => return Err("外壳记录读不了".to_owned()),
            Err(err) => return Err(err.to_string()),
        };
        let ui = FloatShell::new().map_err(|err| format!("外壳没有创建：{err}"))?;
        ui.set_dark(self.dark);
        ui.set_solid_color(rgb(solid_rgb(self.dark)));
        ui.set_pinned(record.pinned != Some(false));
        if tool_id == DEMO_ID {
            ui.set_marker(DEMO_MARKER.into());
        }
        wire(&ui, tool_id);

        let (work, dpi) = primary_work();
        let (logical_width, logical_height) = shell_logical_size(declared);
        let width = u32::try_from(to_physical(logical_width, dpi)).unwrap_or(logical_width);
        let height = u32::try_from(to_physical(logical_height, dpi)).unwrap_or(logical_height);
        let (x, y) = shell_default_origin(
            work,
            i32::try_from(width).unwrap_or(i32::MAX),
            i32::try_from(height).unwrap_or(i32::MAX),
            dpi,
        );
        let session = float_dock::session_for_open(
            record.geometry,
            record.dock,
            PxBox {
                x,
                y,
                width,
                height,
            },
        );
        let chrome = session.chrome();
        ui.set_thin(chrome.thin);
        ui.set_collapsed(chrome.collapsed);
        ui.set_dock_edge(chrome.edge.into());
        apply_limits(&ui, &session);
        let visible = session.visible();
        ui.window()
            .set_size(slint::PhysicalSize::new(visible.width, visible.height));
        ui.window()
            .set_position(slint::PhysicalPosition::new(visible.x, visible.y));
        ui.show().map_err(|err| format!("外壳没有显示：{err}"))?;

        let id = tool_id.to_owned();
        let ready = ui.clone_strong();
        let spawned = slint::spawn_local(async move {
            if let Ok(window) = ready.window().winit_window().await {
                use slint::winit_030::winit::platform::windows::WindowExtWindows as _;
                window.set_skip_taskbar(true);
            }
            with_window(&id, |window, store| window.ready(store));
        });
        if let Err(err) = spawned {
            self.store
                .log_warn(&format!("工具悬浮窗初始化未排队：{err}"));
        }
        let window = ShellWindow {
            id: tool_id.to_owned(),
            ui,
            session,
            timer: slint::Timer::default(),
            hwnd_ready: false,
            origin: Some((visible.x, visible.y)),
            pinned: record.pinned,
            closed: false,
            writable,
        };
        window.persist(&self.store);
        self.windows.insert(tool_id.to_owned(), window);
        Ok(())
    }
}

fn wire(ui: &FloatShell, tool_id: &str) {
    let id = tool_id.to_owned();
    ui.on_close_clicked({
        let id = id.clone();
        move || with_window(&id, ShellWindow::request_close)
    });
    ui.on_pin_clicked({
        let id = id.clone();
        move || with_window(&id, ShellWindow::toggle_pin)
    });
    ui.on_strip_clicked({
        let id = id.clone();
        move || with_window(&id, |window, store| window.on_strip(store))
    });
    ui.on_drag_requested({
        let id = id.clone();
        move || with_window(&id, |window, _| window.drag())
    });
    ui.window().on_close_requested({
        let id = id.clone();
        move || {
            with_window(&id, ShellWindow::request_close);
            CloseRequestResponse::KeepWindowShown
        }
    });
    ui.window().on_winit_window_event({
        let id = id.clone();
        move |_, event| {
            if matches!(event, winit::event::WindowEvent::Focused(false)) {
                with_window(&id, |window, store| window.on_blur(store));
            }
            if let winit::event::WindowEvent::Resized(size) = event {
                let (width, height) = (size.width, size.height);
                with_window(&id, move |window, store| {
                    window.on_resized(width, height, store);
                });
            }
            if crate::float_frame::wants_refresh(event) {
                with_window(&id, |window, _| {
                    if !window.closed {
                        window.refresh_backdrop();
                    }
                });
            }
            EventResult::Propagate
        }
    });
}

impl ShellWindow {
    fn ready(&mut self, store: &Store) {
        if self.closed {
            return;
        }
        self.hwnd_ready = true;
        if let Some(hwnd) = self.hwnd() {
            if !bar_win::remove_caption_buttons(hwnd) {
                store.log_warn("工具悬浮窗样式子类没有装上，显示时可能出现标题栏按钮");
            }
            if !bar_win::watch_move_loop(hwnd) {
                store.log_warn("工具悬浮窗拖动结束通知没有装上，松手后可能不贴边");
            }
            self.apply_pin(store);
            let visible = self.session.visible();
            self.place(visible);
        }
        self.sync_chrome();
        self.refresh_backdrop();
        self.raise();
    }

    fn hwnd(&self) -> Option<HWND> {
        if self.hwnd_ready {
            bar_win::hwnd_of(self.ui.window())
        } else {
            None
        }
    }

    fn hwnd_key(&self) -> Option<isize> {
        self.hwnd().map(bar_win::hwnd_key)
    }

    fn sync_chrome(&mut self) {
        let chrome = self.session.chrome();
        self.ui.set_thin(chrome.thin);
        self.ui.set_collapsed(chrome.collapsed);
        self.ui.set_dock_edge(chrome.edge.into());
        apply_limits(&self.ui, &self.session);
        self.refresh_backdrop();
    }

    fn place(&mut self, rect: PxBox) {
        apply_limits(&self.ui, &self.session);
        self.session.set_absorb_lock(true);
        place_at(
            self.ui.window(),
            self.hwnd(),
            rect,
            self.pinned != Some(false),
        );
        self.session.set_absorb_lock(false);
        self.origin = Some((rect.x, rect.y));
    }

    fn apply_pin(&mut self, store: &Store) {
        let on = self.pinned != Some(false);
        self.ui.set_pinned(on);
        let Some(hwnd) = self.hwnd() else {
            return;
        };
        if on {
            if !bar_win::watch_topmost(hwnd) {
                store.log_warn("工具悬浮窗的前台通知没有装上，钉住时可能被盖住");
            }
        } else {
            bar_win::release_topmost(hwnd);
        }
    }

    fn toggle_pin(&mut self, store: &Store) {
        if self.closed {
            return;
        }
        self.pinned = Some(!self.ui.get_pinned());
        self.apply_pin(store);
        self.persist(store);
    }

    fn refresh_backdrop(&mut self) {
        let collapsed = self.ui.get_collapsed();
        let dark = self.ui.get_dark();
        let actual = match self.hwnd() {
            Some(hwnd) => crate::float_frame::sync(hwnd, collapsed, dark),
            None => Backdrop::Solid,
        };
        self.ui
            .set_acrylic(!collapsed && actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(dark)));
        if collapsed {
            self.ui.window().request_redraw();
        }
    }

    fn raise(&mut self) {
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
        self.refresh_backdrop();
    }

    fn on_resized(&mut self, width: u32, height: u32, store: &Store) {
        if self.closed || !self.hwnd_ready {
            return;
        }
        let origin = self.hwnd().and_then(bar_win::window_origin).or(self.origin);
        let Some(origin) = origin else {
            return;
        };
        let Some(visible) = self.session.absorb_size(origin, width, height) else {
            return;
        };
        self.place(visible);
        self.persist(store);
    }

    fn follow(&mut self, motion: Motion, store: &Store) {
        if motion.animate {
            self.ui.set_thin(true);
            self.sync_chrome();
            self.place(motion.from);
            self.start_anim();
        } else {
            self.ui.set_thin(self.session.chrome().thin);
            self.place(motion.to);
            self.sync_chrome();
        }
        if motion.persist {
            self.persist(store);
        }
    }

    fn start_anim(&mut self) {
        if self.timer.running() {
            return;
        }
        let id = self.id.clone();
        self.timer.start(
            slint::TimerMode::Repeated,
            float_dock::frame_interval(),
            move || with_window(&id, |window, _| window.anim_tick()),
        );
    }

    fn anim_tick(&mut self) {
        let Some(frame) = self.session.next_frame() else {
            self.timer.stop();
            self.sync_chrome();
            return;
        };
        if self.session.animating() {
            self.ui.set_thin(true);
            self.sync_chrome();
            self.place(frame);
        } else {
            self.timer.stop();
            self.place(frame);
            self.sync_chrome();
        }
    }

    fn drag(&mut self) {
        if self.closed {
            return;
        }
        let origin = self
            .hwnd()
            .and_then(bar_win::window_origin)
            .or(self.origin)
            .unwrap_or((0, 0));
        let size = self.ui.window().size();
        self.timer.stop();
        self.session.begin_drag(origin, (size.width, size.height));
        let _ = self.ui.window().with_winit_window(|window| {
            let _ = window.drag_window();
        });
    }

    fn on_strip(&mut self, store: &Store) {
        if self.closed {
            return;
        }
        let motion = self.session.strip_click();
        self.follow(motion, store);
    }

    fn on_blur(&mut self, store: &Store) {
        if self.closed {
            return;
        }
        if self.hwnd_key().is_some_and(bar_win::size_move_blocks_blur) {
            return;
        }
        if let Some(motion) = self.session.blur() {
            self.follow(motion, store);
        }
    }

    fn on_size_move(&mut self, entering: bool, rect: PxBox, store: &Store) {
        if self.closed {
            return;
        }
        if entering {
            self.timer.stop();
            self.session
                .begin_drag((rect.x, rect.y), (rect.width, rect.height));
            return;
        }
        let motion = self.session.end_drag(rect);
        let generation = self.session.hold_generation();
        self.follow(motion, store);
        self.hold_blur(generation);
    }

    fn hold_blur(&mut self, generation: u8) {
        let Some(key) = self.hwnd_key() else {
            return;
        };
        let id = self.id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            slint::Timer::single_shot(Duration::ZERO, move || {
                bar_win::clear_blur_hold(key);
                with_window(&id, move |window, _| {
                    window.session.release_hold(generation)
                });
            });
        });
    }

    fn request_close(&mut self, store: &Store) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.timer.stop();
        self.persist(store);
        if let Some(hwnd) = self.hwnd() {
            bar_win::unwatch_topmost(hwnd);
        }
        let _ = self.ui.hide();
        let id = self.id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            with_shells(move |shells| {
                shells.windows.remove(&id);
            });
        });
    }

    fn persist(&self, store: &Store) {
        if !self.writable {
            return;
        }
        let saved = self.session.saved();
        let record = ShellRecord {
            geometry: Some(saved.geometry),
            dock: saved.dock,
            pinned: self.pinned,
            open: !self.closed,
        };
        if let Err(err) = write_record(&store.data_dir(), &self.id, &record) {
            store.log_warn(&format!("工具悬浮窗没有记下：{err}"));
        }
    }
}

fn apply_limits(ui: &FloatShell, session: &float_dock::Session) {
    let (width, height) = session.limit_lengths();
    ui.set_limit_width(width);
    ui.set_limit_height(height);
}

fn place_at(window: &slint::Window, hwnd: Option<HWND>, rect: PxBox, topmost: bool) {
    let width = rect.width.max(1);
    let height = rect.height.max(1);
    window.set_size(slint::PhysicalSize::new(width, height));
    window.set_position(slint::PhysicalPosition::new(rect.x, rect.y));
    let Some(hwnd) = hwnd else {
        return;
    };
    let after = if topmost {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    let width = i32::try_from(width).unwrap_or(i32::MAX);
    let height = i32::try_from(height).unwrap_or(i32::MAX);
    // SAFETY: hwnd 是本进程的工具悬浮窗。SWP_NOACTIVATE 不抢走焦点。
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(after),
            rect.x,
            rect.y,
            width,
            height,
            SWP_NOACTIVATE,
        );
    }
}

fn on_move(key: isize, entering: bool, x: i32, y: i32, width: u32, height: u32) {
    let posted = slint::invoke_from_event_loop(move || {
        deliver_move(key, entering, x, y, width, height);
    });
    if posted.is_err() {
        deliver_move(key, entering, x, y, width, height);
    }
}

fn deliver_move(key: isize, entering: bool, x: i32, y: i32, width: u32, height: u32) {
    with_shells(move |shells| {
        let id = shells
            .windows
            .iter()
            .find_map(|(id, window)| (window.hwnd_key() == Some(key)).then(|| id.clone()));
        let Some(id) = id else {
            return;
        };
        let store = shells.store.clone();
        if let Some(window) = shells.windows.get_mut(&id) {
            window.on_size_move(
                entering,
                PxBox {
                    x,
                    y,
                    width,
                    height,
                },
                &store,
            );
        }
    });
}
