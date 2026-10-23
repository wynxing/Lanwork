//! 工具悬浮窗。拖动条、关闭、贴边、来源行和钉住画在 `FloatChrome`。便签、收纳、待办放进同一块，
//! 记录仍在各自的文件里，不进 `tool-windows/`。这个模块负责工具窗口，并把它接进同一套叠页。
//!
//! 应用页的「悬浮」走 [`open_tool`]，打开窗口后运行已安装工具。不自动开始计时组件。
//! 隐藏命令 `lanwork --float-shell-demo` 仍打开一个带「示例」的窗口，不跑清单。
//! 记录在数据目录的 `tool-windows/<id>.json`，含 `floatStack`。已安装工具的记录进入导出和备份，不含 `open`。
//! `lanwork.float-shell-demo` 不进入包。启用开关关掉的工具，即使记录里 `open` 为真，启动时也不再打开。
//! 已经叠在一起的工具页由叠页在启动时打开当前页，不在这里按 `open` 再开一遍。
//! 恢复和更换数据目录前先关掉窗口。停住期间不把 `open` 写回去。

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use lanwork_core::config::ConfigCommands;
use lanwork_core::notes::FloatStackSlot;
use lanwork_core::panel::{
    DropEffect, EnabledBytes, Flip, PxBox, ScanSkip, ScreenBounds, ShellBytes, ShellRecord,
    StackMember, StackPage, StartupPage, geometry_for_open, installed_tools, read_enabled,
    read_record, record_relative, scan_records, shell_default_origin, shell_logical_size,
    should_reopen, tool_source, write_failure_text, write_record,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::storage::Store;
use lanwork_core::todos::TodoCommands;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SetWindowPos,
};

use crate::FloatShell;
use crate::bar_win;
use crate::float_dock::{self, Motion, SavedFloat};

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
    stack: Option<FloatStackSlot>,
    closed: bool,
    /// 叠页里这一页藏着，窗口还在。
    parked: bool,
    writable: bool,
    /// `floatWindow` 读不出。合成的贴边矩形只用于显示，用户改窗口之前不写回。
    preserve_file: bool,
    drag_anchor: Option<PxBox>,
    drag_before: Option<SavedFloat>,
    drag_collapsed: bool,
    stack_pages: i32,
    stack_index: i32,
    /// 位置或贴边变了，等这次借用结束再告诉叠页。
    report: bool,
}

struct Shells {
    store: Store,
    dark: bool,
    windows: HashMap<String, ShellWindow>,
}

