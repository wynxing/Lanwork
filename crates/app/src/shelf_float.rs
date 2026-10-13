//! 收纳分组的悬浮窗：只显示该分组条目的独立窗口，使用时创建，关闭时释放。
//!
//! 同一分组已经有悬浮窗时只聚焦它，不把已经收起的细条展开。窗口始终在最上层。
//! 每个分组记住自己的展开位置、大小和贴边：写进分组文件的 `floatWindow` 和 `floatDock`。
//! 贴边对得上当前屏幕时按细条或展开的位置打开；对不上时丢掉贴边。
//! 不贴边时，拖动、改大小、按记住的位置打开，以及内容把窗口撑大，整扇都留在所在显示器的工作区里。
//! 写不成时，本进程里还记着这个值。窗口失焦不关闭，贴边展开时失焦会收回成细条；面板自己的失焦收起不受影响。
//!
//! 条目列表逻辑在 `shelf_refs`，和面板的收纳页共用。每个窗口单独登记 OLE 放下目标（`shelf_dnd`），
//! 关闭时先撤销登记再隐藏。窗口上方有一个可折叠的「关联待办」，默认收起：列出分组关联的待办，
//! 可以勾选完成（`TodoCommands::complete_item`），点标题打开面板并定位。版式在 `ui/shelffloat.slint`。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lanwork_core::dispatch::IconCache;
use lanwork_core::panel::{
    DropEffect, DropZone, FLOAT_HEIGHT, FLOAT_WIDTH, Flip, LinkedTodo, PanelTarget, PxBox,
    ShelfLine, StackPage, float_default_origin, linked_todos, shelf_lines,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::shelves::{IncomingRef, IncomingWeb, ShelfCommands, ShelfError};
use lanwork_core::storage::Store;
use lanwork_core::todos::TodoCommands;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle, SharedString};

use crate::float_dock::{self, Motion, SavedFloat};
use crate::shelf_dnd::{DragOutcome, DropState, IncomingDrop, Registration, register};
use crate::shelf_refs::{
    InlineDrag, Moved, ProbeResult, RefsView, drag_error, drag_now, parent_folder, prepare_drop,
    push_links, release_grab, schedule_drag, skipped_message, spawn_launch, spawn_open_url,
    spawn_probe,
};
use crate::{ShelfFloat, ShelvesPageData, bar_win};

struct FloatWindow {
    ui: ShelfFloat,
    group_id: String,
    lines: Vec<ShelfLine>,
    todo_ids: Vec<String>,
    links: Vec<LinkedTodo>,
    view: RefsView,
    drop_state: Arc<DropState>,
    registration: Option<Registration>,
    session: float_dock::Session,
    timer: slint::Timer,
    hwnd_ready: bool,
    origin: Option<(i32, i32)>,
    /// 贴边变了，等这次借用结束再写盘。
    pending: Option<SavedFloat>,
    /// 已经隐藏、等着从表里去掉。展开位置和贴边在 `last`。
    closed: bool,
    last: Option<SavedFloat>,
    /// 叠页里这一页藏着，窗口还在。
    parked: bool,
    /// 按下时看得见的矩形，用来判断松手有没有离开。
    drag_anchor: Option<PxBox>,
    /// 按下前的展开位置和贴边。拆页时其余页回到这里。
    drag_before: Option<SavedFloat>,
    /// 这次拖动开始时是收起的把手。
    drag_collapsed: bool,
    stack_pages: i32,
    stack_index: i32,
    /// 位置或贴边变了，等这次借用结束再告诉叠页。
    report: bool,
}

struct Floats {
    shelves: ShelfCommands,
    todos: TodoCommands,
    icons: Arc<Mutex<IconCache>>,
    store: Store,
    dark: bool,
    windows: HashMap<String, FloatWindow>,
    /// 写进分组文件没有成功的展开位置和贴边，本进程里先用这个。
    remembered: HashMap<String, SavedFloat>,
}

thread_local! {
    static FLOATS: RefCell<Option<Floats>> = const { RefCell::new(None) };
}

