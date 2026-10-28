//! 应用墙：搜索条右侧按钮打开的全屏本机应用网格。
//!
//! 列表来自 [`Dispatch::wall_apps`]，只含开始菜单来源里未隐藏的条目。
//! 打开时搜索条窗口隐藏，应用墙按钮随这扇窗口离开屏幕。墙上空白处的点击留在墙上。
//! 右键菜单和启动与搜索条的应用结果共用 `app_menu` 和 Shell。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lanwork_core::apps::LaunchError;
use lanwork_core::dispatch::{Dispatch, IconCache, IconKey, RgbaImage, icon_of};
use lanwork_core::shell::{AltRoute, WallGrid};
use lanwork_core::shell::{
    AppMenuAction, Backdrop, MonitorArea, WorkArea, alt_route, app_menu, app_menu_command, grid,
    monitor_at, pointed_row, solid_rgb, tile_xy, to_logical, visible_indices,
};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle, Model as _, ModelRc, VecModel};
use windows::Win32::Foundation::HWND;

use crate::bar_win;
use crate::{AppWall, BarMenuItem, WallTile};

thread_local! {
    static WALL: RefCell<Option<Wall>> = const { RefCell::new(None) };
}

fn with_wall(action: impl FnOnce(&mut Wall) + 'static) {
    let deferred = WALL.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => {
            if let Some(wall) = slot.as_mut() {
                action(wall);
            }
            None
        }
        Err(_) => Some(action),
    });
    if let Some(action) = deferred {
        slint::Timer::single_shot(Duration::ZERO, move || with_wall(action));
    }
}

/// 启动时和搜索条一起创建。窗口保持隐藏，直到搜索条上的按钮。
pub(crate) fn install(ui: AppWall, dispatch: Arc<Dispatch>, store: lanwork_core::storage::Store) {
    let tiles = Rc::new(VecModel::<WallTile>::default());
    ui.set_tiles(ModelRc::from(Rc::clone(&tiles)));
    ui.set_menu_items(ModelRc::new(VecModel::<BarMenuItem>::default()));
    let icons = IconPump::start(dispatch.icon_cache(), store.clone());

    ui.on_edited(|text| {
        let text = text.to_string();
        with_wall(move |wall| wall.show_filter(&text));
    });
    ui.on_escape(|| with_wall(Wall::escape));
    ui.on_activate(|index| with_wall(move |wall| wall.activate(index)));
    ui.on_open_menu(|index, x, y| with_wall(move |wall| wall.open_menu(index, x, y)));
    ui.on_menu_activate(|index| with_wall(move |wall| wall.menu_activate(index)));
    ui.on_menu_shortcut(|digit| with_wall(move |wall| wall.menu_shortcut(digit)));
    ui.on_view_changed(|scroll, viewport| {
        with_wall(move |wall| wall.view_changed(scroll, viewport));
    });

    ui.window().on_winit_window_event(|_, event| {
        if matches!(event, winit::event::WindowEvent::Focused(false)) {
            with_wall(Wall::blur);
        }
        EventResult::Propagate
    });
    // Alt+F4 只收起应用墙，回到搜索条，不结束进程。
    ui.window().on_close_requested(|| {
        with_wall(Wall::dismiss_to_bar);
        CloseRequestResponse::KeepWindowShown
    });

    let ready = ui.clone_strong();
    let spawned = slint::spawn_local(async move {
        let window = ready.window().winit_window().await;
        if let Ok(window) = window {
            use winit::platform::windows::WindowExtWindows as _;
            window.set_skip_taskbar(true);
        }
        with_wall(Wall::window_ready);
    });
    if let Err(err) = spawned {
        store.log_warn(&format!("应用墙窗口初始化未排队：{err}"));
    }

    WALL.with(|cell| {
        *cell.borrow_mut() = Some(Wall {
            ui,
            dispatch,
            store,
            tiles,
            entries: Vec::new(),
            icons: HashMap::new(),
            failed: HashSet::new(),
            inflight: HashSet::new(),
            icon_pump: icons,
            grid: grid(1, 0),
            width: 1,
            scroll: 0,
            viewport: 0,
            open: false,
            dark: false,
            hwnd_ready: false,
            launching: false,
            menu_index: None,
            notice: None,
        });
    });
}

