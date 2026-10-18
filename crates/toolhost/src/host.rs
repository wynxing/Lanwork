//! 一个工具一块 Lua 5.4 状态。未授予的服务不放进状态。
//!
//! 计时组件由宿主逐秒倒数，不走 `timer.every`。调用方用虚拟时钟 `advance` 推进。
//! 同一拍里先处理计时组件，再处理 `timer.every` / `timer.after`。谁先，规格没有写，待定。
//!
//! 协程不放入。钩子按 Lua 线程安装，协程是另一条线程，超时钩子管不到。
//! `require` 和多个 Lua 文件是否允许仍待定。
//! `timer.after` 的最短秒数待定，这次任意正数都可以登记。
//! 待办由调用方注入。完成一条时，若调用方提供了回调，就交给那个回调；否则只改注入的列表。
//! 这个模块不打开数据目录。`storage` 的落盘也由调用方的回调完成。
//! `notify.send` 在 `Options.quiet` 为真时不弹出，只记入待补发。安静时段是否成立由调用方判断。
//!
//! 内存字节上限仍待定。这里的 `MEMORY_CAP` 只是临时天花板，超过后与超时一样中止这次回调，不是产品规则。
//! `xpcall` 换成宿主的函数：处理函数在错误展开之后调用，超时钩子仍然有效。
//! `string.find`、`match`、`gmatch`、`gsub` 在宿主的匹配器里执行，每 4096 步对照这次回调的截止时间。
//! `gsub` 各层还没交还的结果缓冲区加在一起，跟 Lua 已用内存计入同一块 16 MiB 预算；超过则这次调用报错，不置中止标记。复制大约每 64 KiB 再看一次截止时间。
//! 模式函数最多嵌套 8 层。`xpcall` 和 `table.sort` 再进 Lua 时还有一层重入上限，避免把 Rust 栈撑爆。
//! 文本、模式和 `string.rep` 的结果另有临时长度上限。不带比较函数的 `table.sort` 在元素过多时拒绝。这些不是产品规则。
//! HTTP 在另一条线程上连接和读取。脚本回调返回之后，宿主才按等待上限取结果并调用 Lua。等待上限的秒数仍待定，这次默认 5 秒。

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use mlua::{
    Function, HookTriggers, Lua, LuaOptions, MultiValue, RegistryKey, StdLib, Table, Value, VmState,
};

use crate::http::{
    HttpDeny, HttpRequest, HttpTransport, Outgoing, ResolvedSecret, SecretAttach, prepare,
};
use crate::login::{
    LoginError, LoginRoots, LoginValue, cached_login, interpret_login, login_fingerprint,
    prepare_login, store_login,
};
use crate::manifest::{Manifest, WidgetDecl, WidgetKind, load_dir};
use crate::secret::SecretStore;

const CALLBACK_MS: u64 = 1000;
const HOOK_INSTRUCTIONS: u32 = 4_096;
/// 临时内存天花板。产品上的字节数仍待定。
const MEMORY_CAP: usize = 16 * 1024 * 1024;
const MEMORY_HEADROOM: usize = 1024 * 1024;
/// 下面几项只是验证程序的临时天花板。产品上的字节数仍待定。
const STRING_REP_CAP: usize = 1024 * 1024;
/// 不带比较函数时，`table.sort` 的比较在 C 里完成，钩子插不进去。
/// 10 万个元素的默认排序仍落在 1 秒内；再大就拒绝这次调用。
const SORT_LEN_CAP: i64 = 100_000;
const HTTP_BUDGET: Duration = Duration::from_secs(5);
/// 键和值的上限仍待定。有落盘回调时才应用，避免把过大的文本写进数据目录。
const STORAGE_KEY_MAX: usize = 256;
const STORAGE_VALUE_MAX: usize = 256 * 1024;
const STORAGE_COUNT_MAX: usize = 256;
/// 回调里再发请求会再等一轮。不停地一轮接一轮，规格没有写。最多 8 轮，避免宿主停不下来。
const HTTP_DRAIN_ROUNDS: u32 = 8;
const SERVICE_NAMES: &[&str] = &[
    "timer", "notify", "storage", "todo", "http", "secret", "login",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub completed: bool,
    pub due: Option<String>,
    pub current: bool,
    pub trashed: bool,
    /// 为真时 `todo.complete` 返回「待办保存冲突」，不改这条。
    pub conflict: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub line: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetViewKind {
    Text,
    Button,
    Timer,
    Progress,
    List,
    Input,
    Toggle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WidgetView {
    pub id: String,
    pub kind: WidgetViewKind,
    pub caption: String,
    pub label: String,
    pub seconds: i64,
    pub running: bool,
    pub progress: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub widgets: Vec<WidgetView>,
    pub status: String,
}

/// 把整份存储写到调用方的文件。失败时这次 `set` 不改内存。
pub type StoragePersist = Rc<dyn Fn(&BTreeMap<String, String>) -> Result<(), String>>;
/// 完成一条待办。`Err` 的文字原样返回给脚本。
pub type TodoComplete = Rc<dyn Fn(&str) -> Result<(), String>>;

pub struct Options {
    pub todos: Vec<TodoItem>,
    pub quiet: bool,
    pub store: Rc<RefCell<dyn SecretStore>>,
    pub transport: Arc<dyn HttpTransport>,
    /// 连接加读取的等待。产品上的秒数仍待定。
    pub http_budget: Duration,
    /// 打开时已经保存的文本。没有落盘回调时，`set` 只改内存。
    pub storage: BTreeMap<String, String>,
    /// 每次成功写入整份映射。失败时这次 `set` 不改内存，并记下「写盘失败」。
    pub persist_storage: Option<StoragePersist>,
    /// 把完成交给待办存储。`Ok` 表示完成或已经完成。`Err` 的文字原样返回给脚本。
    /// 找不到时调用方应返回「找不到待办」。没有这个回调时只改注入的列表。
    pub complete_todo: Option<TodoComplete>,
    /// 插件登录文件用的家目录。空着则按 `USERPROFILE`，没有再用 `HOME`。
    /// 插件描述不读 `CODEX_HOME` 这一类目录变量。
    pub login_roots: Option<LoginRoots>,
    /// Lanwork 正在使用的数据目录。登录文件解析进这里时拒绝。空着仍拒绝默认的 `Documents/Lanwork`。
    pub data_dir: Option<std::path::PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            todos: Vec::new(),
            quiet: false,
            store: Rc::new(RefCell::new(crate::secret::MemorySecretStore::default())),
            transport: Arc::new(DenyTransport),
            http_budget: HTTP_BUDGET,
            storage: BTreeMap::new(),
            persist_storage: None,
            complete_todo: None,
            login_roots: None,
            data_dir: None,
        }
    }
}

struct DenyTransport;

impl HttpTransport for DenyTransport {
    fn send(
        &self,
        _request: &Outgoing,
    ) -> Result<crate::http::HttpResponse, crate::http::TransportError> {
        Err(crate::http::TransportError)
    }
}

struct Limit {
    start: Instant,
    deadline_ms: AtomicU64,
    timeout_line: AtomicI64,
    /// 超时或临时内存上限已经到了。之后每次钩子都报错，`pcall` 不能把这次回调留住。
    aborted: AtomicBool,
    steps: AtomicU32,
}

impl crate::pattern::CpuWatch for Limit {
    fn deadline_ms(&self) -> u64 {
        self.deadline_ms.load(Ordering::Relaxed)
    }

    fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn aborted(&self) -> bool {
        self.aborted.load(Ordering::Relaxed)
    }

    fn abort(&self, line: i64) {
        self.timeout_line.store(line, Ordering::Relaxed);
        self.aborted.store(true, Ordering::Relaxed);
    }
}

struct WidgetState {
    decl: WidgetDecl,
    text: String,
    label: String,
    seconds: f64,
    running: bool,
    progress: f64,
    on: bool,
}

struct ServiceTimer {
    id: i64,
    every: bool,
    interval: f64,
    remaining: f64,
    key: RegistryKey,
}

