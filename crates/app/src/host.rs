//! 启动顺序：先占住单实例，再打开数据目录，完成导入恢复和待办加载，加载配置，按本地日期自动备份，然后选渲染器、注册热键、创建隐藏窗口和托盘。
//! 便签服务、收纳服务、应用索引和文件索引在存储启动之后打开；应用索引和文件索引交给搜索条的查询调度。
//!
//! 面板在 `panel`，设置页在 `settings_page`，搜索条在 `searchbar`，热角轮询在 `hotcorner`。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lanwork_core::CivilDate;
use lanwork_core::apps::{AppIndex, load_user_catalog};
use lanwork_core::backup::BackupCommands;
use lanwork_core::config::{ConfigCommands, Fallback, Theme};
use lanwork_core::dispatch::{Dispatch, IconCache, Services};
use lanwork_core::files::FileCommands;
use lanwork_core::notes::NoteCommands;
use lanwork_core::panel::PanelTab;
use lanwork_core::shell::{
    HotkeyRole, QuitDecision, QuitGate, ShellCommand, SystemLight, TRAY_ICON_PX, quit_decision,
    resolve_theme, resume_quit_later, tray_icon_marked,
};
use lanwork_core::shelves::ShelfCommands;
use lanwork_core::storage::{
    BootHooks, EntityKind, Error as StorageError, Log, LogSettings, Store, resolve_from_process,
};
use lanwork_core::todos::TodoCommands;
use lanwork_core::update::{Edition, edition_of};
use slint::ComponentHandle;
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::float_group;
use crate::instance::{self, Claim};
use crate::note_float;
use crate::panel;
use crate::platform::{HotkeyControl, Platform};
use crate::registry::{apply_startup, read_system_theme};
use crate::searchbar;
use crate::settings_page::SettingsServices;
use crate::shelf_float;
use crate::todo_float;
use crate::{Panel, SearchBar, Tray};

pub(crate) fn run() -> i32 {
    let claim = match instance::claim(instance::MUTEX_NAME, instance::ACTIVATE_NAME) {
        Ok(claim) => claim,
        Err(message) => {
            // 互斥量失败时存储还没打开。能解析到数据目录就写入 logs/app.log。
            report_startup_error(&message, resolved_data_dir().as_deref());
            return 1;
        }
    };
    let Claim::Primary(primary) = claim else {
        return 0;
    };
    let paths = match resolve_from_process() {
        Ok(paths) => paths,
        Err(err) => {
            // 数据目录解析失败时还没有日志文件。发布版也没有控制台。
            eprintln!("{err}");
            return 1;
        }
    };
    let data_dir = paths.data_dir.clone();
    let store = match Store::open(paths.into()) {
        Ok(store) => store,
        Err(err) => {
            let message = err.to_string();
            // 拒绝 MayDolist 时不能在那个目录下创建 logs。
            let log_dir = if matches!(err, StorageError::MayDolist { .. }) {
                None
            } else {
                Some(data_dir.as_path())
            };
            report_startup_error(&message, log_dir);
            return 1;
        }
    };
    if let Err(message) = run_primary(&store, primary) {
        store.log_error(&message);
        eprintln!("{message}");
        return 1;
    }
    0
}

/// 启动失败写入 `logs/app.log`。`eprintln` 只留给调试构建的控制台，发布版没有控制台接收它。
fn report_startup_error(message: &str, data_dir: Option<&std::path::Path>) {
    if let Some(dir) = data_dir {
        let path = dir.join("logs").join("app.log");
        if let Ok(log) = Log::open(path, LogSettings::default()) {
            log.error(message);
        }
    }
    eprintln!("{message}");
}

fn resolved_data_dir() -> Option<std::path::PathBuf> {
    resolve_from_process().ok().map(|paths| paths.data_dir)
}

fn install_log_panic_hook(store: &Store) {
    let store = store.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        store.log_error(&format!("panic: {info}"));
        previous(info);
    }));
}