pub(crate) fn apply_theme(dark: bool) {
    with_wall(move |wall| {
        wall.dark = dark;
        wall.ui.set_dark(dark);
        wall.refresh_backdrop();
    });
}

/// 盖住锚点所在的显示器。锚点是搜索条窗口的屏幕左上角。
pub(crate) fn open(anchor: Option<(i32, i32)>) {
    with_wall(move |wall| wall.open_on(anchor));
}

/// 应用墙开着，并且前台就是这扇窗口。句柄还没有时为假。
pub(crate) fn has_foreground() -> bool {
    WALL.with(|cell| {
        let Ok(slot) = cell.try_borrow() else {
            return false;
        };
        let Some(wall) = slot.as_ref() else {
            return false;
        };
        if !wall.open {
            return false;
        }
        wall.hwnd()
            .is_some_and(|hwnd| bar_win::foreground_root() == Some(hwnd))
    })
}

/// 收起应用墙，不收起搜索条。
pub(crate) fn dismiss() {
    with_wall(Wall::dismiss);
}

pub(crate) fn is_open() -> bool {
    WALL.with(|cell| cell.borrow().as_ref().is_some_and(|wall| wall.open))
}

/// 应用墙开着，并且前台仍是应用墙或搜索条。句柄还没有时先留着，避免刚打开就被失焦收起。
pub(crate) fn focus_stays() -> bool {
    if !is_open() {
        return false;
    }
    let foreground = bar_win::foreground_root();
    holds_foreground(foreground) || foreground == crate::searchbar::bar_hwnd()
}

/// 前台是这块应用墙。句柄还没有时先算在内。借不到状态时也先留着。
pub(crate) fn holds_foreground(foreground: Option<HWND>) -> bool {
    WALL.with(|cell| {
        let Ok(slot) = cell.try_borrow() else {
            return true;
        };
        let Some(wall) = slot.as_ref() else {
            return false;
        };
        if !wall.open {
            return false;
        }
        match wall.hwnd() {
            Some(hwnd) => foreground == Some(hwnd),
            None => true,
        }
    })
}

/// 隐藏名单变了之后，开着的应用墙按当前过滤再列一次。
pub(crate) fn reload() {
    with_wall(|wall| {
        if !wall.open {
            return;
        }
        let text = wall.ui.get_text().to_string();
        wall.publish(wall.dispatch.wall_apps(&text));
    });
}

struct Wall {
    ui: AppWall,
    dispatch: Arc<Dispatch>,
    store: lanwork_core::storage::Store,
    tiles: Rc<VecModel<WallTile>>,
    entries: Vec<lanwork_core::apps::WallEntry>,
    icons: HashMap<IconKey, slint::Image>,
    failed: HashSet<IconKey>,
    inflight: HashSet<IconKey>,
    icon_pump: IconPump,
    grid: WallGrid,
    width: u32,
    scroll: u32,
    viewport: u32,
    open: bool,
    dark: bool,
    hwnd_ready: bool,
    launching: bool,
    menu_index: Option<usize>,
    notice: Option<String>,
}

struct Screen {
    bounds: WorkArea,
    dpi: u32,
}

impl Wall {
    fn window_ready(&mut self) {
        self.hwnd_ready = true;
        if let Some(hwnd) = self.hwnd()
            && !bar_win::remove_caption_buttons(hwnd)
        {
            self.store
                .log_warn("应用墙样式子类没有装上，显示时可能出现标题栏按钮");
        }
        self.refresh_backdrop();
    }

    fn hwnd(&self) -> Option<HWND> {
        if self.hwnd_ready {
            bar_win::hwnd_of(self.ui.window())
        } else {
            None
        }
    }

