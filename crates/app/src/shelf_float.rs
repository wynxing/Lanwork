//! 收纳分组的悬浮窗：只显示该分组条目的独立窗口，使用时创建，关闭时释放。
//!
//! 同一分组已经有悬浮窗时只聚焦它。每个分组记住自己的位置和大小：关闭和退出时写进分组文件的
//! `floatWindow`，再悬浮时放回；写不成时，本进程里还记着这个值。屏幕变了、位置不在任何工作区内时
//! 拉回工作区，做法与便签悬浮窗相同。窗口失焦不关闭；面板自己的失焦收起不受影响。
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
    DropZone, FLOAT_HEIGHT, FLOAT_MIN_HEIGHT, FLOAT_MIN_WIDTH, FLOAT_WIDTH, LinkedTodo,
    PanelTarget, float_default_origin, linked_todos, restore_float,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::shelves::{FloatGeometry, IncomingRef, ShelfCommands, ShelfError, ShelfRef};
use lanwork_core::storage::Store;
use lanwork_core::todos::TodoCommands;
use slint::winit_030::WinitWindowAccessor;
use slint::{CloseRequestResponse, ComponentHandle, SharedString};

use crate::shelf_dnd::{DragOutcome, DropState, Registration, register};
use crate::shelf_refs::{
    InlineDrag, Moved, ProbeResult, RefsView, drag_error, drag_now, parent_folder, prepare_drop,
    push_links, release_grab, schedule_drag, skipped_message, spawn_launch, spawn_probe,
};
use crate::{ShelfFloat, ShelvesPageData, bar_win};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Geometry {
    origin: (i32, i32),
    size: (u32, u32),
}

struct FloatWindow {
    ui: ShelfFloat,
    group_id: String,
    refs: Vec<ShelfRef>,
    todo_ids: Vec<String>,
    links: Vec<LinkedTodo>,
    view: RefsView,
    drop_state: Arc<DropState>,
    registration: Option<Registration>,
    hwnd_ready: bool,
    origin: Option<(i32, i32)>,
    /// 已经隐藏、等着从表里去掉。位置和大小在 `last`。
    closed: bool,
    last: Option<Geometry>,
}

struct Floats {
    shelves: ShelfCommands,
    todos: TodoCommands,
    icons: Arc<Mutex<IconCache>>,
    store: Store,
    dark: bool,
    windows: HashMap<String, FloatWindow>,
    /// 写进分组文件没有成功的位置和大小，本进程里先用这个。
    remembered: HashMap<String, Geometry>,
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
    with_floats(move |floats| {
        let ctx = Ctx {
            shelves: &floats.shelves,
            todos: &floats.todos,
            icons: &floats.icons,
            store: &floats.store,
            dark: floats.dark,
        };
        if let Some(window) = floats.windows.get_mut(&id) {
            action(window, &ctx);
        }
    });
}

fn to_float(geometry: Geometry) -> FloatGeometry {
    FloatGeometry {
        x: geometry.origin.0,
        y: geometry.origin.1,
        width: geometry.size.0,
        height: geometry.size.1,
    }
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
    with_floats(move |floats| {
        floats.dark = dark;
        for window in floats.windows.values_mut() {
            window.ui.set_dark(dark);
            window.refresh_backdrop(dark);
        }
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
        let open: Vec<(String, Geometry)> = floats
            .windows
            .values()
            .filter(|window| !window.closed)
            .filter_map(|window| Some((window.group_id.clone(), window.geometry()?)))
            .collect();
        for (id, geometry) in open {
            floats.remember(&id, geometry);
        }
    });
}

impl Floats {
    /// 把已经关闭的窗口从表里去掉，并记下它的位置和大小。窗口随之释放。
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

    fn remember(&mut self, group_id: &str, geometry: Geometry) {
        match self.shelves.set_float(group_id, to_float(geometry)) {
            Ok(_) => {
                self.remembered.remove(group_id);
            }
            Err(err) => {
                self.store
                    .log_warn(&format!("收纳悬浮窗位置没有写进分组：{err}"));
                self.remembered.insert(group_id.to_owned(), geometry);
            }
        }
    }