fn run_primary(store: &Store, primary: instance::Primary) -> Result<(), String> {
    install_log_panic_hook(store);
    let todos = TodoCommands::open(store.clone());
    let mut hooks = BootHooks::new();
    BackupCommands::register_boot_hooks(&mut hooks);
    todos.register_boot_hooks(&mut hooks);
    let report = store.boot(hooks).map_err(|err| err.to_string())?;
    IMPORT_RECOVERED.store(report.import_recovered, Ordering::Release);
    if import_was_recovered() {
        store.log_warn("导入未完成");
    }
    let config = ConfigCommands::open(store.clone()).map_err(|err| err.to_string())?;
    match config.fallback() {
        Some(Fallback::Quarantined) => {
            store.log_warn("config.json 无法解析，已隔离，本次使用默认配置");
        }
        Some(Fallback::Invalid) => {
            store.log_warn("config.json 字段不合法，本次使用默认配置，不覆盖原文件");
        }
        Some(Fallback::Missing) | None => {}
    }
    backup_if_due(store);
    select_renderer()?;
    let installed = match std::env::current_exe() {
        Ok(path) => path
            .parent()
            .is_some_and(|dir| edition_of(dir) == Edition::Installed),
        Err(_) => {
            store.log_warn("读不到可执行文件路径，按便携版处理通知");
            false
        }
    };
    let platform = Platform::start(primary, installed)?;
    if let Some(err) = crate::platform::take_activator_error() {
        store.log_warn(&format!("通知激活没有注册：{err}"));
    }
    let hotkeys = platform.control();
    let unregistered = register_configured_hotkeys(&hotkeys, &config, store);
    match std::env::current_exe() {
        Ok(executable) => {
            if let Err(err) = apply_startup(config.current().launch_at_startup, &executable) {
                store.log_warn(&format!("开机启动项未更新：{err}"));
            }
        }
        Err(_) => store.log_warn("读不到可执行文件路径，开机启动项未更新"),
    }

    let search = SearchBar::new().map_err(|err| err.to_string())?;
    let panel_ui = Panel::new().map_err(|err| err.to_string())?;
    let tray = Tray::new().map_err(|err| err.to_string())?;
    let notes = open_notes(store);
    if let Some(notes) = &notes {
        note_float::install(notes.clone(), store.clone());
    }
    let shelves = open_shelves(store, &todos);
    let dispatch = notes
        .clone()
        .and_then(|notes| open_dispatch(store, &todos, notes, shelves.clone()));
    let mut icons = None;
    if let Some(dispatch) = &dispatch {
        dispatch.set_web_search_engine(config.current().web_search_engine);
        icons = Some(dispatch.icon_cache());
        searchbar::install(
            search.clone_strong(),
            Arc::clone(dispatch),
            store.clone(),
            config.clone(),
        );
    }
    let icons = icons.unwrap_or_else(|| Arc::new(Mutex::new(IconCache::shell())));
    crate::web_icons::install(&store.data_dir());
    if let Some(shelves) = &shelves {
        shelf_float::install(
            shelves.clone(),
            todos.clone(),
            Arc::clone(&icons),
            store.clone(),
        );
    }
    todo_float::install(todos.clone(), store.clone());
    float_group::install(notes.clone(), shelves.clone(), todos.clone(), store.clone());
    panel::install(
        panel_ui.clone_strong(),
        todos.clone(),
        notes,
        shelves,
        icons,
        SettingsServices {
            config: config.clone(),
            hotkeys,
            unregistered,
            apps: dispatch,
        },
        store.clone(),
    );
    refresh_theme(&config, store);
    refresh_badge(&tray, &todos, store);
    wire_tray(&tray);
    watch_changes(&tray, &todos, store);
    let config_theme = config.clone();
    let store_theme = store.clone();
    platform.set_theme_callback(Box::new(move || {
        refresh_theme(&config_theme, &store_theme);
    }));
    if let Err(err) = crate::hotcorner::start(config.current().hot_corner, Some(store.clone())) {
        store.log_warn(&format!("热角轮询没有启动：{err}"));
    }
    crate::update::start(store.clone());
    crate::remind::install(todos.clone(), config.clone(), store.clone(), installed);

    // 搜索条收起时所有窗口都隐藏，事件循环不能因此结束。
    let run = slint::run_event_loop_until_quit().map_err(|err| err.to_string());
    panel::end_hotkey_recording();
    crate::hotcorner::shutdown();
    searchbar::shutdown();
    drop(search);
    drop(panel_ui);
    platform.shutdown();
    run
}

/// 便签服务。面板的便签页、悬浮窗和搜索条共用这一份。打不开时写日志，这三处都没有便签。
fn open_notes(store: &Store) -> Option<NoteCommands> {
    match NoteCommands::open(store.clone()) {
        Ok(notes) => Some(notes),
        Err(err) => {
            store.log_error(&format!(
                "便签服务没有打开，便签页、悬浮窗和搜索条不可用：{err}"
            ));
            None
        }
    }
}

/// 收纳服务。打不开时写日志，收纳页显示错误；待办被永久删除时分组的关联由它自动解除。
fn open_shelves(store: &Store, todos: &TodoCommands) -> Option<ShelfCommands> {
    match ShelfCommands::open(store.clone()) {
        Ok(shelves) => {
            shelves.watch_todo_notices(todos);
            Some(shelves)
        }
        Err(err) => {
            store.log_error(&format!("收纳服务没有打开，收纳页不可用：{err}"));
            None
        }
    }
}