    fn refresh_backdrop(&mut self) {
        let actual = match self.hwnd() {
            Some(hwnd) => {
                let actual =
                    bar_win::apply_float_backdrop(hwnd, bar_win::current_backdrop(), self.dark);
                // 全屏四角若是圆的，会露出桌面。悬浮窗那一层亚克力在失焦时仍在，
                // 搜索条拿到焦点时墙不会变成一块不透明的纯色。
                bar_win::square_corners(hwnd);
                actual
            }
            None => Backdrop::Solid,
        };
        self.ui.set_acrylic(actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(self.dark)));
    }

    fn open_on(&mut self, anchor: Option<(i32, i32)>) {
        self.open = true;
        self.notice = None;
        self.push_notice();
        self.close_menu();
        self.ui.set_text("".into());
        let screen = screen_at(anchor);
        self.place(&screen);
        self.refresh_backdrop();
        if let Err(err) = self.ui.show() {
            self.open = false;
            self.store.log_warn(&format!("应用墙没有显示：{err}"));
            crate::searchbar::set_wall_open(false);
            return;
        }
        if let Some(hwnd) = self.hwnd() {
            bar_win::place_rect(hwnd, screen.bounds);
            bar_win::square_corners(hwnd);
            match bar_win::bring_to_front(hwnd) {
                bar_win::ForegroundTake::Plain => {}
                bar_win::ForegroundTake::AfterInput => {
                    self.store.log_info("应用墙在注入输入后到前台");
                }
                bar_win::ForegroundTake::Failed => {
                    self.store.log_warn("应用墙没有到前台");
                }
            }
        }
        self.width = to_logical(screen.bounds.width(), screen.dpi).max(1);
        self.scroll = 0;
        self.viewport = 0;
        self.ui.invoke_scroll_to_top();
        self.publish(self.dispatch.wall_apps(""));
        self.ui.invoke_focus_filter();
        slint::Timer::single_shot(Duration::ZERO, || {
            with_wall(|wall| {
                if !wall.open {
                    return;
                }
                wall.relayout_from_window();
                wall.ui.invoke_report_view();
            });
        });
    }

    fn place(&self, screen: &Screen) {
        let width = u32::try_from(screen.bounds.width()).unwrap_or(1).max(1);
        let height = u32::try_from(screen.bounds.height()).unwrap_or(1).max(1);
        self.ui.window().set_position(slint::PhysicalPosition::new(
            screen.bounds.left,
            screen.bounds.top,
        ));
        self.ui
            .window()
            .set_size(slint::PhysicalSize::new(width, height));
    }

    fn dismiss(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.close_menu();
        let _ = self.ui.hide();
        crate::searchbar::set_wall_open(false);
    }

    /// Alt+F4。收起墙并把焦点还给搜索条。
    fn dismiss_to_bar(&mut self) {
        let was_open = self.open;
        self.dismiss();
        if was_open {
            crate::searchbar::focus_input();
        }
    }

    fn escape(&mut self) {
        if self.ui.get_menu_open() {
            self.close_menu();
            return;
        }
        self.dismiss_to_bar();
    }

    fn blur(&mut self) {
        if !self.open {
            return;
        }
        slint::Timer::single_shot(Duration::ZERO, || {
            if !is_open() || focus_stays() {
                return;
            }
            crate::searchbar::hide_if_visible();
        });
    }

    fn show_filter(&mut self, text: &str) {
        if !self.open {
            return;
        }
        self.notice = None;
        self.push_notice();
        self.close_menu();
        self.scroll = 0;
        self.ui.invoke_scroll_to_top();
        self.publish(self.dispatch.wall_apps(text));
    }

    fn publish(&mut self, entries: Vec<lanwork_core::apps::WallEntry>) {
        self.entries = entries;
        let live: HashSet<IconKey> = self
            .entries
            .iter()
            .filter_map(|entry| icon_of(&entry.entry))
            .collect();
        self.icons.retain(|key, _| live.contains(key));
        self.failed.retain(|key| live.contains(key));
        self.inflight.retain(|key| live.contains(key));
        self.push_tiles();
        self.load_icons();
    }

    fn relayout_from_window(&mut self) {
        let size = self.ui.window().size();
        let scale = self.ui.window().scale_factor();
        if size.width == 0 || scale <= 0.0 {
            return;
        }
        let width = px_round(size.width as f32 / scale).max(1);
        if width == self.width {
            self.load_icons();
            return;
        }
        self.width = width;
        self.push_tiles();
        self.load_icons();
    }

    fn push_tiles(&mut self) {
        let laid = grid(self.width.max(1), self.entries.len());
        self.grid = laid;
        self.ui.set_cell_width(px(laid.cell_w));
        self.ui.set_cell_height(px(laid.cell_h));
        self.ui.set_content_height(px(laid.content_height));
        let tiles: Vec<WallTile> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let (x, y) = tile_xy(&laid, index);
                let key = icon_of(&entry.entry);
                let image = key.as_ref().and_then(|key| self.icons.get(key));
                WallTile {
                    name: entry.entry.name.as_str().into(),
                    icon: image.cloned().unwrap_or_default(),
                    has_icon: image.is_some(),
                    x: px(x),
                    y: px(y),
                }
            })
            .collect();
        if tiles.len() == self.tiles.row_count() {
            for (index, tile) in tiles.into_iter().enumerate() {
                if self.tiles.row_data(index).as_ref() != Some(&tile) {
                    self.tiles.set_row_data(index, tile);
                }
            }
        } else {
            self.tiles.set_vec(tiles);
        }
    }

    fn push_notice(&mut self) {
        self.ui
            .set_notice(self.notice.clone().unwrap_or_default().into());
    }

    fn view_changed(&mut self, scroll: f32, viewport: f32) {
        if !self.open {
            return;
        }
        self.scroll = px_round(scroll);
        self.viewport = px_round(viewport);
        self.load_icons();
    }

    fn load_icons(&mut self) {
        let viewport = if self.viewport == 0 {
            900
        } else {
            self.viewport
        };
        let range = visible_indices(&self.grid, self.entries.len(), self.scroll, viewport);
        let mut needed = HashSet::new();
        if let Some(visible) = self.entries.get(range) {
            for entry in visible {
                let Some(key) = icon_of(&entry.entry) else {
                    continue;
                };
                if self.icons.contains_key(&key) || self.failed.contains(&key) {
                    continue;
                }
                needed.insert(key);
            }
        }
        // 工作线程只留最后一批。这一批必须是当前视口里还缺的全部键，
        // in-flight 也换成这一批，被丢掉的旧键才不会一直占着。
        let Some(keys) = replace_inflight(&mut self.inflight, needed) else {
            return;
        };
        if !self.icon_pump.request(keys) {
            self.inflight.clear();
        }
    }

    fn receive_icons(&mut self, requested: Vec<IconKey>, loaded: Vec<(IconKey, RgbaImage)>) {
        // 墙已经收起时也要清掉，否则这些键一直占着，下次打开取不到图标。
        for key in &requested {
            self.inflight.remove(key);
        }
        if !self.open {
            return;
        }
        let found: HashSet<&IconKey> = loaded.iter().map(|(key, _)| key).collect();
        for key in requested {
            if !found.contains(&key) {
                self.failed.insert(key);
            }
        }
        for (key, image) in loaded {
            self.failed.remove(&key);
            self.icons.insert(key, to_image(&image));
        }
        self.push_tiles();
    }

    fn activate(&mut self, index: i32) {
        if self.launching || self.ui.get_menu_open() {
            return;
        }
        let Some(index) = pointed_row(index, self.entries.len()) else {
            return;
        };
        let target = self.entries[index].entry.target.clone();
        self.launch(lanwork_core::shell::RowAction::Shell(target));
    }

    fn open_menu(&mut self, index: i32, x: f32, y: f32) {
        if self.launching {
            return;
        }
        let Some(index) = pointed_row(index, self.entries.len()) else {
            return;
        };
        let items: Vec<BarMenuItem> = app_menu(&self.entries[index].entry)
            .into_iter()
            .map(|item| BarMenuItem {
                label: item.label.into(),
                hint: item.hint.into(),
                enabled: item.enabled,
            })
            .collect();
        self.menu_index = Some(index);
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

    fn menu_shortcut(&mut self, digit: i32) {
        let Ok(digit) = u32::try_from(digit) else {
            return;
        };
        if let AltRoute::Menu(Some(action)) = alt_route(true, digit) {
            self.run_menu(action);
        }
    }

    fn run_menu(&mut self, action: AppMenuAction) {
        if self.launching {
            return;
        }
        let Some(index) = self.menu_index else {
            return;
        };
        let Some(entry) = self.entries.get(index) else {
            self.close_menu();
            return;
        };
        let Some(command) = app_menu_command(&entry.entry, action) else {
            return;
        };
        self.close_menu();
        match command {
            lanwork_core::shell::RowAction::HideApp { key } => self.hide_entry(&key),
            other => self.launch(other),
        }
    }

    fn close_menu(&mut self) {
        self.menu_index = None;
        self.ui.set_menu_open(false);
    }

    fn hide_entry(&mut self, key: &str) {
        match self.dispatch.hide_app(key) {
            Ok(()) => {
                self.notice = None;
                self.push_notice();
                let text = self.ui.get_text().to_string();
                self.publish(self.dispatch.wall_apps(&text));
                crate::panel::refresh_hidden_apps();
                crate::searchbar::requery();
            }
            Err(message) => {
                self.store
                    .log_warn(&format!("应用墙隐藏没有写入：{message}"));
                self.notice = Some(message);
                self.push_notice();
            }
        }
    }

    fn launch(&mut self, action: lanwork_core::shell::RowAction) {
        if self.launching {
            return;
        }
        self.launching = true;
        let spawned = std::thread::Builder::new()
            .name("lanwork-open".into())
            .spawn(move || {
                let result = crate::searchbar::run_shell(&action);
                let _ = slint::invoke_from_event_loop(move || {
                    with_wall(move |wall| wall.launched(result));
                });
            });
        if let Err(err) = spawned {
            self.launching = false;
            self.notice = Some(format!("打开失败：{err}"));
            self.push_notice();
        }
    }

    fn launched(&mut self, result: Result<(), LaunchError>) {
        self.launching = false;
        if !self.open {
            return;
        }
        match result {
            Ok(()) => {
                slint::Timer::single_shot(Duration::ZERO, crate::searchbar::hide_if_visible);
            }
            Err(err) if err.cancelled => {}
            Err(err) => {
                let message = err.message;
                self.store.log_warn(&format!("应用墙打开失败：{message}"));
                self.notice = Some(message);
                self.push_notice();
            }
        }
    }
}

