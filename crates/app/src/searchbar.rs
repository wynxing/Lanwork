//! 搜索条窗口：热键显示与收起、输入交给查询调度、结果列表和打开动作。
//!
//! 待办和便签结果的落点是面板（`panel`）；便签结果的 Ctrl+Enter 悬浮（`note_float`）。快速收集的预览和提交不在这里。

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use lanwork_core::apps::{LaunchError, LaunchTarget, launch, launch_as_user, launch_elevated};
use lanwork_core::config::{ConfigCommands, SearchBarOrigin};
use lanwork_core::dispatch::{
    Dispatch, FILE_QUERY_DELAY_MS, FileProgress, IconKey, LATENCY_SOURCE_LOCAL, Monotonic,
    QueryView, RgbaImage, RowDetail, SearchGroup, SearchRow, Surface, SystemClock, UsageKey,
    ViewPhase,
};
use lanwork_core::panel::PanelTarget;
use lanwork_core::search::classify_prefix;
use lanwork_core::shell::{
    AltRoute, AppMenuAction, BAR_WIDTH, Backdrop, EnterChord, EscapeAction, HoverArm, MonitorArea,
    PointerPos, ROW_HEIGHT, RowAction, WorkArea, alt_route, app_menu, app_menu_command,
    draws_type_glyph, escape_action, group_label, hover_row, location_always_shown, match_span,
    max_results_height, monitor_at, move_selection, path_target, place_bar, pointed_row, reselect,
    row_action, row_offsets, shortcut_rows, shortcut_target, solid_rgb, status_line,
    text_after_hide, to_physical,
};
use lanwork_core::storage::Store;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle, Model as _, ModelRc, VecModel};

use crate::bar_win;
use crate::{BarMenuItem, BarRow, SearchBar};

/// 测量用。设了这个变量时，热召回和结果延迟的原始记录追加到该文件。界面不暴露它。
pub(crate) const LATENCY_OUT_ENV: &str = "LANWORK_LATENCY_OUT";

const INPUT_HEIGHT: u32 = 56;
/// 结果区与工作区底边至少留出的距离。
const BOTTOM_GAP: u32 = 24;
/// 结果区最多这么高，再多就滚动。约 9 行。
const LIST_CAP: u32 = 9 * ROW_HEIGHT + 2 * lanwork_core::shell::LIST_PAD;
/// 60ms 到点时调度还说要等，就再等这么久。
const POLL_RETRY: Duration = Duration::from_millis(5);

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// 在界面线程上取搜索条。回调里可能已经借用着，这时放到下一轮事件循环。
fn with_bar(action: impl FnOnce(&mut Bar) + 'static) {
    let deferred = BAR.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => {
            if let Some(bar) = slot.as_mut() {
                action(bar);
            }
            None
        }
        Err(_) => Some(action),
    });
    if let Some(action) = deferred {
        slint::Timer::single_shot(Duration::ZERO, move || with_bar(action));
    }
}

