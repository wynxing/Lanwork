//! 已安装工具在悬浮窗外壳里的运行。
//!
//! 外壳仍负责置顶、拖动和贴边。这里只在窗口打开后跑清单和脚本。
//! 调试窗口 `lanwork.float-shell-demo` 不进这里。
//! 不自动把计时组件设成在走；脚本自己的 `on_load` 会执行。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lanwork_core::config::{ConfigCommands, is_quiet_now, parse_clock};
use lanwork_core::panel::{
    INVALID_PACKAGE, UNAUTHORIZED, format_date, read_tool_storage, tool_dir, write_tool_storage,
};
use lanwork_core::storage::Store;
use lanwork_core::todos::{TodoCommands, TodoError};
use lanwork_toolhost::{
    Origin, ProductCredentialStore, RustlsTransport, Session, WidgetViewKind, decode_grant,
};
use slint::{ModelRc, VecModel};

use crate::toast;
use crate::{FloatShell, ToolRow};

#[derive(Clone)]
struct Services {
    store: Store,
    todos: TodoCommands,
    config: ConfigCommands,
}

struct Running {
    session: Session,
    timer: slint::Timer,
    last: Instant,
    todos: TodoCommands,
    config: ConfigCommands,
    store: Store,
    ui: FloatShell,
}

thread_local! {
    static SERVICES: RefCell<Option<Services>> = const { RefCell::new(None) };
    static RUNNING: RefCell<HashMap<String, Running>> = RefCell::new(HashMap::new());
}

pub(crate) fn install(store: Store, todos: TodoCommands, config: ConfigCommands) {
    SERVICES.with(|slot| {
        *slot.borrow_mut() = Some(Services {
            store,
            todos,
            config,
        });
    });
}

pub(crate) fn attach(id: &str, ui: FloatShell) {
    if RUNNING.with(|slot| slot.borrow().contains_key(id)) {
        return;
    }
    let Some(services) = SERVICES.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let Some(dir) = tool_dir(&services.store.data_dir(), id) else {
        return;
    };
    if !dir.join("manifest.toml").is_file() {
        return;
    }
    let grant_bytes = match std::fs::read(dir.join(lanwork_core::panel::GRANT_FILE)) {
        Ok(bytes) => bytes,
        Err(err) => {
            services
                .store
                .log_warn(&format!("权限记录没有读到：{id}：{err}"));
            ui.set_error(UNAUTHORIZED.into());
            return;
        }
    };
    let Some(grant) = decode_grant(&grant_bytes) else {
        services.store.log_warn(&format!("权限记录读不了：{id}"));
        ui.set_error(UNAUTHORIZED.into());
        return;
    };
    let loaded = match lanwork_toolhost::load_dir(&dir, Origin::Imported) {
        Ok(manifest) if grant.allows(&manifest) => manifest,
        Ok(_) => {
            services
                .store
                .log_warn(&format!("工具清单超出已确认的权限：{id}"));
            ui.set_error(UNAUTHORIZED.into());
            return;
        }
        Err(err) => {
            services
                .store
                .log_warn(&format!("工具清单没有读到：{id}：{err}"));
            ui.set_error(INVALID_PACKAGE.into());
            return;
        }
    };
    ui.set_heading(loaded.name.as_str().into());

    let (storage, persist) = match read_tool_storage(&services.store.data_dir(), id) {
        Ok(map) => {
            let data_dir = services.store.data_dir();
            let tool_id = id.to_owned();
            let store = services.store.clone();
            let persist: Option<lanwork_toolhost::StoragePersist> = Some(Rc::new(move |entries| {
                write_tool_storage(&data_dir, &tool_id, entries).map_err(|err| {
                    store.log_warn(&format!("工具存储没有写下：{err}"));
                    "写盘失败".to_string()
                })
            }));
            (map, persist)
        }
        Err(err) => {
            services
                .store
                .log_warn(&format!("工具存储没有读到：{id}：{err}"));
            ui.set_error("写盘失败".into());
            (std::collections::BTreeMap::new(), None)
        }
    };

    let todos = services.todos.clone();
    let complete = Rc::new(move |item_id: &str| match todos.complete_item(item_id) {
        Ok(()) | Err(TodoError::AlreadyComplete) => Ok(()),
        Err(err) => Err(err.script_complete_message()),
    });

    let mut options = lanwork_toolhost::Options {
        todos: host_todos(&services.todos),
        quiet: quiet_now(&services.config),
        store: Rc::new(RefCell::new(ProductCredentialStore)),
        storage,
        persist_storage: persist,
        complete_todo: Some(complete),
        ..lanwork_toolhost::Options::default()
    };
    match RustlsTransport::native() {
        Ok(transport) => options.transport = Arc::new(transport),
        Err(err) => services.store.log_warn(&format!("工具的网络不可用：{err}")),
    }

    let session = match Session::open(&dir, Origin::Imported, options) {
        Ok(session) => session,
        Err(message) => {
            services
                .store
                .log_warn(&format!("工具没有启动：{id}：{message}"));
            ui.set_error(shown_open_error(&message).into());
            return;
        }
    };
    session.on_load();
    session.poll_http();
    paint(&ui, &session);

    let tool_id = id.to_owned();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || tick(&tool_id),
    );
    RUNNING.with(|slot| {
        slot.borrow_mut().insert(
            id.to_owned(),
            Running {
                session,
                timer,
                last: Instant::now(),
                todos: services.todos,
                config: services.config,
                store: services.store,
                ui,
            },
        );
    });
}