fn screen_at(anchor: Option<(i32, i32)>) -> Screen {
    let point = anchor.or_else(bar_win::cursor_screen).unwrap_or((0, 0));
    let readings = bar_win::monitors();
    let areas: Vec<MonitorArea> = readings
        .iter()
        .map(|reading| MonitorArea {
            bounds: reading.bounds,
            work: reading.work,
        })
        .collect();
    if let Some(index) = monitor_at(&areas, point.0, point.1)
        && let Some(reading) = readings.get(index)
    {
        return Screen {
            bounds: reading.bounds,
            dpi: reading.dpi,
        };
    }
    Screen {
        bounds: WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        },
        dpi: lanwork_core::shell::BASE_DPI,
    }
}

fn receive_icons(requested: Vec<IconKey>, loaded: Vec<(IconKey, RgbaImage)>) {
    with_wall(move |wall| wall.receive_icons(requested, loaded));
}

struct IconPump {
    tx: Option<Sender<Vec<IconKey>>>,
}

impl IconPump {
    fn start(cache: Arc<Mutex<IconCache>>, store: lanwork_core::storage::Store) -> Self {
        let (tx, rx) = mpsc::channel();
        let log = store.clone();
        let spawned = std::thread::Builder::new()
            .name("lanwork-wall".into())
            .spawn(move || icon_main(cache, store, rx));
        match spawned {
            Ok(_) => Self { tx: Some(tx) },
            Err(err) => {
                log.log_warn(&format!("应用墙图标线程没有启动：{err}"));
                Self { tx: None }
            }
        }
    }

