//! 启动顺序：先占住单实例，再打开数据目录，然后选渲染器、注册热键、创建隐藏窗口和托盘。
//!
//! 搜索条、面板、设置和便签的可见内容不在这里绘制。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lanwork_core::CivilDate;
use lanwork_core::config::{ConfigCommands, Fallback, Theme};
use lanwork_core::github::{GithubCommands, GithubError, ProcessGh, RefreshReport, RepoResult};
use lanwork_core::shell::{
    PANEL_HOTKEY_ID, QuitDecision, SEARCH_HOTKEY_ID, ShellCommand, SystemLight, TRAY_ICON_PX,
    desired_bindings, quit_without_note_editors, resolve_theme, tray_icon_rgba,
};
use lanwork_core::storage::{EntityKind, Store, resolve_from_process};
use lanwork_core::todos::TodoCommands;
use slint::ComponentHandle;
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::instance::{self, Claim};
use crate::platform::Platform;
use crate::registry::{apply_startup, read_system_theme};
use crate::{PanelHost, SearchHost, Tray};

pub(crate) fn run() -> i32 {
    let claim = match instance::claim(instance::MUTEX_NAME, instance::ACTIVATE_NAME) {
        Ok(claim) => claim,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    let Claim::Primary(primary) = claim else {
        return 0;
    };
    let paths = match resolve_from_process() {
        Ok(paths) => paths,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let store = match Store::open(paths.into()) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("{err}");
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

fn run_primary(store: &Store, primary: instance::Primary) -> Result<(), String> {
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
    let todos = TodoCommands::open(store.clone());
    todos.boot().map_err(|err| err.to_string())?;
    select_renderer()?;
    let platform = Platform::start(primary)?;
    match desired_bindings(&config.current()) {
        Ok(next) => {
            if let Err(err) = platform.rebind(next) {
                store.log_warn(&err.to_string());
            }
        }
        Err(err) => store.log_warn(&err.to_string()),
    }
    match std::env::current_exe() {
        Ok(executable) => {
            if let Err(err) = apply_startup(config.current().launch_at_startup, &executable) {
                store.log_warn(&format!("开机启动项未更新：{err}"));
            }
        }
        Err(_) => store.log_warn("读不到可执行文件路径，开机启动项未更新"),
    }

    let search = SearchHost::new().map_err(|err| err.to_string())?;
    let panel = PanelHost::new().map_err(|err| err.to_string())?;
    let tray = Tray::new().map_err(|err| err.to_string())?;
    refresh_theme(&search, &panel, &config, store);
    refresh_badge(&tray, &todos, store);
    wire_tray(&tray, &config, store, &todos);
    watch_todos(&tray, &todos, store);
    let search_weak = search.as_weak();
    let panel_weak = panel.as_weak();
    let config_theme = config.clone();
    let store_theme = store.clone();
    platform.set_theme_callback(Box::new(move || {
        let (Some(search), Some(panel)) = (search_weak.upgrade(), panel_weak.upgrade()) else {
            return;
        };
        refresh_theme(&search, &panel, &config_theme, &store_theme);
    }));

    slint::run_event_loop().map_err(|err| err.to_string())?;
    platform.shutdown();
    Ok(())
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

fn wire_tray(tray: &Tray, config: &ConfigCommands, store: &Store, todos: &TodoCommands) {
    tray.on_open_panel(move || pending_visible(ShellCommand::OpenPanel));
    tray.on_open_settings(move || pending_visible(ShellCommand::OpenSettings));
    tray.on_new_note(move || pending_visible(ShellCommand::NewNote));

    let busy = Arc::new(AtomicBool::new(false));
    let github = GithubCommands::open(store.clone(), Arc::new(ProcessGh::system()), todos.clone());
    let config = config.clone();
    let store = store.clone();
    tray.on_refresh_github(move || {
        if busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let busy = Arc::clone(&busy);
        let github = github.clone();
        let config = config.clone();
        let store = store.clone();
        std::thread::spawn(move || {
            let result = refresh_github(&github, &config);
            busy.store(false, Ordering::Release);
            log_refresh(&store, result);
        });
    });

    let tray_quit = tray.as_weak();
    tray.on_quit(move || request_quit(&tray_quit));
}

fn watch_todos(tray: &Tray, todos: &TodoCommands, store: &Store) {
    let weak = tray.as_weak();
    let todos = todos.clone();
    let store = store.clone();
    let events = store.subscribe();
    std::thread::spawn(move || {
        while let Ok(event) = events.recv() {
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
            });
        }
    });
}

fn refresh_github(
    github: &GithubCommands,
    config: &ConfigCommands,
) -> Result<RefreshReport, GithubError> {
    github.load()?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let settings = config.github_settings();
    github.refresh_all(now_ms, &settings)
}

fn log_refresh(store: &Store, result: Result<RefreshReport, GithubError>) {
    match result {
        Ok(report) => {
            for outcome in report.repos {
                match outcome.result {
                    RepoResult::Unchanged { failure } => {
                        store.log_warn(&format!("刷新 GitHub 未写入快照：{failure}"));
                    }
                    RepoResult::Updated { sync, .. } => {
                        for failure in sync.failures {
                            store.log_warn(&format!("来源同步未完成：{}", failure.message));
                        }
                    }
                }
            }
        }
        Err(err) => store.log_warn(&format!("刷新 GitHub 失败：{err}")),
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
    tray.set_badge_icon(tray_image(overdue));
}

fn tray_image(overdue: u32) -> slint::Image {
    let rgba = tray_icon_rgba(overdue);
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

fn refresh_theme(search: &SearchHost, panel: &PanelHost, config: &ConfigCommands, store: &Store) {
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
    search.invoke_apply_theme(dark);
    panel.invoke_apply_theme(dark);
}

fn local_today() -> Option<CivilDate> {
    // SAFETY: GetLocalTime 只读系统时钟，没有输出缓冲区。
    let time = unsafe { GetLocalTime() };
    CivilDate::try_from_ymd(
        i32::from(time.wYear),
        u8::try_from(time.wMonth).ok()?,
        u8::try_from(time.wDay).ok()?,
    )
}

fn request_quit(tray: &slint::Weak<Tray>) {
    match quit_without_note_editors() {
        QuitDecision::Exit => {
            if let Some(tray) = tray.upgrade() {
                let _ = tray.hide();
            }
            let _ = slint::quit_event_loop();
        }
        QuitDecision::Stay => {}
    }
}

pub(crate) fn on_hotkey(id: i32) {
    let command = if id == SEARCH_HOTKEY_ID {
        ShellCommand::SearchHotkey
    } else if id == PANEL_HOTKEY_ID {
        ShellCommand::PanelHotkey
    } else {
        return;
    };
    pending_visible(command);
}

pub(crate) fn on_second_instance() {
    pending_visible(ShellCommand::SecondInstance);
}

fn pending_visible(command: ShellCommand) {
    let _ = command;
}