thread_local! {
    static SHELLS: RefCell<Option<Shells>> = const { RefCell::new(None) };
    static FREEZE_PERSIST: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn install(store: Store, todos: TodoCommands, config: ConfigCommands) {
    crate::tool_runtime::install(store.clone(), todos, config);
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
/// `floatWindow` 读不出时仍打开，贴边留下，这次也不改文件。
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

/// 打开这个工具的悬浮窗。已经开着时只把焦点给它。不自动开始计时组件。
pub(crate) fn open_tool(tool_id: &str) -> Result<(), String> {
    open(tool_id, None)
}

/// 关掉这个工具的悬浮窗。没开着时没有动作。位置和大小仍按外壳自己的规则记下。
pub(crate) fn close_tool(tool_id: &str) {
    let tool_id = tool_id.to_owned();
    with_window(&tool_id, ShellWindow::request_close);
}

/// 这扇窗还开着时，用新副本和新授权重载脚本。位置、大小、贴边和钉住不动。
pub(crate) fn reload_tool(tool_id: &str) {
    let tool_id = tool_id.to_owned();
    with_window(&tool_id, move |window, _| {
        if window.closed {
            return;
        }
        let ui = window.ui.clone_strong();
        let id = window.id.clone();
        crate::tool_runtime::reload(&id, ui);
    });
}

pub(crate) fn on_todos_changed() {
    crate::tool_runtime::on_todos_changed();
}

pub(crate) fn save_for_quit() {
    crate::tool_runtime::detach_all();
    if !lanwork_core::backup::persist_open_allowed(persist_frozen()) {
        return;
    }
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

/// 恢复前关掉开着的工具窗口，并且不再把 `open` 写回去。返回关掉前开着的 id。
pub(crate) fn quiesce_for_import() -> Vec<String> {
    set_persist_frozen(true);
    dismiss_open()
}

/// 更换数据目录前先把存储写盘，再关掉窗口且不改 `open`。写盘失败时窗口保持原样。
pub(crate) fn quiesce_for_migrate() -> Result<Vec<String>, String> {
    crate::tool_runtime::flush_all()?;
    set_persist_frozen(true);
    Ok(dismiss_open())
}

/// 清掉停住，并打开这些窗口。导入成功时名单是空的；迁移后是切换前开着的。
pub(crate) fn resume_tools(ids: &[String]) {
    set_persist_frozen(false);
    for id in ids {
        if let Err(message) = open(id, None) {
            let id = id.clone();
            with_shells(move |shells| {
                shells
                    .store
                    .log_warn(&format!("工具悬浮窗没有重新打开：{id}：{message}"));
            });
        }
    }
}

fn persist_frozen() -> bool {
    FREEZE_PERSIST.with(Cell::get)
}

fn set_persist_frozen(frozen: bool) {
    FREEZE_PERSIST.with(|cell| cell.set(frozen));
}

fn dismiss_open() -> Vec<String> {
    let windows = SHELLS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return Vec::new();
        };
        let Some(shells) = slot.as_mut() else {
            return Vec::new();
        };
        let ids: Vec<String> = shells
            .windows
            .iter()
            .filter(|(_, window)| !window.closed)
            .map(|(id, _)| id.clone())
            .collect();
        let mut taken = Vec::new();
        for id in ids {
            if let Some(window) = shells.windows.remove(&id) {
                taken.push(window);
            }
        }
        taken
    });
    let ids = windows.iter().map(|window| window.id.clone()).collect();
    for window in windows {
        dismiss(window);
    }
    ids
}