    fn request(&self, keys: Vec<IconKey>) -> bool {
        if keys.is_empty() {
            return true;
        }
        self.tx.as_ref().is_some_and(|tx| tx.send(keys).is_ok())
    }
}

/// 视口没变就不重复发。变了就把 in-flight 换成新的一整组，并返回要发给工作线程的键。
fn replace_inflight(
    inflight: &mut HashSet<IconKey>,
    needed: HashSet<IconKey>,
) -> Option<Vec<IconKey>> {
    if needed == *inflight {
        return None;
    }
    let keys = needed.iter().cloned().collect();
    *inflight = needed;
    Some(keys)
}

fn icon_main(
    cache: Arc<Mutex<IconCache>>,
    store: lanwork_core::storage::Store,
    rx: Receiver<Vec<IconKey>>,
) {
    while let Ok(mut requested) = rx.recv() {
        // 通道里新的一批已经是那时的整个视口，旧的一批丢掉。
        while let Ok(newer) = rx.try_recv() {
            requested = newer;
        }
        let loaded = {
            let mut cache = lock_cache(&cache);
            let mut loaded = Vec::new();
            for key in &requested {
                if let Some(image) = cache.get(key) {
                    loaded.push((key.clone(), image));
                }
            }
            loaded
        };
        let requested_for_ui = requested;
        if let Err(err) = slint::invoke_from_event_loop(move || {
            receive_icons(requested_for_ui, loaded);
        }) {
            store.log_warn(&format!("应用墙图标没有交回界面：{err}"));
        }
    }
}

