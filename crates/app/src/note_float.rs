//! 便签悬浮窗：只含正文的独立窗口，使用时创建，关闭时释放。
//!
//! 同一篇已经有悬浮窗时只聚焦它，不把已经收起的细条展开。窗口始终在最上层。
//! 每篇记住自己的展开位置、大小和贴边：写进便签文件的 `floatWindow` 和 `floatDock`。
//! 贴边对得上当前屏幕时按细条或展开的位置打开；对不上时这一次丢掉贴边，拉回工作区。
//! 写不成时，本进程里还记着这个值。保存流程在 `note_editor`，贴边几何在 `lanwork_core::panel`。
//! 版式在 `ui/notefloat.slint`。

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use lanwork_core::notes::NoteCommands;
use lanwork_core::panel::{
    FLOAT_HEIGHT, FLOAT_MIN_HEIGHT, FLOAT_MIN_WIDTH, FLOAT_WIDTH, PxBox, float_default_origin,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::storage::Store;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle};

use crate::NoteFloat;
use crate::bar_win;
use crate::float_dock::{self, Motion, SavedFloat};
use crate::note_editor::Editor;

enum After {
    Close,
    Quit,
}

struct FloatWindow {
    ui: NoteFloat,
    editor: Editor<After>,
    session: float_dock::Session,
    timer: slint::Timer,
    hwnd_ready: bool,
    origin: Option<(i32, i32)>,
    /// 贴边变了，等这次借用结束再写盘。
    pending: Option<SavedFloat>,
    /// 已经隐藏、等着从表里去掉。展开位置和贴边在 `last`。
    closed: bool,
    last: Option<SavedFloat>,
}

struct Floats {
    notes: NoteCommands,
    store: Store,
    dark: bool,
    windows: HashMap<String, FloatWindow>,
    /// 写进便签文件没有成功的展开位置和贴边，本进程里先用这个。
    remembered: HashMap<String, SavedFloat>,
}

thread_local! {
    static FLOATS: RefCell<Option<Floats>> = const { RefCell::new(None) };
}

