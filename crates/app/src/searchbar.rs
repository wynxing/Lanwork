//! 搜索条窗口：热键显示与收起、输入交给查询调度、结果列表和打开动作。
//! 右侧按钮打开应用墙（`appwall`）。应用墙打开时这个窗口隐藏，文字留在输入框。
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
    QueryView, RgbaImage, RowDetail, SearchGroup, SearchRow, Surface, SystemClock,
    USAGE_SAVE_DELAY, UsageKey, ViewPhase,
};
use lanwork_core::panel::PanelTarget;
use lanwork_core::search::{classify_prefix, folder_tab_target, looks_like_local_path};
use lanwork_core::shell::{
    AppMenuAction, BAR_WIDTH, Backdrop, EnterChord, EscapeAction, HoverArm, MonitorArea,
    PointerPos, ROW_HEIGHT, ResultMenuAction, RowAction, WorkArea, app_menu_command, caret_at_end,
    draws_type_glyph, escape_action, group_label, hover_row, location_always_shown, match_span,
    max_results_height, menu_index, monitor_at, move_selection, path_target, place_bar,
    pointed_row, result_menu, row_action, row_offsets, selection_after_refresh, shelf_pick_menu,
    shortcut_rows, shortcut_target, solid_rgb, status_line, text_after_hide, to_physical,
};
use lanwork_core::shelves::IncomingRef;
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
/// 路径线程还在读时，隔这么久看一眼它有没有返回。没有路径请求时不轮询。
const PATH_POLL: Duration = Duration::from_millis(15);

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
    static USAGE_DISPATCH: RefCell<Option<Arc<Dispatch>>> = const { RefCell::new(None) };
}

/// 按键回调要立刻知道有没有接住。借不到搜索条时不接住，把键交回输入框。
fn with_bar_sync(action: impl FnOnce(&mut Bar) -> bool) -> bool {
    BAR.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => slot.as_mut().map(action).unwrap_or(false),
        Err(_) => false,
    })
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
    ui.on_toggle_wall(|| with_bar(Bar::toggle_wall));
    ui.on_escape(|| with_bar(Bar::escape));
    ui.on_move_selection(|down| with_bar(move |bar| bar.move_selection(down)));
    ui.on_enter(|ctrl, shift, alt, meta| {
        let chord = EnterChord::from_modifiers(ctrl, shift, alt, meta);
        with_bar(move |bar| bar.enter(chord));
    });
    ui.on_complete_folder(|| with_bar_sync(Bar::complete_folder));
    ui.on_path_up(|| with_bar_sync(Bar::path_up));
    ui.on_hover(|index, x, y| with_bar(move |bar| bar.hover(index, x, y)));
    ui.on_activate(|index| with_bar(move |bar| bar.activate(index)));
    ui.on_shortcut(|digit| with_bar(move |bar| bar.shortcut(digit)));
    ui.on_open_menu(|index, x, y| with_bar(move |bar| bar.open_menu(index, x, y)));
    ui.on_open_selected_menu(|| with_bar(Bar::open_selected_menu));
    ui.on_caret_at_end(|offset| with_bar_sync(move |bar| bar.caret_at_end(offset)));
    ui.on_menu_activate(|index| with_bar_sync(move |bar| bar.menu_activate(index)));
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

    USAGE_DISPATCH.with(|cell| *cell.borrow_mut() = Some(Arc::clone(&dispatch)));
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
        concealed: false,
        window_shown: false,
        restore_guard: 0,
        dark: false,
        hwnd_ready: false,
        launching: false,
        menu_row: None,
        menu_commands: Vec::new(),
        dragging: false,
        drag_origin: None,
        first_sequence: None,
        hover: HoverArm::new(),
        path_up: None,
        path_count: 0,
        usage_save: slint::Timer::default(),
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

/// 搜索条还显示着就收起。应用墙启动成功，或点到两者以外时用。
pub(crate) fn hide_if_visible() {
    with_bar(|bar| {
        if bar.visible {
            bar.hide();
        }
    });
}

pub(crate) fn set_wall_open(open: bool) {
    with_bar(move |bar| bar.ui.set_wall_open(open));
}

pub(crate) fn focus_input() {
    with_bar(Bar::restore_after_wall);
}

pub(crate) fn bar_hwnd() -> Option<windows::Win32::Foundation::HWND> {
    BAR.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Bar::hwnd))
    })
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
    flush_usage();
    with_bar(|bar| bar.flush_latency());
}