fn dismiss(mut window: ShellWindow) {
    let id = window.id.clone();
    crate::tool_runtime::detach(&window.id);
    window.closed = true;
    window.parked = false;
    window.timer.stop();
    if let Some(hwnd) = window.hwnd() {
        crate::float_group::detach_owner_hwnd(hwnd);
        bar_win::unwatch_topmost(hwnd);
    }
    let _ = window.ui.hide();
    crate::float_group::page_closed(StackPage::Tool(id));
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
                    .filter(|item| item.record.stack.is_none())
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

fn primary_screen() -> (WorkArea, WorkArea, u32) {
    if let Some(reading) = bar_win::primary_reading() {
        return (reading.bounds, reading.work, reading.dpi);
    }
    if let Some(reading) = bar_win::cursor_reading() {
        return (reading.bounds, reading.work, reading.dpi);
    }
    let desktop = WorkArea {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    (desktop, desktop, lanwork_core::shell::BASE_DPI)
}

impl Shells {
    fn open(&mut self, tool_id: &str, declared: Option<(u32, u32)>) -> Result<(), String> {
        if record_relative(tool_id).is_none() {
            return Err("工具 id 不能作为文件名".to_owned());
        }
        if let Some(window) = self.windows.get_mut(tool_id) {
            if !window.closed {
                window.raise();
                notify_opened(tool_id);
                return Ok(());
            }
            self.windows.remove(tool_id);
        }
        let bytes = match read_record(&self.store.data_dir(), tool_id) {
            Ok(bytes) => bytes,
            Err(err) => return Err(err.to_string()),
        };
        let preserve_file = bytes.keep_file_on_open();
        let Some(record) = bytes.openable() else {
            return Err(match bytes {
                ShellBytes::UnsupportedSchema => "外壳记录的 schemaVersion 不受支持".to_owned(),
                _ => "外壳记录读不了".to_owned(),
            });
        };
        if preserve_file {
            self.store.log_warn(&format!(
                "工具悬浮窗的位置读不了，贴边仍用记下的，这次不改文件：{tool_id}"
            ));
        }
        let ui = FloatShell::new().map_err(|err| format!("外壳没有创建：{err}"))?;
        ui.set_dark(self.dark);
        ui.set_solid_color(rgb(solid_rgb(self.dark)));
        ui.set_pinned(record.pinned != Some(false));
        ui.set_show_source(true);
        ui.set_source(source_for(&self.store, tool_id).into());
        if tool_id == DEMO_ID {
            ui.set_marker(DEMO_MARKER.into());
        }
        wire(&ui, tool_id);

        let (bounds, work, dpi) = primary_screen();
        let (logical_width, logical_height) = shell_logical_size(declared);
        let width = u32::try_from(to_physical(logical_width, dpi)).unwrap_or(logical_width);
        let height = u32::try_from(to_physical(logical_height, dpi)).unwrap_or(logical_height);
        let (x, y) = shell_default_origin(
            work,
            i32::try_from(width).unwrap_or(i32::MAX),
            i32::try_from(height).unwrap_or(i32::MAX),
            dpi,
        );
        let geometry = geometry_for_open(
            &record,
            preserve_file,
            (width, height),
            ScreenBounds {
                left: bounds.left,
                top: bounds.top,
                right: bounds.right,
                bottom: bounds.bottom,
                dpi,
            },
        );
        let session = float_dock::session_for_open(
            geometry,
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
            stack: record.stack.clone(),
            closed: false,
            parked: false,
            writable: true,
            preserve_file,
            drag_anchor: None,
            drag_before: None,
            drag_collapsed: false,
            stack_pages: 1,
            stack_index: 0,
            report: false,
        };
        window.persist(&self.store);
        self.windows.insert(tool_id.to_owned(), window);
        notify_opened(tool_id);
        if tool_id != DEMO_ID
            && let Some(window) = self.windows.get(tool_id)
        {
            crate::tool_runtime::attach(tool_id, window.ui.clone_strong());
        }
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
    ui.on_flip_page({
        let id = id.clone();
        move |dir| {
            let flip = if dir < 0 { Flip::Left } else { Flip::Right };
            crate::float_group::flip(StackPage::Tool(id.clone()), flip);
        }
    });
    ui.on_row_clicked({
        let id = id.clone();
        move |row| {
            let row = row.to_string();
            crate::tool_runtime::click(&id, &row);
        }
    });
    ui.window().on_close_requested({
        let id = id.clone();
        move || {
            // 下一轮再关。`taskkill` 会给每个窗口投递 `WM_CLOSE`；若这里先写成 `open: false`，
            // 随后的正常退出就记不住窗口开着。退出已经开始时这一轮关掉不再写。
            let id = id.clone();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if crate::host::quit_in_progress() {
                    return;
                }
                with_window(&id, ShellWindow::request_close);
            });
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
        let id = self.id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            crate::float_group::sync_tool_dots(&id);
        });
    }

    fn toggle_pin(&mut self, store: &Store) {
        if self.closed {
            return;
        }
        self.pinned = Some(!self.ui.get_pinned());
        self.apply_pin(store);
        self.refresh_backdrop();
        self.preserve_file = false;
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
        self.parked = false;
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
        self.refresh_backdrop();
        if !self.session.chrome().collapsed && self.stack_pages > 1 {
            self.ui.invoke_focus_stack_keys();
        }
    }

    fn on_resized(&mut self, width: u32, height: u32, store: &Store) {
        if self.closed || self.parked || !self.hwnd_ready {
            return;
        }
        let origin = self.hwnd().and_then(bar_win::window_origin).or(self.origin);
        let Some(origin) = origin else {
            return;
        };
        let Some(visible) = self.session.absorb_size(origin, width, height) else {
            return;
        };
        self.preserve_file = false;
        self.place(visible);
        self.persist(store);
        self.report = true;
        self.report_move();
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
            if !self.session.chrome().collapsed && self.stack_pages > 1 {
                self.ui.invoke_focus_stack_keys();
            }
        }
        self.report_move();
    }

    fn report_move(&mut self) {
        if !self.report {
            return;
        }
        self.report = false;
        let id = self.id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            crate::float_group::page_moved(StackPage::Tool(id));
        });
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
        if self.closed || self.parked {
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
        if self.closed || self.parked {
            return;
        }
        self.preserve_file = false;
        self.report = true;
        let motion = self.session.strip_click();
        self.follow(motion, store);
    }

    fn on_blur(&mut self, store: &Store) {
        if self.closed || self.parked {
            return;
        }
        if self.hwnd_key().is_some_and(bar_win::size_move_blocks_blur) {
            return;
        }
        let page = StackPage::Tool(self.id.clone());
        if crate::float_group::companion_blocks_blur(&page) {
            return;
        }
        if let Some(motion) = self.session.blur() {
            self.report = true;
            self.follow(motion, store);
        }
    }

    fn on_size_move(&mut self, entering: bool, rect: PxBox, store: &Store) {
        if self.closed || self.parked {
            return;
        }
        if entering {
            self.timer.stop();
            self.drag_collapsed = self.session.chrome().collapsed;
            self.drag_before = Some(self.session.saved());
            self.drag_anchor = Some(rect);
            self.session
                .begin_drag((rect.x, rect.y), (rect.width, rect.height));
            return;
        }
        let anchor = self.drag_anchor.take().unwrap_or(rect);
        let before = self
            .drag_before
            .take()
            .unwrap_or_else(|| self.session.saved());
        let from_collapsed = self.drag_collapsed;
        self.preserve_file = false;
        let page = StackPage::Tool(self.id.clone());
        let effect = crate::float_group::classify_title(page.clone(), anchor, rect, from_collapsed);
        match effect {
            DropEffect::MoveTogether | DropEffect::MergeCurrent { .. } => {
                self.report = true;
                let motion = self.session.end_drag(rect);
                let generation = self.session.hold_generation();
                self.follow(motion, store);
                self.hold_blur(generation);
            }
            DropEffect::DetachCurrent => {
                let motion = self.session.end_drag(rect);
                let generation = self.session.hold_generation();
                self.follow(motion, store);
                let detached = self.session.saved();
                crate::float_group::queue_detach(page, before, detached);
                self.hold_blur(generation);
            }
            DropEffect::MergeUnit { target } => {
                self.session.abandon_drag();
                let generation = self.session.hold_generation();
                crate::float_group::queue_merge_unit(page, target);
                self.hold_blur(generation);
            }
        }
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
        crate::tool_runtime::detach(&self.id);
        self.preserve_file = false;
        self.closed = true;
        self.timer.stop();
        self.persist(store);
        if let Some(hwnd) = self.hwnd() {
            bar_win::unwatch_topmost(hwnd);
        }
        let _ = self.ui.hide();
        let id = self.id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let hwnd = SHELLS.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .and_then(|shells| shells.windows.get(&id))
                    .and_then(ShellWindow::hwnd)
            });
            if let Some(hwnd) = hwnd {
                crate::float_group::detach_owner_hwnd(hwnd);
            }
            let closed = id.clone();
            with_shells(move |shells| {
                shells.windows.remove(&id);
            });
            crate::float_group::page_closed(StackPage::Tool(closed));
        });
    }

    fn present(&mut self, saved: SavedFloat, pages: i32, index: i32) {
        self.parked = false;
        self.report = false;
        self.set_stack_chrome(pages, index);
        self.timer.stop();
        self.session.replace(expanded_of(saved), saved.dock);
        let visible = self.session.visible();
        self.place(visible);
        self.sync_chrome();
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
        self.refresh_backdrop();
        if !self.session.chrome().collapsed && pages > 1 {
            self.ui.invoke_focus_stack_keys();
        }
    }

    fn park(&mut self) {
        if self.closed {
            return;
        }
        self.parked = true;
        self.report = false;
        if let Some(hwnd) = self.hwnd() {
            crate::float_frame::forget(hwnd);
        }
        let _ = self.ui.hide();
    }

    fn set_stack_chrome(&mut self, pages: i32, index: i32) {
        self.stack_pages = pages;
        self.stack_index = index;
        self.ui.set_stack_pages(pages);
        self.ui.set_stack_index(index);
    }

    fn persist(&self, store: &Store) {
        if !lanwork_core::backup::persist_open_allowed(persist_frozen())
            || !self.writable
            || self.preserve_file
        {
            return;
        }
        let saved = self.session.saved();
        let record = ShellRecord {
            geometry: Some(saved.geometry),
            dock: saved.dock,
            pinned: self.pinned,
            open: !self.closed,
            stack: self.stack.clone(),
        };
        match write_record(&store.data_dir(), &self.id, &record) {
            Ok(()) => self.ui.set_error(slint::SharedString::default()),
            Err(err) => {
                store.log_warn(&format!("工具悬浮窗没有记下：{err}"));
                self.ui.set_error(write_failure_text(&err).into());
            }
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

fn notify_opened(tool_id: &str) {
    let id = tool_id.to_owned();
    slint::Timer::single_shot(Duration::ZERO, move || {
        crate::float_group::page_opened(StackPage::Tool(id));
    });
}

fn source_for(store: &Store, tool_id: &str) -> String {
    if tool_id == DEMO_ID {
        return tool_source("");
    }
    let name = installed_tools(&store.data_dir())
        .into_iter()
        .find(|tool| tool.id == tool_id)
        .map(|tool| tool.name)
        .unwrap_or_default();
    tool_source(&name)
}

fn expanded_of(saved: SavedFloat) -> PxBox {
    PxBox {
        x: saved.geometry.x,
        y: saved.geometry.y,
        width: saved.geometry.width,
        height: saved.geometry.height,
    }
}

/// 窗口还在表里（含叠页里藏着的页）。已经关闭、等着释放的不算。
pub(crate) fn contains(tool_id: &str) -> bool {
    SHELLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|shells| shells.windows.get(tool_id))
            .is_some_and(|window| !window.closed)
    })
}