/// 启动时调用一次。窗口保持隐藏，直到热键。
pub(crate) fn install(
    ui: SearchBar,
    dispatch: Arc<Dispatch>,
    store: Store,
    config: ConfigCommands,
) {
    let rows = Rc::new(VecModel::<BarRow>::default());
    ui.set_rows(ModelRc::from(Rc::clone(&rows)));
    let marks = Rc::new(RefCell::new(Marks {
        dispatch: Arc::clone(&dispatch),
        log: LatencyLog::from_env(store.clone()),
        hot_start: None,
        hot_seq: 0,
        hot_warm: false,
        result: None,
    }));
    let worker = Worker::start(Arc::clone(&dispatch), &store);

    ui.on_edited(|text| {
        let text = text.to_string();
        with_bar(move |bar| bar.edited(&text));
    });
    ui.on_escape(|| with_bar(Bar::escape));
    ui.on_move_selection(|down| with_bar(move |bar| bar.move_selection(down)));
    ui.on_enter(|ctrl, shift, alt, meta| {
        let chord = EnterChord::from_modifiers(ctrl, shift, alt, meta);
        with_bar(move |bar| bar.enter(chord));
    });
    ui.on_hover(|index, x, y| with_bar(move |bar| bar.hover(index, x, y)));
    ui.on_activate(|index| with_bar(move |bar| bar.activate(index)));
    ui.on_shortcut(|digit| with_bar(move |bar| bar.shortcut(digit)));
    ui.on_open_menu(|index, x, y| with_bar(move |bar| bar.open_menu(index, x, y)));
    ui.on_menu_activate(|index| with_bar(move |bar| bar.menu_activate(index)));
    ui.on_menu_shortcut(|digit| with_bar(move |bar| bar.menu_shortcut(digit)));
    ui.on_scrolled(|top| with_bar(move |bar| bar.scrolled(top)));
    ui.on_drag_window(|| with_bar(Bar::drag));
    bar_win::add_move_hook(move_loop);

    let render_marks = Rc::clone(&marks);
    let notifier = ui.window().set_rendering_notifier(move |state, _| {
        if matches!(state, slint::RenderingState::AfterRendering) {
            render_marks.borrow_mut().after_render();
        }
    });
    if let Err(err) = notifier {
        store.log_warn(&format!("搜索条渲染通知不可用，不记录延迟：{err}"));
    }

    ui.window().on_winit_window_event(|_, event| {
        if matches!(event, winit::event::WindowEvent::Focused(false)) {
            with_bar(Bar::blur);
        }
        EventResult::Propagate
    });
    // Alt+F4 只收起搜索条，不结束进程。未提交的 `/note` 由 `Bar::hide` 留在输入框。
    // 外部 `taskkill`、注销和关机由隐藏的事件目标窗口和热键窗口处理。
    ui.window().on_close_requested(|| {
        with_bar(Bar::hide);
        CloseRequestResponse::KeepWindowShown
    });

    let ready = ui.clone_strong();
    let spawned = slint::spawn_local(async move {
        let window = ready.window().winit_window().await;
        if let Ok(window) = window {
            use winit::platform::windows::WindowExtWindows as _;
            window.set_skip_taskbar(true);
        }
        with_bar(Bar::window_ready);
    });
    if let Err(err) = spawned {
        store.log_warn(&format!("搜索条窗口初始化未排队：{err}"));
    }

    let bar = Bar {
        ui,
        config,
        rows,
        dispatch,
        store,
        worker,
        marks,
        results: Vec::new(),
        selected: 0,
        offsets: Vec::new(),
        content_height: 0,
        max_list: LIST_CAP,
        scroll_top: 0.0,
        phase: ViewPhase::Empty,
        view_text: String::new(),
        sequence: None,
        file: FileProgress::Idle,
        file_unavailable: None,
        error: None,
        icons: HashMap::new(),
        web_icons: HashMap::new(),
        visible: false,
        dark: false,
        hwnd_ready: false,
        launching: false,
        menu_row: None,
        dragging: false,
        drag_origin: None,
        first_sequence: None,
        hover: HoverArm::new(),
    };
    BAR.with(|cell| *cell.borrow_mut() = Some(bar));
}

/// 隐藏名单变了之后，开着的搜索条按当前输入再查一次。
pub(crate) fn requery() {
    with_bar(|bar| {
        if !bar.visible {
            return;
        }
        let text = bar.ui.get_text().to_string();
        bar.submit(&text);
    });
}

/// 热键。已显示时收起。
pub(crate) fn toggle(received_ns: u64) {
    with_bar(move |bar| {
        if bar.visible {
            bar.hide();
        } else {
            bar.show(received_ns);
        }
    });
}

pub(crate) fn apply_theme(dark: bool) {
    with_bar(move |bar| {
        bar.dark = dark;
        bar.ui.set_dark(dark);
        bar.refresh_backdrop();
    });
}

pub(crate) fn shutdown() {
    with_bar(|bar| bar.flush_latency());
}

struct Bar {
    ui: SearchBar,
    config: ConfigCommands,
    rows: Rc<VecModel<BarRow>>,
    dispatch: Arc<Dispatch>,
    store: Store,
    worker: Worker,
    marks: Rc<RefCell<Marks>>,
    results: Vec<SearchRow>,
    selected: usize,
    offsets: Vec<u32>,
    content_height: u32,
    max_list: u32,
    scroll_top: f32,
    phase: ViewPhase,
    view_text: String,
    sequence: Option<u64>,
    file: FileProgress,
    file_unavailable: Option<&'static str>,
    error: Option<String>,
    icons: HashMap<IconKey, slint::Image>,
    web_icons: HashMap<String, slint::Image>,
    visible: bool,
    dark: bool,
    hwnd_ready: bool,
    launching: bool,
    /// 右键菜单对应的结果行。菜单关着时为 `None`。
    menu_row: Option<usize>,
    /// 拖动过程中，包括循环结束后留给迟到失焦的那两轮。
    dragging: bool,
    /// 这次按下时的左上角。松开后没变就不写配置。
    drag_origin: Option<(i32, i32)>,
    first_sequence: Option<u64>,
    /// 悬停改选中之前，指针要相对打开时和最近一次结果刷新时的位置移动过。
    hover: HoverArm,
}