/// 启动时调用一次。没有收纳服务时不调用，悬浮窗打不开。
pub(crate) fn install(
    shelves: ShelfCommands,
    todos: TodoCommands,
    icons: Arc<Mutex<IconCache>>,
    store: Store,
) {
    bar_win::add_move_hook(on_move);
    FLOATS.with(|cell| {
        *cell.borrow_mut() = Some(Floats {
            shelves,
            todos,
            icons,
            store,
            dark: false,
            windows: HashMap::new(),
            remembered: HashMap::new(),
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

struct Ctx<'a> {
    shelves: &'a ShelfCommands,
    todos: &'a TodoCommands,
    icons: &'a Arc<Mutex<IconCache>>,
    store: &'a Store,
    dark: bool,
}

fn with_window(id: &str, action: impl FnOnce(&mut FloatWindow, &Ctx<'_>) + 'static) {
    let id = id.to_owned();
    with_floats(move |floats| floats.dispatch(&id, action));
}

fn rgb(value: u32) -> slint::Color {
    let [_, r, g, b] = value.to_be_bytes();
    slint::Color::from_rgb_u8(r, g, b)
}

/// 悬浮这个分组。已经有悬浮窗时只聚焦它；失败时返回要显示的错误文字。
pub(crate) fn open(group_id: &str) -> Result<(), String> {
    FLOATS.with(|cell| {
        let mut slot = cell
            .try_borrow_mut()
            .map_err(|_| "悬浮窗正在处理，请再试一次".to_owned())?;
        let floats = slot.as_mut().ok_or_else(|| "收纳服务没有打开".to_owned())?;
        floats.open(group_id)
    })
}

pub(crate) fn apply_theme(dark: bool) {
    crate::float_group::set_dark(dark);
    with_floats(move |floats| {
        floats.dark = dark;
        for window in floats.windows.values_mut() {
            window.ui.set_dark(dark);
            if !window.parked {
                window.refresh_backdrop(dark);
            }
        }
    });
}

/// 窗口还在表里（含叠页里藏着的页）。已经关闭、等着释放的不算。
pub(crate) fn contains(group_id: &str) -> bool {
    FLOATS.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|floats| floats.windows.get(group_id))
            .is_some_and(|window| !window.closed)
    })
}

/// 当前看得见的展开位置、贴边和窗口矩形。藏着或已经关闭时没有。
pub(crate) fn view(group_id: &str) -> Option<(SavedFloat, PxBox)> {
    FLOATS.with(|cell| {
        let floats = cell.borrow();
        let window = floats.as_ref()?.windows.get(group_id)?;
        if window.closed || window.parked {
            return None;
        }
        Some((window.session.saved(), window.session.visible()))
    })
}

/// 按叠页记下的位置显示这一页，并标上页数。
pub(crate) fn present(group_id: &str, saved: SavedFloat, pages: i32, index: i32) {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(window) = slot
            .as_mut()
            .and_then(|floats| floats.windows.get_mut(group_id))
        else {
            return;
        };
        if window.closed {
            return;
        }
        window.present(saved, pages, index);
    });
}

/// 叠页切走这一页：藏起窗口，不释放。
pub(crate) fn park(group_id: &str) {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(window) = slot
            .as_mut()
            .and_then(|floats| floats.windows.get_mut(group_id))
        else {
            return;
        };
        window.park();
    });
}

/// 只改页数和当前下标，不动位置。
pub(crate) fn set_chrome(group_id: &str, pages: i32, index: i32) {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(window) = slot
            .as_mut()
            .and_then(|floats| floats.windows.get_mut(group_id))
        else {
            return;
        };
        window.set_stack_chrome(pages, index);
    });
}

fn notify_opened(group_id: &str) {
    let id = group_id.to_owned();
    slint::Timer::single_shot(Duration::ZERO, move || {
        crate::float_group::page_opened(StackPage::Shelf(id));
    });
}

/// 收纳有变更消息时调用：已经打开的悬浮窗重读自己的分组。
pub(crate) fn on_changed() {
    with_floats(|floats| {
        let ctx = Ctx {
            shelves: &floats.shelves,
            todos: &floats.todos,
            icons: &floats.icons,
            store: &floats.store,
            dark: floats.dark,
        };
        for window in floats.windows.values_mut().filter(|window| !window.closed) {
            window.reload(&ctx, true);
        }
    });
}

/// 待办有变更消息时调用：已经打开的悬浮窗重读自己关联的待办（标题、完成状态）。
pub(crate) fn on_todos_changed() {
    with_floats(|floats| {
        for window in floats.windows.values_mut().filter(|window| !window.closed) {
            window.refresh_links(&floats.todos, &floats.store);
        }
    });
}