/// 把尚未落盘的使用次数写上。退出时调用，不等待防抖计时。
pub(crate) fn flush_usage() {
    let dispatch = USAGE_DISPATCH.with(|cell| cell.borrow().clone());
    if let Some(dispatch) = dispatch {
        dispatch.flush_usage();
    }
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
    /// 失焦时不收起。为应用墙藏起窗口时为真。重新显示之后再留两轮，
    /// 让已经入队的 `Focused(false)` 过去。窗口已经重新显示时，它不表示窗口仍藏着。
    concealed: bool,
    /// 搜索条窗口是否在屏幕上。为应用墙藏起时 [`Self::visible`] 仍为真，这里为假。
    window_shown: bool,
    /// 最近一次重新显示的失焦保护。更早排上的定时器不再清 [`Self::concealed`]。
    restore_guard: u64,
    dark: bool,
    hwnd_ready: bool,
    launching: bool,
    /// 右键菜单对应的结果行。菜单关着时为 `None`。
    menu_row: Option<usize>,
    /// 当前菜单的每一项。分组列表换上来时换成那些分组。
    menu_commands: Vec<MenuCommand>,
    /// 拖动过程中，包括循环结束后留给迟到失焦的那两轮。
    dragging: bool,
    /// 这次按下时的左上角。松开后没变就不写配置。
    drag_origin: Option<(i32, i32)>,
    first_sequence: Option<u64>,
    /// 悬停改选中之前，指针要相对打开时和最近一次结果刷新时的位置移动过。
    hover: HoverArm,
    /// 本机路径时，Ctrl+Backspace 写回这一层。
    path_up: Option<String>,
    /// 当前结果最前面有多少条本机路径行。用来判断这一组是不是这次新接上的。
    path_count: usize,
    usage_save: slint::Timer,
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
            self.set_window_shown(false);
            self.marks.borrow_mut().hot_start = None;
            self.store.log_warn(&format!("搜索条没有显示：{err}"));
            return;
        }
        self.set_window_shown(true);
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

    fn set_window_shown(&mut self, shown: bool) {
        self.window_shown = shown;
        crate::appwall::set_search_shown(shown);
    }

    fn hide(&mut self) {
        self.ui.set_wall_open(false);
        crate::appwall::dismiss();
        self.hover.reset(None);
        self.close_menu();
        self.visible = false;
        self.concealed = false;
        self.set_window_shown(false);
        self.restore_guard = self.restore_guard.wrapping_add(1);
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
        if self.dragging || !self.visible || self.concealed {
            return;
        }
        if !crate::appwall::is_open() {
            self.hide();
            return;
        }
        // 点到应用墙上时，失焦先到，前台句柄随后才换成那扇窗。下一轮再看。
        // 为应用墙隐藏搜索条时也会失焦，那时 `concealed` 已经为真，不再收起。
        slint::Timer::single_shot(Duration::ZERO, || {
            with_bar(|bar| {
                if bar.dragging || !bar.visible || bar.concealed {
                    return;
                }
                if crate::appwall::is_open() {
                    let foreground = bar_win::foreground_root();
                    if foreground == bar.hwnd() || crate::appwall::holds_foreground(foreground) {
                        return;
                    }
                }
                bar.hide();
            });
        });
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

    fn toggle_wall(&mut self) {
        if self.ui.get_menu_open() {
            self.close_menu();
        }
        if crate::appwall::is_open() {
            crate::appwall::dismiss();
            self.restore_after_wall();
            return;
        }
        self.ui.set_wall_open(true);
        let origin = self.hwnd().and_then(bar_win::window_origin);
        crate::appwall::open(origin);
        if crate::appwall::is_open() {
            self.conceal_for_wall();
        } else {
            self.ui.set_wall_open(false);
        }
    }

    /// 应用墙已经在前台时藏起搜索条。文字留在输入框，不走 [`Self::hide`]。
    /// `concealed` 在重新显示之后还会再留两轮，那时窗口已经在屏幕上，这里仍要藏起。
    fn conceal_for_wall(&mut self) {
        if !self.visible {
            return;
        }
        if !crate::appwall::has_foreground() {
            self.store.log_warn("应用墙未在前台，搜索条保持显示");
            return;
        }
        self.concealed = true;
        let _ = self.ui.hide();
        self.set_window_shown(false);
    }

    /// 应用墙收起后把搜索条显示回来，焦点回到输入框，已输入的文字还在。
    fn restore_after_wall(&mut self) {
        self.ui.set_wall_open(false);
        if self.concealed && self.visible {
            if let Err(err) = self.ui.show() {
                self.concealed = false;
                self.visible = false;
                self.set_window_shown(false);
                self.store.log_warn(&format!("搜索条没有重新显示：{err}"));
                return;
            }
            self.set_window_shown(true);
            if let Some(hwnd) = self.hwnd() {
                match bar_win::bring_to_front(hwnd) {
                    bar_win::ForegroundTake::Plain => {}
                    bar_win::ForegroundTake::AfterInput => {
                        self.store.log_info("搜索条在注入输入后回到前台");
                    }
                    bar_win::ForegroundTake::Failed => {
                        self.store.log_warn("搜索条没有回到前台");
                    }
                }
            }
            self.ui.invoke_focus_input();
            // 失焦排在这次调用返回之后。先留一整轮给已经入队的失焦，下一轮才放下保护。
            // 清掉之后不再聚焦，避免再排进一次 Focused(false)。
            self.restore_guard = self.restore_guard.wrapping_add(1);
            let guard = self.restore_guard;
            slint::Timer::single_shot(Duration::ZERO, move || {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    with_bar(move |bar| bar.release_restore_guard(guard));
                });
            });
            return;
        }
        self.concealed = false;
        if self.visible {
            self.ui.invoke_focus_input();
        }
    }

    /// 两轮之后放下恢复保护。窗口若又显示在屏幕上，不把 `concealed` 留成真。
    fn release_restore_guard(&mut self, guard: u64) {
        if guard != self.restore_guard || !self.visible {
            return;
        }
        if crate::appwall::is_open() && !self.window_shown {
            return;
        }
        self.concealed = false;
    }

    fn escape(&mut self) {
        if self.ui.get_menu_open() {
            self.close_menu();
            return;
        }
        if crate::appwall::is_open() {
            crate::appwall::dismiss();
            self.restore_after_wall();
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
        // 同一段输入沿用已选中的那一条。路径这一组是这次新接到最前面时，改选第一条。
        let path_prepended = self.path_count == 0 && view.path_count > 0;
        self.selected =
            selection_after_refresh(previous.as_ref(), same_text, &view.rows, path_prepended);
        self.results = view.rows;
        self.phase = view.phase;
        self.view_text = view.text;
        self.sequence = view.sequence;
        self.file = view.file;
        self.file_unavailable = view.file_unavailable;
        self.path_up = view.path_up;
        self.path_count = view.path_count;
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
                let kind = if matches!(row.detail, RowDetail::PathTimeout) {
                    String::new()
                } else {
                    group_label(row.group).to_owned()
                };
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
                    kind: kind.into(),
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

    /// Alt+数字。菜单开着时按当前菜单的项序选择。和 Enter 一样，组合输入期间、快速收集前缀下不打开结果。
    fn shortcut(&mut self, digit: i32) {
        let Ok(digit) = u32::try_from(digit) else {
            return;
        };
        if self.ui.get_menu_open() {
            self.menu_shortcut(i32::try_from(digit).unwrap_or(0));
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
        if let Some(index) = menu_index(digit, self.menu_commands.len()) {
            let _ = self.run_menu_index(index);
        }
    }

    fn caret_at_end(&self, offset: i32) -> bool {
        caret_at_end(&self.ui.get_text(), offset)
    }

    /// 光标在输入末尾时，右方向键打开当前选中行的菜单。
    fn open_selected_menu(&mut self) {
        if !self.ui.get_preedit().is_empty() {
            return;
        }
        let Some(index) = i32::try_from(self.selected).ok() else {
            return;
        };
        let (x, y) = self.keyboard_anchor(self.selected);
        self.open_menu(index, x, y);
    }

    fn keyboard_anchor(&self, index: usize) -> (f32, f32) {
        let row_y = px(self.offsets.get(index).copied().unwrap_or(0));
        let viewport = px(self.content_height.min(self.max_list));
        let max_top = (viewport - px(ROW_HEIGHT)).max(0.0);
        let top = (row_y - self.scroll_top).clamp(0.0, max_top);
        let y = px(INPUT_HEIGHT) + 1.0 + top + px(ROW_HEIGHT) / 2.0;
        (px(BAR_WIDTH) - 24.0, y)
    }

    fn open_menu(&mut self, index: i32, x: f32, y: f32) {
        let Some(index) = pointed_row(index, self.results.len()) else {
            return;
        };
        let Some(row) = self.results.get(index) else {
            return;
        };
        let Some(items) = result_menu(&row.detail) else {
            return;
        };
        self.selected = index;
        self.ui
            .set_selected(i32::try_from(self.selected).unwrap_or(0));
        self.menu_row = Some(index);
        self.menu_commands = items
            .iter()
            .map(|item| MenuCommand {
                action: MenuAction::Result(item.action),
                enabled: item.enabled,
            })
            .collect();
        let shown: Vec<BarMenuItem> = items
            .into_iter()
            .map(|item| BarMenuItem {
                label: item.label.into(),
                hint: item.hint.into(),
                enabled: item.enabled,
            })
            .collect();
        self.show_menu(shown, x, y);
    }

    fn show_menu(&mut self, items: Vec<BarMenuItem>, x: f32, y: f32) {
        self.ui.set_menu_items(ModelRc::new(VecModel::from(items)));
        self.ui.set_menu_anchor_x(x);
        self.ui.set_menu_anchor_y(y);
        self.ui.set_menu_open(true);
        self.ui.invoke_refresh_menu();
    }

    /// 返回真表示菜单留着。
    fn menu_activate(&mut self, index: i32) -> bool {
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        self.run_menu_index(index)
    }

    fn run_menu_index(&mut self, index: usize) -> bool {
        let Some(command) = self.menu_commands.get(index).cloned() else {
            return false;
        };
        if !command.enabled || self.launching {
            return true;
        }
        match command.action {
            MenuAction::Result(action) => self.run_result(action),
            MenuAction::Shelf(group_id) => {
                self.add_to_shelf(group_id);
                false
            }
        }
    }

    fn run_result(&mut self, action: ResultMenuAction) -> bool {
        match action {
            ResultMenuAction::CopyPath => {
                self.copy_path();
                false
            }
            ResultMenuAction::CopyFile => {
                self.copy_file();
                false
            }
            ResultMenuAction::CopyUrl => {
                self.copy_url();
                false
            }
            ResultMenuAction::AddToShelf => self.show_shelf_groups(),
            other => self.run_row_action(other),
        }
    }

    fn run_row_action(&mut self, action: ResultMenuAction) -> bool {
        let Some(index) = self.menu_row else {
            self.close_menu();
            return false;
        };
        let Some(row) = self.results.get(index) else {
            self.close_menu();
            return false;
        };
        let command = match (&row.detail, action) {
            (RowDetail::App(hit), action) => {
                app_action(action).and_then(|action| app_menu_command(&hit.entry, action))
            }
            (
                RowDetail::File { .. } | RowDetail::Folder { .. },
                ResultMenuAction::OpenContainingFolder,
            ) => row_action(&row.detail, EnterChord::Ctrl),
            (RowDetail::Note { id }, ResultMenuAction::FloatNote) => {
                Some(RowAction::FloatNote { id: id.clone() })
            }
            _ => None,
        };
        let Some(command) = command else {
            return true;
        };
        let usage = row.usage.clone();
        let query = self.view_text.clone();
        self.close_menu();
        self.perform(command, usage, query);
        false
    }

    fn show_shelf_groups(&mut self) -> bool {
        let Some(shelves) = self.dispatch.shelf_commands() else {
            self.fail_menu("收纳服务没有打开");
            return false;
        };
        let groups = match shelves.list() {
            Ok(groups) => groups,
            Err(err) => {
                self.fail_menu(&err.to_string());
                return false;
            }
        };
        let pairs: Vec<(&str, &str)> = groups
            .iter()
            .map(|group| (group.id.as_str(), group.name.as_str()))
            .collect();
        let picks = shelf_pick_menu(&pairs);
        self.menu_commands = picks
            .iter()
            .map(|pick| MenuCommand {
                action: MenuAction::Shelf(pick.group_id.clone()),
                enabled: true,
            })
            .collect();
        let items: Vec<BarMenuItem> = picks
            .into_iter()
            .map(|pick| BarMenuItem {
                label: pick.label.into(),
                hint: pick.hint.into(),
                enabled: true,
            })
            .collect();
        self.ui.set_menu_items(ModelRc::new(VecModel::from(items)));
        self.ui.invoke_refresh_menu();
        true
    }

    fn add_to_shelf(&mut self, group_id: Option<String>) {
        let Some((path, folder)) = self.menu_path() else {
            self.close_menu();
            return;
        };
        let Some(shelves) = self.dispatch.shelf_commands() else {
            self.fail_menu("收纳服务没有打开");
            return;
        };
        match shelves.add_refs(group_id.as_deref(), &[IncomingRef { path, folder }]) {
            Ok(_) => self.note_shelf_added(),
            Err(err) => self.fail_menu(&err.to_string()),
        }
    }

    fn note_shelf_added(&mut self) {
        self.error = None;
        let text = self.ui.get_text().to_string();
        self.close_menu();
        self.submit(&text);
        self.push_status();
    }

    fn copy_path(&mut self) {
        let Some((path, _)) = self.menu_path() else {
            self.close_menu();
            return;
        };
        self.copy_text(&path);
    }

    fn copy_file(&mut self) {
        let Some((path, _)) = self.menu_path() else {
            self.close_menu();
            return;
        };
        match crate::clipboard::copy_explorer_file(&path) {
            Ok(()) => self.copied(),
            Err(message) => self.fail_menu(&message),
        }
    }

    fn copy_url(&mut self) {
        let url = self.menu_row.and_then(|index| {
            self.results.get(index).and_then(|row| match &row.detail {
                RowDetail::Web { url } => Some(url.clone()),
                _ => None,
            })
        });
        let Some(url) = url else {
            self.close_menu();
            return;
        };
        self.copy_text(&url);
    }

    fn copy_text(&mut self, text: &str) {
        match crate::clipboard::copy_text(text) {
            Ok(()) => self.copied(),
            Err(message) => self.fail_menu(&message),
        }
    }

    fn copied(&mut self) {
        self.error = None;
        self.close_menu();
        self.push_status();
    }

    fn menu_path(&self) -> Option<(String, bool)> {
        let row = self.results.get(self.menu_row?)?;
        match &row.detail {
            RowDetail::File { path } => Some((path.clone(), false)),
            RowDetail::Folder { path } => Some((path.clone(), true)),
            _ => None,
        }
    }

    fn fail_menu(&mut self, message: &str) {
        self.store.log_warn(&format!("搜索条菜单：{message}"));
        self.error = Some(message.to_owned());
        self.close_menu();
        self.push_status();
    }

    fn close_menu(&mut self) {
        self.menu_row = None;
        self.menu_commands.clear();
        self.ui.set_menu_open(false);
    }

    fn hide_app(&mut self, key: &str) {
        match self.dispatch.hide_app(key) {
            Ok(()) => {
                self.error = None;
                let text = self.ui.get_text().to_string();
                self.submit(&text);
                crate::panel::refresh_hidden_apps();
                crate::appwall::reload();
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

    /// Tab：当前输入是本机路径、且选中的是文件夹时，把该路径补进输入框并列出下一层。
    fn complete_folder(&mut self) -> bool {
        if self.ui.get_menu_open() || !self.ui.get_preedit().is_empty() || self.launching {
            return false;
        }
        let text = self.ui.get_text();
        if classify_prefix(&text).is_capture() {
            return false;
        }
        let folder = self
            .results
            .get(self.selected)
            .and_then(|row| match &row.detail {
                RowDetail::Folder { path } => Some(path.as_str()),
                _ => None,
            });
        let Some(next) = folder_tab_target(&text, folder) else {
            return false;
        };
        self.replace_input(&next);
        true
    }

    /// Ctrl+Backspace：回到当前列出的上一层。不是路径时不接住，输入框按词删除。
    /// 已经在盘符根或 UNC 共享根时接住这个键，输入保持不变。
    fn path_up(&mut self) -> bool {
        if self.ui.get_menu_open() || !self.ui.get_preedit().is_empty() || self.launching {
            return false;
        }
        let text = self.ui.get_text();
        if !looks_like_local_path(&text) {
            return false;
        }
        let Some(next) = self.path_up.clone() else {
            return true;
        };
        if next != text.as_str() {
            self.replace_input(&next);
        }
        true
    }

    fn replace_input(&mut self, text: &str) {
        let end = i32::try_from(text.len()).unwrap_or(i32::MAX);
        self.ui.invoke_replace_input(text.into(), end);
        self.submit(text);
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
        let query = self.view_text.clone();
        self.perform(action, usage, query);
    }

    fn perform(&mut self, action: RowAction, usage: UsageKey, query: String) {
        match action {
            RowAction::HideApp { key } => self.hide_app(&key),
            RowAction::Shell(_)
            | RowAction::Elevated(_)
            | RowAction::RunAsUser(_)
            | RowAction::OpenFolder(_)
            | RowAction::WebSearch { .. } => {
                self.launch(action, usage, query);
            }
            RowAction::PanelTodo { .. }
            | RowAction::PanelNote { .. }
            | RowAction::FloatNote { .. } => {
                self.note_usage(&usage, &query);
                self.hide();
                hand_off(action);
            }
        }
    }

    fn note_usage(&mut self, key: &UsageKey, query: &str) {
        if self.dispatch.record_open_for_query(key, query) {
            self.usage_save
                .start(slint::TimerMode::SingleShot, USAGE_SAVE_DELAY, || {
                    flush_usage();
                });
        }
    }

    /// Shell 调用可能等提权提示或慢速网络路径，不放在界面线程上。
    fn launch(&mut self, action: RowAction, usage: UsageKey, query: String) {
        self.launching = true;
        let counts = action.counts_as_open();
        let spawned = std::thread::Builder::new()
            .name("lanwork-open".into())
            .spawn(move || {
                let result = run_shell(&action);
                let _ = slint::invoke_from_event_loop(move || {
                    with_bar(move |bar| bar.launched(result, &usage, counts, &query));
                });
            });
        if let Err(err) = spawned {
            self.launching = false;
            self.error = Some(format!("打开失败：{err}"));
            self.push_status();
        }
    }

    fn launched(
        &mut self,
        result: Result<(), LaunchError>,
        usage: &UsageKey,
        counts: bool,
        query: &str,
    ) {
        self.launching = false;
        match result {
            Ok(()) => {
                if counts {
                    self.note_usage(usage, query);
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

pub(crate) fn run_shell(action: &RowAction) -> Result<(), LaunchError> {
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

#[derive(Debug, Clone)]
struct MenuCommand {
    action: MenuAction,
    enabled: bool,
}

#[derive(Debug, Clone)]
enum MenuAction {
    Result(ResultMenuAction),
    /// `None` 表示还没有分组，由服务创建「收纳」。
    Shelf(Option<String>),
}

fn app_action(action: ResultMenuAction) -> Option<AppMenuAction> {
    Some(match action {
        ResultMenuAction::RunAsUser => AppMenuAction::RunAsUser,
        ResultMenuAction::RunAsAdmin => AppMenuAction::RunAsAdmin,
        ResultMenuAction::OpenFileFolder => AppMenuAction::OpenFileFolder,
        ResultMenuAction::OpenTargetFolder => AppMenuAction::OpenTargetFolder,
        ResultMenuAction::Hide => AppMenuAction::Hide,
        _ => return None,
    })
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
        RowDetail::PathTimeout => 3,
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

fn worker_main(dispatch: &Arc<Dispatch>, rx: &Receiver<Job>) {
    // 路径线程只回报这次请求是否还有效。投递时再读 view，避免盖住随后合并的文件结果。
    let (path_tx, path_rx) = mpsc::channel::<bool>();
    let mut deadline: Option<Instant> = None;
    let mut path_pending = 0u32;
    loop {
        if path_pending > 0
            && let Ok(current) = path_rx.try_recv()
        {
            path_pending = path_pending.saturating_sub(1);
            if current {
                post(Some(dispatch.view()), dispatch.load_visible_icons());
            }
            continue;
        }
        let job = match deadline {
            None if path_pending == 0 => match rx.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            },
            None => match rx.recv_timeout(PATH_POLL) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
        };
        if let Some(job) = job {
            post(None, dispatch.load_visible_icons());
            if let Some(request) = dispatch.take_path_request() {
                path_pending = path_pending.saturating_add(1);
                let tx = path_tx.clone();
                let dispatch = Arc::clone(dispatch);
                let spawned = std::thread::Builder::new()
                    .name("lanwork-path-list".to_owned())
                    .spawn(move || {
                        let current = dispatch.complete_path(request);
                        let _ = tx.send(current);
                    });
                if spawned.is_err() {
                    path_pending = path_pending.saturating_sub(1);
                }
            }
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