/// 应用索引和文件索引，然后建内存索引。失败时写日志，搜索条按热键不出现。
fn open_dispatch(
    store: &Store,
    todos: &TodoCommands,
    notes: NoteCommands,
    shelves: Option<ShelfCommands>,
) -> Option<Arc<Dispatch>> {
    let apps = match AppIndex::open_from_process() {
        Ok(apps) => apps,
        Err(err) => {
            store.log_error(&format!("应用索引没有打开，搜索条不可用：{err}"));
            return None;
        }
    };
    match load_user_catalog(&store.user_apps_path()) {
        Ok(catalog) => {
            if let Err(err) = apps.set_user_catalog(catalog) {
                store.log_warn(&format!("便携应用和别名没有载入：{err}"));
            }
        }
        Err(err) => store.log_warn(&format!("便携应用和别名没有载入：{err}")),
    }
    let dispatch = Dispatch::open(
        store.clone(),
        Services {
            todos: todos.clone(),
            notes,
            shelves,
        },
        apps,
        FileCommands::open(store.clone()),
    );
    if let Err(err) = dispatch.build() {
        store.log_error(&format!("搜索索引没有建起来，搜索条不可用：{err}"));
        return None;
    }
    Some(Arc::new(dispatch))
}

/// 按配置注册热键，逐条进行。注册不上的写日志并返回，设置页把它们留在错误条里。
fn register_configured_hotkeys(
    hotkeys: &HotkeyControl,
    config: &ConfigCommands,
    store: &Store,
) -> Vec<HotkeyRole> {
    let Some(unregistered) = hotkeys.register(config.clone()) else {
        store.log_warn("热键服务没有应答，热键没有注册");
        return Vec::new();
    };
    for role in &unregistered {
        store.log_warn(&format!("{}没有注册上：热键已被占用", role.title()));
    }
    unregistered
}

fn select_renderer() -> Result<(), String> {
    for name in lanwork_core::shell::RENDERER_ORDER {
        if slint::BackendSelector::new()
            .renderer_name(name.to_owned())
            .select()
            .is_ok()
        {
            return Ok(());
        }
    }
    Err("渲染器初始化失败".to_owned())
}

fn wire_tray(tray: &Tray) {
    tray.on_open_panel(|| panel::show(None));
    tray.on_open_settings(|| panel::show(Some(PanelTab::Settings)));
    tray.on_new_note(panel::new_note);
    TRAY.with(|cell| *cell.borrow_mut() = Some(tray.as_weak()));
    tray.on_quit(request_quit);
}

fn watch_changes(tray: &Tray, todos: &TodoCommands, store: &Store) {
    let weak = tray.as_weak();
    let todos = todos.clone();
    let store = store.clone();
    let events = store.subscribe();
    std::thread::spawn(move || {
        while let Ok(event) = events.recv() {
            if event.kind == EntityKind::Config {
                let _ = slint::invoke_from_event_loop(crate::remind::on_external);
                continue;
            }
            if event.kind == EntityKind::Note {
                let id = event.id.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    panel::on_notes_changed();
                    note_float::on_note_changed(&id);
                });
                continue;
            }
            if event.kind == EntityKind::Shelf {
                let _ = slint::invoke_from_event_loop(|| {
                    panel::on_shelves_changed();
                    shelf_float::on_changed();
                });
                continue;
            }
            if event.kind != EntityKind::Todo {
                continue;
            }
            let weak = weak.clone();
            let todos = todos.clone();
            let store = store.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(tray) = weak.upgrade() else {
                    return;
                };
                refresh_badge(&tray, &todos, &store);
                crate::remind::on_external();
                panel::on_todos_changed();
                shelf_float::on_todos_changed();
                todo_float::on_todos_changed();
            });
        }
    });
}

/// 这次启动是否用 `import.pending` 里的备份恢复过。
///
/// 界面以后用它显示「导入未完成」。现在只在启动时记日志。
pub(crate) fn import_was_recovered() -> bool {
    IMPORT_RECOVERED.load(Ordering::Acquire)
}

static IMPORT_RECOVERED: AtomicBool = AtomicBool::new(false);

fn backup_if_due(store: &Store) {
    let Some(today) = local_today() else {
        store.log_warn("读不到本地日期，本次不自动备份");
        return;
    };
    if let Err(err) = BackupCommands::open(store.clone()).backup_on_launch(today, unix_now_ms()) {
        store.log_warn(&format!("自动备份未完成：{err}"));
    }
}

pub(crate) fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub(crate) fn refresh_overdue(todos: &TodoCommands, store: &Store) {
    let tray = TRAY.with(|cell| cell.borrow().as_ref().and_then(slint::Weak::upgrade));
    if let Some(tray) = tray {
        refresh_badge(&tray, todos, store);
    }
}

