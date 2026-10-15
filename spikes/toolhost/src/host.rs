//! 一个工具一块 Lua 5.4 状态。未授予的服务不放进状态。
//!
//! 计时组件由宿主逐秒倒数，不走 `timer.every`。这次验证用虚拟时钟 `advance`，
//! 窗口上的外壳每一秒调用一次。同一拍里先处理计时组件，再处理 `timer.every` /
//! `timer.after`。谁先，规格没有写，待定。
//!
//! 协程不放入。钩子按 Lua 线程安装，协程是另一条线程，超时钩子管不到。
//! `require` 和多个 Lua 文件是否允许仍待定。
//! `timer.after` 的最短秒数待定，这次任意正数都可以登记。
//! 待办是验证程序里的内存替身，不调用产品的 `TodoCommands`。
//!
//! 内存字节上限仍待定。这里的 `MEMORY_CAP` 只是验证程序的临时天花板，超过后与超时一样中止这次回调，不是产品规则。
//! `xpcall` 换成宿主的函数：处理函数在错误展开之后调用，超时钩子仍然有效。
//! `string.find`、`match`、`gmatch`、`gsub`、`rep` 在进入 C 之前有临时的长度和回溯上限。那些字节数不是产品规则。
//! HTTP 在另一条线程上连接和读取。脚本回调返回之后，宿主才按等待上限取结果并调用 Lua。等待上限的秒数仍待定，这次默认 5 秒。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use mlua::{
    Function, HookTriggers, Lua, LuaOptions, MultiValue, RegistryKey, StdLib, Table, Value, VmState,
};

use crate::http::{HttpDeny, HttpRequest, HttpTransport, Outgoing, SecretAttach, prepare};
use crate::manifest::{Manifest, Origin, WidgetDecl, WidgetKind, load_dir};
use crate::secret::SecretStore;

const CALLBACK_MS: u64 = 1000;
const HOOK_INSTRUCTIONS: u32 = 4_096;
/// 临时内存天花板。产品上的字节数仍待定。
const MEMORY_CAP: usize = 16 * 1024 * 1024;
const MEMORY_HEADROOM: usize = 1024 * 1024;
/// 下面几项只是验证程序的临时天花板。产品上的字节数仍待定。
/// 量词个数决定被搜索文本的上限：0 个 256 KiB，1 个 64 KiB，2 个 4 KiB，3 个 256 字节，
/// 之后每多一个再收紧，8 个及以上直接拒绝。三个 `*` 是 O(n³)，256³ 大约一千六百万步。
const PATTERN_LEN_CAP: usize = 256;
const STRING_REP_CAP: usize = 1024 * 1024;
const HTTP_BUDGET: Duration = Duration::from_secs(5);
/// 回调里再发请求会再等一轮。不停地一轮接一轮，规格没有写。最多 8 轮，避免宿主停不下来。
const HTTP_DRAIN_ROUNDS: u32 = 8;
const SERVICE_NAMES: &[&str] = &["timer", "notify", "storage", "todo", "http", "secret"];

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Durations {
    pub work_min: u32,
    pub break_min: u32,
    pub long_min: u32,
}

impl Durations {
    pub const DEFAULT: Self = Self {
        work_min: 25,
        break_min: 5,
        long_min: 15,
    };

    pub fn check(self) -> Result<(), String> {
        if !(1..=120).contains(&self.work_min) {
            return Err("工作时长只接受 1 至 120 分钟".to_string());
        }
        if !(1..=60).contains(&self.break_min) {
            return Err("休息时长只接受 1 至 60 分钟".to_string());
        }
        if !(1..=60).contains(&self.long_min) {
            return Err("长休息时长只接受 1 至 60 分钟".to_string());
        }
        Ok(())
    }
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

pub struct Options {
    pub todos: Vec<TodoItem>,
    pub durations: Durations,
    pub quiet: bool,
    pub store: Rc<RefCell<dyn SecretStore>>,
    pub transport: Arc<dyn HttpTransport>,
    /// 连接加读取的等待。产品上的秒数仍待定。
    pub http_budget: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            todos: Vec::new(),
            durations: Durations::DEFAULT,
            quiet: false,
            store: Rc::new(RefCell::new(crate::secret::MemorySecretStore::default())),
            transport: Arc::new(DenyTransport),
            http_budget: HTTP_BUDGET,
        }
    }
}

struct DenyTransport;