/// 退出前记下仍然打开的悬浮窗的位置和大小。写不成只写日志，不拦着退出。
pub(crate) fn save_for_quit() {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(floats) = slot.as_mut() else {
            return;
        };
        let open: Vec<(String, SavedFloat)> = floats
            .windows
            .values()
            .filter(|window| !window.closed)
            .map(|window| (window.group_id.clone(), window.session.saved()))
            .collect();
        for (id, saved) in open {
            floats.remember(&id, saved);
        }
    });
}

impl Floats {
    fn dispatch(&mut self, id: &str, action: impl FnOnce(&mut FloatWindow, &Ctx<'_>)) {
        let pending = {
            let ctx = Ctx {
                shelves: &self.shelves,
                todos: &self.todos,
                icons: &self.icons,
                store: &self.store,
                dark: self.dark,
            };
            if let Some(window) = self.windows.get_mut(id) {
                action(window, &ctx);
                window.pending.take()
            } else {
                None
            }
        };
        if let Some(saved) = pending {
            self.remember(id, saved);
        }
    }

    /// 把已经关闭的窗口从表里去掉，并记下它的展开位置和贴边。窗口随之释放。
    fn reap(&mut self, group_id: &str) {
        let closed = self
            .windows
            .get(group_id)
            .is_some_and(|window| window.closed);
        if !closed {
            return;
        }
        if let Some(window) = self.windows.remove(group_id)
            && let Some(last) = window.last
        {
            self.remember(group_id, last);
        }
    }

    fn remember(&mut self, group_id: &str, saved: SavedFloat) {
        match self.shelves.set_float(group_id, saved.geometry, saved.dock) {
            Ok(_) => {
                self.remembered.remove(group_id);
            }
            Err(err) => {
                self.store
                    .log_warn(&format!("收纳悬浮窗位置没有写进分组：{err}"));
                self.remembered.insert(group_id.to_owned(), saved);
            }
        }
    }

    fn open(&mut self, group_id: &str) -> Result<(), String> {
        if let Some(window) = self.windows.get_mut(group_id) {
            if !window.closed {
                window.parked = false;
                window.raise();
                notify_opened(group_id);
                return Ok(());
            }
            self.reap(group_id);
        }
        let shelf = self.shelves.get(group_id).map_err(|err| err.to_string())?;
        let ui = ShelfFloat::new().map_err(|err| format!("悬浮窗没有创建：{err}"))?;
        ui.set_dark(self.dark);
        ui.set_solid_color(rgb(solid_rgb(self.dark)));
        wire(&ui, group_id);

        let (work, dpi) = bar_win::cursor_monitor().unwrap_or((
            WorkArea {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            lanwork_core::shell::BASE_DPI,
        ));
        let (saved_geometry, saved_dock) = self
            .remembered
            .get(group_id)
            .map(|saved| (Some(saved.geometry), saved.dock))
            .unwrap_or((shelf.float, shelf.float_dock));
        let default_size = (
            u32::try_from(to_physical(FLOAT_WIDTH, dpi)).unwrap_or(FLOAT_WIDTH),
            u32::try_from(to_physical(FLOAT_HEIGHT, dpi)).unwrap_or(FLOAT_HEIGHT),
        );
        let default_origin = float_default_origin(
            work,
            i32::try_from(default_size.0).unwrap_or(i32::MAX),
            i32::try_from(default_size.1).unwrap_or(i32::MAX),
            dpi,
            self.windows.len(),
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

        let id = group_id.to_owned();
        let ready = ui.clone_strong();
        let spawned = slint::spawn_local(async move {
            if let Ok(window) = ready.window().winit_window().await {
                use slint::winit_030::winit::platform::windows::WindowExtWindows as _;
                window.set_skip_taskbar(true);
            }
            with_window(&id, |window, ctx| window.ready(ctx));
        });
        if let Err(err) = spawned {
            self.store
                .log_warn(&format!("收纳悬浮窗初始化未排队：{err}"));
        }
        let mut window = FloatWindow {
            ui,
            group_id: group_id.to_owned(),
            lines: shelf_lines(&shelf),
            todo_ids: shelf.todo_ids,
            links: Vec::new(),
            view: RefsView::new(),
            drop_state: Arc::new(DropState::default()),
            registration: None,
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
        window
            .ui
            .global::<ShelvesPageData>()
            .set_refs(slint::ModelRc::from(window.view.model()));
        window.ui.global::<ShelvesPageData>().set_selected_ref(-1);
        window.push();
        window.refresh_links(&self.todos, &self.store);
        self.windows.insert(group_id.to_owned(), window);
        notify_opened(group_id);
        Ok(())
    }
}

fn wire(ui: &ShelfFloat, group_id: &str) {
    let id = group_id.to_owned();
    ui.on_close_clicked({
        let id = id.clone();
        move || with_window(&id, |window, ctx| window.request_close(ctx))
    });
    ui.window().on_close_requested({
        let id = id.clone();
        move || {
            with_window(&id, |window, ctx| window.request_close(ctx));
            CloseRequestResponse::KeepWindowShown
        }
    });
    ui.on_strip_clicked({
        let id = id.clone();
        move || with_window(&id, |window, _| window.on_strip())
    });
    ui.on_drag_window({
        let id = id.clone();
        move || with_window(&id, |window, _| window.drag())
    });
    ui.on_flip_page({
        let id = id.clone();
        move |dir| {
            let flip = if dir < 0 { Flip::Left } else { Flip::Right };
            crate::float_group::flip(StackPage::Shelf(id.clone()), flip);
        }
    });
    ui.window().on_winit_window_event({
        let id = id.clone();
        move |_, event| {
            if matches!(event, winit::event::WindowEvent::Focused(false)) {
                with_window(&id, |window, _| window.on_blur());
            }
            if let winit::event::WindowEvent::Resized(size) = event {
                let (width, height) = (size.width, size.height);
                with_window(&id, move |window, _| window.on_resized(width, height));
            }
            if crate::float_frame::wants_refresh(event) {
                with_window(&id, |window, _| {
                    if !window.parked {
                        window.refresh_backdrop(window.ui.get_dark());
                    }
                });
            }
            EventResult::Propagate
        }
    });
    let data = ui.global::<ShelvesPageData>();
    data.on_select_ref({
        let id = id.clone();
        move |index| with_window(&id, move |window, _| window.select_ref(index))
    });
    data.on_open_ref({
        let id = id.clone();
        move || with_window(&id, |window, _| window.open_ref())
    });
    data.on_open_ref_folder({
        let id = id.clone();
        move || with_window(&id, |window, _| window.open_ref_folder())
    });
    data.on_ref_press({
        let id = id.clone();
        move |index, x, y| with_window(&id, move |window, _| window.ref_press(index, x, y))
    });
    data.on_ref_move({
        let id = id.clone();
        move |_, x, y| {
            let inline = InlineDrag::new();
            let offered = Rc::clone(&inline);
            with_window(&id, move |window, _| window.ref_move(x, y, &offered));
            if let Some((path, origin)) = inline.take() {
                let id = id.clone();
                drag_now(
                    path,
                    || {},
                    move |outcome| {
                        with_window(&id, move |window, _| window.drag_finished(outcome, origin));
                    },
                );
            }
        }
    });
    data.on_remove_ref({
        let id = id.clone();
        move || with_window(&id, |window, ctx| window.remove_ref(ctx))
    });
    data.on_complete_todo({
        let id = id.clone();
        move |index| with_window(&id, move |window, ctx| window.complete_todo(ctx, index))
    });
    data.on_open_todo({
        let id = id.clone();
        move |index| with_window(&id, move |window, _| window.open_todo(index))
    });
    data.on_ref_release({
        let id = id.clone();
        move || with_window(&id, |window, _| window.view.release())
    });
    data.on_visible_changed(move |first, visible| {
        with_window(&id, move |window, ctx| {
            window.view.set_visible(first, visible);
            window.refresh_visible(ctx, false);
        });
    });
}

/// OLE 放下目标读到路径或网址之后调用。文件夹标记在后台线程里查，网址直接回到界面线程写入。
fn dropped(group_id: &str, incoming: IncomingDrop) {
    match incoming {
        IncomingDrop::Web { url, title } => {
            let id = group_id.to_owned();
            let _ = slint::invoke_from_event_loop(move || {
                with_window(&id, move |window, ctx| {
                    window.add_dropped_web(ctx, &url, &title)
                });
            });
        }
        IncomingDrop::Paths(paths) => dropped_paths(group_id, paths),
    }
}

fn dropped_paths(group_id: &str, paths: Vec<PathBuf>) {
    let id = group_id.to_owned();
    let on_done = id.clone();
    let spawned = prepare_drop(paths, move |incoming, skipped| {
        let _ = slint::invoke_from_event_loop(move || {
            with_window(&on_done, move |window, ctx| {
                window.add_dropped(ctx, &incoming, skipped);
            });
        });
    });
    if let Err(err) = spawned {
        let message = format!("放下的路径没有处理：{err}");
        let _ = slint::invoke_from_event_loop(move || {
            with_window(&id, move |window, _| window.fail(&message));
        });
    }
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
                    .log_warn("收纳悬浮窗样式子类没有装上，显示时可能出现标题栏按钮");
            }
            if !bar_win::watch_move_loop(hwnd) {
                ctx.store
                    .log_warn("收纳悬浮窗拖动结束通知没有装上，松手后可能不贴边");
            }
            if !bar_win::watch_topmost(hwnd) {
                ctx.store
                    .log_warn("收纳悬浮窗的前台通知没有装上，全屏时细条可能被盖住");
            }
            self.drop_state.set_accepting(true);
            self.drop_state.set_zone(DropZone::everywhere());
            let id = self.group_id.clone();
            match register(
                hwnd,
                Arc::clone(&self.drop_state),
                Arc::new(move |incoming| dropped(&id, incoming)),
            ) {
                Ok(registration) => self.registration = Some(registration),
                Err(message) => ctx.store.log_warn(&format!(
                    "收纳悬浮窗拖入没有装上，不能拖入文件或网址：{message}"
                )),
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
        self.reload(ctx, true);
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
        }
        self.report_move();
    }

    fn report_move(&mut self) {
        if !self.report {
            return;
        }
        self.report = false;
        let id = self.group_id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            crate::float_group::page_moved(StackPage::Shelf(id));
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
        // 收起的把手不许被一键回到桌面藏起来。这一页要藏，先卸掉那条登记。
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
        let id = self.group_id.clone();
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
        let page = StackPage::Shelf(self.group_id.clone());
        if crate::float_group::companion_blocks_blur(&page) {
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
        let page = StackPage::Shelf(self.group_id.clone());
        let effect = crate::float_group::classify_title(page.clone(), anchor, rect, from_collapsed);
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
                crate::float_group::queue_detach(page, before, detached);
                self.hold_blur_after_drag(generation);
            }
            DropEffect::MergeUnit { target } => {
                self.session.abandon_drag();
                let generation = self.session.hold_generation();
                crate::float_group::queue_merge_unit(page, target);
                self.hold_blur_after_drag(generation);
            }
        }
    }

    fn hold_blur_after_drag(&mut self, generation: u8) {
        let Some(key) = self.hwnd_key() else {
            return;
        };
        let id = self.group_id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            slint::Timer::single_shot(Duration::ZERO, move || {
                bar_win::clear_blur_hold(key);
                with_window(&id, move |window, _| {
                    window.session.release_hold(generation)
                });
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

    /// 显示在最前。位置和大小不动。
    fn raise(&self) {
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
    }

    fn push(&self) {
        self.view
            .push(&self.lines, &self.ui.global::<ShelvesPageData>());
    }

    fn fail(&self, message: &str) {
        self.ui.set_error(message.into());
    }

    fn clear_error(&self) {
        self.ui.set_error(SharedString::new());
    }

    /// 重读这个分组。分组已经不在时窗口留着，条目为空；这时怎么处理，产品规格没有写。
    fn reload(&mut self, ctx: &Ctx<'_>, force: bool) {
        match ctx.shelves.get(&self.group_id) {
            Ok(shelf) => {
                self.lines = shelf_lines(&shelf);
                self.todo_ids = shelf.todo_ids;
                self.clear_error();
            }
            Err(ShelfError::NotFound { .. }) => {
                self.lines = Vec::new();
                self.todo_ids = Vec::new();
                self.view.clear();
            }
            Err(err) => {
                self.fail(&err.to_string());
                return;
            }
        }
        self.view.keep_selected(&self.lines);
        self.push();
        self.refresh_links(ctx.todos, ctx.store);
        self.refresh_visible(ctx, force);
    }

    /// 按待办现在的状态重排「关联待办」列表。读不到待办时列表按「找不到」显示，并记日志。
    fn refresh_links(&mut self, todos: &TodoCommands, store: &Store) {
        let lists = todos.lists().unwrap_or_else(|err| {
            store.log_warn(&format!("收纳悬浮窗读不到待办：{err}"));
            Vec::new()
        });
        self.links = linked_todos(&self.todo_ids, &lists);
        push_links(&self.links, &self.ui.global::<ShelvesPageData>());
    }

    /// 勾选完成一条关联待办，走待办现有的完成逻辑（含重复待办）。已完成或在回收站的不处理：
    /// 悬浮窗里能不能取消完成，产品规格没有写。
    fn complete_todo(&mut self, ctx: &Ctx<'_>, index: i32) {
        let Some(link) = usize::try_from(index)
            .ok()
            .and_then(|index| self.links.get(index))
        else {
            return;
        };
        if !link.found || link.completed || link.in_trash {
            return;
        }
        match ctx.todos.complete_item(&link.id) {
            Ok(()) => {
                self.clear_error();
                self.refresh_links(ctx.todos, ctx.store);
            }
            Err(err) => self.fail(&err.to_string()),
        }
    }

    /// 点标题：打开面板，切到待办并定位这一条。
    fn open_todo(&self, index: i32) {
        let Some(link) = usize::try_from(index)
            .ok()
            .and_then(|index| self.links.get(index))
        else {
            return;
        };
        if link.found {
            crate::panel::open_target(PanelTarget::Todo {
                item_id: link.id.clone(),
            });
        }
    }

    fn refresh_visible(&mut self, ctx: &Ctx<'_>, force: bool) {
        let data = self.ui.global::<ShelvesPageData>();
        let Some(plan) = self.view.refresh(&self.lines, force, &data) else {
            return;
        };
        let id = self.group_id.clone();
        let spawned = spawn_probe(
            ctx.shelves.clone(),
            Arc::clone(ctx.icons),
            plan,
            move |result| {
                let _ = slint::invoke_from_event_loop(move || {
                    with_window(&id, move |window, _| window.receive(result));
                });
            },
        );
        if let Err(err) = spawned {
            ctx.store
                .log_warn(&format!("收纳悬浮窗没有启动路径检查线程：{err}"));
        }
    }

    fn receive(&mut self, result: ProbeResult) {
        if self.closed {
            return;
        }
        self.view.receive(result);
        self.push();
    }

    fn select_ref(&mut self, index: i32) {
        self.view.select(&self.lines, index);
        self.push();
    }

    /// Enter、双击或「打开」：路径用 Windows Shell 打开，网页引用用默认浏览器打开保存的网址。
    fn open_ref(&mut self) {
        let Some(key) = self.view.selected().map(str::to_owned) else {
            return;
        };
        if self.view.selected_is_web() {
            self.clear_error();
            self.spawn_open_url(key);
            return;
        }
        if self.view.is_missing(&key) {
            self.fail("路径不存在");
            return;
        }
        self.clear_error();
        self.spawn_open(key);
    }

    /// Ctrl+Enter：打开路径的上一级。网页引用没有所在目录，这次按键不做任何事。
    fn open_ref_folder(&mut self) {
        if self.view.selected_is_web() {
            return;
        }
        let Some(path) = self.view.selected() else {
            return;
        };
        let Some(parent) = parent_folder(path) else {
            self.fail("没有所在目录");
            return;
        };
        self.clear_error();
        self.spawn_open(parent);
    }

    fn spawn_open(&self, path: String) {
        let id = self.group_id.clone();
        let spawned = spawn_launch(path, move |message| {
            let _ = slint::invoke_from_event_loop(move || {
                with_window(&id, move |window, _| window.fail(&message));
            });
        });
        if let Err(err) = spawned {
            self.fail(&format!("打开失败：{err}"));
        }
    }

    fn spawn_open_url(&self, url: String) {
        let id = self.group_id.clone();
        let spawned = spawn_open_url(url, move |message| {
            let _ = slint::invoke_from_event_loop(move || {
                with_window(&id, move |window, _| window.fail(&message));
            });
        });
        if let Err(err) = spawned {
            self.fail(&format!("打开失败：{err}"));
        }
    }

    /// 「移除引用」：只从这个分组删掉选中的那一条。路径不删原文件，网址不关闭标签。
    fn remove_ref(&mut self, ctx: &Ctx<'_>) {
        if self.closed {
            return;
        }
        let Some(key) = self.view.selected().map(str::to_owned) else {
            return;
        };
        let removed = if self.view.selected_is_web() {
            ctx.shelves.remove_web_ref(&self.group_id, &key)
        } else {
            ctx.shelves.remove_ref(&self.group_id, &key)
        };
        match removed {
            Ok(_) => {
                self.view.deselect();
                self.reload(ctx, true);
            }
            Err(err) => self.fail(&err.to_string()),
        }
    }

    fn ref_press(&mut self, index: i32, x: f32, y: f32) {
        self.view.press(&self.lines, index, x, y);
    }

    /// 移动超过系统拖动阈值就拖出按下的那一条。同步执行时把路径交给 `inline`，由回调在借用结束后
    /// 开始拖出；动作被推迟时退回下一轮事件循环。
    fn ref_move(&mut self, x: f32, y: f32, inline: &InlineDrag) {
        match self.view.moved(self.ui.window().scale_factor(), x, y) {
            Moved::Nothing => {}
            Moved::Missing => self.fail("路径不存在"),
            Moved::Start { path, origin } => {
                self.clear_error();
                if !inline.offer(path.clone(), origin) {
                    let id = self.group_id.clone();
                    schedule_drag(
                        path,
                        || {},
                        move |outcome| {
                            with_window(&id, move |window, _| {
                                window.drag_finished(outcome, origin)
                            });
                        },
                    );
                }
            }
        }
    }

    fn drag_finished(&mut self, outcome: Result<DragOutcome, String>, origin: (f32, f32)) {
        self.view.release();
        release_grab(self.ui.window(), origin);
        if let Some(message) = drag_error(outcome) {
            self.fail(&message);
        }
    }

    /// 放下的路径加入这个窗口对应的分组。
    fn add_dropped(&mut self, ctx: &Ctx<'_>, incoming: &[IncomingRef], skipped: usize) {
        if self.closed {
            return;
        }
        if !incoming.is_empty() {
            match ctx.shelves.add_refs(Some(&self.group_id), incoming) {
                Ok(_) => self.reload(ctx, true),
                Err(err) => {
                    self.fail(&err.to_string());
                    return;
                }
            }
        }
        if let Some(message) = skipped_message(skipped) {
            self.fail(&message);
        }
    }

    fn add_dropped_web(&mut self, ctx: &Ctx<'_>, url: &str, title: &str) {
        if self.closed {
            return;
        }
        match ctx.shelves.add_web_refs(
            Some(&self.group_id),
            &[IncomingWeb {
                url: url.to_owned(),
                title: title.to_owned(),
            }],
        ) {
            Ok(_) => self.reload(ctx, true),
            Err(err) => self.fail(&err.to_string()),
        }
    }

    fn request_close(&mut self, _: &Ctx<'_>) {
        if self.closed {
            return;
        }
        self.finish_close();
    }

    /// 撤销放下目标，记下展开位置和贴边，隐藏窗口，然后在下一轮把它从表里去掉，释放绘图资源。
    /// 分组和条目留在收纳里；这里不删除分组。细条的矩形不写入文件。
    fn finish_close(&mut self) {
        let id = self.group_id.clone();
        self.timer.stop();
        self.drop_state.set_accepting(false);
        self.registration = None;
        if let Some(hwnd) = self.hwnd() {
            bar_win::unwatch_topmost(hwnd);
        }
        self.last = Some(self.session.saved());
        self.closed = true;
        let _ = self.ui.hide();
        let closed = id.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            with_floats(move |floats| floats.reap(&id));
            crate::float_group::page_closed(StackPage::Shelf(closed));
        });
    }
}

fn apply_limits(ui: &ShelfFloat, session: &float_dock::Session) {
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
        let id = floats
            .windows
            .iter()
            .find_map(|(id, window)| (window.hwnd_key() == Some(key)).then(|| id.clone()));
        if let Some(id) = id {
            floats.dispatch(&id, move |window, _| {
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