struct Inner {
    manifest: Manifest,
    widgets: Vec<WidgetState>,
    notices: Vec<Notice>,
    popped: Vec<Notification>,
    deferred: Vec<Notification>,
    todos: Vec<TodoItem>,
    quiet: bool,
    storage: BTreeMap<String, String>,
    timers: Vec<ServiceTimer>,
    next_timer: i64,
    unauthorized: bool,
    todo_reads: u32,
    store: Rc<RefCell<dyn SecretStore>>,
    transport: Arc<dyn HttpTransport>,
    http_sends: u32,
    inflight: Vec<Inflight>,
    persist_storage: Option<StoragePersist>,
    complete_todo: Option<TodoComplete>,
    login_roots: Option<LoginRoots>,
    data_dir: Option<std::path::PathBuf>,
}

enum FlightKind {
    Http {
        secrets: Vec<String>,
        header_values: Vec<String>,
    },
    Login {
        tool_id: String,
        id: String,
        fingerprint: u64,
        remember: bool,
    },
}

enum FlightOutcome {
    Http(Result<crate::http::HttpResponse, crate::http::TransportError>),
    Login(Result<Vec<LoginValue>, LoginError>),
}

struct Inflight {
    rx: mpsc::Receiver<FlightOutcome>,
    callbacks: Vec<RegistryKey>,
    kind: FlightKind,
}

pub struct Session {
    inner: Rc<RefCell<Inner>>,
    limit: Arc<Limit>,
    lua: Lua,
    http_budget: Duration,
}

fn manifest_open_error(reason: String) -> String {
    if reason.starts_with("读不到") || reason.starts_with("工具文件夹") {
        reason
    } else if reason.contains("入口脚本") || (reason.contains("UTF-8") && reason.contains("脚本"))
    {
        format!("脚本错误：{reason}")
    } else {
        format!("清单无效：{reason}")
    }
}

impl Session {
    pub fn open(dir: &std::path::Path, options: Options) -> Result<Self, String> {
        let manifest = load_dir(dir).map_err(manifest_open_error)?;
        Self::open_manifest(manifest, options)
    }