/// 钉住开，或还没记过。窗口不在表里时也按开，页点留在最上层。
pub(crate) fn above_ordinary(tool_id: &str) -> bool {
    SHELLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|shells| shells.windows.get(tool_id))
            .is_none_or(|window| !window.closed && window.pinned != Some(false))
    })
}

/// 看得见的工具窗口句柄。藏着、已关或句柄还没就绪时没有。
pub(crate) fn content_hwnd(tool_id: &str) -> Option<HWND> {
    SHELLS.with(|cell| {
        let shells = cell.borrow();
        let window = shells.as_ref()?.windows.get(tool_id)?;
        if window.closed || window.parked {
            return None;
        }
        window.hwnd()
    })
}

/// 当前看得见的展开位置、贴边和窗口矩形。藏着或已经关闭时没有。
pub(crate) fn view(tool_id: &str) -> Option<(SavedFloat, PxBox)> {
    SHELLS.with(|cell| {
        let shells = cell.borrow();
        let window = shells.as_ref()?.windows.get(tool_id)?;
        if window.closed || window.parked {
            return None;
        }
        Some((window.session.saved(), window.session.visible()))
    })
}

pub(crate) fn present(tool_id: &str, saved: SavedFloat, pages: i32, index: i32) {
    let tool_id = tool_id.to_owned();
    with_window(&tool_id, move |window, _| {
        if window.closed {
            return;
        }
        window.present(saved, pages, index);
    });
}

