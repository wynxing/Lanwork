//! 便签悬浮窗：只含正文的独立窗口，使用时创建，关闭时释放。
//!
//! 同一篇已经有悬浮窗时只聚焦它。每篇记住自己的位置和大小：关闭和退出时写进便签文件的 `floatWindow`，
//! 重启后再悬浮时放回；写不成时，本进程里还记着这个值。屏幕变了、位置不在任何工作区内时拉回工作区。
//! 保存流程在 `note_editor`，规则在 `lanwork_core::panel`。版式在 `ui/notefloat.slint`。

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use lanwork_core::notes::{FloatGeometry, NoteCommands};
use lanwork_core::panel::{
    FLOAT_HEIGHT, FLOAT_MIN_HEIGHT, FLOAT_MIN_WIDTH, FLOAT_WIDTH, float_default_origin,
    restore_float,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::storage::Store;
use slint::winit_030::WinitWindowAccessor;
use slint::{CloseRequestResponse, ComponentHandle};

use crate::NoteFloat;
use crate::bar_win;
use crate::note_editor::Editor;

enum After {
    Close,
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Geometry {
    origin: (i32, i32),
    size: (u32, u32),
}

struct FloatWindow {
    ui: NoteFloat,
    editor: Editor<After>,
    hwnd_ready: bool,
    origin: Option<(i32, i32)>,
    /// 已经隐藏、等着从表里去掉。位置和大小在 `last`。
    closed: bool,
    last: Option<Geometry>,
}

struct Floats {
    notes: NoteCommands,
    store: Store,
    dark: bool,
    windows: HashMap<String, FloatWindow>,
    /// 写进便签文件没有成功的位置和大小，本进程里先用这个。
    remembered: HashMap<String, Geometry>,
}

thread_local! {
    static FLOATS: RefCell<Option<Floats>> = const { RefCell::new(None) };
}

/// 启动时调用一次。没有便签服务时不调用，悬浮窗打不开。
pub(crate) fn install(notes: NoteCommands, store: Store) {
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
    with_floats(move |floats| {
        let ctx = Ctx {
            notes: &floats.notes,
            store: &floats.store,
            dark: floats.dark,
        };
        if let Some(window) = floats.windows.get_mut(&id) {
            action(window, &ctx);
        }
    });
}

struct Ctx<'a> {
    notes: &'a NoteCommands,
    store: &'a Store,
    dark: bool,
}

fn persist_geometry(notes: &NoteCommands, note_id: &str, geometry: Geometry) -> Result<(), String> {
    notes
        .set_float(
            note_id,
            FloatGeometry {
                x: geometry.origin.0,
                y: geometry.origin.1,
                width: geometry.size.0,
                height: geometry.size.1,
            },
        )
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
                if let Some(geometry) = window.geometry()
                    && let Err(message) =
                        persist_geometry(&notes, window.editor.session.id(), geometry)
                {
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
    /// 把已经关闭的窗口从表里去掉，并记下它的位置和大小。窗口随之释放。
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

    fn remember(&mut self, note_id: &str, geometry: Geometry) {
        match persist_geometry(&self.notes, note_id, geometry) {
            Ok(()) => {
                self.remembered.remove(note_id);
            }
            Err(message) => {
                self.store
                    .log_warn(&format!("悬浮窗位置没有写进便签：{message}"));
                self.remembered.insert(note_id.to_owned(), geometry);
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
        let saved = self.remembered.get(note_id).copied().or_else(|| {
            note.float.map(|float| Geometry {
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
                let placed = restore_float(
                    FloatGeometry {
                        x: saved.origin.0,
                        y: saved.origin.1,
                        width: saved.size.0,
                        height: saved.size.1,
                    },
                    &bar_win::work_areas(),
                    work,
                    min,
                );
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
                hwnd_ready: false,
                origin: Some(geometry.origin),
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
            if let Some(origin) = self.origin
                && bar_win::window_origin(hwnd) != Some(origin)
            {
                bar_win::move_window(hwnd, origin.0, origin.1);
            }
        }
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
        let actual = match self.hwnd() {
            Some(hwnd) => bar_win::apply_dwm(hwnd, bar_win::current_backdrop(), dark),
            None => Backdrop::Solid,
        };
        self.ui.set_acrylic(actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(dark)));
    }

    /// 显示在最前并把焦点给正文。位置和大小不动。
    fn raise(&self) {
        let _ = self.ui.show();
        if let Some(hwnd) = self.hwnd() {
            bar_win::bring_to_front(hwnd);
        }
        self.ui.invoke_focus_body();
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

    fn geometry(&self) -> Option<Geometry> {
        let origin = self.hwnd().and_then(bar_win::window_origin).or(self.origin);
        let size = self.ui.window().size();
        origin.map(|origin| Geometry {
            origin,
            size: (size.width, size.height),
        })
    }

    /// 记下位置和大小，隐藏窗口，然后在下一轮把它从表里去掉，释放文本和绘图资源。
    /// 正文留在便签列表里；这里不删除便签。
    fn finish_close(&mut self) {
        let id = self.editor.session.id().to_owned();
        self.last = self.geometry();
        self.closed = true;
        let _ = self.ui.hide();
        slint::Timer::single_shot(Duration::ZERO, move || {
            with_floats(move |floats| floats.reap(&id));
        });
    }
}
