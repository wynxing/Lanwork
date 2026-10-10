//! 搜索条窗口：热键显示与收起、输入交给查询调度、结果列表和打开动作。
//!
//! 待办和便签的落点在面板与便签悬浮窗里，这两个窗口还没有。快速收集的预览和提交也不在这里。

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use lanwork_core::apps::{LaunchError, launch, launch_elevated};
use lanwork_core::dispatch::{
    Dispatch, FILE_QUERY_DELAY_MS, FileProgress, IconKey, LATENCY_SOURCE_LOCAL, Monotonic,
    QueryView, RgbaImage, RowDetail, SearchGroup, SearchRow, Surface, SystemClock, UsageKey,
    ViewPhase,
};
use lanwork_core::search::classify_prefix;
use lanwork_core::shell::{
    BAR_WIDTH, Backdrop, EnterChord, EscapeAction, ROW_HEIGHT, RowAction, WorkArea, bar_origin,
    escape_action, group_label, max_results_height, move_selection, path_target, pointed_row,
    reselect, row_action, row_offsets, solid_rgb, status_line, text_after_hide, to_physical,
};
use lanwork_core::storage::Store;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, Model as _, ModelRc, VecModel};

use crate::bar_win;
use crate::{BarRow, SearchBar};

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
pub(crate) fn install(ui: SearchBar, dispatch: Arc<Dispatch>, store: Store) {
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
    ui.on_hover(|index| with_bar(move |bar| bar.hover(index)));
    ui.on_activate(|index| with_bar(move |bar| bar.activate(index)));

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
        rows,
        dispatch,
        store,
        worker,
        marks,
        results: Vec::new(),
        selected: 0,
        phase: ViewPhase::Empty,
        view_text: String::new(),
        sequence: None,
        file: FileProgress::Idle,
        file_unavailable: None,
        error: None,
        icons: HashMap::new(),
        visible: false,
        dark: false,
        hwnd_ready: false,
        launching: false,
        first_sequence: None,
    };
    BAR.with(|cell| *cell.borrow_mut() = Some(bar));
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
    rows: Rc<VecModel<BarRow>>,
    dispatch: Arc<Dispatch>,
    store: Store,
    worker: Worker,
    marks: Rc<RefCell<Marks>>,
    results: Vec<SearchRow>,
    selected: usize,
    phase: ViewPhase,
    view_text: String,
    sequence: Option<u64>,
    file: FileProgress,
    file_unavailable: Option<&'static str>,
    error: Option<String>,
    icons: HashMap<IconKey, slint::Image>,
    visible: bool,
    dark: bool,
    hwnd_ready: bool,
    launching: bool,
    first_sequence: Option<u64>,
}

impl Bar {
    fn window_ready(&mut self) {
        self.hwnd_ready = true;
        if let Some(hwnd) = self.hwnd()
            && !bar_win::remove_caption_buttons(hwnd)
        {
            self.store
                .log_warn("搜索条样式子类没有装上，显示时可能出现标题栏按钮");
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

    fn refresh_backdrop(&mut self) {
        let actual = match self.hwnd() {
            Some(hwnd) => bar_win::apply_dwm(hwnd, bar_win::current_backdrop(), self.dark),
            None => Backdrop::Solid,
        };
        self.ui.set_acrylic(actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(self.dark)));
    }

    fn show(&mut self, received_ns: u64) {
        let (work, dpi) = bar_win::cursor_monitor().unwrap_or((
            WorkArea {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            lanwork_core::shell::BASE_DPI,
        ));
        let origin = bar_origin(work, to_physical(BAR_WIDTH, dpi));
        let room = max_results_height(work, dpi, INPUT_HEIGHT, BOTTOM_GAP);
        self.ui.set_max_list_height(px(room.min(LIST_CAP)));
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
        if let Some(hwnd) = self.hwnd() {
            // 跨 DPI 移动时系统建议的矩形可能偏离目标位置，显示后再对一次。
            if bar_win::window_origin(hwnd) != Some(origin) {
                bar_win::move_window(hwnd, origin.0, origin.1);
            }
            bar_win::bring_to_front(hwnd);
        }
        self.ui.invoke_focus_input();
    }

    fn hide(&mut self) {
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
        if self.visible {
            self.hide();
        }
    }

    fn edited(&mut self, text: &str) {
        self.error = None;
        self.submit(text);
    }

    fn escape(&mut self) {
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
        self.selected = reselect(previous.as_ref(), same_text, &view.rows);
        self.results = view.rows;
        self.phase = view.phase;
        self.view_text = view.text;
        self.sequence = view.sequence;
        self.file = view.file;
        self.file_unavailable = view.file_unavailable;
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
        }
        self.ui.invoke_ensure_visible();
        self.marks.borrow_mut().result = self
            .sequence
            .map(|seq| (seq, self.file == FileProgress::Settled));
    }

    fn push_rows(&mut self) {
        let groups: Vec<SearchGroup> = self.results.iter().map(|row| row.group).collect();
        let (offsets, height) = row_offsets(&groups);
        let rows: Vec<BarRow> = self
            .results
            .iter()
            .zip(offsets)
            .enumerate()
            .map(|(index, (row, y))| {
                let icon = row.icon.as_ref().and_then(|key| self.icons.get(key));
                BarRow {
                    label: row.label.clone().into(),
                    kind: group_label(row.group).into(),
                    location: row.location.clone().into(),
                    icon: icon.cloned().unwrap_or_default(),
                    has_icon: icon.is_some(),
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

    fn hover(&mut self, index: i32) {
        if let Some(index) = pointed_row(index, self.results.len()) {
            self.selected = index;
            self.ui
                .set_selected(i32::try_from(self.selected).unwrap_or(0));
        }
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
        if !self.ui.get_preedit().is_empty() || self.launching {
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
        match action {
            RowAction::Shell(_) | RowAction::Elevated(_) | RowAction::OpenFolder(_) => {
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
            Err(err) => {
                self.store
                    .log_warn(&format!("搜索结果打开失败：{}", err.message));
                if self.visible {
                    self.error = Some(err.message);
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

/// 面板（#23）和便签悬浮窗（#26）还没有。这里只占住调用点，不显示任何东西。
fn hand_off(action: RowAction) {
    let _ = action;
}

fn run_shell(action: &RowAction) -> Result<(), LaunchError> {
    match action {
        RowAction::Shell(target) => launch(target),
        RowAction::Elevated(target) => launch_elevated(target),
        RowAction::OpenFolder(folder) => launch(&path_target(&folder.to_string_lossy())),
        RowAction::PanelTodo { .. } | RowAction::PanelNote { .. } | RowAction::FloatNote { .. } => {
            Ok(())
        }
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

fn post(view: Option<QueryView>, icons: Vec<(IconKey, RgbaImage)>) {
    if view.is_none() && icons.is_empty() {
        return;
    }
    let _ = slint::invoke_from_event_loop(move || {
        with_bar(move |bar| bar.receive(view, icons));
    });
}