impl Bar {
    fn window_ready(&mut self) {
        self.hwnd_ready = true;
        if let Some(hwnd) = self.hwnd() {
            if !bar_win::remove_caption_buttons(hwnd) {
                self.store
                    .log_warn("搜索条样式子类没有装上，显示时可能出现标题栏按钮");
            }
            if !bar_win::watch_move_loop(hwnd) {
                self.store
                    .log_warn("搜索条拖动结束通知没有装上，松开后可能不记住位置");
            }
        }
        self.refresh_backdrop();
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

    fn refresh_backdrop(&mut self) {
        let actual = match self.hwnd() {
            Some(hwnd) => bar_win::apply_dwm(hwnd, bar_win::current_backdrop(), self.dark),
            None => Backdrop::Solid,
        };
        self.ui.set_acrylic(actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(self.dark)));
    }

    fn show(&mut self, received_ns: u64) {
        // 先取消上一次的悬停。窗口到位后再记下屏幕上的光标，静止指针不会选中下面的行。
        self.hover.reset(None);
        self.selected = 0;
        self.ui.set_selected(0);
        let (cursor_work, cursor_dpi) = bar_win::cursor_monitor().unwrap_or((
            WorkArea {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            lanwork_core::shell::BASE_DPI,
        ));
        let readings = bar_win::monitors();
        let areas: Vec<MonitorArea> = readings
            .iter()
            .map(|reading| MonitorArea {
                bounds: reading.bounds,
                work: reading.work,
            })
            .collect();
        let saved = self
            .config
            .current()
            .search_bar_origin
            .map(|origin| (origin.x, origin.y));
        let placement = place_bar(
            saved,
            &areas,
            cursor_work,
            to_physical(BAR_WIDTH, cursor_dpi),
        );
        let dpi = placement
            .monitor
            .and_then(|index| readings.get(index))
            .map(|reading| reading.dpi)
            .unwrap_or(cursor_dpi);
        let origin = placement.origin;
        self.apply_results_cap(placement.work, origin.1, dpi);
        self.ui
            .window()
            .set_position(slint::PhysicalPosition::new(origin.0, origin.1));
        self.refresh_backdrop();
        self.error = None;
        self.push_status();
        self.marks.borrow_mut().hot_start = Some(received_ns);
        self.visible = true;
        if let Err(err) = self.ui.show() {
            self.visible = false;
            self.marks.borrow_mut().hot_start = None;
            self.store.log_warn(&format!("搜索条没有显示：{err}"));
            return;
        }
        let hwnd = self.hwnd();
        let foreground = if let Some(hwnd) = hwnd {
            // 跨 DPI 移动时系统建议的矩形可能偏离目标位置，显示后再对一次。
            if bar_win::window_origin(hwnd) != Some(origin) {
                bar_win::move_window(hwnd, origin.0, origin.1);
            }
            if bar_win::window_origin(hwnd).is_some_and(|actual| actual.1 != origin.1) {
                self.refit_results_to_window();
            }
            Some(bar_win::bring_to_front(hwnd))
        } else {
            None
        };
        // 与面板同一套前台办法。没到前台就收起，避免 visible 为真但窗口在别的程序后面。
        // 规格没写搜索条拿不到前台时怎么办，待定。
        match foreground {
            Some(bar_win::ForegroundTake::Plain) => {}
            Some(bar_win::ForegroundTake::AfterInput) => {
                self.store.log_info("搜索条在注入输入后到前台");
            }
            Some(bar_win::ForegroundTake::Failed) | None => {
                self.store.log_warn(if hwnd.is_none() {
                    "搜索条没有窗口句柄，已收起"
                } else {
                    "搜索条没有到前台，已收起"
                });
                self.hide();
                return;
            }
        }
        self.rearm_hover();
        self.ui.invoke_focus_input();
    }

    fn hide(&mut self) {
        self.hover.reset(None);
        self.close_menu();
        self.visible = false;
        self.marks.borrow_mut().hot_start = None;
        let _ = self.ui.hide();
        self.error = None;
        let text = self.ui.get_text().to_string();
        let keep = text_after_hide(&text).to_owned();
        if keep != text {
            self.ui.set_text(keep.clone().into());
            self.submit(&keep);
        } else {
            self.push_status();
        }
        self.flush_latency();
    }

    fn blur(&mut self) {
        if self.dragging {
            return;
        }
        if self.visible {
            self.hide();
        }
    }

    /// 左侧拖动条按下。真正的移动在之后的模态循环里，这里先挡住失焦收起。
    fn drag(&mut self) {
        self.dragging = true;
        self.drag_origin = self.hwnd().and_then(bar_win::window_origin);
        let started = self
            .ui
            .window()
            .with_winit_window(|window| window.drag_window().is_ok());
        if started != Some(true) {
            self.dragging = false;
            self.store.log_warn("搜索条拖动没有开始");
        }
    }

    fn on_move_loop(&mut self, entering: bool) {
        if entering {
            self.dragging = true;
            return;
        }
        self.finish_drag();
    }

    fn finish_drag(&mut self) {
        self.remember_origin();
        self.refit_results_to_window();
        if self.visible {
            self.ui.invoke_focus_input();
        }
        // 失焦可能排在拖动返回之后。先留一轮给已经入队的失焦，再下一轮才允许收起。
        slint::Timer::single_shot(Duration::ZERO, || {
            slint::Timer::single_shot(Duration::ZERO, || {
                with_bar(|bar| bar.dragging = false);
            });
        });
    }

    fn apply_results_cap(&mut self, work: WorkArea, origin_y: i32, dpi: u32) {
        let room = max_results_height(work, origin_y, dpi, INPUT_HEIGHT, BOTTOM_GAP);
        self.max_list = room.min(LIST_CAP);
        self.ui.set_max_list_height(px(self.max_list));
        self.push_shortcuts();
    }

    /// 按窗口现在的左上角重算列表高度。点不在任何显示器上时保持当前高度。
    fn refit_results_to_window(&mut self) {
        let Some((x, y)) = self.hwnd().and_then(bar_win::window_origin) else {
            return;
        };
        let readings = bar_win::monitors();
        let areas: Vec<MonitorArea> = readings
            .iter()
            .map(|reading| MonitorArea {
                bounds: reading.bounds,
                work: reading.work,
            })
            .collect();
        let Some(index) = monitor_at(&areas, x, y) else {
            return;
        };
        let Some(reading) = readings.get(index) else {
            return;
        };
        self.apply_results_cap(reading.work, y, reading.dpi);
    }

    fn remember_origin(&mut self) {
        let Some(hwnd) = self.hwnd() else {
            return;
        };
        let Some((x, y)) = bar_win::window_origin(hwnd) else {
            self.store.log_warn("读不到搜索条位置，没有写入");
            return;
        };
        // 只点一下拖动条、窗口没动时不把当次的默认位置写进去。
        if self.drag_origin == Some((x, y)) {
            return;
        }
        let mut next = self.config.current();
        if next
            .search_bar_origin
            .is_some_and(|origin| origin.x == x && origin.y == y)
        {
            return;
        }
        next.search_bar_origin = Some(SearchBarOrigin { x, y });
        if let Err(err) = self.config.replace(next) {
            self.store.log_warn(&format!("搜索条位置没有写入：{err}"));
            self.error = Some(err.to_string());
            self.push_status();
        }
    }

    fn edited(&mut self, text: &str) {
        self.error = None;
        self.submit(text);
    }

    fn escape(&mut self) {
        if self.ui.get_menu_open() {
            self.close_menu();
            return;
        }
        let text = self.ui.get_text().to_string();
        match escape_action(&text) {
            EscapeAction::Hide => self.hide(),
            EscapeAction::Clear => {
                self.ui.set_text("".into());
                self.error = None;
                self.submit("");
            }
        }
    }

    fn submit(&mut self, text: &str) {
        match self.dispatch.submit(Surface::SearchBar, text) {
            Ok(view) => {
                if self.first_sequence.is_none() {
                    self.first_sequence = view.sequence;
                }
                let wait_file = view.file == FileProgress::Waiting;
                let searching = view.sequence.is_some();
                self.apply(view);
                if searching {
                    self.worker.send(wait_file);
                }
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.push_status();
            }
        }
    }

    /// 后台线程送来的图标和文件结果。旧序号的结果丢掉。
    fn receive(&mut self, view: Option<QueryView>, icons: Vec<(IconKey, RgbaImage)>) {
        for (key, image) in icons {
            self.icons.insert(key, to_image(&image));
        }
        match view {
            Some(view) if view.sequence.is_some_and(|seq| self.dispatch.accepts(seq)) => {
                self.apply(view);
            }
            _ => self.push_rows(),
        }
    }

    fn apply(&mut self, view: QueryView) {
        let same_text = self.phase == ViewPhase::Results
            && view.phase == ViewPhase::Results
            && view.text == self.view_text;
        let previous = self.results.get(self.selected).map(|row| row.usage.clone());
        let menu_usage = self
            .menu_row
            .and_then(|index| self.results.get(index))
            .map(|row| row.usage.clone());
        self.selected = reselect(previous.as_ref(), same_text, &view.rows);
        self.results = view.rows;
        self.phase = view.phase;
        self.view_text = view.text;
        self.sequence = view.sequence;
        self.file = view.file;
        self.file_unavailable = view.file_unavailable;
        // 在把新行交给界面之前记下光标。结果画出来时的那一次移动，位置没变就不改选中。
        self.rearm_hover();
        let visible_keys: Vec<IconKey> = self
            .results
            .iter()
            .filter_map(|row| row.icon.clone())
            .collect();
        self.icons.retain(|key, _| visible_keys.contains(key));
        self.push_rows();
        self.push_status();
        if !same_text {
            self.ui.invoke_scroll_to_top();
            self.scroll_top = 0.0;
            self.push_shortcuts();
        }
        if self.ui.get_menu_open() {
            match menu_usage
                .as_ref()
                .and_then(|usage| self.results.iter().position(|row| &row.usage == usage))
            {
                Some(index) => self.menu_row = Some(index),
                None => self.close_menu(),
            }
        }
        self.ui.invoke_ensure_visible();
        self.marks.borrow_mut().result = self
            .sequence
            .map(|seq| (seq, self.file == FileProgress::Settled));
    }

    fn push_rows(&mut self) {
        self.prepare_web_icons();
        let groups: Vec<SearchGroup> = self.results.iter().map(|row| row.group).collect();
        let (offsets, height) = row_offsets(&groups);
        let rows: Vec<BarRow> = self
            .results
            .iter()
            .zip(offsets.iter().copied())
            .enumerate()
            .map(|(index, (row, y))| {
                let shell = row.icon.as_ref().and_then(|key| self.icons.get(key));
                let web = match &row.detail {
                    RowDetail::Web { url } => self.web_icons.get(url),
                    _ => None,
                };
                let icon = shell.or(web);
                let (head, matched, tail) = match match_span(&row.label, &self.view_text) {
                    Some((start, end)) => (
                        &row.label[..start],
                        &row.label[start..end],
                        &row.label[end..],
                    ),
                    None => (row.label.as_str(), "", ""),
                };
                BarRow {
                    label_head: head.into(),
                    label_match: matched.into(),
                    label_tail: tail.into(),
                    kind: group_label(row.group).into(),
                    location: row.location.clone().into(),
                    location_always: location_always_shown(row.group),
                    icon: icon.cloned().unwrap_or_default(),
                    has_icon: icon.is_some(),
                    draw_glyph: draws_type_glyph(&row.detail),
                    glyph: glyph(&row.detail),
                    y: px(y),
                    group_start: index > 0 && groups[index - 1] != row.group,
                }
            })
            .collect();
        if rows.len() == self.rows.row_count() {
            for (index, row) in rows.into_iter().enumerate() {
                if self.rows.row_data(index).as_ref() != Some(&row) {
                    self.rows.set_row_data(index, row);
                }
            }
        } else {
            self.rows.set_vec(rows);
        }
        self.ui.set_list_content_height(px(height));
        self.ui
            .set_selected(i32::try_from(self.selected).unwrap_or(0));
        self.offsets = offsets;
        self.content_height = height;
        self.push_shortcuts();
    }

    /// 收纳网页引用用网站图标缓存。有缓存就显示；没有则后台再取。失败时图标位留空。
    fn prepare_web_icons(&mut self) {
        let urls: Vec<String> = self
            .results
            .iter()
            .filter_map(|row| match &row.detail {
                RowDetail::Web { url } => Some(url.clone()),
                _ => None,
            })
            .collect();
        self.web_icons
            .retain(|url, _| urls.iter().any(|item| item == url));
        let mut missing = Vec::new();
        for url in &urls {
            if self.web_icons.contains_key(url) {
                continue;
            }
            if let Some(image) = crate::web_icons::cached(url) {
                self.web_icons.insert(url.clone(), to_image(&image));
            } else {
                missing.push(url.clone());
            }
        }
        if !missing.is_empty() {
            crate::web_icons::ensure(missing);
        }
    }

    fn shortcut_range(&self) -> std::ops::Range<usize> {
        let viewport = self.content_height.min(self.max_list);
        shortcut_rows(&self.offsets, self.scroll_top, viewport)
    }

    fn push_shortcuts(&mut self) {
        let range = self.shortcut_range();
        self.ui
            .set_first_shortcut(i32::try_from(range.start).unwrap_or(0));
        self.ui
            .set_end_shortcut(i32::try_from(range.end).unwrap_or(0));
    }

    fn scrolled(&mut self, top: f32) {
        self.scroll_top = top;
        self.push_shortcuts();
    }

    /// Alt+数字。菜单开着时改选菜单。和 Enter 一样，组合输入期间、快速收集前缀下不打开结果。
    fn shortcut(&mut self, digit: i32) {
        let Ok(digit) = u32::try_from(digit) else {
            return;
        };
        if let AltRoute::Menu(action) = alt_route(self.ui.get_menu_open(), digit) {
            if let Some(action) = action {
                self.run_menu(action);
            }
            return;
        }
        if let Some(index) = shortcut_target(self.shortcut_range(), digit) {
            self.selected = index;
            self.ui
                .set_selected(i32::try_from(self.selected).unwrap_or(0));
            self.enter(EnterChord::Plain);
        }
    }

    fn menu_shortcut(&mut self, digit: i32) {
        let Ok(digit) = u32::try_from(digit) else {
            return;
        };
        if let AltRoute::Menu(Some(action)) = alt_route(true, digit) {
            self.run_menu(action);
        }
    }

    fn open_menu(&mut self, index: i32, x: f32, y: f32) {
        let Some(index) = pointed_row(index, self.results.len()) else {
            return;
        };
        let Some(row) = self.results.get(index) else {
            return;
        };
        let lanwork_core::dispatch::RowDetail::App(hit) = &row.detail else {
            return;
        };
        let items: Vec<BarMenuItem> = app_menu(&hit.entry)
            .into_iter()
            .map(|item| BarMenuItem {
                label: item.label.into(),
                hint: item.hint.into(),
                enabled: item.enabled,
            })
            .collect();
        self.selected = index;
        self.ui
            .set_selected(i32::try_from(self.selected).unwrap_or(0));
        self.menu_row = Some(index);
        self.ui.set_menu_items(ModelRc::new(VecModel::from(items)));
        self.ui.set_menu_anchor_x(x);
        self.ui.set_menu_anchor_y(y);
        self.ui.set_menu_open(true);
    }

    fn menu_activate(&mut self, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let Some(action) = [
            AppMenuAction::RunAsUser,
            AppMenuAction::RunAsAdmin,
            AppMenuAction::OpenFileFolder,
            AppMenuAction::OpenTargetFolder,
            AppMenuAction::Hide,
        ]
        .get(index)
        .copied() else {
            return;
        };
        self.run_menu(action);
    }

    fn run_menu(&mut self, action: AppMenuAction) {
        if self.launching {
            return;
        }
        let Some(index) = self.menu_row else {
            return;
        };
        let Some(row) = self.results.get(index) else {
            self.close_menu();
            return;
        };
        let lanwork_core::dispatch::RowDetail::App(hit) = &row.detail else {
            self.close_menu();
            return;
        };
        let Some(command) = app_menu_command(&hit.entry, action) else {
            return;
        };
        let usage = row.usage.clone();
        self.close_menu();
        self.perform(command, usage);
    }

    fn close_menu(&mut self) {
        self.menu_row = None;
        self.ui.set_menu_open(false);
    }

    fn hide_app(&mut self, key: &str) {
        match self.dispatch.hide_app(key) {
            Ok(()) => {
                self.error = None;
                let text = self.ui.get_text().to_string();
                self.submit(&text);
                crate::panel::refresh_hidden_apps();
            }
            Err(message) => {
                self.store.log_warn(&format!("隐藏应用没有写入：{message}"));
                self.error = Some(message);
                self.push_status();
            }
        }
    }

    fn push_status(&mut self) {
        let status = status_line(
            self.error.as_deref(),
            self.phase,
            self.results.len(),
            self.file,
            self.file_unavailable,
        );
        self.ui.set_status_is_error(self.error.is_some());
        self.ui.set_status(status.into());
    }

    fn move_selection(&mut self, down: bool) {
        self.selected = move_selection(self.selected, self.results.len(), down);
        self.ui
            .set_selected(i32::try_from(self.selected).unwrap_or(0));
        self.ui.invoke_ensure_visible();
    }

    /// 记下当前屏幕坐标。之后指针相对它移动过，悬停才改选中项。
    fn rearm_hover(&mut self) {
        let cursor = bar_win::cursor_screen().map(|(x, y)| PointerPos::from_physical(x, y));
        self.hover.reset(cursor);
    }

    /// 行上的逻辑坐标换成屏幕坐标，再和锚点比。窗口在指针下面移动时，屏幕坐标不变。
    fn hover(&mut self, index: i32, x: f32, y: f32) {
        let Some(pos) = self.pointer_screen(x, y) else {
            return;
        };
        let Some(index) = hover_row(&mut self.hover, pos, index, self.results.len()) else {
            return;
        };
        self.selected = index;
        self.ui
            .set_selected(i32::try_from(self.selected).unwrap_or(0));
    }

    fn pointer_screen(&self, x: f32, y: f32) -> Option<PointerPos> {
        let hwnd = self.hwnd()?;
        let local = PointerPos::from_logical(x, y, self.ui.window().scale_factor())?;
        let (sx, sy) = bar_win::client_to_screen(hwnd, local.x(), local.y())?;
        Some(PointerPos::from_physical(sx, sy))
    }

    fn activate(&mut self, index: i32) {
        if let Some(index) = pointed_row(index, self.results.len()) {
            self.selected = index;
            self.ui
                .set_selected(i32::try_from(self.selected).unwrap_or(0));
            self.enter(EnterChord::Plain);
        }
    }

    fn enter(&mut self, chord: EnterChord) {
        if self.ui.get_menu_open() || !self.ui.get_preedit().is_empty() || self.launching {
            return;
        }
        let text = self.ui.get_text();
        if classify_prefix(&text).is_capture() {
            return;
        }
        if !self.sequence.is_some_and(|seq| self.dispatch.accepts(seq)) {
            return;
        }
        let Some(row) = self.results.get(self.selected) else {
            return;
        };
        let Some(action) = row_action(&row.detail, chord) else {
            return;
        };
        let usage = row.usage.clone();
        self.perform(action, usage);
    }

    fn perform(&mut self, action: RowAction, usage: UsageKey) {
        match action {
            RowAction::HideApp { key } => self.hide_app(&key),
            RowAction::Shell(_)
            | RowAction::Elevated(_)
            | RowAction::RunAsUser(_)
            | RowAction::OpenFolder(_)
            | RowAction::WebSearch { .. } => {
                self.launch(action, usage);
            }
            RowAction::PanelTodo { .. }
            | RowAction::PanelNote { .. }
            | RowAction::FloatNote { .. } => {
                self.dispatch.record_open(&usage);
                self.hide();
                hand_off(action);
            }
        }
    }

    /// Shell 调用可能等提权提示或慢速网络路径，不放在界面线程上。
    fn launch(&mut self, action: RowAction, usage: UsageKey) {
        self.launching = true;
        let counts = action.counts_as_open();
        let spawned = std::thread::Builder::new()
            .name("lanwork-open".into())
            .spawn(move || {
                let result = run_shell(&action);
                let _ = slint::invoke_from_event_loop(move || {
                    with_bar(move |bar| bar.launched(result, &usage, counts));
                });
            });
        if let Err(err) = spawned {
            self.launching = false;
            self.error = Some(format!("打开失败：{err}"));
            self.push_status();
        }
    }

    fn launched(&mut self, result: Result<(), LaunchError>, usage: &UsageKey, counts: bool) {
        self.launching = false;
        match result {
            Ok(()) => {
                if counts {
                    self.dispatch.record_open(usage);
                }
                if self.visible {
                    self.hide();
                }
            }
            Err(err) if err.cancelled => {}
            Err(err) => {
                let message = err.message;
                self.store.log_warn(&format!("搜索结果打开失败：{message}"));
                if self.visible {
                    self.error = Some(message);
                    self.push_status();
                }
            }
        }
    }

    fn flush_latency(&mut self) {
        let mut records = self.dispatch.drain_latency_records();
        let mut marks = self.marks.borrow_mut();
        let Some(log) = marks.log.as_mut() else {
            return;
        };
        for record in &mut records {
            if Some(record.seq) == self.first_sequence {
                record.warmup = true;
            }
        }
        log.write(
            records
                .iter()
                .map(lanwork_core::dispatch::LatencyRecord::to_json_line),
        );
    }
}

/// 待办和便签结果打开面板并定位；便签结果的 Ctrl+Enter 悬浮该便签。
fn hand_off(action: RowAction) {
    match action {
        RowAction::PanelTodo { item_id, .. } => {
            crate::panel::open_target(PanelTarget::Todo { item_id });
        }
        RowAction::PanelNote { id } => {
            crate::panel::open_target(PanelTarget::Note { note_id: id });
        }
        RowAction::FloatNote { id } => {
            if let Err(message) = crate::note_float::open(&id) {
                crate::panel::show_error(&message);
            }
        }
        _ => {}
    }
}

fn run_shell(action: &RowAction) -> Result<(), LaunchError> {
    match action {
        RowAction::Shell(target) => launch(target),
        RowAction::Elevated(target) => launch_elevated(target),
        RowAction::RunAsUser(target) => launch_as_user(target),
        RowAction::OpenFolder(folder) => launch(&path_target(&folder.to_string_lossy())),
        RowAction::WebSearch { url } => launch(&LaunchTarget::Url { url: url.clone() }),
        RowAction::HideApp { .. }
        | RowAction::PanelTodo { .. }
        | RowAction::PanelNote { .. }
        | RowAction::FloatNote { .. } => Ok(()),
    }
}

fn glyph(detail: &RowDetail) -> i32 {
    match detail {
        RowDetail::App(_) => 0,
        RowDetail::Todo { .. } => 1,
        RowDetail::Note { .. } => 2,
        RowDetail::File { .. } => 3,
        RowDetail::Folder { .. } => 4,
        RowDetail::Browser { .. } => 5,
        RowDetail::WebSearch { .. } => 6,
        RowDetail::Web { .. } => 7,
    }
}

#[allow(clippy::cast_precision_loss)]
fn px(value: u32) -> f32 {
    value as f32
}

fn rgb(value: u32) -> slint::Color {
    let [_, r, g, b] = value.to_be_bytes();
    slint::Color::from_rgb_u8(r, g, b)
}

fn to_image(image: &RgbaImage) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &image.pixels,
        image.width,
        image.height,
    );
    slint::Image::from_rgba8(buffer)
}