    fn open(&mut self, group_id: &str) -> Result<(), String> {
        if let Some(window) = self.windows.get_mut(group_id) {
            if !window.closed {
                window.raise();
                return Ok(());
            }
            self.reap(group_id);
        }
        let shelf = self.shelves.get(group_id).map_err(|err| err.to_string())?;
        let ui = ShelfFloat::new().map_err(|err| format!("悬浮窗没有创建：{err}"))?;
        ui.set_pinned(shelf.float_pinned);
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
        let saved = self.remembered.get(group_id).copied().or_else(|| {
            shelf.float.map(|float| Geometry {
                origin: (float.x, float.y),
                size: (float.width, float.height),
            })
        });
        let min = (
            u32::try_from(to_physical(FLOAT_MIN_WIDTH, dpi)).unwrap_or(FLOAT_MIN_WIDTH),
            u32::try_from(to_physical(FLOAT_MIN_HEIGHT, dpi)).unwrap_or(FLOAT_MIN_HEIGHT),
        );
        let geometry = saved.map_or_else(
            || {
                let size = (
                    u32::try_from(to_physical(FLOAT_WIDTH, dpi)).unwrap_or(FLOAT_WIDTH),
                    u32::try_from(to_physical(FLOAT_HEIGHT, dpi)).unwrap_or(FLOAT_HEIGHT),
                );
                let origin = float_default_origin(
                    work,
                    i32::try_from(size.0).unwrap_or(i32::MAX),
                    i32::try_from(size.1).unwrap_or(i32::MAX),
                    dpi,
                    self.windows.len(),
                );
                Geometry { origin, size }
            },
            |saved| {
                let placed = restore_float(to_float(saved), &bar_win::work_areas(), work, min);
                Geometry {
                    origin: placed.origin,
                    size: placed.size,
                }
            },
        );
        ui.window()
            .set_size(slint::PhysicalSize::new(geometry.size.0, geometry.size.1));
        ui.window().set_position(slint::PhysicalPosition::new(
            geometry.origin.0,
            geometry.origin.1,
        ));
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
            refs: shelf.refs,
            todo_ids: shelf.todo_ids,
            links: Vec::new(),
            view: RefsView::new(),
            drop_state: Arc::new(DropState::default()),
            registration: None,
            hwnd_ready: false,
            origin: Some(geometry.origin),
            closed: false,
            last: None,
        };
        window
            .ui
            .global::<ShelvesPageData>()
            .set_refs(slint::ModelRc::from(window.view.model()));
        window.ui.global::<ShelvesPageData>().set_selected_ref(-1);
        window.push();
        window.refresh_links(&self.todos, &self.store);
        self.windows.insert(group_id.to_owned(), window);
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
    ui.on_pin_clicked({
        let id = id.clone();
        move || with_window(&id, |window, ctx| window.toggle_pin(ctx))
    });
    ui.on_drag_window({
        let id = id.clone();
        move || {
            with_window(&id, |window, _| {
                let _ = window.ui.window().with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            });
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

/// OLE 放下目标读到路径之后调用。文件夹标记在后台线程里查，结果回到界面线程再写入服务。
fn dropped(group_id: &str, paths: Vec<PathBuf>) {
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
            if let Some(origin) = self.origin
                && bar_win::window_origin(hwnd) != Some(origin)
            {
                bar_win::move_window(hwnd, origin.0, origin.1);
            }
            self.drop_state.set_accepting(true);
            self.drop_state.set_zone(DropZone::everywhere());
            let id = self.group_id.clone();
            match register(
                hwnd,
                Arc::clone(&self.drop_state),
                Arc::new(move |paths| dropped(&id, paths)),
            ) {
                Ok(registration) => self.registration = Some(registration),
                Err(message) => ctx
                    .store
                    .log_warn(&format!("收纳悬浮窗拖入没有装上，不能拖入文件：{message}")),
            }
        }
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

    fn refresh_backdrop(&mut self, dark: bool) {
        let actual = match self.hwnd() {
            Some(hwnd) => bar_win::apply_dwm(hwnd, bar_win::current_backdrop(), dark),
            None => Backdrop::Solid,
        };
        self.ui.set_acrylic(actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(dark)));
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
            .push(&self.refs, &self.ui.global::<ShelvesPageData>());
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
                self.refs = shelf.refs;
                self.todo_ids = shelf.todo_ids;
                self.clear_error();
            }
            Err(ShelfError::NotFound { .. }) => {
                self.refs = Vec::new();
                self.todo_ids = Vec::new();
                self.view.clear();
            }
            Err(err) => {
                self.fail(&err.to_string());
                return;
            }
        }
        self.view.keep_selected(&self.refs);
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
        let Some(plan) = self.view.refresh(&self.refs, force, &data) else {
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
        self.view.select(&self.refs, index);
        self.push();
    }

    /// Enter 或双击：用 Windows Shell 打开。已知不存在的路径直接显示错误。
    fn open_ref(&mut self) {
        let Some(path) = self.view.selected().map(str::to_owned) else {
            return;
        };
        if self.view.is_missing(&path) {
            self.fail("路径不存在");
            return;
        }
        self.clear_error();
        self.spawn_open(path);
    }

    /// Ctrl+Enter：打开所在目录，即路径的上一级。
    fn open_ref_folder(&mut self) {
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

    /// 「移除引用」：只从这个分组删掉选中的那一条，不动原文件。
    fn remove_ref(&mut self, ctx: &Ctx<'_>) {
        if self.closed {
            return;
        }
        let Some(path) = self.view.selected().map(str::to_owned) else {
            return;
        };
        match ctx.shelves.remove_ref(&self.group_id, &path) {
            Ok(_) => {
                self.view.deselect();
                self.reload(ctx, true);
            }
            Err(err) => self.fail(&err.to_string()),
        }
    }

    fn ref_press(&mut self, index: i32, x: f32, y: f32) {
        self.view.press(&self.refs, index, x, y);
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

    /// 点「钉住」：先写进分组文件，成功后窗口才改层级，所以按钮的外观总是和已保存的状态一致。
    /// 写不成时状态不变，错误显示在窗口里。
    fn toggle_pin(&mut self, ctx: &Ctx<'_>) {
        if self.closed {
            return;
        }
        let next = !self.ui.get_pinned();
        match ctx.shelves.set_float_pinned(&self.group_id, next) {
            Ok(_) => {
                self.ui.set_pinned(next);
                self.clear_error();
            }
            Err(err) => {
                ctx.store
                    .log_warn(&format!("收纳悬浮窗的钉住状态没有写进分组：{err}"));
                self.fail(&err.to_string());
            }
        }
    }

    fn request_close(&mut self, _: &Ctx<'_>) {
        if self.closed {
            return;
        }
        self.finish_close();
    }

    fn geometry(&self) -> Option<Geometry> {
        let origin = self.hwnd().and_then(bar_win::window_origin).or(self.origin);
        let size = self.ui.window().size();
        origin.map(|origin| Geometry {
            origin,
            size: (size.width, size.height),
        })
    }

    /// 撤销放下目标，记下位置和大小，隐藏窗口，然后在下一轮把它从表里去掉，释放绘图资源。
    /// 分组和条目留在收纳里；这里不删除分组。
    fn finish_close(&mut self) {
        let id = self.group_id.clone();
        self.drop_state.set_accepting(false);
        self.registration = None;
        self.last = self.geometry();
        self.closed = true;
        let _ = self.ui.hide();
        slint::Timer::single_shot(Duration::ZERO, move || {
            with_floats(move |floats| floats.reap(&id));
        });
    }
}
