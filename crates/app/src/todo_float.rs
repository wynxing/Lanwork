//! 待办悬浮窗。全程序只有一个，使用时创建，关闭时释放。
//!
//! 列出全部清单里未进回收站的待办，含已完成。勾选走 `complete_item`，取消勾选走
//! `uncomplete_item`。不能新建，也不能改标题。位置在 `todo-float.json`。
//! 贴边和叠页用现有的 `float_dock`、`float_group`。
//! 贴边对得上当前屏幕时按细条或展开的位置打开；对不上时丢掉贴边。
//! 不贴边时，拖动、改大小、按记住的位置打开，以及内容把窗口撑大，整扇都留在所在显示器的工作区里。

use std::cell::RefCell;
use std::time::Duration;

use lanwork_core::panel::{
    DropEffect, FLOAT_HEIGHT, FLOAT_WIDTH, Flip, FloatRow, PxBox, StackPage, float_default_origin,
    float_rows, todo_source,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::storage::Store;
use lanwork_core::todos::{FloatWindowFile, TodoCommands, TodoError};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::bar_win;
use crate::float_dock::{self, Motion, SavedFloat};
use crate::{TodoFloat, TodoFloatRow};

struct FloatWindow {
    ui: TodoFloat,
    rows: Vec<FloatRow>,
    session: float_dock::Session,
    timer: slint::Timer,
    hwnd_ready: bool,
    origin: Option<(i32, i32)>,
    pending: Option<SavedFloat>,
    closed: bool,
    last: Option<SavedFloat>,
    parked: bool,
    drag_anchor: Option<PxBox>,
    drag_before: Option<SavedFloat>,
    drag_collapsed: bool,
    stack_pages: i32,
    stack_index: i32,
    report: bool,
}

struct Floats {
    todos: TodoCommands,
    store: Store,
    dark: bool,
    window: Option<FloatWindow>,
    remembered: Option<SavedFloat>,
    /// 文件的 schema 不受支持。本进程不写回去。
    refused: bool,
}

thread_local! {
    static FLOATS: RefCell<Option<Floats>> = const { RefCell::new(None) };
}

pub(crate) fn install(todos: TodoCommands, store: Store) {
    bar_win::add_move_hook(on_move);
    FLOATS.with(|cell| {
        *cell.borrow_mut() = Some(Floats {
            todos,
            store,
            dark: false,
            window: None,
            remembered: None,
            refused: false,
        });
    });
}

fn with_floats(action: impl FnOnce(&mut Floats) + 'static) {
    let deferred = FLOATS.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => {
            if let Some(floats) = slot.as_mut() {
                action(floats);
            }
            None
        }
        Err(_) => Some(action),
    });
    if let Some(action) = deferred {
        slint::Timer::single_shot(Duration::ZERO, move || with_floats(action));
    }
}

fn with_window(action: impl FnOnce(&mut FloatWindow, &Ctx<'_>) + 'static) {
    with_floats(move |floats| floats.dispatch(action));
}

struct Ctx<'a> {
    todos: &'a TodoCommands,
    store: &'a Store,
    dark: bool,
}

fn rgb(value: u32) -> slint::Color {
    let [_, r, g, b] = value.to_be_bytes();
    slint::Color::from_rgb_u8(r, g, b)
}

/// 打开待办悬浮窗。已经打开时只把焦点给它，不另开第二个。
pub(crate) fn open() -> Result<(), String> {
    FLOATS.with(|cell| {
        let mut slot = cell
            .try_borrow_mut()
            .map_err(|_| "悬浮窗正在处理，请再试一次".to_owned())?;
        let floats = slot
            .as_mut()
            .ok_or_else(|| "待办悬浮窗没有准备好".to_owned())?;
        floats.open()
    })
}

pub(crate) fn apply_theme(dark: bool) {
    crate::float_group::set_dark(dark);
    with_floats(move |floats| {
        floats.dark = dark;
        if let Some(window) = floats.window.as_mut() {
            window.ui.set_dark(dark);
            if !window.parked {
                window.refresh_backdrop(dark);
            }
        }
    });
}