fn refresh_badge(tray: &Tray, todos: &TodoCommands, store: &Store) {
    let overdue = match local_today() {
        Some(today) => match todos.overdue_count(today) {
            Ok(count) => count,
            Err(err) => {
                store.log_warn(&format!("逾期数量读取失败：{err}"));
                0
            }
        },
        None => {
            store.log_warn("读不到本地日期，不显示逾期数字");
            0
        }
    };
    OVERDUE.store(overdue, Ordering::Release);
    paint_tray(tray);
}

/// 有新版本时在托盘图标右上角加一个小点。逾期徽章仍在右下角。
pub(crate) fn set_update_dot(on: bool) {
    UPDATE_DOT.store(on, Ordering::Release);
    let tray = TRAY.with(|cell| cell.borrow().as_ref().and_then(slint::Weak::upgrade));
    if let Some(tray) = tray {
        paint_tray(&tray);
    }
}

fn paint_tray(tray: &Tray) {
    let overdue = OVERDUE.load(Ordering::Acquire);
    let update = UPDATE_DOT.load(Ordering::Acquire);
    tray.set_badge_icon(tray_image(overdue, update));
}

fn tray_image(overdue: u32, update: bool) -> slint::Image {
    let rgba = tray_icon_marked(overdue, update);
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(TRAY_ICON_PX, TRAY_ICON_PX);
    for (pixel, chunk) in buffer
        .make_mut_slice()
        .iter_mut()
        .zip(rgba.as_chunks::<4>().0)
    {
        *pixel = slint::Rgba8Pixel {
            r: chunk[0],
            g: chunk[1],
            b: chunk[2],
            a: chunk[3],
        };
    }
    slint::Image::from_rgba8(buffer)
}

pub(crate) fn refresh_theme(config: &ConfigCommands, store: &Store) {
    let system = read_system_theme();
    if config.current().theme == Theme::System && system == SystemLight::Unknown {
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::AcqRel) {
            store.log_warn("读不到 AppsUseLightTheme，不猜测主题");
        }
    }
    let Some(dark) = resolve_theme(config.current().theme, system).is_dark() else {
        return;
    };
    searchbar::apply_theme(dark);
    panel::apply_theme(dark);
    note_float::apply_theme(dark);
    shelf_float::apply_theme(dark);
    todo_float::apply_theme(dark);
}

pub(crate) fn local_today() -> Option<CivilDate> {
    // SAFETY: GetLocalTime 只读系统时钟，没有输出缓冲区。
    let time = unsafe { GetLocalTime() };
    CivilDate::try_from_ymd(
        i32::from(time.wYear),
        u8::try_from(time.wMonth).ok()?,
        u8::try_from(time.wDay).ok()?,
    )
}

thread_local! {
    static TRAY: RefCell<Option<slint::Weak<Tray>>> = const { RefCell::new(None) };
}

static QUIT: QuitGate = QuitGate::new();
static OVERDUE: AtomicU32 = AtomicU32::new(0);
static UPDATE_DOT: AtomicBool = AtomicBool::new(false);

/// 先保存便签窗口里没写盘的修改。都写成了才结束进程；有写不成的，留下，等用户在那个窗口里
/// 重试、放弃修改或选定冲突版本，之后由 [`continue_quit`] 接着退出。
/// 已经核对过的安装包会在真正退出前交给助手。
pub(crate) fn request_quit() {
    QUIT.request();
    attempt_quit();
}

/// 便签窗口里的保存问题解决之后调用。调用方还借用着窗口表，所以下一轮事件循环才再尝试退出。
pub(crate) fn continue_quit() {
    resume_quit_later(
        &QUIT,
        |task| slint::Timer::single_shot(Duration::ZERO, task),
        attempt_quit,
    );
}

fn attempt_quit() {
    let unsaved = panel::flush_notes_for_quit() + note_float::flush_for_quit();
    match quit_decision(unsaved) {
        QuitDecision::Exit => {
            shelf_float::save_for_quit();
            todo_float::save_for_quit();
            if crate::update::spawn_pending_install().is_err() {
                QUIT.clear();
                crate::update::install_spawn_failed();
                return;
            }
            QUIT.clear();
            let tray = TRAY.with(|cell| cell.borrow().as_ref().and_then(slint::Weak::upgrade));
            if let Some(tray) = tray {
                let _ = tray.hide();
            }
            let _ = slint::quit_event_loop();
        }
        QuitDecision::Stay => {}
    }
}

/// `received_ns` 是平台线程收到热键消息时的单调时间，热召回从这里算起。
pub(crate) fn on_hotkey(role: HotkeyRole, received_ns: u64) {
    match role {
        HotkeyRole::Search => searchbar::toggle(received_ns),
        HotkeyRole::Panel => panel::show(None),
    }
}

pub(crate) fn on_second_instance() {
    pending_visible(ShellCommand::SecondInstance);
}

fn pending_visible(command: ShellCommand) {
    let _ = command;
}