impl HttpTransport for DenyTransport {
    fn get(&self, _request: &Outgoing) -> Result<crate::http::HttpResponse, ()> {
        Err(())
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
    durations: Durations,
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
}

struct Inflight {
    rx: mpsc::Receiver<Result<crate::http::HttpResponse, ()>>,
    callback: RegistryKey,
    secret: String,
    header_value: Option<String>,
}

pub struct Session {
    inner: Rc<RefCell<Inner>>,
    limit: Arc<Limit>,
    lua: Lua,
    http_budget: Duration,
}

impl Session {
    pub fn open(dir: &std::path::Path, origin: Origin, options: Options) -> Result<Self, String> {
        options.durations.check()?;
        let manifest = load_dir(dir, origin).map_err(|reason| {
            if reason.starts_with("读不到") || reason.starts_with("工具文件夹") {
                reason
            } else if reason.contains("入口脚本")
                || (reason.contains("UTF-8") && reason.contains("脚本"))
            {
                format!("脚本错误：{reason}")
            } else {
                format!("清单无效：{reason}")
            }
        })?;
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
            durations: options.durations,
            quiet: options.quiet,
            storage: BTreeMap::new(),
            timers: Vec::new(),
            next_timer: 1,
            unauthorized: false,
            todo_reads: 0,
            store: options.store,
            transport: options.transport,
            http_sends: 0,
            inflight: Vec::new(),
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
                let callback = self.lua.registry_value::<Function>(&item.callback).ok();
                self.lua.remove_registry_value(item.callback).ok();
                let Some(callback) = callback else {
                    continue;
                };
                let (ok, status, body, error) = match response {
                    Ok(Ok(response)) => {
                        if response_leaks(&response, &item.secret, item.header_value.as_deref()) {
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
                    Ok(Err(())) | Err(_) => {
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
        }
        self.drop_inflight();
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
    install_xpcall(lua, limit)?;
    install_string_limits(lua)?;
    Ok(())
}

/// Lua 自带的 `xpcall` 在处理函数里关掉钩子。这里先 `pcall` 被保护的函数，
/// 错误展开之后再普通调用处理函数，钩子仍然开着。已经超时就不再进入处理函数。
fn install_xpcall(lua: &Lua, limit: Arc<Limit>) -> Result<(), String> {
    let xpcall = lua
        .create_function(move |_, mut args: MultiValue| -> mlua::Result<MultiValue> {
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
        })
        .map_err(|err| format!("脚本错误：{err}"))?;
    lua.globals()
        .set("xpcall", xpcall)
        .map_err(|err| format!("脚本错误：{err}"))?;
    Ok(())
}

fn install_string_limits(lua: &Lua) -> Result<(), String> {
    let library: Table = lua
        .globals()
        .get("string")
        .map_err(|err| format!("脚本错误：{err}"))?;
    for name in ["find", "match", "gmatch", "gsub"] {
        wrap_pattern(lua, &library, name)?;
    }
    wrap_rep(lua, &library)?;
    Ok(())
}

fn wrap_pattern(lua: &Lua, library: &Table, name: &'static str) -> Result<(), String> {
    let original: Function = library
        .get(name)
        .map_err(|err| format!("脚本错误：{err}"))?;
    let wrapper = lua
        .create_function(move |_, args: MultiValue| {
            check_pattern(name, &args)?;
            original.call::<MultiValue>(args)
        })
        .map_err(|err| format!("脚本错误：{err}"))?;
    library
        .set(name, wrapper)
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

fn check_pattern(name: &str, args: &MultiValue) -> mlua::Result<()> {
    let (Some(subject), Some(pattern)) = (args.front(), args.get(1)) else {
        return Ok(());
    };
    let (Some(subject_len), Some(pattern_len)) = (byte_len(subject), byte_len(pattern)) else {
        return Ok(());
    };
    if pattern_len > PATTERN_LEN_CAP {
        return Err(mlua::Error::runtime("模式过于复杂"));
    }
    let plain = name == "find" && args.get(3).is_some_and(is_truthy);
    let quantifiers = if plain {
        0
    } else {
        let Value::String(text) = pattern else {
            return Ok(());
        };
        // `find` / `match` / `gsub` 会剥掉开头的 `^`。`gmatch` 不会，开头的 `^` 在匹配器里是字面量。
        count_quantifiers(&text.as_bytes(), name != "gmatch")
    };
    let Some(cap) = subject_cap(quantifiers) else {
        return Err(mlua::Error::runtime("模式过于复杂"));
    };
    if subject_len > cap {
        return Err(mlua::Error::runtime("文本过长"));
    }
    Ok(())
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

fn subject_cap(quantifiers: usize) -> Option<usize> {
    Some(match quantifiers {
        0 => 256 * 1024,
        1 => 64 * 1024,
        2 => 4 * 1024,
        3 => 256,
        4 => 64,
        5 => 32,
        6 => 16,
        7 => 8,
        _ => return None,
    })
}

/// 按 Lua 5.4 `lstrlib.c` 的 `match` / `classend` 数回溯量词。
///
/// `^` 只有在 `caret_is_anchor` 且位于开头时是锚点。`$` 只有作为模式最后一个字节时是锚点。
/// 其余位置的 `^` / `$` 是字面量，后面的 `*+?-` 算量词。`%b`、`%f` 和 `%0`–`%9` 不带量词后缀。
/// 解析不了的片段按量词计，避免把回溯漏掉。
fn count_quantifiers(pattern: &[u8], caret_is_anchor: bool) -> usize {
    let mut index = 0;
    let mut count = 0;
    if caret_is_anchor && pattern.first() == Some(&b'^') {
        index = 1;
    }
    while index < pattern.len() {
        match pattern_step(pattern, index) {
            PatternStep::Item(next) => index = next,
            PatternStep::Quantified(next) => {
                count += 1;
                index = next;
            }
            PatternStep::Unknown => {
                count += 1;
                count += pattern[index + 1..]
                    .iter()
                    .filter(|byte| matches!(byte, b'*' | b'+' | b'-' | b'?'))
                    .count();
                break;
            }
        }
    }
    count
}

enum PatternStep {
    Item(usize),
    Quantified(usize),
    Unknown,
}

fn pattern_step(pattern: &[u8], index: usize) -> PatternStep {
    match pattern[index] {
        b'(' => {
            let next = if pattern.get(index + 1) == Some(&b')') {
                index + 2
            } else {
                index + 1
            };
            PatternStep::Item(next)
        }
        b')' => PatternStep::Item(index + 1),
        b'$' if index + 1 == pattern.len() => PatternStep::Item(index + 1),
        b'%' => match pattern.get(index + 1).copied() {
            None => PatternStep::Unknown,
            Some(b'b') => {
                if pattern.len() - index < 4 {
                    PatternStep::Unknown
                } else {
                    PatternStep::Item(index + 4)
                }
            }
            Some(b'f') => {
                if pattern.get(index + 2) != Some(&b'[') {
                    PatternStep::Unknown
                } else {
                    match skip_class(pattern, index + 2) {
                        Some(end) => PatternStep::Item(end),
                        None => PatternStep::Unknown,
                    }
                }
            }
            Some(byte) if byte.is_ascii_digit() => PatternStep::Item(index + 2),
            Some(_) => with_suffix(pattern, index + 2),
        },
        b'[' => match skip_class(pattern, index) {
            Some(end) => with_suffix(pattern, end),
            None => PatternStep::Unknown,
        },
        _ => with_suffix(pattern, index + 1),
    }
}

fn with_suffix(pattern: &[u8], index: usize) -> PatternStep {
    if index < pattern.len() && matches!(pattern[index], b'*' | b'+' | b'-' | b'?') {
        PatternStep::Quantified(index + 1)
    } else {
        PatternStep::Item(index)
    }
}

fn skip_class(pattern: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if index < pattern.len() && pattern[index] == b'^' {
        index += 1;
    }
    if index < pattern.len() && pattern[index] == b']' {
        index += 1;
    }
    while index < pattern.len() {
        if pattern[index] == b'%' && index + 1 < pattern.len() {
            index += 2;
            continue;
        }
        if pattern[index] == b']' {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn byte_len(value: &Value) -> Option<usize> {
    match value {
        Value::String(text) => Some(text.as_bytes().len()),
        _ => None,
    }
}

fn is_truthy(value: &Value) -> bool {
    !matches!(value, Value::Nil | Value::Boolean(false))
}

fn response_leaks(
    response: &crate::http::HttpResponse,
    secret: &str,
    header_value: Option<&str>,
) -> bool {
    if secret.is_empty() {
        return false;
    }
    let mut parts = Vec::with_capacity(1 + response.headers.len() * 2);
    parts.push(response.body.as_str());
    for (name, value) in &response.headers {
        parts.push(name.as_str());
        parts.push(value.as_str());
    }
    crate::http::response_exposes_secret(&parts, secret, header_value)
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
    let tool_id = inner.borrow().manifest.id.clone();
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
    if tool_id == "lanwork.pomodoro" {
        bind_pomodoro(lua, inner)?;
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
                set_inner.borrow_mut().storage.insert(key, value);
                Ok(true)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    lua.globals()
        .set("storage", storage)
        .map_err(|err| err.to_string())?;
    Ok(())
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
    Send { outgoing: Outgoing, secret: String },
}

fn http_request(
    lua: &Lua,
    inner: &Rc<RefCell<Inner>>,
    options: Table,
    callback: Function,
) -> mlua::Result<()> {
    let method: String = options.get("method").unwrap_or_default();
    let url: String = options.get("url").unwrap_or_default();
    let secret = options
        .get::<Option<Table>>("secret")?
        .map(|table| SecretAttach {
            id: table.get("id").unwrap_or_default(),
            header: table.get("header").unwrap_or_default(),
            prefix: table.get("prefix").unwrap_or_default(),
        });
    let request = HttpRequest {
        method,
        url,
        secret,
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
        Gate::Send { outgoing, secret } => {
            let transport = Arc::clone(&inner.borrow().transport);
            let header_value = outgoing.header_value.clone();
            let (tx, rx) = mpsc::channel();
            thread::Builder::new()
                .name("toolhost-http".to_string())
                .spawn(move || {
                    let _ = tx.send(transport.get(&outgoing));
                })
                .map_err(|_| mlua::Error::runtime("请求失败"))?;
            let key = lua.create_registry_value(callback)?;
            let mut inner = inner.borrow_mut();
            inner.http_sends += 1;
            inner.inflight.push(Inflight {
                rx,
                callback: key,
                secret,
                header_value,
            });
            Ok(())
        }
    }
}

fn decide(inner: &Inner, request: &HttpRequest) -> Gate {
    let declared = request.secret.as_ref().is_some_and(|secret| {
        inner
            .manifest
            .secrets
            .iter()
            .any(|item| item.id == secret.id)
    });
    let value = if let Some(secret) = &request.secret {
        if declared {
            match inner.store.borrow().read(&inner.manifest.id, &secret.id) {
                Ok(value) => value,
                Err(_) => return Gate::Deny(HttpDeny::RequestFailed.text()),
            }
        } else {
            None
        }
    } else {
        None
    };
    let secret_text = value.clone().unwrap_or_default();
    match prepare(
        request,
        &inner.manifest.domains,
        value.as_deref(),
        declared || request.secret.is_none(),
    ) {
        Ok(outgoing) => Gate::Send {
            outgoing,
            secret: secret_text,
        },
        Err(deny) => Gate::Deny(deny.text()),
    }
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
    callback.call::<()>(result)
}

fn bind_pomodoro(lua: &Lua, inner: Rc<RefCell<Inner>>) -> Result<(), String> {
    let pomodoro = lua.create_table().map_err(|err| err.to_string())?;
    pomodoro
        .set(
            "durations",
            lua.create_function(move |_, (): ()| {
                let durations = inner.borrow().durations;
                Ok((durations.work_min, durations.break_min, durations.long_min))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    lua.globals()
        .set("pomodoro", pomodoro)
        .map_err(|err| err.to_string())?;
    Ok(())
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
    use super::{RepCount, count_quantifiers, repetition_from_bytes};

    #[test]
    fn quantifiers_follow_lua_pattern_grammar() {
        let count = |pattern: &str| count_quantifiers(pattern.as_bytes(), true);
        assert_eq!(count("%d+"), 1);
        assert_eq!(count("%%"), 0);
        assert_eq!(count("%?"), 0);
        assert_eq!(count("[a?]+"), 1);
        assert_eq!(count("%b()"), 0);
        assert_eq!(count(".*a.*"), 2);
        // `%f` 在 lstrlib 里不带量词后缀，后面的 `+` 是字面量。
        assert_eq!(count("%f[%w]+"), 0);
        assert_eq!(count(&"a?".repeat(30)), 30);
        assert_eq!(count("$?"), 1);
        assert_eq!(count("$"), 0);
        assert_eq!(count("$$"), 0);
        assert_eq!(count("a$"), 0);
        assert_eq!(count("^a?"), 1);
        assert_eq!(count("^?"), 0);
        assert_eq!(count("^^?"), 1);
        assert_eq!(count("a^?"), 1);
        assert_eq!(count("(a?)"), 1);
        assert_eq!(count("(a)?"), 0);
        assert_eq!(count("%1*"), 0);
        assert_eq!(count("%ba?"), 0);
        let dollars = format!("{}{}", "$?".repeat(26), "$".repeat(26));
        assert_eq!(count(&dollars), 26);
        assert_eq!(count_quantifiers(b"^?", false), 1);
    }

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