fn lock_cache(cache: &Mutex<IconCache>) -> std::sync::MutexGuard<'_, IconCache> {
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn px_round(value: f32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let rounded = value.round();
    if rounded >= u32::MAX as f32 {
        u32::MAX
    } else {
        rounded as u32
    }
}

#[allow(clippy::cast_precision_loss)]
fn px(value: u32) -> f32 {
    value as f32
}

fn rgb(value: u32) -> slint::Color {
    let [_, red, green, blue] = value.to_be_bytes();
    slint::Color::from_rgb_u8(red, green, blue)
}

fn to_image(image: &RgbaImage) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &image.pixels,
        image.width,
        image.height,
    );
    slint::Image::from_rgba8(buffer)
}

#[cfg(test)]
mod tests {
    use super::replace_inflight;
    use lanwork_core::dispatch::IconKey;
    use std::collections::HashSet;

    fn key(path: &str) -> IconKey {
        IconKey {
            path: path.to_owned(),
            index: 0,
        }
    }

    #[test]
    fn a_new_visible_set_replaces_keys_from_the_dropped_batch() {
        let mut inflight = HashSet::new();
        let first = replace_inflight(&mut inflight, HashSet::from([key("a"), key("b")]))
            .expect("first batch");
        assert_eq!(first.len(), 2);
        assert!(inflight.contains(&key("a")));

        let second = replace_inflight(&mut inflight, HashSet::from([key("b"), key("c")]))
            .expect("scrolled batch");
        assert_eq!(second.len(), 2);
        assert!(
            !inflight.contains(&key("a")),
            "a key from the dropped batch stayed in-flight"
        );
        assert!(inflight.contains(&key("b")));
        assert!(inflight.contains(&key("c")));

        assert!(replace_inflight(&mut inflight, HashSet::from([key("c"), key("b")])).is_none());
    }
}