/// 启动时调用一次。没有便签服务时不调用，悬浮窗打不开。
pub(crate) fn install(notes: NoteCommands, store: Store) {
    bar_win::add_move_hook(on_move);
    FLOATS.with(|cell| {
        *cell.borrow_mut() = Some(Floats {
            notes,
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

fn with_window(id: &str, action: impl FnOnce(&mut FloatWindow, &Ctx<'_>) + 'static) {
    let id = id.to_owned();
    with_floats(move |floats| floats.dispatch(&id, action));
}

struct Ctx<'a> {
    notes: &'a NoteCommands,
    store: &'a Store,
    dark: bool,
}

fn persist_saved(notes: &NoteCommands, note_id: &str, saved: SavedFloat) -> Result<(), String> {
    notes
        .set_float(note_id, saved.geometry, saved.dock)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn rgb(value: u32) -> slint::Color {
    let [_, r, g, b] = value.to_be_bytes();
    slint::Color::from_rgb_u8(r, g, b)
}

/// 悬浮这一篇。已经有悬浮窗时只聚焦它；失败时返回要显示的错误文字，调用方可以只重试这一步。
pub(crate) fn open(note_id: &str) -> Result<(), String> {
    FLOATS.with(|cell| {
        let mut slot = cell
            .try_borrow_mut()
            .map_err(|_| "悬浮窗正在处理，请再试一次".to_owned())?;
        let floats = slot.as_mut().ok_or_else(|| "便签服务没有打开".to_owned())?;
        floats.open(note_id)
    })
}

/// 快速收集已经保存了便签、只差悬浮时用。只重试悬浮，不创建第二篇；仍失败时返回规格里的提示。
/// 搜索条的收集提交还没有接上，接上之后调用这里。
#[expect(dead_code, reason = "快速收集的提交还没有接入搜索条")]
pub(crate) fn open_saved(note_id: &str) -> Result<(), String> {
    open(note_id)
        .or_else(|_| open(note_id))
        .map_err(|_| "便签已保存，悬浮失败".to_owned())
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

/// 别处保存了这一篇之后调用。这个悬浮窗没有未保存的修改、输入法也没在组合时，换成最新正文。
pub(crate) fn on_note_changed(note_id: &str) {
    with_window(note_id, FloatWindow::adopt_latest);
}

/// 退出前保存所有悬浮窗里没写盘的修改。返回保存之后仍没写盘的窗口数，这些窗口会被显示出来。
pub(crate) fn flush_for_quit() -> usize {
    FLOATS.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return 1;
        };
        let Some(floats) = slot.as_mut() else {
            return 0;
        };
        let notes = floats.notes.clone();
        let mut blocked = 0;
        for window in floats.windows.values_mut() {
            if window.editor.flush(&notes) {
                let saved = window.session.saved();
                let id = window.editor.session.id().to_owned();
                if let Err(message) = persist_saved(&notes, &id, saved) {
                    floats
                        .store
                        .log_warn(&format!("悬浮窗位置没有写进便签：{message}"));
                }
                continue;
            }
            window.editor.after = Some(After::Quit);
            window.push_problem();
            window.raise();
            blocked += 1;
        }
        blocked
    })
}

impl Floats {
    fn dispatch(&mut self, id: &str, action: impl FnOnce(&mut FloatWindow, &Ctx<'_>)) {
        let pending = {
            let ctx = Ctx {
                notes: &self.notes,
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
    fn reap(&mut self, note_id: &str) {
        let closed = self
            .windows
            .get(note_id)
            .is_some_and(|window| window.closed);
        if !closed {
            return;
        }
        if let Some(window) = self.windows.remove(note_id)
            && let Some(last) = window.last
        {
            self.remember(note_id, last);
        }
    }

    fn remember(&mut self, note_id: &str, saved: SavedFloat) {
        match persist_saved(&self.notes, note_id, saved) {
            Ok(()) => {
                self.remembered.remove(note_id);
            }
            Err(message) => {
                self.store
                    .log_warn(&format!("悬浮窗位置没有写进便签：{message}"));
                self.remembered.insert(note_id.to_owned(), saved);
            }
        }
    }

    fn open(&mut self, note_id: &str) -> Result<(), String> {
        if let Some(window) = self.windows.get_mut(note_id) {
            if !window.closed {
                window.raise();
                return Ok(());
            }
            self.reap(note_id);
        }
        let note = self.notes.get(note_id).map_err(|err| err.to_string())?;
        if note.is_deleted() {
            return Err("便签在回收站".to_owned());
        }
        let ui = NoteFloat::new().map_err(|err| format!("悬浮窗没有创建：{err}"))?;
        ui.set_body(note.body.as_str().into());
        ui.set_dark(self.dark);
        ui.set_solid_color(rgb(solid_rgb(self.dark)));
        wire(&ui, note_id);

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
            .get(note_id)
            .map(|saved| (Some(saved.geometry), saved.dock))
            .unwrap_or((note.float, note.float_dock));
        let min = (
            u32::try_from(to_physical(FLOAT_MIN_WIDTH, dpi)).unwrap_or(FLOAT_MIN_WIDTH),
            u32::try_from(to_physical(FLOAT_MIN_HEIGHT, dpi)).unwrap_or(FLOAT_MIN_HEIGHT),
        );
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
            &bar_win::work_areas(),
            work,
            min,
        );
        let chrome = session.chrome();
        ui.set_thin(chrome.thin);
        ui.set_collapsed(chrome.collapsed);
        ui.set_dock_edge(chrome.edge.into());
        let visible = session.visible();
        ui.window()
            .set_size(slint::PhysicalSize::new(visible.width, visible.height));
        ui.window()
            .set_position(slint::PhysicalPosition::new(visible.x, visible.y));
        ui.show().map_err(|err| format!("悬浮窗没有显示：{err}"))?;

        let id = note_id.to_owned();
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
                .log_warn(&format!("便签悬浮窗初始化未排队：{err}"));
        }
        self.windows.insert(
            note_id.to_owned(),
            FloatWindow {
                ui,
                editor: Editor::new(&note),
                session,
                timer: slint::Timer::default(),
                hwnd_ready: false,
                origin: Some((visible.x, visible.y)),
                pending: None,
                closed: false,
                last: None,
            },
        );
        Ok(())
    }
}

fn wire(ui: &NoteFloat, note_id: &str) {
    let id = note_id.to_owned();
    ui.on_body_edited({
        let id = id.clone();
        move |text| {
            let text = text.to_string();
            with_window(&id, move |window, _| {
                let id = window.editor.session.id().to_owned();
                window
                    .editor
                    .edit(|session, now| session.set_body(&text, now), tick(&id));
            });
        }
    });
    ui.on_close_clicked({
        let id = id.clone();
        move || with_window(&id, FloatWindow::request_close)
    });
    ui.window().on_close_requested({
        let id = id.clone();
        move || {
            with_window(&id, FloatWindow::request_close);
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
    ui.window().on_winit_window_event({
        let id = id.clone();
        move |_, event| {
            if matches!(event, winit::event::WindowEvent::Focused(false)) {
                with_window(&id, |window, _| window.on_blur());
            }
            EventResult::Propagate
        }
    });
    ui.on_retry({
        let id = id.clone();
        move || with_window(&id, FloatWindow::retry)
    });
    ui.on_discard({
        let id = id.clone();
        move || with_window(&id, FloatWindow::discard)
    });
    ui.on_keep_mine({
        let id = id.clone();
        move || with_window(&id, FloatWindow::keep_mine)
    });
    ui.on_keep_saved(move || with_window(&id, FloatWindow::keep_saved));
}

fn tick(id: &str) -> impl Fn() + 'static {
    let id = id.to_owned();
    move || with_window(&id, FloatWindow::tick)
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
                    .log_warn("便签悬浮窗拖动结束通知没有装上，松手后可能不贴边");
            }
            if !bar_win::watch_topmost(hwnd) {
                ctx.store
                    .log_warn("便签悬浮窗的前台通知没有装上，全屏时细条可能被盖住");
            }
            let visible = self.session.visible();
            float_dock::place(self.ui.window(), Some(hwnd), visible);
            self.origin = Some((visible.x, visible.y));
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
        self.refresh_backdrop(self.ui.get_dark());
    }

    fn place(&mut self, rect: PxBox) {
        float_dock::place(self.ui.window(), self.hwnd(), rect);
        self.origin = Some((rect.x, rect.y));
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
                self.ui.invoke_focus_body();
            }
        }
    }

    fn start_anim(&mut self) {
        if self.timer.running() {
            return;
        }
        let id = self.editor.session.id().to_owned();
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
        if self.closed {
            return;
        }
        let motion = self.session.strip_click();
        self.follow(motion);
    }

    fn on_blur(&mut self) {
        if self.closed {
            return;
        }
        if self.hwnd_key().is_some_and(bar_win::size_move_blocks_blur) {
            return;
        }
        if let Some(motion) = self.session.blur() {
            self.follow(motion);
        }
    }

    fn on_size_move(&mut self, entering: bool, rect: PxBox) {
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
        self.follow(motion);
        let Some(key) = self.hwnd_key() else {
            return;
        };
        let id = self.editor.session.id().to_owned();
        slint::Timer::single_shot(Duration::ZERO, move || {
            slint::Timer::single_shot(Duration::ZERO, move || {
                bar_win::clear_blur_hold(key);
                with_window(&id, move |window, _| {
                    window.session.release_hold(generation)
                });
            });
        });
    }

    fn adopt_latest(&mut self, ctx: &Ctx<'_>) {
        if self.closed || !self.ui.get_preedit().is_empty() {
            return;
        }
        let Ok(latest) = ctx.notes.get(self.editor.session.id()) else {
            return;
        };
        if self.editor.session.adopt_if_clean(&latest) {
            self.ui.set_body(latest.body.as_str().into());
        }
    }

    fn refresh_backdrop(&mut self, dark: bool) {
        let collapsed = self.ui.get_collapsed();
        let actual = match self.hwnd() {
            Some(hwnd) if collapsed => {
                bar_win::apply_collapsed_frame(hwnd, dark);
                Backdrop::Solid
            }
            Some(hwnd) => bar_win::apply_dwm(hwnd, bar_win::current_backdrop(), dark),
            None => Backdrop::Solid,
        };
        self.ui
            .set_acrylic(!collapsed && actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(dark)));
    }

    /// 显示在最前。已经收起时不展开。展开时把焦点给正文。位置和大小不动。
    fn raise(&self) {
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
        if !self.session.chrome().collapsed {
            self.ui.invoke_focus_body();
        }
    }

    fn push_problem(&self) {
        let view = self.editor.problem_view();
        self.ui.set_problem(view.kind);
        self.ui.set_problem_text(view.text);
        self.ui.set_closing(view.closing);
    }

    fn tick(&mut self, ctx: &Ctx<'_>) {
        let id = self.editor.session.id().to_owned();
        let preedit_empty = self.ui.get_preedit().is_empty();
        self.editor.on_tick(preedit_empty, ctx.notes, tick(&id));
        self.push_problem();
    }

    fn request_close(&mut self, ctx: &Ctx<'_>) {
        if self.closed {
            return;
        }
        if self.editor.flush(ctx.notes) {
            self.editor.after = None;
            self.push_problem();
            self.finish_close();
        } else {
            self.editor.after = Some(After::Close);
            self.push_problem();
        }
    }

    fn proceed(&mut self) {
        let after = self.editor.after.take();
        self.push_problem();
        match after {
            Some(After::Close) => self.finish_close(),
            Some(After::Quit) => crate::host::continue_quit(),
            None => {}
        }
    }

    fn retry(&mut self, ctx: &Ctx<'_>) {
        if self.editor.flush(ctx.notes) {
            self.proceed();
        } else {
            self.push_problem();
        }
    }

    fn discard(&mut self, _: &Ctx<'_>) {
        self.editor.session.discard();
        self.ui
            .set_body(self.editor.session.draft().body.as_str().into());
        self.proceed();
    }

    fn keep_mine(&mut self, ctx: &Ctx<'_>) {
        self.editor.session.keep_mine(ctx.notes);
        if self.editor.resolved() {
            self.proceed();
        } else {
            self.push_problem();
        }
    }

    fn keep_saved(&mut self, ctx: &Ctx<'_>) {
        match self.editor.session.keep_saved(ctx.notes) {
            Some(latest) => {
                self.ui.set_body(latest.body.as_str().into());
                self.proceed();
            }
            None => self.push_problem(),
        }
    }

    /// 记下展开位置和贴边，隐藏窗口，然后在下一轮把它从表里去掉，释放文本和绘图资源。
    /// 正文留在便签列表里；这里不删除便签。细条的矩形不写入文件。
    fn finish_close(&mut self) {
        let id = self.editor.session.id().to_owned();
        self.timer.stop();
        if let Some(hwnd) = self.hwnd() {
            bar_win::unwatch_topmost(hwnd);
        }
        self.last = Some(self.session.saved());
        self.closed = true;
        let _ = self.ui.hide();
        slint::Timer::single_shot(Duration::ZERO, move || {
            with_floats(move |floats| floats.reap(&id));
        });
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