pub(crate) fn contains() -> bool {
    FLOATS.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|floats| floats.window.as_ref())
            .is_some_and(|window| !window.closed)
    })
}

pub(crate) fn view() -> Option<(SavedFloat, PxBox)> {
    FLOATS.with(|cell| {
        let floats = cell.borrow();
        let window = floats.as_ref()?.window.as_ref()?;
        if window.closed || window.parked {
            return None;
        }
        Some((window.session.saved(), window.session.visible()))
    })
}

pub(crate) fn present(saved: SavedFloat, pages: i32, index: i32) {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(window) = slot.as_mut().and_then(|floats| floats.window.as_mut()) else {
            return;
        };
        if window.closed {
            return;
        }
        window.present(saved, pages, index);
    });
}

pub(crate) fn park() {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(window) = slot.as_mut().and_then(|floats| floats.window.as_mut()) else {
            return;
        };
        window.park();
    });
}

pub(crate) fn set_chrome(pages: i32, index: i32) {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(window) = slot.as_mut().and_then(|floats| floats.window.as_mut()) else {
            return;
        };
        window.set_stack_chrome(pages, index);
    });
}

/// 待办有变更时，开着的窗口按当前列表重读。
pub(crate) fn on_todos_changed() {
    with_floats(|floats| {
        let ctx = Ctx {
            todos: &floats.todos,
            store: &floats.store,
            dark: floats.dark,
        };
        if let Some(window) = floats.window.as_mut().filter(|window| !window.closed) {
            window.reload(&ctx);
        }
    });
}

/// 退出前记下仍然打开的窗口的位置和大小。没有正文要保存。
pub(crate) fn save_for_quit() {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(floats) = slot.as_mut() else {
            return;
        };
        let saved = floats
            .window
            .as_ref()
            .filter(|window| !window.closed)
            .map(|window| window.session.saved());
        if let Some(saved) = saved {
            floats.remember(saved);
        }
    });
}

fn notify_opened() {
    slint::Timer::single_shot(Duration::ZERO, || {
        crate::float_group::page_opened(StackPage::Todo);
    });
}