    /// 用已经读过、并且已经和授权对过的清单打开。不再读一次磁盘。
    pub fn open_manifest(manifest: Manifest, options: Options) -> Result<Self, String> {
        let limit = Arc::new(Limit {
            start: Instant::now(),
            deadline_ms: AtomicU64::new(0),
            timeout_line: AtomicI64::new(-1),
            aborted: AtomicBool::new(false),
            steps: AtomicU32::new(0),
        });
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
            LuaOptions::default(),
        )
        .map_err(|err| format!("脚本错误：{err}"))?;
        lua.set_memory_limit(MEMORY_CAP)
            .map_err(|err| format!("脚本错误：{err}"))?;
        sandbox(&lua, Arc::clone(&limit))?;
        install_hook(&lua, Arc::clone(&limit))?;
        let inner = Rc::new(RefCell::new(Inner {
            widgets: manifest
                .widgets
                .iter()
                .cloned()
                .map(|decl| WidgetState {
                    text: decl.text.clone(),
                    label: decl.label.clone(),
                    on: decl.on,
                    decl,
                    seconds: 0.0,
                    running: false,
                    progress: 0.0,
                })
                .collect(),
            manifest,
            notices: Vec::new(),
            popped: Vec::new(),
            deferred: Vec::new(),
            todos: options.todos,
            quiet: options.quiet,
            storage: options.storage,
            timers: Vec::new(),
            next_timer: 1,
            unauthorized: false,
            todo_reads: 0,
            store: options.store,
            transport: options.transport,
            http_sends: 0,
            inflight: Vec::new(),
            persist_storage: options.persist_storage,
            complete_todo: options.complete_todo,
            login_roots: options.login_roots,
            data_dir: options.data_dir,
        }));
        lua.set_app_data(Rc::clone(&inner));
        bind(&lua, Rc::clone(&inner))?;
        let script = inner.borrow().manifest.script.clone();
        let session = Self {
            inner,
            limit,
            lua,
            http_budget: options.http_budget,
        };
        session.arm();
        let loaded = session.lua.load(&script).set_name("main.lua").exec();
        session.disarm();
        if session.limit.aborted.load(Ordering::Relaxed) {
            let line = session.limit.timeout_line.load(Ordering::Relaxed);
            return Err(timeout_text(line));
        }
        loaded.map_err(|err| format!("脚本错误：{}", display_error(&err)))?;
        session.drain_http();
        Ok(session)
    }

    pub fn on_load(&self) {
        self.call("on_load", ());
    }

    /// 把当前存储再写一次盘。没有写盘回调时什么都不做。
    pub fn flush_storage(&self) -> Result<(), String> {
        let (persist, storage) = {
            let inner = self.inner.borrow();
            (inner.persist_storage.clone(), inner.storage.clone())
        };
        if let Some(persist) = persist {
            persist(&storage)?;
        }
        Ok(())
    }

    pub fn on_unload(&self) {
        self.drop_inflight();
        self.invoke("on_unload", (), false);
        self.drop_inflight();
        self.inner.borrow_mut().timers.clear();
    }

    pub fn click(&self, id: &str) {
        self.call("on_click", id.to_string());
    }

    pub fn todos_changed(&self) {
        if self.inner.borrow().manifest.services.contains("todo.read") {
            self.call("on_todos_changed", ());
        }
    }

    pub fn set_todos(&self, todos: Vec<TodoItem>) {
        self.inner.borrow_mut().todos = todos;
    }

    pub fn set_quiet(&self, quiet: bool) {
        self.inner.borrow_mut().quiet = quiet;
    }

    pub fn advance(&self, seconds: f64) {
        if !(seconds.is_finite() && seconds > 0.0) {
            return;
        }
        let fired = {
            let mut inner = self.inner.borrow_mut();
            inner.tick_widgets(seconds)
        };
        for id in fired {
            self.call("on_timer", id);
        }
        let due = {
            let mut inner = self.inner.borrow_mut();
            inner.mark_due_timers(seconds)
        };
        for id in due {
            self.call_timer(id);
        }
        self.inner
            .borrow_mut()
            .timers
            .retain(|timer| timer.every || timer.remaining > 0.0);
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = self.inner.borrow();
        Snapshot {
            name: inner.manifest.name.clone(),
            widgets: inner.widgets.iter().map(WidgetState::view).collect(),
            status: inner
                .notices
                .last()
                .map(|notice| notice.text.clone())
                .unwrap_or_default(),
        }
    }

    pub fn notices(&self) -> Vec<Notice> {
        self.inner.borrow().notices.clone()
    }

    pub fn popped(&self) -> Vec<Notification> {
        self.inner.borrow().popped.clone()
    }

    pub fn deferred(&self) -> Vec<Notification> {
        self.inner.borrow().deferred.clone()
    }

    pub fn todo_reads(&self) -> u32 {
        self.inner.borrow().todo_reads
    }

    pub fn http_sends(&self) -> u32 {
        self.inner.borrow().http_sends
    }

    pub fn widget(&self, id: &str) -> Option<WidgetView> {
        self.inner
            .borrow()
            .widgets
            .iter()
            .find(|widget| widget.decl.id == id)
            .map(WidgetState::view)
    }

    fn call<A>(&self, name: &str, args: A)
    where
        A: mlua::IntoLuaMulti,
    {
        self.invoke(name, args, true);
    }

    fn invoke<A>(&self, name: &str, args: A, drain: bool)
    where
        A: mlua::IntoLuaMulti,
    {
        self.inner.borrow_mut().unauthorized = false;
        let func = match self.lua.globals().get::<Option<Function>>(name) {
            Ok(Some(func)) => func,
            Ok(None) => return,
            Err(err) => {
                self.note_script(&err);
                return;
            }
        };
        self.run_lua(|| func.call::<()>(args), drain);
    }

    fn call_timer(&self, id: i64) {
        self.inner.borrow_mut().unauthorized = false;
        let func = {
            let inner = self.inner.borrow();
            let Some(timer) = inner.timers.iter().find(|timer| timer.id == id) else {
                return;
            };
            self.lua.registry_value::<Function>(&timer.key).ok()
        };
        let Some(func) = func else {
            return;
        };
        self.run_lua(|| func.call::<()>(()), true);
    }

    fn run_lua(&self, invoke: impl FnOnce() -> mlua::Result<()>, drain: bool) {
        self.arm();
        let result = invoke();
        self.disarm();
        self.finish(result);
        if drain {
            self.drain_http();
        }
    }

    fn arm(&self) {
        self.limit.aborted.store(false, Ordering::Relaxed);
        self.limit.timeout_line.store(-1, Ordering::Relaxed);
        let now = self.limit.start.elapsed().as_millis() as u64;
        self.limit
            .deadline_ms
            .store(now.saturating_add(CALLBACK_MS), Ordering::Relaxed);
    }

    fn disarm(&self) {
        self.limit.deadline_ms.store(0, Ordering::Relaxed);
    }

    fn drop_inflight(&self) {
        self.inner.borrow_mut().inflight.clear();
    }

    fn drain_http(&self) {
        for _ in 0..HTTP_DRAIN_ROUNDS {
            let batch = std::mem::take(&mut self.inner.borrow_mut().inflight);
            if batch.is_empty() {
                return;
            }
            for item in batch {
                let response = item.rx.recv_timeout(self.http_budget);
                self.settle_http(item, response);
            }
        }
        self.drop_inflight();
    }

    /// 取已经回来的响应。还在等的留在队列里，不阻塞。
    pub fn poll_http(&self) {
        let batch = std::mem::take(&mut self.inner.borrow_mut().inflight);
        let mut waiting = Vec::new();
        let mut ready = Vec::new();
        for item in batch {
            match item.rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => waiting.push(item),
                Err(mpsc::TryRecvError::Disconnected) => {
                    ready.push((item, Err(mpsc::RecvTimeoutError::Disconnected)));
                }
                Ok(response) => ready.push((item, Ok(response))),
            }
        }
        {
            let mut inner = self.inner.borrow_mut();
            let fresh = std::mem::take(&mut inner.inflight);
            inner.inflight = waiting;
            inner.inflight.extend(fresh);
        }
        for (item, response) in ready {
            self.settle_http(item, response);
        }
    }

    fn settle_http(&self, item: Inflight, response: Result<FlightOutcome, mpsc::RecvTimeoutError>) {
        match item.kind {
            FlightKind::Http {
                secrets,
                header_values,
            } => self.settle_tool_http(item.callbacks, secrets, header_values, response),
            FlightKind::Login {
                tool_id,
                id,
                fingerprint,
                remember,
            } => self.settle_login(
                item.callbacks,
                &tool_id,
                &id,
                fingerprint,
                remember,
                response,
            ),
        }
    }

    fn settle_tool_http(
        &self,
        callbacks: Vec<RegistryKey>,
        secrets: Vec<String>,
        header_values: Vec<String>,
        response: Result<FlightOutcome, mpsc::RecvTimeoutError>,
    ) {
        let Some(key) = callbacks.into_iter().next() else {
            return;
        };
        let callback = self.lua.registry_value::<Function>(&key).ok();
        self.lua.remove_registry_value(key).ok();
        let Some(callback) = callback else {
            return;
        };
        let http = match response {
            Ok(FlightOutcome::Http(result)) => result,
            Ok(FlightOutcome::Login(_)) | Err(_) => Err(crate::http::TransportError),
        };
        let (ok, status, body, error) = match http {
            Ok(response) => {
                if response_leaks(&response, &secrets, &header_values) {
                    let text = HttpDeny::RequestFailed.text().to_string();
                    self.inner.borrow_mut().note(&text);
                    (false, Some(response.status), String::new(), text)
                } else {
                    let ok = (200..300).contains(&response.status);
                    let error = if ok {
                        String::new()
                    } else {
                        HttpDeny::RequestFailed.text().to_string()
                    };
                    if !ok {
                        self.inner.borrow_mut().note(&error);
                    }
                    (ok, Some(response.status), response.body, error)
                }
            }
            Err(_) => {
                let text = HttpDeny::RequestFailed.text().to_string();
                self.inner.borrow_mut().note(&text);
                (false, None, String::new(), text)
            }
        };
        self.run_lua(
            || deliver(&self.lua, &callback, ok, status, &body, &error),
            false,
        );
    }

    fn settle_login(
        &self,
        callbacks: Vec<RegistryKey>,
        tool_id: &str,
        id: &str,
        fingerprint: u64,
        remember: bool,
        response: Result<FlightOutcome, mpsc::RecvTimeoutError>,
    ) {
        let outcome = match response {
            Ok(FlightOutcome::Login(result)) => result,
            Ok(FlightOutcome::Http(_)) | Err(_) => Err(LoginError::Failed),
        };
        if remember {
            store_login(tool_id, id, fingerprint, outcome.clone());
        }
        if let Err(err) = &outcome {
            self.inner.borrow_mut().note(err.text());
        }
        for key in callbacks {
            let callback = self.lua.registry_value::<Function>(&key).ok();
            self.lua.remove_registry_value(key).ok();
            let Some(callback) = callback else {
                continue;
            };
            let outcome = outcome.clone();
            self.run_lua(|| deliver_login(&self.lua, &callback, &outcome), false);
        }
    }

    pub fn take_popped(&self) -> Vec<Notification> {
        std::mem::take(&mut self.inner.borrow_mut().popped)
    }

    pub fn take_deferred(&self) -> Vec<Notification> {
        std::mem::take(&mut self.inner.borrow_mut().deferred)
    }

    fn finish(&self, result: mlua::Result<()>) {
        if self.limit.aborted.load(Ordering::Relaxed) {
            let line = self.limit.timeout_line.load(Ordering::Relaxed);
            self.inner.borrow_mut().notices.push(Notice {
                text: timeout_text(line),
                line: (line >= 0).then_some(line as usize),
            });
            return;
        }
        if self.inner.borrow().unauthorized {
            self.inner.borrow_mut().unauthorized = false;
            self.inner.borrow_mut().notices.push(Notice {
                text: "未授权".to_string(),
                line: None,
            });
            return;
        }
        if let Err(err) = result {
            if timeout_error(&err) {
                let line = self.limit.timeout_line.load(Ordering::Relaxed);
                self.inner.borrow_mut().notices.push(Notice {
                    text: timeout_text(line),
                    line: (line >= 0).then_some(line as usize),
                });
            } else {
                let line = line_of(&err);
                self.inner.borrow_mut().notices.push(Notice {
                    text: script_text(line),
                    line,
                });
            }
        }
    }

    fn note_script(&self, err: &mlua::Error) {
        let line = line_of(err);
        self.inner.borrow_mut().notices.push(Notice {
            text: script_text(line),
            line,
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.inner.borrow_mut().timers.clear();
    }
}

impl Inner {
    fn tick_widgets(&mut self, seconds: f64) -> Vec<String> {
        let mut fired = Vec::new();
        for widget in &mut self.widgets {
            if widget.decl.kind != WidgetKind::Timer || !widget.running || widget.seconds <= 0.0 {
                continue;
            }
            let before = widget.seconds;
            widget.seconds = (widget.seconds - seconds).max(0.0);
            if widget.running && before > 0.0 && widget.seconds == 0.0 {
                fired.push(widget.decl.id.clone());
            }
        }
        fired
    }

    /// 到期的定时器 id。`every` 一次 `advance` 只记一次，剩余时间拨到下一个间隔。
    /// 一次前进跨过多个间隔要不要补叫，规格没有写，待定。
    fn mark_due_timers(&mut self, seconds: f64) -> Vec<i64> {
        let mut due = Vec::new();
        for timer in &mut self.timers {
            timer.remaining -= seconds;
            if timer.remaining > 0.0 {
                continue;
            }
            due.push(timer.id);
            if timer.every {
                while timer.remaining <= 0.0 {
                    timer.remaining += timer.interval;
                }
            }
        }
        due
    }

    fn note(&mut self, text: &str) {
        self.notices.push(Notice {
            text: text.to_string(),
            line: None,
        });
    }
}

fn sandbox(lua: &Lua, limit: Arc<Limit>) -> Result<(), String> {
    let globals = lua.globals();
    for name in [
        "load",
        "loadfile",
        "dofile",
        "require",
        "collectgarbage",
        "print",
        "warn",
        "rawlen",
        "os",
        "io",
        "package",
        "debug",
        "coroutine",
        "module",
    ] {
        globals
            .set(name, Value::Nil)
            .map_err(|err| format!("脚本错误：{err}"))?;
    }
    install_xpcall(lua, Arc::clone(&limit))?;
    install_string_limits(lua, limit)?;
    Ok(())
}

/// Lua 自带的 `xpcall` 在处理函数里关掉钩子。这里先 `pcall` 被保护的函数，
/// 错误展开之后再普通调用处理函数，钩子仍然开着。已经超时就不再进入处理函数。
fn install_xpcall(lua: &Lua, limit: Arc<Limit>) -> Result<(), String> {
    let xpcall = lua
        .create_function(
            move |lua, mut args: MultiValue| -> mlua::Result<MultiValue> {
                let _guard = crate::pattern::enter_host(lua)?;
                let Some(function) = args.pop_front() else {
                    return Err(mlua::Error::runtime("xpcall 需要函数"));
                };
                let Some(handler) = args.pop_front() else {
                    return Err(mlua::Error::runtime("xpcall 需要处理函数"));
                };
                let Value::Function(function) = function else {
                    return Err(mlua::Error::runtime("xpcall 需要函数"));
                };
                let Value::Function(handler) = handler else {
                    return Err(mlua::Error::runtime("xpcall 需要处理函数"));
                };
                match function.call::<MultiValue>(args) {
                    Ok(mut values) => {
                        values.push_front(Value::Boolean(true));
                        Ok(values)
                    }
                    Err(err) => {
                        if limit.aborted.load(Ordering::Relaxed) || timeout_error(&err) {
                            return Err(err);
                        }
                        match handler.call::<MultiValue>(err.to_string()) {
                            Ok(mut values) => {
                                values.push_front(Value::Boolean(false));
                                Ok(values)
                            }
                            Err(err) => Err(err),
                        }
                    }
                }
            },
        )
        .map_err(|err| format!("脚本错误：{err}"))?;
    lua.globals()
        .set("xpcall", xpcall)
        .map_err(|err| format!("脚本错误：{err}"))?;
    Ok(())
}

fn install_string_limits(lua: &Lua, limit: Arc<Limit>) -> Result<(), String> {
    crate::pattern::install(lua, limit)?;
    let library: Table = lua
        .globals()
        .get("string")
        .map_err(|err| format!("脚本错误：{err}"))?;
    wrap_rep(lua, &library)?;
    install_sort_limit(lua)?;
    Ok(())
}

/// `table.concat` 按元素线性拼接，结果受临时内存上限约束。
/// `string.format` 的宽度和精度最多两位数字。`string.lower`、`upper`、`reverse`、`sub`、`pack`，
/// 以及 `utf8.len`、`utf8.offset` 按文本长度线性完成，同样受临时内存上限约束。
/// `utf8.char` 的参数个数受 Lua 栈限制，`utf8.codes` 每次回到 Lua。这些都不再单独包一层。
fn install_sort_limit(lua: &Lua) -> Result<(), String> {
    let library: Table = lua
        .globals()
        .get("table")
        .map_err(|err| format!("脚本错误：{err}"))?;
    let original: Function = library
        .get("sort")
        .map_err(|err| format!("脚本错误：{err}"))?;
    let wrapper = lua
        .create_function(move |lua, args: MultiValue| {
            let _guard = crate::pattern::enter_host(lua)?;
            let has_comparator = args
                .get(1)
                .is_some_and(|value| !matches!(value, Value::Nil));
            if !has_comparator && let Some(Value::Table(table)) = args.front() {
                let len = table.len()?;
                if len > SORT_LEN_CAP {
                    return Err(mlua::Error::runtime("表过长"));
                }
            }
            original.call::<MultiValue>(args)
        })
        .map_err(|err| format!("脚本错误：{err}"))?;
    library
        .set("sort", wrapper)
        .map_err(|err| format!("脚本错误：{err}"))
}

fn wrap_rep(lua: &Lua, library: &Table) -> Result<(), String> {
    let original: Function = library
        .get("rep")
        .map_err(|err| format!("脚本错误：{err}"))?;
    let wrapper = lua
        .create_function(move |_, args: MultiValue| {
            check_rep(&args)?;
            original.call::<MultiValue>(args)
        })
        .map_err(|err| format!("脚本错误：{err}"))?;
    library
        .set("rep", wrapper)
        .map_err(|err| format!("脚本错误：{err}"))
}

fn check_rep(args: &MultiValue) -> mlua::Result<()> {
    let (Some(unit), Some(count)) = (args.front(), args.get(1)) else {
        return Ok(());
    };
    let times = match repetition_count(count) {
        RepCount::Times(times) => times,
        // 转不成整数时，Lua 在分配之前就报错。
        RepCount::LeaveToLua => return Ok(()),
        RepCount::Reject => return Err(mlua::Error::runtime("结果过长")),
    };
    if times == 0 {
        return Ok(());
    }
    let Some(unit_len) = coerced_len(unit) else {
        return Ok(());
    };
    let separator = match args.get(2) {
        None | Some(Value::Nil) => 0,
        Some(value) => match coerced_len(value) {
            Some(len) => len,
            None => {
                if unit_len
                    .checked_mul(times)
                    .is_none_or(|body| body > STRING_REP_CAP)
                {
                    return Err(mlua::Error::runtime("结果过长"));
                }
                return Ok(());
            }
        },
    };
    let total = unit_len.checked_mul(times).and_then(|body| {
        separator
            .checked_mul(times.saturating_sub(1))?
            .checked_add(body)
    });
    match total {
        Some(total) if total <= STRING_REP_CAP => Ok(()),
        _ => Err(mlua::Error::runtime("结果过长")),
    }
}

enum RepCount {
    Times(usize),
    /// 不是 Lua 能转成整数的值。`string.rep` 会在分配之前报错。
    LeaveToLua,
    /// 像数字，但放不进整数，或大到不能安全交给 C。
    Reject,
}

fn repetition_count(value: &Value) -> RepCount {
    match value {
        Value::Integer(count) => RepCount::Times(non_negative_times(*count)),
        Value::Number(count) => match lua_integral_i64(*count) {
            Some(count) => RepCount::Times(non_negative_times(count)),
            None => RepCount::LeaveToLua,
        },
        Value::String(text) => repetition_from_bytes(&text.as_bytes()),
        _ => RepCount::LeaveToLua,
    }
}

fn repetition_from_bytes(bytes: &[u8]) -> RepCount {
    let trimmed = trim_lua_space(bytes);
    if trimmed.is_empty() {
        return RepCount::LeaveToLua;
    }
    if let Some(count) = parse_lua_integer(trimmed) {
        return RepCount::Times(non_negative_times(count));
    }
    if let Some(count) = parse_lua_integral_float(trimmed) {
        return RepCount::Times(non_negative_times(count));
    }
    if looks_numeric(trimmed) {
        RepCount::Reject
    } else {
        RepCount::LeaveToLua
    }
}

fn non_negative_times(count: i64) -> usize {
    usize::try_from(count).unwrap_or(0)
}

fn lua_integral_i64(number: f64) -> Option<i64> {
    if !number.is_finite() || number.fract() != 0.0 {
        return None;
    }
    if number < i64::MIN as f64 || number >= (i64::MAX as f64) {
        return None;
    }
    Some(number as i64)
}

fn trim_lua_space(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !is_lua_space(*byte))
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !is_lua_space(*byte))
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn is_lua_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | 0x0b | b'\n' | b'\r' | 0x0c)
}