/// 卸掉正在跑的会话，再用同一扇窗口装上新副本和新授权。
///
/// 不新建窗口。位置、大小、贴边和钉住留在外壳上。
pub(crate) fn reload(id: &str, ui: FloatShell) {
    detach(id);
    ui.set_error(slint::SharedString::default());
    attach(id, ui);
}

pub(crate) fn detach(id: &str) {
    let Some(running) = RUNNING.with(|slot| slot.borrow_mut().remove(id)) else {
        return;
    };
    running.timer.stop();
    running.session.on_unload();
}

pub(crate) fn detach_all() {
    let ids: Vec<String> = RUNNING.with(|slot| slot.borrow().keys().cloned().collect());
    for id in ids {
        detach(&id);
    }
}

pub(crate) fn click(id: &str, row: &str) {
    RUNNING.with(|slot| {
        let map = slot.borrow();
        let Some(running) = map.get(id) else {
            return;
        };
        running.session.click(row);
        running.session.poll_http();
        paint(&running.ui, &running.session);
        flush_notes(&running.session, quiet_now(&running.config), &running.store);
    });
}

pub(crate) fn on_todos_changed() {
    let ids: Vec<String> = RUNNING.with(|slot| slot.borrow().keys().cloned().collect());
    for id in ids {
        RUNNING.with(|slot| {
            let map = slot.borrow();
            let Some(running) = map.get(&id) else {
                return;
            };
            running.session.set_todos(host_todos(&running.todos));
            running.session.set_quiet(quiet_now(&running.config));
            running.session.todos_changed();
            running.session.poll_http();
            paint(&running.ui, &running.session);
            flush_notes(&running.session, quiet_now(&running.config), &running.store);
        });
    }
}

fn tick(id: &str) {
    RUNNING.with(|slot| {
        let map = slot.borrow();
        let Some(running) = map.get(id) else {
            return;
        };
        let elapsed = running.last.elapsed().as_secs_f64().clamp(0.0, 2.0);
        let quiet = quiet_now(&running.config);
        running.session.set_quiet(quiet);
        running.session.set_todos(host_todos(&running.todos));
        running.session.advance(elapsed);
        running.session.poll_http();
        paint(&running.ui, &running.session);
        flush_notes(&running.session, quiet, &running.store);
    });
    RUNNING.with(|slot| {
        if let Some(running) = slot.borrow_mut().get_mut(id) {
            running.last = Instant::now();
        }
    });
}

fn paint(ui: &FloatShell, session: &Session) {
    let snapshot = session.snapshot();
    if !snapshot.name.is_empty() {
        ui.set_heading(snapshot.name.as_str().into());
    }
    if !snapshot.status.is_empty() {
        ui.set_error(snapshot.status.as_str().into());
    }
    let rows: Vec<ToolRow> = snapshot.widgets.iter().map(to_row).collect();
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
}

fn to_row(widget: &lanwork_toolhost::WidgetView) -> ToolRow {
    let kind = match widget.kind {
        WidgetViewKind::Text => "text",
        WidgetViewKind::Button => "button",
        WidgetViewKind::Timer => "timer",
        WidgetViewKind::Progress => "progress",
        WidgetViewKind::List => "list",
        WidgetViewKind::Input => "input",
        WidgetViewKind::Toggle => "toggle",
    };
    let progress = widget.progress.clamp(0.0, 1.0) as f32;
    ToolRow {
        kind: kind.into(),
        id: widget.id.as_str().into(),
        caption: widget.caption.as_str().into(),
        label: widget.label.as_str().into(),
        progress,
    }
}

fn flush_notes(session: &Session, quiet: bool, store: &Store) {
    if quiet {
        return;
    }
    let deferred = session.take_deferred();
    let popped = session.take_popped();
    for note in popped {
        if let Err(err) = toast::show_text(&note.title, &note.body) {
            store.log_warn(&format!("工具通知没有弹出：{err}"));
        }
    }
    if deferred.is_empty() {
        return;
    }
    let title = deferred
        .first()
        .map(|note| note.title.clone())
        .unwrap_or_default();
    let lines: Vec<String> = deferred.into_iter().map(|note| note.body).collect();
    if let Err(err) = toast::show_lines(&title, &lines) {
        store.log_warn(&format!("工具通知没有弹出：{err}"));
    }
}

fn host_todos(todos: &TodoCommands) -> Vec<lanwork_toolhost::TodoItem> {
    let Ok(lists) = todos.lists() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for list in lists {
        for item in list.items {
            let trashed = item.in_trash();
            out.push(lanwork_toolhost::TodoItem {
                id: item.id,
                list_id: list.id.clone(),
                title: item.title,
                completed: item.completed,
                due: item.due.map(format_date),
                current: item.current,
                trashed,
                conflict: false,
            });
        }
    }
    out
}

fn quiet_now(config: &ConfigCommands) -> bool {
    let Some(hours) = config.current().quiet_hours else {
        return false;
    };
    // SAFETY: GetLocalTime 只读系统时钟，没有输出缓冲区。
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let text = format!("{:02}:{:02}", time.wHour, time.wMinute);
    parse_clock(&text).is_some_and(|clock| is_quiet_now(Some(hours), clock))
}

fn shown_open_error(message: &str) -> String {
    if message.contains("脚本超时") || message.contains("脚本错误") {
        message.to_string()
    } else {
        INVALID_PACKAGE.to_string()
    }
}