/// 渲染完成时填终点。只在渲染通知里用，不和 [`Bar`] 共用借用。
struct Marks {
    dispatch: Arc<Dispatch>,
    log: Option<LatencyLog>,
    hot_start: Option<u64>,
    hot_seq: u64,
    hot_warm: bool,
    result: Option<(u64, bool)>,
}

impl Marks {
    fn after_render(&mut self) {
        let now = SystemClock.now_ns();
        if let Some(start) = self.hot_start.take() {
            self.hot_seq += 1;
            let warmup = !self.hot_warm;
            self.hot_warm = true;
            if let Some(log) = self.log.as_mut() {
                log.write(std::iter::once(format!(
                    "{{\"metric\":\"hot_recall\",\"seq\":{},\"start_ns\":{start},\"end_ns\":{now},\"warmup\":{warmup},\"superseded\":false}}",
                    self.hot_seq
                )));
            }
        }
        let Some((seq, file_settled)) = self.result.take() else {
            return;
        };
        if !self.dispatch.accepts(seq) {
            return;
        }
        for record in self.dispatch.latency_records() {
            let Some(source) = record.source else {
                continue;
            };
            if record.seq == seq
                && record.end_ns.is_none()
                && !record.superseded
                && (source == LATENCY_SOURCE_LOCAL || file_settled)
            {
                self.dispatch.mark_rendered(seq, source, now);
            }
        }
    }
}