fn looks_numeric(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(b'+' | b'-' | b'.' | b'0'..=b'9'))
}

/// Lua `l_str2int`：可选符号，十进制或 `0x` 十六进制，必须吃完整个字符串。
fn parse_lua_integer(bytes: &[u8]) -> Option<i64> {
    let mut index = 0;
    let negative = match bytes.first() {
        Some(b'+') => {
            index = 1;
            false
        }
        Some(b'-') => {
            index = 1;
            true
        }
        _ => false,
    };
    if index >= bytes.len() {
        return None;
    }
    let mut acc: u64 = 0;
    if bytes[index] == b'0' && matches!(bytes.get(index + 1), Some(b'x' | b'X')) {
        index += 2;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_hexdigit() {
            let digit = hex_digit(bytes[index])?;
            acc = acc.checked_mul(16)?.checked_add(u64::from(digit))?;
            index += 1;
        }
        if index == start {
            return None;
        }
    } else {
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            let digit = u64::from(bytes[index] - b'0');
            acc = acc.checked_mul(10)?.checked_add(digit)?;
            index += 1;
        }
        if index == start {
            return None;
        }
    }
    if index != bytes.len() {
        return None;
    }
    if negative {
        if acc == 0 {
            Some(0)
        } else if acc == i64::MAX as u64 + 1 {
            Some(i64::MIN)
        } else {
            i64::try_from(acc).ok().map(|count| -count)
        }
    } else {
        i64::try_from(acc).ok()
    }
}