impl Floats {
    fn dispatch(&mut self, action: impl FnOnce(&mut FloatWindow, &Ctx<'_>)) {
        let pending = {
            let ctx = Ctx {
                todos: &self.todos,
                store: &self.store,
                dark: self.dark,
            };
            if let Some(window) = self.window.as_mut() {
                action(window, &ctx);
                window.pending.take()
            } else {
                None
            }
        };
        if let Some(saved) = pending {
            self.remember(saved);
        }
    }

    fn reap(&mut self) {
        let closed = self.window.as_ref().is_some_and(|window| window.closed);
        if !closed {
            return;
        }
        if let Some(window) = self.window.take()
            && let Some(last) = window.last
        {
            self.remember(last);
        }
    }

    fn remember(&mut self, saved: SavedFloat) {
        if self.refused {
            self.remembered = Some(saved);
            return;
        }
        match self.todos.set_float_geometry(saved.geometry, saved.dock) {
            Ok(()) => self.remembered = None,
            Err(TodoError::FloatSchema) => {
                self.refused = true;
                self.remembered = Some(saved);
                self.store
                    .log_warn("todo-float.json 的 schemaVersion 不受支持，位置只留在这次运行里");
            }
            Err(err) => {
                self.remembered = Some(saved);
                self.store
                    .log_warn(&format!("待办悬浮窗位置没有写进去：{err}"));
            }
        }
    }

    fn saved_geometry(
        &mut self,
    ) -> (
        Option<lanwork_core::notes::FloatGeometry>,
        Option<lanwork_core::notes::FloatDock>,
    ) {
        if let Some(saved) = self.remembered {
            return (Some(saved.geometry), saved.dock);
        }
        match self.todos.read_float_window() {
            Ok(FloatWindowFile::Record(record)) => (record.geometry, record.dock),
            Ok(FloatWindowFile::UnsupportedSchema) => {
                self.refused = true;
                self.store.log_warn(
                    "todo-float.json 的 schemaVersion 不受支持，这次不使用也不覆盖这份记录",
                );
                (None, None)
            }
            Err(err) => {
                self.store
                    .log_warn(&format!("待办悬浮窗位置没有读出来：{err}"));
                (None, None)
            }
        }
    }

    fn open(&mut self) -> Result<(), String> {
        if let Some(window) = self.window.as_mut() {
            if !window.closed {
                window.parked = false;
                window.raise();
                notify_opened();
                return Ok(());
            }
            self.reap();
        }
        let ui = TodoFloat::new().map_err(|err| format!("悬浮窗没有创建：{err}"))?;
        ui.set_source(todo_source().into());
        ui.set_dark(self.dark);
        ui.set_solid_color(rgb(solid_rgb(self.dark)));
        wire(&ui);

        let (work, dpi) = bar_win::cursor_monitor().unwrap_or((
            WorkArea {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            lanwork_core::shell::BASE_DPI,
        ));
        let (saved_geometry, saved_dock) = self.saved_geometry();
        let default_size = (
            u32::try_from(to_physical(FLOAT_WIDTH, dpi)).unwrap_or(FLOAT_WIDTH),
            u32::try_from(to_physical(FLOAT_HEIGHT, dpi)).unwrap_or(FLOAT_HEIGHT),
        );
        let default_origin = float_default_origin(
            work,
            i32::try_from(default_size.0).unwrap_or(i32::MAX),
            i32::try_from(default_size.1).unwrap_or(i32::MAX),
            dpi,
            0,
        );
        let session = float_dock::session_for_open(
            saved_geometry,
            saved_dock,
            PxBox {
                x: default_origin.0,
                y: default_origin.1,
                width: default_size.0,
                height: default_size.1,
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
        ui.show().map_err(|err| format!("悬浮窗没有显示：{err}"))?;

        let ready = ui.clone_strong();
        let spawned = slint::spawn_local(async move {
            if let Ok(window) = ready.window().winit_window().await {
                use slint::winit_030::winit::platform::windows::WindowExtWindows as _;
                window.set_skip_taskbar(true);
            }
            with_window(|window, ctx| window.ready(ctx));
        });
        if let Err(err) = spawned {
            self.store
                .log_warn(&format!("待办悬浮窗初始化未排队：{err}"));
        }
        let mut window = FloatWindow {
            ui,
            rows: Vec::new(),
            session,
            timer: slint::Timer::default(),
            hwnd_ready: false,
            origin: Some((visible.x, visible.y)),
            pending: None,
            closed: false,
            last: None,
            parked: false,
            drag_anchor: None,
            drag_before: None,
            drag_collapsed: false,
            stack_pages: 1,
            stack_index: 0,
            report: false,
        };
        let ctx = Ctx {
            todos: &self.todos,
            store: &self.store,
            dark: self.dark,
        };
        window.reload(&ctx);
        self.window = Some(window);
        notify_opened();
        Ok(())
    }
}

fn wire(ui: &TodoFloat) {
    ui.on_close_clicked(|| with_window(|window, _| window.request_close()));
    ui.window().on_close_requested(|| {
        with_window(|window, _| window.request_close());
        CloseRequestResponse::KeepWindowShown
    });
    ui.on_strip_clicked(|| with_window(|window, _| window.on_strip()));
    ui.on_drag_window(|| with_window(|window, _| window.drag()));
    ui.on_flip_page(|dir| {
        let flip = if dir < 0 { Flip::Left } else { Flip::Right };
        crate::float_group::flip(StackPage::Todo, flip);
    });
    ui.on_toggle_row(|index| with_window(move |window, ctx| window.toggle(ctx, index)));
    ui.window().on_winit_window_event(|_, event| {
        if matches!(event, winit::event::WindowEvent::Focused(false)) {
            with_window(|window, _| window.on_blur());
        }
        if let winit::event::WindowEvent::Resized(size) = event {
            let (width, height) = (size.width, size.height);
            with_window(move |window, _| window.on_resized(width, height));
        }
        if crate::float_frame::wants_refresh(event) {
            with_window(|window, _| {
                if !window.parked {
                    window.refresh_backdrop(window.ui.get_dark());
                }
            });
        }
        EventResult::Propagate
    });
}

impl FloatWindow {
    fn ready(&mut self, ctx: &Ctx<'_>) {
        if self.closed {
            return;
        }
        self.hwnd_ready = true;
        if let Some(hwnd) = self.hwnd() {
            if !bar_win::remove_caption_buttons(hwnd) {
                ctx.store
                    .log_warn("悬浮窗样式子类没有装上，显示时可能出现标题栏按钮");
            }
            if !bar_win::watch_move_loop(hwnd) {
                ctx.store
                    .log_warn("待办悬浮窗拖动结束通知没有装上，松手后可能不贴边");
            }
            if !bar_win::watch_topmost(hwnd) {
                ctx.store
                    .log_warn("待办悬浮窗的前台通知没有装上，全屏时细条可能被盖住");
            }
            if self.parked {
                crate::float_frame::forget(hwnd);
                let _ = self.ui.hide();
                return;
            }
            let visible = self.session.visible();
            self.place(visible);
        }
        if self.parked {
            let _ = self.ui.hide();
            return;
        }
        self.sync_chrome();
        self.refresh_backdrop(ctx.dark);
        self.raise();
    }

    fn hwnd(&self) -> Option<windows::Win32::Foundation::HWND> {
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
        self.refresh_backdrop(self.ui.get_dark());
    }

    fn place(&mut self, rect: PxBox) {
        apply_limits(&self.ui, &self.session);
        self.session.set_absorb_lock(true);
        float_dock::place(self.ui.window(), self.hwnd(), rect);
        self.session.set_absorb_lock(false);
        self.origin = Some((rect.x, rect.y));
    }

    /// 内容或系统把窗口撑到工作区外面时拉回来。拖动和自己摆放时不收。
    fn on_resized(&mut self, width: u32, height: u32) {
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
        self.place(visible);
        self.pending = Some(self.session.saved());
        self.report = true;
        self.report_move();
    }

    fn follow(&mut self, motion: Motion) {
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
            self.pending = Some(self.session.saved());
            if !self.session.chrome().collapsed {
                self.ui.invoke_focus_float();
            }
        }
        self.report_move();
    }

    fn report_move(&mut self) {
        if !self.report {
            return;
        }
        self.report = false;
        slint::Timer::single_shot(Duration::ZERO, || {
            crate::float_group::page_moved(StackPage::Todo);
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
        self.ui.set_source(todo_source().into());
        self.sync_chrome();
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
        // 叠页把藏着的这一页重新显示时，show 可能把系统背景设回默认。显示之后再写一次。
        self.refresh_backdrop(self.ui.get_dark());
        if !self.session.chrome().collapsed {
            self.ui.invoke_focus_float();
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

    fn start_anim(&mut self) {
        if self.timer.running() {
            return;
        }
        self.timer.start(
            slint::TimerMode::Repeated,
            float_dock::frame_interval(),
            || with_window(|window, _| window.anim_tick()),
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

    fn on_strip(&mut self) {
        if self.closed || self.parked {
            return;
        }
        self.report = true;
        let motion = self.session.strip_click();
        self.follow(motion);
    }

    fn on_blur(&mut self) {
        if self.closed || self.parked {
            return;
        }
        if self.hwnd_key().is_some_and(bar_win::size_move_blocks_blur) {
            return;
        }
        if crate::float_group::companion_blocks_blur(&StackPage::Todo) {
            return;
        }
        if let Some(motion) = self.session.blur() {
            self.report = true;
            self.follow(motion);
        }
    }

    fn on_size_move(&mut self, entering: bool, rect: PxBox) {
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
        let effect =
            crate::float_group::classify_title(StackPage::Todo, anchor, rect, from_collapsed);
        match effect {
            DropEffect::MoveTogether | DropEffect::MergeCurrent { .. } => {
                self.report = true;
                let motion = self.session.end_drag(rect);
                let generation = self.session.hold_generation();
                self.follow(motion);
                self.hold_blur_after_drag(generation);
            }
            DropEffect::DetachCurrent => {
                let motion = self.session.end_drag(rect);
                let generation = self.session.hold_generation();
                self.follow(motion);
                let detached = self.session.saved();
                crate::float_group::queue_detach(StackPage::Todo, before, detached);
                self.hold_blur_after_drag(generation);
            }
            DropEffect::MergeUnit { target } => {
                self.session.abandon_drag();
                let generation = self.session.hold_generation();
                crate::float_group::queue_merge_unit(StackPage::Todo, target);
                self.hold_blur_after_drag(generation);
            }
        }
    }

    fn hold_blur_after_drag(&mut self, generation: u8) {
        let Some(key) = self.hwnd_key() else {
            return;
        };
        slint::Timer::single_shot(Duration::ZERO, move || {
            slint::Timer::single_shot(Duration::ZERO, move || {
                bar_win::clear_blur_hold(key);
                with_window(move |window, _| window.session.release_hold(generation));
            });
        });
    }

    fn refresh_backdrop(&mut self, dark: bool) {
        let collapsed = self.ui.get_collapsed();
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
        self.ui.set_source(todo_source().into());
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
        self.refresh_backdrop(self.ui.get_dark());
    }

    fn fail(&self, message: &str) {
        self.ui.set_error(message.into());
    }

    fn clear_error(&self) {
        self.ui.set_error(SharedString::new());
    }

    fn reload(&mut self, ctx: &Ctx<'_>) {
        let lists = match ctx.todos.lists() {
            Ok(lists) => lists,
            Err(err) => {
                self.fail(&err.to_string());
                return;
            }
        };
        self.rows = float_rows(&lists, crate::host::local_today());
        self.push();
        self.clear_error();
    }

    fn push(&self) {
        let rows: Vec<TodoFloatRow> = self
            .rows
            .iter()
            .map(|row| TodoFloatRow {
                title: row.title.as_str().into(),
                completed: row.completed,
                overdue: row.overdue,
            })
            .collect();
        self.ui.set_rows(ModelRc::new(VecModel::from(rows)));
    }

    fn toggle(&mut self, ctx: &Ctx<'_>, index: i32) {
        let Some(row) = usize::try_from(index)
            .ok()
            .and_then(|index| self.rows.get(index))
        else {
            return;
        };
        let id = row.id.clone();
        let completed = row.completed;
        let result = if completed {
            ctx.todos.uncomplete_item(&id)
        } else {
            ctx.todos.complete_item(&id)
        };
        match result {
            Ok(()) => {
                self.clear_error();
                self.reload(ctx);
            }
            Err(err) => self.fail(&err.to_string()),
        }
    }

    fn request_close(&mut self) {
        if self.closed {
            return;
        }
        self.finish_close();
    }

    fn finish_close(&mut self) {
        self.timer.stop();
        if let Some(hwnd) = self.hwnd() {
            bar_win::unwatch_topmost(hwnd);
        }
        self.last = Some(self.session.saved());
        self.closed = true;
        let _ = self.ui.hide();
        slint::Timer::single_shot(Duration::ZERO, || {
            with_floats(Floats::reap);
            crate::float_group::page_closed(StackPage::Todo);
        });
    }
}

fn apply_limits(ui: &TodoFloat, session: &float_dock::Session) {
    let (width, height) = session.limit_lengths();
    ui.set_limit_width(width);
    ui.set_limit_height(height);
}

fn expanded_of(saved: SavedFloat) -> PxBox {
    PxBox {
        x: saved.geometry.x,
        y: saved.geometry.y,
        width: saved.geometry.width,
        height: saved.geometry.height,
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
    with_floats(move |floats| {
        let hit = floats
            .window
            .as_ref()
            .is_some_and(|window| !window.closed && window.hwnd_key() == Some(key));
        if hit {
            floats.dispatch(|window, _| {
                window.on_size_move(
                    entering,
                    PxBox {
                        x,
                        y,
                        width,
                        height,
                    },
                );
            });
        }
    });
}