struct LatencyLog {
    path: PathBuf,
    store: Store,
    failed: bool,
}

impl LatencyLog {
    fn from_env(store: Store) -> Option<Self> {
        let path = std::env::var_os(LATENCY_OUT_ENV)?;
        if path.is_empty() {
            return None;
        }
        Some(Self {
            path: PathBuf::from(path),
            store,
            failed: false,
        })
    }

    fn write(&mut self, lines: impl Iterator<Item = String>) {
        let mut text = String::new();
        for line in lines {
            text.push_str(&line);
            text.push('\n');
        }
        if text.is_empty() || self.failed {
            return;
        }
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(err) = written {
            self.failed = true;
            self.store
                .log_warn(&format!("延迟记录写入失败，本次不再写：{err}"));
        }
    }
}

/// 取图标和 60ms 之后的文件查询。新输入打断等待，旧序号的结果由界面丢掉。
struct Worker {
    tx: Sender<Job>,
}

struct Job {
    wait_file: bool,
}

impl Worker {
    fn start(dispatch: Arc<Dispatch>, store: &Store) -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("lanwork-search".into())
            .spawn(move || worker_main(&dispatch, &rx));
        if let Err(err) = spawned {
            // 没有后台线程时仍显示本地结果，只是没有图标和文件结果。
            store.log_warn(&format!("搜索后台线程没有启动：{err}"));
        }
        Self { tx }
    }

    fn send(&self, wait_file: bool) {
        let _ = self.tx.send(Job { wait_file });
    }
}