fn parse_lua_integral_float(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.to_ascii_lowercase().contains("nan") || text.to_ascii_lowercase().contains("inf") {
        return None;
    }
    let number: f64 = text.parse().ok()?;
    lua_integral_i64(number)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `string.rep` 会把数字收成字符串。长度按这个结果算，避免数字单元或分隔符绕过上限。
fn coerced_len(value: &Value) -> Option<usize> {
    match value {
        Value::String(text) => Some(text.as_bytes().len()),
        Value::Integer(number) => Some(number.to_string().len()),
        Value::Number(number) if number.is_finite() => Some(format!("{number}").len()),
        _ => None,
    }
}

fn response_leaks(
    response: &crate::http::HttpResponse,
    secrets: &[String],
    header_values: &[String],
) -> bool {
    if secrets.iter().all(|secret| secret.is_empty()) {
        return false;
    }
    let mut parts = Vec::with_capacity(1 + response.headers.len() * 2);
    parts.push(response.body.as_str());
    for (name, value) in &response.headers {
        parts.push(name.as_str());
        parts.push(value.as_str());
    }
    secrets.iter().enumerate().any(|(index, secret)| {
        let header = header_values.get(index).map(String::as_str);
        crate::http::response_exposes_secret(&parts, secret, header)
    })
}

fn install_hook(lua: &Lua, limit: Arc<Limit>) -> Result<(), String> {
    // 计数是 1：超时一旦置位，下一条指令（包括 `pcall` 返回之后）也会报错。
    // 时间只在每 4096 步看一次，避免每条指令都读时钟。
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(1),
        move |lua, debug| {
            if limit.aborted.load(Ordering::Relaxed) {
                return Err(mlua::Error::runtime("脚本超时"));
            }
            let step = limit.steps.fetch_add(1, Ordering::Relaxed);
            let deadline = limit.deadline_ms.load(Ordering::Relaxed);
            if deadline == 0 || !step.is_multiple_of(HOOK_INSTRUCTIONS) {
                return Ok(VmState::Continue);
            }
            let timed_out = limit.start.elapsed().as_millis() as u64 >= deadline;
            let used = lua.used_memory();
            let over_memory =
                used.saturating_add(MEMORY_HEADROOM) >= MEMORY_CAP || used >= MEMORY_CAP;
            if !timed_out && !over_memory {
                return Ok(VmState::Continue);
            }
            let line = debug.current_line().unwrap_or(0) as i64;
            limit.timeout_line.store(line, Ordering::Relaxed);
            limit.aborted.store(true, Ordering::Relaxed);
            Err(mlua::Error::runtime("脚本超时"))
        },
    )
    .map_err(|err| format!("脚本错误：{err}"))
}