pub(crate) fn park(tool_id: &str) {
    let tool_id = tool_id.to_owned();
    with_window(&tool_id, |window, _| window.park());
}

pub(crate) fn set_chrome(tool_id: &str, pages: i32, index: i32) {
    let tool_id = tool_id.to_owned();
    with_window(&tool_id, move |window, _| {
        window.set_stack_chrome(pages, index)
    });
}

pub(crate) fn read_slot(tool_id: &str) -> Option<FloatStackSlot> {
    if let Some(slot) = memory_slot(tool_id) {
        return slot;
    }
    file_record(tool_id).and_then(|record| record.stack)
}

pub(crate) fn disk_saved(tool_id: &str) -> Option<SavedFloat> {
    if let Some(saved) = memory_saved(tool_id) {
        return Some(saved);
    }
    let record = file_record(tool_id)?;
    Some(SavedFloat {
        geometry: record.geometry?,
        dock: record.dock,
    })
}

pub(crate) fn write_geometry(tool_id: &str, saved: SavedFloat) {
    let tool_id_owned = tool_id.to_owned();
    let deferred = SHELLS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(shells) = slot.as_mut() else {
            return false;
        };
        if let Some(window) = shells.windows.get_mut(&tool_id_owned)
            && !window.closed
        {
            let current = window.session.saved();
            if current.geometry != saved.geometry || current.dock != saved.dock {
                window.session.replace(expanded_of(saved), saved.dock);
                // `floatWindow` 读不出时留着原文件。启动还原会把叠页的位置写到这里，不能清掉保护。
                if !window.preserve_file {
                    window.persist(&shells.store);
                }
            }
            return false;
        }
        let _ = update_file(&shells.store, &tool_id_owned, |record| {
            record.geometry = Some(saved.geometry);
            record.dock = saved.dock;
        });
        false
    });
    if deferred {
        slint::Timer::single_shot(Duration::ZERO, move || {
            write_geometry(&tool_id_owned, saved)
        });
    }
}