fn worker_main(dispatch: &Dispatch, rx: &Receiver<Job>) {
    let mut deadline: Option<Instant> = None;
    loop {
        let job = match deadline {
            None => match rx.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            },
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
        };
        if let Some(job) = job {
            post(None, dispatch.load_visible_icons());
            deadline = job
                .wait_file
                .then(|| Instant::now() + Duration::from_millis(FILE_QUERY_DELAY_MS));
            continue;
        }
        deadline = None;
        let Ok(report) = dispatch.poll() else {
            continue;
        };
        if report.sent {
            post(Some(report.view), dispatch.load_visible_icons());
        } else if report.view.file == FileProgress::Waiting {
            deadline = Some(Instant::now() + POLL_RETRY);
        }
    }
}

/// 窗口过程里调用。转到界面队列，避免在模态循环的窗口过程里写盘。
/// 悬浮窗的拖动也走同一个通知，这里只认搜索条自己的句柄。
fn move_loop(key: isize, entering: bool, _x: i32, _y: i32, _width: u32, _height: u32) {
    let posted = slint::invoke_from_event_loop(move || {
        with_bar(move |bar| {
            if bar.hwnd_key() == Some(key) {
                bar.on_move_loop(entering);
            }
        });
    });
    if posted.is_err() {
        with_bar(move |bar| {
            if bar.hwnd_key() == Some(key) {
                bar.on_move_loop(entering);
            }
        });
    }
}

/// 网站图标取到之后画到对应的收纳网页行上。搜索条还没装好时不做任何事。
pub(crate) fn receive_web_icons(icons: Vec<(String, RgbaImage)>) {
    if icons.is_empty() {
        return;
    }
    with_bar(move |bar| {
        for (url, image) in icons {
            bar.web_icons.insert(url, to_image(&image));
        }
        bar.push_rows();
    });
}

fn post(view: Option<QueryView>, icons: Vec<(IconKey, RgbaImage)>) {
    if view.is_none() && icons.is_empty() {
        return;
    }
    let _ = slint::invoke_from_event_loop(move || {
        with_bar(move |bar| bar.receive(view, icons));
    });
}