fn bind(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    bind_ui(lua, Rc::clone(&inner))?;
    bind_json(lua)?;
    bind_guard(lua, Rc::clone(&inner))?;
    let services = inner.borrow().manifest.services.clone();
    if services.contains("timer") {
        bind_timer(lua, Rc::clone(&inner))?;
    }
    if services.contains("notify") {
        bind_notify(lua, Rc::clone(&inner))?;
    }
    if services.contains("storage") {
        bind_storage(lua, Rc::clone(&inner))?;
    }
    if services.contains("todo.read") || services.contains("todo.complete") {
        bind_todo(
            lua,
            Rc::clone(&inner),
            services.contains("todo.read"),
            services.contains("todo.complete"),
        )?;
    }
    if services.contains("http") {
        bind_http(lua, Rc::clone(&inner))?;
    }
    if services.contains("secret") {
        lua.globals()
            .set("secret", lua.create_table().map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;
    }
    if !inner.borrow().manifest.logins.is_empty() {
        bind_login(lua, inner)?;
    }
    Ok(())
}

fn bind_guard(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let globals = lua.globals();
    let metatable = lua.create_table().map_err(|err| err.to_string())?;
    let inner_for_index = Rc::clone(&inner);
    metatable
        .set(
            "__index",
            lua.create_function(move |lua, (_table, key): (Value, String)| {
                if SERVICE_NAMES.contains(&key.as_str()) {
                    inner_for_index.borrow_mut().unauthorized = true;
                    return empty_call(lua);
                }
                Ok(Value::Nil)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    globals
        .set_metatable(Some(metatable))
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn empty_call(lua: &Lua) -> mlua::Result<Value> {
    Ok(Value::Function(lua.create_function(
        |_, _: mlua::MultiValue| Ok(Value::Nil),
    )?))
}

fn bind_ui(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let ui = lua.create_table().map_err(|err| err.to_string())?;
    let text_inner = Rc::clone(&inner);
    ui.set(
        "text",
        lua.create_function(move |_, (id, text): (String, String)| {
            text_inner.borrow_mut().set_text(&id, text)
        })
        .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    let button_inner = Rc::clone(&inner);
    ui.set(
        "button",
        lua.create_function(move |_, (id, label): (String, String)| {
            button_inner.borrow_mut().set_label(&id, label)
        })
        .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    let timer_inner = Rc::clone(&inner);
    ui.set(
        "timer",
        lua.create_function(move |_, (id, seconds): (String, Option<f64>)| {
            let mut inner = timer_inner.borrow_mut();
            if let Some(seconds) = seconds {
                inner.set_seconds(&id, seconds)?;
                Ok(Value::Nil)
            } else {
                Ok(Value::Number(inner.seconds_of(&id)?))
            }
        })
        .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    let running_inner = Rc::clone(&inner);
    ui.set(
        "timer_running",
        lua.create_function(move |_, (id, on): (String, bool)| {
            running_inner.borrow_mut().set_running(&id, on)
        })
        .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    let progress_inner = Rc::clone(&inner);
    ui.set(
        "progress",
        lua.create_function(move |_, (id, value): (String, f64)| {
            progress_inner.borrow_mut().set_progress(&id, value)
        })
        .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    lua.globals().set("ui", ui).map_err(|err| err.to_string())?;
    Ok(())
}

fn bind_json(lua: &Lua) -> Result<(), String> {
    let json = lua.create_table().map_err(|err| err.to_string())?;
    json.set(
        "decode",
        lua.create_function(|lua, text: String| {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(value) => Ok((Some(json_to_lua(lua, &value)?), None::<String>)),
                Err(err) => Ok((None::<Value>, Some(err.to_string()))),
            }
        })
        .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    json.set(
        "encode",
        lua.create_function(|_, value: Value| lua_to_json(&value))
            .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    lua.globals()
        .set("json", json)
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn bind_timer(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let timer = lua.create_table().map_err(|err| err.to_string())?;
    let every_inner = Rc::clone(&inner);
    timer
        .set(
            "every",
            lua.create_function(move |lua, (seconds, func): (f64, Function)| {
                every_inner.borrow_mut().schedule(lua, seconds, true, func)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    let after_inner = Rc::clone(&inner);
    timer
        .set(
            "after",
            lua.create_function(move |lua, (seconds, func): (f64, Function)| {
                after_inner.borrow_mut().schedule(lua, seconds, false, func)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    let cancel_inner = Rc::clone(&inner);
    timer
        .set(
            "cancel",
            lua.create_function(move |_, id: i64| {
                cancel_inner
                    .borrow_mut()
                    .timers
                    .retain(|timer| timer.id != id);
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    lua.globals()
        .set("timer", timer)
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn bind_notify(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let notify = lua.create_table().map_err(|err| err.to_string())?;
    notify
        .set(
            "send",
            lua.create_function(move |_, (title, body): (String, String)| {
                let mut inner = inner.borrow_mut();
                let note = Notification { title, body };
                if inner.quiet {
                    inner.deferred.push(note);
                } else {
                    inner.popped.push(note);
                }
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    lua.globals()
        .set("notify", notify)
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn bind_storage(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let storage = lua.create_table().map_err(|err| err.to_string())?;
    let get_inner = Rc::clone(&inner);
    storage
        .set(
            "get",
            lua.create_function(move |_, key: String| {
                Ok(get_inner.borrow().storage.get(&key).cloned())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    let set_inner = Rc::clone(&inner);
    storage
        .set(
            "set",
            lua.create_function(move |_, (key, value): (String, String)| {
                let persist = set_inner.borrow().persist_storage.clone();
                let Some(persist) = persist else {
                    set_inner.borrow_mut().storage.insert(key, value);
                    return Ok((Some(true), None::<String>));
                };
                let mut next = set_inner.borrow().storage.clone();
                if storage_rejected(&next, &key, &value) {
                    set_inner.borrow_mut().note("写盘失败");
                    return Ok((None::<bool>, Some("写盘失败".to_string())));
                }
                next.insert(key, value);
                if persist(&next).is_err() {
                    set_inner.borrow_mut().note("写盘失败");
                    return Ok((None::<bool>, Some("写盘失败".to_string())));
                }
                set_inner.borrow_mut().storage = next;
                Ok((Some(true), None::<String>))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    lua.globals()
        .set("storage", storage)
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn storage_rejected(map: &BTreeMap<String, String>, key: &str, value: &str) -> bool {
    if key.chars().count() > STORAGE_KEY_MAX || value.len() > STORAGE_VALUE_MAX {
        return true;
    }
    let extra = usize::from(!map.contains_key(key));
    map.len().saturating_add(extra) > STORAGE_COUNT_MAX
}

fn bind_todo(
    lua: &Lua,
    inner: Rc<RefCell<Inner>>,
    read: bool,
    complete: bool,
) -> Result<(), String> {
    let todo = lua.create_table().map_err(|err| err.to_string())?;
    if read {
        let read_inner = Rc::clone(&inner);
        todo.set(
            "list",
            lua.create_function(move |lua, (): ()| {
                let mut inner = read_inner.borrow_mut();
                inner.todo_reads += 1;
                let list = lua.create_table()?;
                let mut index = 1;
                for item in &inner.todos {
                    if item.trashed {
                        continue;
                    }
                    let row = lua.create_table()?;
                    row.set("id", item.id.clone())?;
                    row.set("listId", item.list_id.clone())?;
                    row.set("title", item.title.clone())?;
                    row.set("completed", item.completed)?;
                    match &item.due {
                        Some(due) => row.set("due", due.clone())?,
                        None => row.set("due", Value::Nil)?,
                    }
                    row.set("current", item.current)?;
                    list.set(index, row)?;
                    index += 1;
                }
                Ok(list)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    }
    if complete {
        let complete_inner = Rc::clone(&inner);
        todo.set(
            "complete",
            lua.create_function(move |_, id: String| {
                let callback = complete_inner.borrow().complete_todo.clone();
                if let Some(callback) = callback {
                    return match callback(&id) {
                        Ok(()) => {
                            if let Some(item) = complete_inner
                                .borrow_mut()
                                .todos
                                .iter_mut()
                                .find(|item| item.id == id && !item.trashed)
                            {
                                item.completed = true;
                            }
                            Ok((Some(true), None::<String>))
                        }
                        Err(message) => Ok((None::<bool>, Some(message))),
                    };
                }
                let mut inner = complete_inner.borrow_mut();
                let Some(item) = inner
                    .todos
                    .iter_mut()
                    .find(|item| item.id == id && !item.trashed)
                else {
                    return Ok((None::<bool>, Some("找不到待办".to_string())));
                };
                if item.conflict {
                    return Ok((None::<bool>, Some("待办保存冲突".to_string())));
                }
                if item.completed {
                    return Ok((Some(true), None::<String>));
                }
                item.completed = true;
                Ok((Some(true), None::<String>))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    }
    let missing = lua
        .create_function(move |_, _: mlua::MultiValue| {
            inner.borrow_mut().unauthorized = true;
            Ok(Value::Nil)
        })
        .map_err(|err| err.to_string())?;
    let metatable = lua.create_table().map_err(|err| err.to_string())?;
    metatable
        .set("__index", missing)
        .map_err(|err| err.to_string())?;
    todo.set_metatable(Some(metatable))
        .map_err(|err| err.to_string())?;
    lua.globals()
        .set("todo", todo)
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn bind_http(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let http = lua.create_table().map_err(|err| err.to_string())?;
    http.set(
        "request",
        lua.create_function(move |lua, (options, callback): (Table, Function)| {
            http_request(lua, &inner, options, callback)
        })
        .map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    lua.globals()
        .set("http", http)
        .map_err(|err| err.to_string())?;
    Ok(())
}

enum Gate {
    Deny(&'static str),
    Send {
        outgoing: Outgoing,
        secrets: Vec<String>,
        header_values: Vec<String>,
    },
}

fn http_request(
    lua: &Lua,
    inner: &Rc<RefCell<Inner>>,
    options: Table,
    callback: Function,
) -> mlua::Result<()> {
    let method: String = options.get("method").unwrap_or_default();
    let url: String = options.get("url").unwrap_or_default();
    let secrets = match read_attaches(&options) {
        Ok(secrets) => secrets,
        Err(text) => {
            inner.borrow_mut().note(text);
            return deliver(lua, &callback, false, None, "", text);
        }
    };
    let body = match read_body(&options) {
        Ok(body) => body,
        Err(text) => {
            inner.borrow_mut().note(text);
            return deliver(lua, &callback, false, None, "", text);
        }
    };
    let request = HttpRequest {
        method,
        url,
        secrets,
        body,
    };
    let gate = {
        let inner = inner.borrow();
        decide(&inner, &request)
    };
    match gate {
        Gate::Deny(text) => {
            inner.borrow_mut().note(text);
            deliver(lua, &callback, false, None, "", text)
        }
        Gate::Send {
            outgoing,
            secrets,
            header_values,
        } => {
            let transport = Arc::clone(&inner.borrow().transport);
            let (tx, rx) = mpsc::channel();
            thread::Builder::new()
                .name("toolhost-http".to_string())
                .spawn(move || {
                    let _ = tx.send(FlightOutcome::Http(transport.send(&outgoing)));
                })
                .map_err(|_| mlua::Error::runtime("请求失败"))?;
            let key = lua.create_registry_value(callback)?;
            let mut inner = inner.borrow_mut();
            inner.http_sends += 1;
            inner.inflight.push(Inflight {
                rx,
                callbacks: vec![key],
                kind: FlightKind::Http {
                    secrets,
                    header_values,
                },
            });
            Ok(())
        }
    }
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

impl Inner {
    fn join_login(&mut self, id: &str, key: RegistryKey) -> Result<(), RegistryKey> {
        for item in &mut self.inflight {
            if let FlightKind::Login { id: existing, .. } = &item.kind
                && existing == id
            {
                item.callbacks.push(key);
                return Ok(());
            }
        }
        Err(key)
    }
}

fn bind_login(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let login = lua.create_table().map_err(|err| err.to_string())?;
    login
        .set(
            "fetch",
            lua.create_function(move |lua, (name, callback): (String, Function)| {
                login_fetch(lua, &inner, &name, callback)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    lua.globals()
        .set("login", login)
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn login_fetch(
    lua: &Lua,
    inner: &Rc<RefCell<Inner>>,
    name: &str,
    callback: Function,
) -> mlua::Result<()> {
    // 先核对这个工具自己声明的描述，再查缓存。别的工具同名 id 不能从这里读走。
    let (tool_id, descriptor) = {
        let inner = inner.borrow();
        let tool_id = inner.manifest.id.clone();
        let descriptor = inner
            .manifest
            .logins
            .iter()
            .find(|item| item.id == name)
            .cloned();
        (tool_id, descriptor)
    };
    let Some(descriptor) = descriptor else {
        inner.borrow_mut().note(LoginError::Unauthorized.text());
        return deliver_login(lua, &callback, &Err(LoginError::Unauthorized));
    };
    let fingerprint = login_fingerprint(&descriptor);
    let key = lua.create_registry_value(callback)?;
    let key = match inner.borrow_mut().join_login(&descriptor.id, key) {
        Ok(()) => return Ok(()),
        Err(key) => key,
    };
    if let Some(cached) = cached_login(&tool_id, &descriptor.id, fingerprint) {
        return queue_login(
            inner,
            &tool_id,
            &descriptor.id,
            fingerprint,
            key,
            cached,
            false,
        );
    }
    let roots = inner
        .borrow()
        .login_roots
        .clone()
        .unwrap_or_else(LoginRoots::from_env);
    let data_dir = inner.borrow().data_dir.clone();
    match prepare_login(&descriptor, &roots, data_dir.as_deref(), unix_now_ms()) {
        Ok(call) => spawn_login(inner, &tool_id, &descriptor.id, fingerprint, key, call),
        Err(err) => queue_login(
            inner,
            &tool_id,
            &descriptor.id,
            fingerprint,
            key,
            Err(err),
            true,
        ),
    }
}

fn queue_login(
    inner: &Rc<RefCell<Inner>>,
    tool_id: &str,
    id: &str,
    fingerprint: u64,
    key: RegistryKey,
    outcome: Result<Vec<LoginValue>, LoginError>,
    remember: bool,
) -> mlua::Result<()> {
    let (tx, rx) = mpsc::channel();
    let _ = tx.send(FlightOutcome::Login(outcome));
    inner.borrow_mut().inflight.push(Inflight {
        rx,
        callbacks: vec![key],
        kind: FlightKind::Login {
            tool_id: tool_id.to_string(),
            id: id.to_string(),
            fingerprint,
            remember,
        },
    });
    Ok(())
}

fn spawn_login(
    inner: &Rc<RefCell<Inner>>,
    tool_id: &str,
    id: &str,
    fingerprint: u64,
    key: RegistryKey,
    call: crate::login::LoginCall,
) -> mlua::Result<()> {
    if !call.outgoing.url.starts_with("https://") {
        inner.borrow_mut().note(LoginError::Failed.text());
        return queue_login(
            inner,
            tool_id,
            id,
            fingerprint,
            key,
            Err(LoginError::Failed),
            true,
        );
    }
    let descriptor = inner
        .borrow()
        .manifest
        .logins
        .iter()
        .find(|item| item.id == id)
        .cloned();
    let Some(descriptor) = descriptor else {
        inner.borrow_mut().note(LoginError::Unauthorized.text());
        return queue_login(
            inner,
            tool_id,
            id,
            fingerprint,
            key,
            Err(LoginError::Unauthorized),
            false,
        );
    };
    let crate::login::LoginCall {
        outgoing,
        sensitive,
    } = call;
    let transport = Arc::clone(&inner.borrow().transport);
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("toolhost-login".to_string())
        .spawn(move || {
            let outcome = match transport.send(&outgoing) {
                Ok(response) => {
                    interpret_login(&descriptor, response.status, &response.body, &sensitive)
                }
                Err(_) => Err(LoginError::Failed),
            };
            let _ = tx.send(FlightOutcome::Login(outcome));
        })
        .map_err(|_| mlua::Error::runtime("请求失败"))?;
    let mut inner = inner.borrow_mut();
    inner.http_sends += 1;
    inner.inflight.push(Inflight {
        rx,
        callbacks: vec![key],
        kind: FlightKind::Login {
            tool_id: tool_id.to_string(),
            id: id.to_string(),
            fingerprint,
            remember: true,
        },
    });
    Ok(())
}

fn deliver_login(
    lua: &Lua,
    callback: &Function,
    outcome: &Result<Vec<LoginValue>, LoginError>,
) -> mlua::Result<()> {
    let result = lua.create_table()?;
    match outcome {
        Ok(values) => {
            result.set("ok", true)?;
            let table = lua.create_table()?;
            for item in values {
                match &item.value {
                    crate::login::LoginScalar::Number(number) => {
                        table.set(item.name.as_str(), *number)?;
                    }
                    crate::login::LoginScalar::String(text) => {
                        table.set(item.name.as_str(), text.as_str())?;
                    }
                    crate::login::LoginScalar::Bool(flag) => {
                        table.set(item.name.as_str(), *flag)?;
                    }
                }
            }
            result.set("values", table)?;
        }
        Err(err) => {
            result.set("ok", false)?;
            result.set("error", err.text())?;
        }
    }
    let _guard = crate::pattern::enter_host(lua)?;
    callback.call::<()>(result)
}

fn read_attaches(options: &Table) -> Result<Vec<SecretAttach>, &'static str> {
    let one = options.get::<Value>("secret").map_err(|_| "地址无效")?;
    let many = options.get::<Value>("secrets").map_err(|_| "地址无效")?;
    match (one, many) {
        (Value::Nil, Value::Nil) => Ok(Vec::new()),
        (Value::Table(table), Value::Nil) => read_attach(&table).map(|item| vec![item]),
        (Value::Nil, Value::Table(table)) => read_attach_list(&table),
        _ => Err("地址无效"),
    }
}

fn read_attach_list(table: &Table) -> Result<Vec<SecretAttach>, &'static str> {
    if table.raw_len() == 0 && table.contains_key("id").unwrap_or(false) {
        return Err("地址无效");
    }
    let mut secrets = Vec::new();
    for index in 1..=table.raw_len() {
        match table.get::<Value>(index).map_err(|_| "地址无效")? {
            Value::Table(item) => secrets.push(read_attach(&item)?),
            _ => return Err("地址无效"),
        }
    }
    if secrets.len() > crate::http::SECRET_LIMIT {
        return Err("地址无效");
    }
    Ok(secrets)
}

fn read_attach(table: &Table) -> Result<SecretAttach, &'static str> {
    let Some(id) = field_string(table, "id")? else {
        return Err("地址无效");
    };
    let header = field_string(table, "header")?.unwrap_or_default();
    let prefix = field_string(table, "prefix")?;
    Ok(SecretAttach {
        id,
        header,
        prefix_set: prefix.is_some(),
        prefix: prefix.unwrap_or_default(),
    })
}

fn read_body(options: &Table) -> Result<Option<String>, &'static str> {
    match options.get::<Value>("body").map_err(|_| "数据无效")? {
        Value::Nil => Ok(None),
        Value::String(text) => text
            .to_str()
            .map(|text| Some(text.to_string()))
            .map_err(|_| "数据无效"),
        _ => Err("数据无效"),
    }
}

fn field_string(table: &Table, key: &str) -> Result<Option<String>, &'static str> {
    match table.get::<Value>(key).map_err(|_| "地址无效")? {
        Value::Nil => Ok(None),
        Value::String(text) => text
            .to_str()
            .map(|text| Some(text.to_string()))
            .map_err(|_| "地址无效"),
        _ => Err("地址无效"),
    }
}

fn decide(inner: &Inner, request: &HttpRequest) -> Gate {
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::new();
    let mut raw = Vec::new();
    for attach in &request.secrets {
        if !seen.insert(attach.id.as_str()) {
            return Gate::Deny(HttpDeny::BadAddress.text());
        }
        let Some(declared) = inner
            .manifest
            .secrets
            .iter()
            .find(|item| item.id == attach.id)
        else {
            return Gate::Deny(HttpDeny::SecretMissing.text());
        };
        let (header, prefix) = match binding(declared, attach) {
            Ok(pair) => pair,
            Err(text) => return Gate::Deny(text),
        };
        let value = match inner.store.borrow().read(&inner.manifest.id, &attach.id) {
            Ok(Some(value)) if !value.is_empty() => value,
            Ok(_) => return Gate::Deny(HttpDeny::SecretMissing.text()),
            Err(_) => return Gate::Deny(HttpDeny::RequestFailed.text()),
        };
        raw.push(value.clone());
        resolved.push(ResolvedSecret {
            header,
            prefix,
            value,
        });
    }
    match prepare(
        request,
        &inner.manifest.domains,
        inner.manifest.post,
        &inner.manifest.headers,
        &resolved,
    ) {
        Ok(outgoing) => {
            if outgoing.headers.len() < resolved.len() {
                return Gate::Deny(HttpDeny::RequestFailed.text());
            }
            let start = outgoing.headers.len() - resolved.len();
            let header_values = outgoing.headers[start..]
                .iter()
                .map(|(_, value)| value.clone())
                .collect();
            Gate::Send {
                outgoing,
                secrets: raw,
                header_values,
            }
        }
        Err(deny) => Gate::Deny(deny.text()),
    }
}

fn binding(
    declared: &crate::manifest::SecretDecl,
    attach: &SecretAttach,
) -> Result<(String, String), &'static str> {
    let Some(header) = declared.header.clone() else {
        return Err(HttpDeny::BadAddress.text());
    };
    if !attach.header.is_empty() && !attach.header.eq_ignore_ascii_case(&header) {
        return Err(HttpDeny::BadAddress.text());
    }
    if attach.prefix_set && attach.prefix != declared.prefix {
        return Err(HttpDeny::BadAddress.text());
    }
    Ok((header, declared.prefix.clone()))
}

fn deliver(
    lua: &Lua,
    callback: &Function,
    ok: bool,
    status: Option<u16>,
    body: &str,
    error: &str,
) -> mlua::Result<()> {
    let result = lua.create_table()?;
    result.set("ok", ok)?;
    if let Some(status) = status {
        result.set("status", status)?;
    }
    result.set("body", body)?;
    if !ok {
        result.set("error", error)?;
    }
    let _guard = crate::pattern::enter_host(lua)?;
    callback.call::<()>(result)
}

impl Inner {
    fn widget_mut(&mut self, id: &str) -> mlua::Result<&mut WidgetState> {
        self.widgets
            .iter_mut()
            .find(|widget| widget.decl.id == id)
            .ok_or_else(|| mlua::Error::runtime(format!("没有组件 {id}")))
    }

    fn set_text(&mut self, id: &str, text: String) -> mlua::Result<()> {
        let widget = self.widget_mut(id)?;
        if widget.decl.kind != WidgetKind::Text {
            return Err(mlua::Error::runtime("组件不是文本"));
        }
        widget.text = text;
        Ok(())
    }

    fn set_label(&mut self, id: &str, label: String) -> mlua::Result<()> {
        let widget = self.widget_mut(id)?;
        if widget.decl.kind != WidgetKind::Button {
            return Err(mlua::Error::runtime("组件不是按钮"));
        }
        widget.label = label;
        Ok(())
    }

    fn set_seconds(&mut self, id: &str, seconds: f64) -> mlua::Result<()> {
        let widget = self.widget_mut(id)?;
        if widget.decl.kind != WidgetKind::Timer {
            return Err(mlua::Error::runtime("组件不是计时"));
        }
        widget.seconds = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
        Ok(())
    }

    fn seconds_of(&self, id: &str) -> mlua::Result<f64> {
        let widget = self
            .widgets
            .iter()
            .find(|widget| widget.decl.id == id)
            .ok_or_else(|| mlua::Error::runtime(format!("没有组件 {id}")))?;
        Ok(widget.seconds)
    }

    fn set_running(&mut self, id: &str, on: bool) -> mlua::Result<()> {
        let widget = self.widget_mut(id)?;
        if widget.decl.kind != WidgetKind::Timer {
            return Err(mlua::Error::runtime("组件不是计时"));
        }
        widget.running = on;
        Ok(())
    }

    fn set_progress(&mut self, id: &str, value: f64) -> mlua::Result<()> {
        let widget = self.widget_mut(id)?;
        if widget.decl.kind != WidgetKind::Progress {
            return Err(mlua::Error::runtime("组件不是进度条"));
        }
        widget.progress = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Ok(())
    }

    fn schedule(
        &mut self,
        lua: &Lua,
        seconds: f64,
        every: bool,
        func: Function,
    ) -> mlua::Result<Value> {
        if !seconds.is_finite() || seconds <= 0.0 {
            self.note("参数无效");
            return Ok(Value::Nil);
        }
        if every && seconds < 30.0 {
            self.note("间隔过短");
            return Ok(Value::Nil);
        }
        let id = self.next_timer;
        self.next_timer += 1;
        let key = lua.create_registry_value(func)?;
        self.timers.push(ServiceTimer {
            id,
            every,
            interval: seconds,
            remaining: seconds,
            key,
        });
        Ok(Value::Integer(id))
    }
}

impl WidgetState {
    fn view(&self) -> WidgetView {
        let kind = match self.decl.kind {
            WidgetKind::Text => WidgetViewKind::Text,
            WidgetKind::Button => WidgetViewKind::Button,
            WidgetKind::Timer => WidgetViewKind::Timer,
            WidgetKind::Progress => WidgetViewKind::Progress,
            WidgetKind::List => WidgetViewKind::List,
            WidgetKind::Input => WidgetViewKind::Input,
            WidgetKind::Toggle => WidgetViewKind::Toggle,
        };
        let caption = match self.decl.kind {
            WidgetKind::Timer => format!("{}", self.seconds.floor() as i64),
            WidgetKind::Toggle => {
                if self.on {
                    format!("{} 开", self.label)
                } else {
                    format!("{} 关", self.label)
                }
            }
            _ => self.text.clone(),
        };
        WidgetView {
            id: self.decl.id.clone(),
            kind,
            caption,
            label: self.label.clone(),
            seconds: self.seconds.floor() as i64,
            running: self.running,
            progress: self.progress,
        }
    }
}

fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(value) => Value::Boolean(*value),
        serde_json::Value::Number(value) => Value::Number(value.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(value) => Value::String(lua.create_string(value)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table()?;
            for (index, item) in items.iter().enumerate() {
                table.set(index + 1, json_to_lua(lua, item)?)?;
            }
            Value::Table(table)
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table()?;
            for (key, item) in map {
                table.set(key.as_str(), json_to_lua(lua, item)?)?;
            }
            Value::Table(table)
        }
    })
}

fn lua_to_json(value: &Value) -> mlua::Result<String> {
    let json = lua_value_to_json(value)?;
    serde_json::to_string(&json).map_err(|err| mlua::Error::runtime(err.to_string()))
}

fn lua_value_to_json(value: &Value) -> mlua::Result<serde_json::Value> {
    Ok(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(value) => serde_json::Value::Bool(*value),
        Value::Integer(value) => serde_json::Value::from(*value),
        Value::Number(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(value) => serde_json::Value::String(value.to_str()?.to_string()),
        Value::Table(table) => table_to_json(table)?,
        _ => return Err(mlua::Error::runtime("json 不能编码这个值")),
    })
}

fn table_to_json(table: &Table) -> mlua::Result<serde_json::Value> {
    let mut array = Vec::new();
    let mut object = serde_json::Map::new();
    let mut saw_key = false;
    let mut sequence = true;
    let mut expect = 1i64;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        saw_key = true;
        match key {
            Value::Integer(index) if sequence && index == expect => {
                array.push(lua_value_to_json(&value)?);
                expect += 1;
            }
            Value::String(key) => {
                sequence = false;
                object.insert(key.to_str()?.to_string(), lua_value_to_json(&value)?);
            }
            _ => return Err(mlua::Error::runtime("json 的键只能是字符串")),
        }
    }
    if !saw_key || sequence {
        Ok(serde_json::Value::Array(array))
    } else {
        Ok(serde_json::Value::Object(object))
    }
}

fn timeout_error(err: &mlua::Error) -> bool {
    display_error(err).contains("脚本超时")
}

fn timeout_text(line: i64) -> String {
    if line >= 0 {
        format!("脚本超时（第 {line} 行）")
    } else {
        "脚本超时".to_string()
    }
}

fn script_text(line: Option<usize>) -> String {
    match line {
        Some(line) => format!("脚本错误（第 {line} 行）"),
        None => "脚本错误".to_string(),
    }
}

fn display_error(err: &mlua::Error) -> String {
    err.to_string()
}

fn line_of(err: &mlua::Error) -> Option<usize> {
    let text = display_error(err);
    let mut line = None;
    for (index, ch) in text.char_indices() {
        if ch != ':' {
            continue;
        }
        let rest = &text[index + 1..];
        let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        if rest[digits.len()..].starts_with(':') {
            line = digits.parse().ok();
        }
    }
    line
}

#[cfg(test)]
mod pattern_limits {
    use super::{RepCount, repetition_from_bytes};

    #[test]
    fn string_repetition_accepts_lua_integer_strings() {
        assert!(matches!(
            repetition_from_bytes(b"2000000"),
            RepCount::Times(2_000_000)
        ));
        assert!(matches!(repetition_from_bytes(b"3"), RepCount::Times(3)));
        assert!(matches!(repetition_from_bytes(b" 3 "), RepCount::Times(3)));
        assert!(matches!(
            repetition_from_bytes(b"2e6"),
            RepCount::Times(2_000_000)
        ));
        assert!(matches!(repetition_from_bytes(b"-1"), RepCount::Times(0)));
        assert!(matches!(
            repetition_from_bytes(b"0x10"),
            RepCount::Times(16)
        ));
        assert!(matches!(repetition_from_bytes(b"1.5"), RepCount::Reject));
        assert!(matches!(
            repetition_from_bytes(b"hello"),
            RepCount::LeaveToLua
        ));
    }
}