pub(crate) fn write_slot(tool_id: &str, slot: Option<FloatStackSlot>) {
    let tool_id_owned = tool_id.to_owned();
    let deferred = SHELLS.with(|cell| {
        let Ok(mut cell_slot) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(shells) = cell_slot.as_mut() else {
            return false;
        };
        if let Some(window) = shells.windows.get_mut(&tool_id_owned)
            && !window.closed
        {
            if window.stack != slot {
                window.stack = slot.clone();
                // 叠页写回同样留着读不出的 `floatWindow`。用户之后改窗口时再整份记下。
                if !window.preserve_file {
                    window.persist(&shells.store);
                }
            }
            return false;
        }
        let _ = update_file(&shells.store, &tool_id_owned, |record| {
            record.stack = slot.clone();
        });
        false
    });
    if deferred {
        slint::Timer::single_shot(Duration::ZERO, move || write_slot(&tool_id_owned, slot));
    }
}

/// 同一个叠页 id 下的工具页。窗口开着时用内存里的那一份。
pub(crate) fn stack_members(stack_id: &str) -> Vec<StackMember> {
    let Some(store) = store_of() else {
        return Vec::new();
    };
    let Ok((stored, _)) = scan_records(&store.data_dir()) else {
        return Vec::new();
    };
    stored
        .into_iter()
        .filter_map(|item| {
            let slot = memory_slot(&item.id).unwrap_or(item.record.stack)?;
            (slot.id == stack_id).then_some(StackMember {
                page: StackPage::Tool(item.id),
                slot,
            })
        })
        .collect()
}

/// 启动时读到的、带叠页的工具页。打不开的（已卸载或已关闭启用）清掉叠页，不放进这一叠。
pub(crate) fn startup_pages() -> Vec<StartupPage> {
    let Some(store) = store_of() else {
        return Vec::new();
    };
    let (stored, skipped) = match scan_records(&store.data_dir()) {
        Ok(pair) => pair,
        Err(err) => {
            store.log_warn(&format!("工具叠页没有读到：{err}"));
            return Vec::new();
        }
    };
    for skip in &skipped {
        store.log_warn(&skip_line(skip));
    }
    let mut pages = Vec::new();
    for item in stored {
        let Some(slot) = item.record.stack.clone() else {
            continue;
        };
        if !tool_openable(&store, &item.id) {
            let _ = update_file(&store, &item.id, |record| record.stack = None);
            continue;
        }
        pages.push(StartupPage {
            page: StackPage::Tool(item.id),
            slot,
            geometry: item.record.geometry,
            dock: item.record.dock,
        });
    }
    pages
}

/// 没有进已还原叠页、并且退出时开着的工具窗口。单独一页的叠页记录会在打开时清掉。
pub(crate) fn reopen_orphans(in_stack: &[String]) {
    let Some(store) = store_of() else {
        return;
    };
    let Ok((stored, _)) = scan_records(&store.data_dir()) else {
        return;
    };
    let disabled = disabled_ids(&store);
    let ids: Vec<String> = stored
        .into_iter()
        .filter(|item| should_reopen(item.record.open, !disabled.contains(&item.id)))
        .map(|item| item.id)
        .filter(|id| !in_stack.iter().any(|stacked| stacked == id))
        .filter(|id| !contains(id))
        .collect();
    for id in ids {
        if let Err(message) = open(&id, None) {
            let id = id.clone();
            with_shells(move |shells| {
                shells
                    .store
                    .log_warn(&format!("工具悬浮窗没有重新打开：{id}：{message}"));
            });
        }
    }
}

pub(crate) fn tool_openable(store: &Store, tool_id: &str) -> bool {
    if disabled_ids(store).contains(tool_id) {
        return false;
    }
    tool_id == DEMO_ID
        || installed_tools(&store.data_dir())
            .iter()
            .any(|tool| tool.id == tool_id)
}

fn store_of() -> Option<Store> {
    SHELLS.with(|cell| cell.borrow().as_ref().map(|shells| shells.store.clone()))
}

fn memory_slot(tool_id: &str) -> Option<Option<FloatStackSlot>> {
    SHELLS.with(|cell| {
        let shells = cell.borrow();
        let window = shells.as_ref()?.windows.get(tool_id)?;
        if window.closed {
            return None;
        }
        Some(window.stack.clone())
    })
}

fn memory_saved(tool_id: &str) -> Option<SavedFloat> {
    SHELLS.with(|cell| {
        let shells = cell.borrow();
        let window = shells.as_ref()?.windows.get(tool_id)?;
        if window.closed {
            return None;
        }
        Some(window.session.saved())
    })
}

fn file_record(tool_id: &str) -> Option<ShellRecord> {
    let store = store_of()?;
    match read_record(&store.data_dir(), tool_id) {
        Ok(bytes) => bytes.openable(),
        Err(err) => {
            store.log_warn(&format!("工具悬浮窗记录没有读到：{tool_id}：{err}"));
            None
        }
    }
}

fn update_file(
    store: &Store,
    tool_id: &str,
    change: impl FnOnce(&mut ShellRecord),
) -> Result<(), String> {
    let bytes = read_record(&store.data_dir(), tool_id).map_err(|err| err.to_string())?;
    if bytes.keep_file_on_open() {
        store.log_warn(&format!("工具悬浮窗的位置读不了，这次不改叠页：{tool_id}"));
        return Ok(());
    }
    let Some(mut record) = bytes.openable() else {
        return Err("外壳记录读不了".to_owned());
    };
    change(&mut record);
    write_record(&store.data_dir(), tool_id, &record).map_err(|err| err.to_string())?;
    Ok(())
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
