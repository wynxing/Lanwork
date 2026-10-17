//! 外部 AI。本机回环上的 MCP，只改待办和便签。
//!
//! 开关默认关。关着时不监听。每个客户端单独一把 token，磁盘上只存哈希。
//! 授权文件不进入导出和备份。写入走待办命令和便签命令。

mod http;
mod tools;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::notes::NoteCommands;
use crate::storage::{
    self, Error as StorageError, IoAction, Store, is_supported_schema, lock_mutex,
};
use crate::todos::{TodoCommands, TodoError, TodoPatch};

pub use http::DEFAULT_PORT;

/// 授权记录的文件名。不在 `config.json` 里，也不进入导出和备份。文件名仍待定，这一版用这个。
pub const CLIENTS_FILE: &str = "external-ai.json";

const MAX_NAME_CHARS: usize = 80;

/// 设置页要刷新的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Clients,
    Conflict,
}

/// 已授权的客户端。没有 token。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientView {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

/// 还没选定的外部提交。名称是先到的那个客户端。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictView {
    pub id: String,
    pub title: String,
    pub client: String,
}

/// 授权刚完成时交还的原文。调用方只展示这一次，本结构不进入日志。
pub struct IssuedToken {
    pub id: String,
    pub name: String,
    pub token: String,
}

impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// 设置页和监听用的错误。文字里没有 token，也没有数据目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalAiError {
    BlankName,
    InvalidName,
    Write,
    Listen,
    Schema,
}

impl std::fmt::Display for ExternalAiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlankName => write!(f, "名称为空"),
            Self::InvalidName => write!(f, "名称无效"),
            Self::Write => write!(f, "写盘失败"),
            Self::Listen => write!(f, "无法监听"),
            Self::Schema => write!(f, "schemaVersion 不受支持"),
        }
    }
}

impl std::error::Error for ExternalAiError {}

#[derive(Debug, Clone)]
struct ClientRecord {
    id: String,
    name: String,
    token_hash: String,
    created_at: i64,
}

#[derive(Debug, Clone)]
enum PendingOp {
    Update(TodoPatch),
    Complete,
    Uncomplete,
    Delete,
    Restore,
}

#[derive(Debug, Clone)]
struct Pending {
    id: String,
    title: String,
    client: String,
    op: PendingOp,
}

struct LiveSession {
    client_id: String,
    seen: Instant,
}

struct FileState {
    enabled: bool,
    clients: Vec<ClientRecord>,
    /// 认不出的 `schemaVersion`。不覆盖文件，也不监听。
    refused: bool,
}

struct Shared {
    store: Store,
    todos: TodoCommands,
    notes: Option<NoteCommands>,
    file: Mutex<FileState>,
    sessions: Mutex<HashMap<String, LiveSession>>,
    inflight: Mutex<HashMap<String, usize>>,
    dirty: Arc<Mutex<HashSet<String>>>,
    pending: Arc<Mutex<VecDeque<Pending>>>,
    /// 测试在替换正式文件之前调用，用来模拟这一刻才出现的未写盘修改。
    replace_probe: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    listeners: Mutex<Vec<Sender<Notice>>>,
    enabled: AtomicBool,
    port: AtomicU16,
    connections: AtomicUsize,
    /// 正在回 503 的短线程。不计入 `connections`。
    busy: AtomicUsize,
    /// 会话多久没有请求就不再算已连接。默认 60 秒。
    session_idle_ms: AtomicU64,
}

struct Running {
    shutdown: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// 外部 AI。克隆后仍是同一份授权和同一个监听。
#[derive(Clone)]
pub struct ExternalAi {
    shared: Arc<Shared>,
    listener: Arc<Mutex<Option<Running>>>,
}

impl ExternalAi {
    pub fn open(store: Store, todos: TodoCommands, notes: Option<NoteCommands>) -> Self {
        Self::open_at(store, todos, notes, DEFAULT_PORT)
    }

    /// `port` 为启动时尝试绑定的端口。绑定失败时保留文件里的开关。
    pub fn open_at(
        store: Store,
        todos: TodoCommands,
        notes: Option<NoteCommands>,
        port: u16,
    ) -> Self {
        let file = load_file(&store);
        let enabled = file.enabled && !file.refused;
        let ai = Self {
            shared: Arc::new(Shared {
                store,
                todos,
                notes,
                file: Mutex::new(file),
                sessions: Mutex::new(HashMap::new()),
                inflight: Mutex::new(HashMap::new()),
                dirty: Arc::new(Mutex::new(HashSet::new())),
                pending: Arc::new(Mutex::new(VecDeque::new())),
                replace_probe: Mutex::new(None),
                listeners: Mutex::new(Vec::new()),
                enabled: AtomicBool::new(false),
                port: AtomicU16::new(0),
                connections: AtomicUsize::new(0),
                busy: AtomicUsize::new(0),
                session_idle_ms: AtomicU64::new(60_000),
            }),
            listener: Arc::new(Mutex::new(None)),
        };
        if enabled {
            match ai.start(port) {
                Ok(port) => {
                    ai.shared
                        .store
                        .log_info(&format!("外部 AI 正在监听 127.0.0.1:{port}"));
                }
                Err(_) => {
                    ai.shared.store.log_warn("外部 AI 无法监听");
                }
            }
        }
        ai
    }

    pub fn subscribe(&self) -> Receiver<Notice> {
        let (sender, receiver) = mpsc::channel();
        lock_mutex(&self.shared.listeners).push(sender);
        receiver
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        lock_mutex(&self.shared.file).enabled && !lock_mutex(&self.shared.file).refused
    }

    /// 正在接受连接时的端口。没在听时是 `None`。
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        let port = self.shared.port.load(Ordering::Acquire);
        if port == 0 || !self.shared.enabled.load(Ordering::Acquire) {
            None
        } else {
            Some(port)
        }
    }

    #[must_use]
    pub fn endpoint(&self) -> Option<String> {
        self.port()
            .map(|port| format!("http://127.0.0.1:{port}/mcp"))
    }

    pub fn clients(&self) -> Vec<ClientView> {
        if self.shared.expire_sessions() {
            self.shared.notify(Notice::Clients);
        }
        let file = lock_mutex(&self.shared.file);
        let sessions = lock_mutex(&self.shared.sessions);
        let inflight = lock_mutex(&self.shared.inflight);
        file.clients
            .iter()
            .map(|client| ClientView {
                id: client.id.clone(),
                name: client.name.clone(),
                connected: sessions
                    .values()
                    .any(|session| session.client_id == client.id)
                    || inflight.get(&client.id).is_some_and(|count| *count > 0),
            })
            .collect()
    }

    /// 文件里开关是开的，但是没有在听。设置这一行显示「无法监听」。
    #[must_use]
    pub fn unavailable(&self) -> bool {
        let file = lock_mutex(&self.shared.file);
        file.enabled && !file.refused && self.port().is_none()
    }

    /// 数据目录已经换过。按新目录重读授权，丢掉旧 token 和会话。
    pub fn reload_auth(&self) {
        let file = load_file(&self.shared.store);
        let start = file.enabled && !file.refused;
        *lock_mutex(&self.shared.file) = file;
        self.stop();
        if start {
            match self.start(DEFAULT_PORT) {
                Ok(port) => {
                    self.shared
                        .store
                        .log_info(&format!("外部 AI 正在监听 127.0.0.1:{port}"));
                }
                Err(_) => {
                    self.shared.store.log_warn("外部 AI 无法监听");
                }
            }
        }
        self.notify(Notice::Clients);
    }

    /// 先写授权文件，写成了才开始监听。写盘或监听失败时开关保持关闭。
    pub fn set_enabled(&self, enabled: bool) -> Result<(), ExternalAiError> {
        self.set_enabled_at(enabled, DEFAULT_PORT)
    }

    /// `port` 为 0 时由系统分配。测试用这个避开固定端口。
    pub fn set_enabled_at(&self, enabled: bool, port: u16) -> Result<(), ExternalAiError> {
        if lock_mutex(&self.shared.file).refused {
            return Err(ExternalAiError::Schema);
        }
        self.write_enabled(enabled)?;
        if enabled {
            match self.start(port) {
                Ok(actual) => {
                    self.shared
                        .store
                        .log_info(&format!("外部 AI 正在监听 127.0.0.1:{actual}"));
                    Ok(())
                }
                Err(err) => {
                    self.stop();
                    let _ = self.write_enabled(false);
                    Err(err)
                }
            }
        } else {
            self.stop();
            self.shared.store.log_info("外部 AI 已停止监听");
            Ok(())
        }
    }

    /// 发给这个客户端一把新 token。原文只在返回值里出现一次。
    pub fn authorize(&self, name: &str) -> Result<IssuedToken, ExternalAiError> {
        if lock_mutex(&self.shared.file).refused {
            return Err(ExternalAiError::Schema);
        }
        let name = normalize_name(name)?;
        let token = match issue_token() {
            Ok(token) => token,
            Err(_) => {
                self.shared.store.log_warn("外部 AI 无法生成 token");
                return Err(ExternalAiError::Write);
            }
        };
        self.shared.store.remember_secret(&token);
        let record = ClientRecord {
            id: fresh_id(),
            name: name.clone(),
            token_hash: token_hash(&token),
            created_at: now_ms(),
        };
        let id = record.id.clone();
        {
            let mut file = lock_mutex(&self.shared.file);
            file.clients.push(record);
            if let Err(err) = write_file(&self.shared.store, &file) {
                file.clients.pop();
                return Err(err);
            }
        }
        self.notify(Notice::Clients);
        Ok(IssuedToken { id, name, token })
    }

    /// 这把 token 立刻失效。之后的调用不写盘。
    pub fn revoke(&self, id: &str) -> Result<(), ExternalAiError> {
        if lock_mutex(&self.shared.file).refused {
            return Err(ExternalAiError::Schema);
        }
        let mut file = lock_mutex(&self.shared.file);
        let Some(index) = file.clients.iter().position(|client| client.id == id) else {
            return Ok(());
        };
        let removed = file.clients.remove(index);
        if let Err(err) = write_file(&self.shared.store, &file) {
            file.clients.insert(index, removed);
            return Err(err);
        }
        drop(file);
        lock_mutex(&self.shared.sessions).retain(|_, session| session.client_id != id);
        self.notify(Notice::Clients);
        Ok(())
    }

    /// 待办页上还没选定的冲突。按出现顺序，先来的在前。
    #[must_use]
    pub fn conflicts(&self) -> Vec<ConflictView> {
        lock_mutex(&self.shared.pending)
            .iter()
            .map(|pending| ConflictView {
                id: pending.id.clone(),
                title: pending.title.clone(),
                client: pending.client.clone(),
            })
            .collect()
    }

    /// 丢掉外部这次提交，不写盘。
    pub fn keep_local(&self, id: &str) {
        lock_mutex(&self.shared.pending).retain(|pending| pending.id != id);
        self.notify(Notice::Conflict);
    }

    /// 把外部提交的字段写下去。省略的字段保持磁盘上现在的值。
    ///
    /// 这是用户已经看见提示之后的选择。开关关掉或 token 撤销之后仍然写。
    pub fn keep_external(&self, id: &str) -> Result<(), String> {
        let pending = lock_mutex(&self.shared.pending)
            .iter()
            .find(|pending| pending.id == id)
            .cloned();
        let Some(pending) = pending else {
            return Ok(());
        };
        let result = apply_pending(&self.shared, &pending);
        if result.is_ok() {
            lock_mutex(&self.shared.pending).retain(|item| item.id != id);
            self.notify(Notice::Conflict);
        }
        result
    }

    /// 本机还没写盘的待办。外部再改这些 id 时不覆盖。
    pub fn set_dirty(&self, ids: HashSet<String>) {
        *lock_mutex(&self.shared.dirty) = ids;
    }

    #[must_use]
    pub fn is_dirty(&self, id: &str) -> bool {
        self.shared.dirty(id)
    }

    /// 测试把会话空闲改短。正式路径是 60 秒。
    #[cfg(test)]
    pub fn set_session_idle(&self, idle: Duration) {
        let millis = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
        self.shared.session_idle_ms.store(millis, Ordering::Release);
    }

    /// 测试在向前替换正式文件之前插入动作。回滚和修复不调用它。
    #[cfg(test)]
    pub fn set_replace_probe(&self, probe: Option<Arc<dyn Fn() + Send + Sync>>) {
        *lock_mutex(&self.shared.replace_probe) = probe;
    }

    /// 停掉监听。测试在临时目录删掉之前调用。
    pub fn shutdown(&self) {
        let _ = self.set_enabled(false);
    }

    fn start(&self, port: u16) -> Result<u16, ExternalAiError> {
        self.stop();
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .map_err(|_| ExternalAiError::Listen)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| ExternalAiError::Listen)?;
        let actual = listener
            .local_addr()
            .map_err(|_| ExternalAiError::Listen)?
            .port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(&self.shared);
        let flag = Arc::clone(&shutdown);
        let thread = std::thread::Builder::new()
            .name("lanwork-mcp".to_owned())
            .spawn(move || http::accept_loop(listener, shared, flag))
            .map_err(|_| ExternalAiError::Listen)?;
        *lock_mutex(&self.listener) = Some(Running { shutdown, thread });
        self.shared.port.store(actual, Ordering::Release);
        self.shared.enabled.store(true, Ordering::Release);
        Ok(actual)
    }

    fn stop(&self) {
        self.shared.enabled.store(false, Ordering::Release);
        let running = lock_mutex(&self.listener).take();
        if let Some(running) = running {
            running.shutdown.store(true, Ordering::Release);
            let _ = running.thread.join();
        }
        self.shared.port.store(0, Ordering::Release);
        lock_mutex(&self.shared.sessions).clear();
        lock_mutex(&self.shared.inflight).clear();
        self.notify(Notice::Clients);
    }

    fn write_enabled(&self, enabled: bool) -> Result<(), ExternalAiError> {
        let mut file = lock_mutex(&self.shared.file);
        if file.refused {
            return Err(ExternalAiError::Schema);
        }
        let previous = file.enabled;
        file.enabled = enabled;
        if let Err(err) = write_file(&self.shared.store, &file) {
            file.enabled = previous;
            return Err(err);
        }
        Ok(())
    }

    fn notify(&self, notice: Notice) {
        self.shared.notify(notice);
    }
}

impl Shared {
    fn notify(&self, notice: Notice) {
        let mut listeners = lock_mutex(&self.listeners);
        listeners.retain(|sender| sender.send(notice).is_ok());
    }

    fn invoke(&self, token: &str, tool: &str, args: &Value) -> Result<Value, String> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err("未授权".to_owned());
        }
        let Some(client_id) = self.authenticate(token) else {
            return Err("未授权".to_owned());
        };
        self.bump_inflight(&client_id, 1);
        self.notify(Notice::Clients);
        let name = self.client_name(&client_id);
        let result = tools::call(self, &name, tool, args);
        self.bump_inflight(&client_id, -1);
        self.notify(Notice::Clients);
        result
    }

    fn authenticate(&self, token: &str) -> Option<String> {
        if token.is_empty() || token.len() > 512 {
            return None;
        }
        let hash = token_hash(token);
        let file = lock_mutex(&self.file);
        let mut found = None;
        for client in &file.clients {
            if ct_eq(client.token_hash.as_bytes(), hash.as_bytes()) {
                found = Some(client.id.clone());
            }
        }
        found
    }

    fn bump_inflight(&self, id: &str, delta: i32) {
        let mut inflight = lock_mutex(&self.inflight);
        let count = inflight.entry(id.to_owned()).or_insert(0);
        if delta < 0 {
            *count = count.saturating_sub(1);
        } else {
            *count = count.saturating_add(delta as usize);
        }
        if *count == 0 {
            inflight.remove(id);
        }
    }

    fn remember_session(&self, session: String, client_id: String) {
        lock_mutex(&self.sessions).insert(
            session,
            LiveSession {
                client_id,
                seen: Instant::now(),
            },
        );
        self.notify(Notice::Clients);
    }

    fn session_owner(&self, session: &str) -> Option<String> {
        if self.expire_sessions() {
            self.notify(Notice::Clients);
        }
        let mut sessions = lock_mutex(&self.sessions);
        let live = sessions.get_mut(session)?;
        live.seen = Instant::now();
        Some(live.client_id.clone())
    }

    fn end_session(&self, session: &str, client_id: &str) -> bool {
        let mut sessions = lock_mutex(&self.sessions);
        match sessions.get(session) {
            Some(live) if live.client_id == client_id => {
                sessions.remove(session);
                drop(sessions);
                self.notify(Notice::Clients);
                true
            }
            _ => false,
        }
    }

    fn expire_sessions(&self) -> bool {
        let idle = Duration::from_millis(self.session_idle_ms.load(Ordering::Acquire));
        let mut sessions = lock_mutex(&self.sessions);
        let before = sessions.len();
        sessions.retain(|_, session| session.seen.elapsed() < idle);
        sessions.len() != before
    }

    fn client_name(&self, id: &str) -> String {
        lock_mutex(&self.file)
            .clients
            .iter()
            .find(|client| client.id == id)
            .map(|client| client.name.clone())
            .unwrap_or_default()
    }

    fn store_conflict(&self, id: &str, title: &str, client: &str, op: PendingOp) {
        let mut pending = lock_mutex(&self.pending);
        if pending.iter().any(|item| item.id == id) {
            return;
        }
        pending.push_back(Pending {
            id: id.to_owned(),
            title: title.to_owned(),
            client: client.to_owned(),
            op,
        });
        drop(pending);
        self.notify(Notice::Conflict);
    }

    /// 每一次向前替换之前再看一次未写盘的修改。看见了就不替换。
    ///
    /// 一次写入里前面已经换过的文件，由待办服务写回操作前的内容。那次回写和修复不走这个检查。
    fn guarded_todo<T>(
        &self,
        id: &str,
        body: impl FnOnce() -> Result<T, TodoError>,
    ) -> Result<T, TodoError> {
        if self.dirty(id) || self.conflict_pending(id) {
            return Err(TodoError::Conflict { id: id.to_owned() });
        }
        let tripped = Arc::new(AtomicBool::new(false));
        let tripped_hook = Arc::clone(&tripped);
        let dirty = Arc::clone(&self.dirty);
        let pending = Arc::clone(&self.pending);
        let item = id.to_owned();
        let probe = lock_mutex(&self.replace_probe).clone();
        let hook = Arc::new(move || {
            if let Some(probe) = probe.as_ref() {
                probe();
            }
            let blocked = lock_mutex(&dirty).contains(&item)
                || lock_mutex(&pending)
                    .iter()
                    .any(|pending| pending.id == item);
            if blocked {
                tripped_hook.store(true, Ordering::Release);
            }
            !blocked
        });
        crate::storage::set_before_replace(Some(hook));
        let _clear = ReplaceHook;
        let result = body();
        drop(_clear);
        if tripped.load(Ordering::Acquire) {
            return Err(TodoError::Conflict { id: id.to_owned() });
        }
        result
    }

    fn todo_title(&self, id: &str) -> String {
        self.todos
            .item(id)
            .map(|stored| stored.item.title)
            .unwrap_or_else(|_| id.to_owned())
    }

    fn dirty(&self, id: &str) -> bool {
        lock_mutex(&self.dirty).contains(id)
    }

    fn conflict_pending(&self, id: &str) -> bool {
        lock_mutex(&self.pending).iter().any(|item| item.id == id)
    }
}

struct ReplaceHook;

impl Drop for ReplaceHook {
    fn drop(&mut self) {
        crate::storage::set_before_replace(None);
    }
}

fn apply_pending(shared: &Shared, pending: &Pending) -> Result<(), String> {
    let result = match &pending.op {
        PendingOp::Update(patch) => shared
            .todos
            .apply_patch(&pending.id, None, patch.clone())
            .map(|_| ()),
        PendingOp::Complete => shared.todos.complete_item(&pending.id),
        PendingOp::Uncomplete => shared.todos.uncomplete_item(&pending.id),
        PendingOp::Delete => shared.todos.soft_delete(&pending.id),
        PendingOp::Restore => shared.todos.restore(&pending.id),
    };
    result.map_err(|err| tools::todo_message(&err))
}

fn normalize_name(name: &str) -> Result<String, ExternalAiError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ExternalAiError::BlankName);
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(|ch| ch.is_control()) {
        return Err(ExternalAiError::InvalidName);
    }
    Ok(name.to_owned())
}

fn issue_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)?;
    let mut token = String::from("lw_");
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    Ok(token)
}

fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn ct_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn fresh_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("c{:x}-{seq:x}", now_ms())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn load_file(store: &Store) -> FileState {
    let path = store.data_dir().join(CLIENTS_FILE);
    if !storage::fs_exists(&path) {
        return FileState {
            enabled: false,
            clients: Vec::new(),
            refused: false,
        };
    }
    let bytes = match storage::fs_read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            store.log_warn(&StorageError::io(IoAction::Read, &path, err).to_string());
            return FileState {
                enabled: false,
                clients: Vec::new(),
                refused: false,
            };
        }
    };
    match parse_file(&bytes) {
        Ok(Parse::State(state)) => state,
        Ok(Parse::Unsupported) => {
            store.log_warn("外部 AI 授权文件的 schemaVersion 不受支持");
            FileState {
                enabled: false,
                clients: Vec::new(),
                refused: true,
            }
        }
        Err(err) => {
            quarantine(store, &path, &err);
            FileState {
                enabled: false,
                clients: Vec::new(),
                refused: false,
            }
        }
    }
}

enum Parse {
    State(FileState),
    Unsupported,
}

fn parse_file(bytes: &[u8]) -> Result<Parse, FileError> {
    let value: Value = serde_json::from_slice(bytes).map_err(FileError::Json)?;
    let Some(obj) = value.as_object() else {
        return Err(FileError::NotObject);
    };
    match obj.get("schemaVersion") {
        None => {}
        Some(Value::Number(number)) => match number.as_u64() {
            Some(version) if is_supported_schema(u32::try_from(version).unwrap_or(u32::MAX)) => {}
            _ => return Ok(Parse::Unsupported),
        },
        Some(_) => return Ok(Parse::Unsupported),
    }
    let enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let mut clients = Vec::new();
    if let Some(entries) = obj.get("clients").and_then(Value::as_array) {
        for entry in entries {
            if let Some(client) = client_of(entry) {
                clients.push(client);
            }
        }
    }
    Ok(Parse::State(FileState {
        enabled,
        clients,
        refused: false,
    }))
}

fn client_of(value: &Value) -> Option<ClientRecord> {
    let obj = value.as_object()?;
    let id = obj.get("id")?.as_str()?.trim();
    let name = obj.get("name")?.as_str()?.trim();
    let token_hash = obj.get("tokenHash")?.as_str()?.trim();
    let created_at = obj.get("createdAt")?.as_i64()?;
    if storage::validate_id(id).is_err() || name.is_empty() || token_hash.len() != 64 {
        return None;
    }
    if !token_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(ClientRecord {
        id: id.to_owned(),
        name: name.to_owned(),
        token_hash: token_hash.to_ascii_lowercase(),
        created_at,
    })
}

fn write_file(store: &Store, file: &FileState) -> Result<(), ExternalAiError> {
    let clients: Vec<_> = file
        .clients
        .iter()
        .map(|client| {
            json!({
                "id": client.id,
                "name": client.name,
                "tokenHash": client.token_hash,
                "createdAt": client.created_at,
            })
        })
        .collect();
    let mut bytes = serde_json::to_vec(&json!({
        "schemaVersion": storage::SCHEMA_VERSION,
        "enabled": file.enabled,
        "clients": clients,
    }))
    .map_err(|_| ExternalAiError::Write)?;
    bytes.push(b'\n');
    let path = store.data_dir().join(CLIENTS_FILE);
    store.with_write_lock(|| {
        if let Some(parent) = path.parent() {
            storage::fs_create_dir_all(parent).map_err(|err| {
                store.log_warn(&StorageError::io(IoAction::CreateDir, parent, err).to_string());
                ExternalAiError::Write
            })?;
        }
        storage::atomic_write(&path, &bytes).map_err(|err| {
            store.log_warn(&StorageError::io(IoAction::Replace, &path, err).to_string());
            ExternalAiError::Write
        })
    })
}

enum FileError {
    Json(serde_json::Error),
    NotObject,
}

impl FileError {
    fn line(&self) -> usize {
        match self {
            Self::Json(err) => err.line(),
            Self::NotObject => 1,
        }
    }

    fn column(&self) -> usize {
        match self {
            Self::Json(err) => err.column(),
            Self::NotObject => 1,
        }
    }
}

fn quarantine(store: &Store, path: &std::path::Path, err: &FileError) {
    let file_name = path.file_name().unwrap_or_default();
    let mut name = std::ffi::OsString::from(file_name);
    name.push(format!(".corrupt-{}", now_ms()));
    let target = path.with_file_name(name);
    store.with_write_lock(|| match std::fs::rename(path, &target) {
        Ok(()) => store.log_warn(&format!(
            "json quarantined path={} quarantine={} line={} column={}",
            path.display(),
            target.display(),
            err.line(),
            err.column(),
        )),
        Err(source) => store.log_warn(&format!(
            "json quarantine rename failed path={} line={} column={} os={:?}",
            path.display(),
            err.line(),
            err.column(),
            source.raw_os_error(),
        )),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::NoteCommands;
    use crate::storage::{Store, StorePaths};
    use crate::todos::TodoCommands;

    fn opened() -> (crate::storage::test_temp::TempDir, ExternalAi) {
        let temp = crate::storage::test_temp::TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            user_profile: temp.path().join("profile"),
            local_app_data: temp.path().join("local"),
        })
        .unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let notes = NoteCommands::open(store.clone()).unwrap();
        let ai = ExternalAi::open(store, todos, Some(notes));
        (temp, ai)
    }

    #[test]
    fn switch_starts_off_and_names_are_checked() {
        let (_temp, ai) = opened();
        assert!(!ai.enabled());
        assert!(ai.port().is_none());
        assert!(ai.endpoint().is_none());
        assert_eq!(ai.authorize("  ").unwrap_err(), ExternalAiError::BlankName);
        assert_eq!(
            ai.authorize("a\nb").unwrap_err(),
            ExternalAiError::InvalidName
        );
        assert_eq!(
            ai.authorize(&"名".repeat(81)).unwrap_err(),
            ExternalAiError::InvalidName
        );
        assert!(ai.clients().is_empty());
    }

    #[test]
    fn disk_keeps_the_hash_and_a_revoked_token_cannot_write() {
        let (_temp, ai) = opened();
        ai.set_enabled_at(true, 0).unwrap();
        let issued = ai.authorize("Claude").unwrap();
        let path = ai.shared.store.data_dir().join(CLIENTS_FILE);
        let bytes = std::fs::read(&path).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains(&issued.token), "授权文件含有 token 原文");
        assert!(text.contains(&token_hash(&issued.token)));
        let created = ai
            .shared
            .invoke(&issued.token, "create_todo", &json!({ "title": "买菜" }))
            .unwrap();
        assert_eq!(created["title"], "买菜");
        assert_eq!(created["revision"], 1);
        ai.revoke(&issued.id).unwrap();
        let denied = ai.shared.invoke(
            &issued.token,
            "create_todo",
            &json!({ "title": "不该写下" }),
        );
        assert_eq!(denied.unwrap_err(), "未授权");
        let lists = ai.shared.todos.lists().unwrap();
        let titles: Vec<_> = lists
            .iter()
            .flat_map(|list| list.items.iter().map(|item| item.title.as_str()))
            .collect();
        assert_eq!(titles, ["买菜"]);
        ai.shutdown();
    }

    #[test]
    fn dirty_or_stale_revision_does_not_write_until_keep_external() {
        let (_temp, ai) = opened();
        ai.set_enabled_at(true, 0).unwrap();
        let issued = ai.authorize("Cursor").unwrap();
        let created = ai
            .shared
            .invoke(&issued.token, "create_todo", &json!({ "title": "本机" }))
            .unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        ai.set_dirty(HashSet::from([id.clone()]));
        let conflict = ai.shared.invoke(
            &issued.token,
            "update_todo",
            &json!({ "id": id, "revision": 1, "title": "外部一" }),
        );
        assert_eq!(conflict.unwrap_err(), format!("待办保存冲突：{id}"));
        assert_eq!(ai.shared.todos.item(&id).unwrap().item.title, "本机");
        assert_eq!(ai.conflicts()[0].client, "Cursor");
        let again = ai.shared.invoke(
            &issued.token,
            "update_todo",
            &json!({ "id": id, "revision": 1, "title": "外部二" }),
        );
        assert_eq!(again.unwrap_err(), format!("待办保存冲突：{id}"));
        ai.set_dirty(HashSet::new());
        ai.keep_external(&id).unwrap();
        assert_eq!(ai.shared.todos.item(&id).unwrap().item.title, "外部一");
        let stale = ai.shared.invoke(
            &issued.token,
            "update_todo",
            &json!({ "id": id, "revision": 1, "title": "外部三" }),
        );
        assert_eq!(stale.unwrap_err(), format!("待办保存冲突：{id}"));
        assert_eq!(ai.shared.todos.item(&id).unwrap().item.title, "外部一");
        ai.keep_local(&id);
        assert!(ai.conflicts().is_empty());
        assert_eq!(ai.shared.todos.item(&id).unwrap().item.title, "外部一");
        ai.shutdown();
    }

    #[test]
    fn a_keystroke_during_replace_conflicts_and_leaves_the_file() {
        let (_temp, ai) = opened();
        ai.set_enabled_at(true, 0).unwrap();
        let issued = ai.authorize("Claude").unwrap();
        let created = ai
            .shared
            .invoke(&issued.token, "create_todo", &json!({ "title": "本机" }))
            .unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        let list_id = created["listId"].as_str().unwrap();
        let path = ai
            .shared
            .store
            .data_dir()
            .join("todos")
            .join(format!("{list_id}.json"));
        let before = std::fs::read(&path).unwrap();
        let marker = ai.clone();
        let marked = id.clone();
        ai.set_replace_probe(Some(Arc::new(move || {
            marker.set_dirty(HashSet::from([marked.clone()]));
        })));
        let conflict = ai.shared.invoke(
            &issued.token,
            "update_todo",
            &json!({ "id": id, "revision": 1, "title": "外部" }),
        );
        assert_eq!(conflict.unwrap_err(), format!("待办保存冲突：{id}"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(ai.shared.todos.item(&id).unwrap().item.title, "本机");
        assert_eq!(ai.conflicts()[0].client, "Claude");
        ai.set_replace_probe(None);
        ai.shutdown();
    }

    #[test]
    fn a_keystroke_during_the_second_replace_of_a_move_restores_both_files() {
        let (_temp, ai) = opened();
        ai.set_enabled_at(true, 0).unwrap();
        let issued = ai.authorize("Claude").unwrap();
        let source = ai.shared.todos.create_list("来源").unwrap();
        let target = ai.shared.todos.create_list("目标").unwrap();
        let created = ai
            .shared
            .invoke(
                &issued.token,
                "create_todo",
                &json!({ "title": "本机", "listId": source }),
            )
            .unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        let revision = created["revision"].as_u64().unwrap();
        let dir = ai.shared.store.data_dir().join("todos");
        let source_path = dir.join(format!("{source}.json"));
        let target_path = dir.join(format!("{target}.json"));
        let source_before = std::fs::read(&source_path).unwrap();
        let target_before = std::fs::read(&target_path).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_probe = Arc::clone(&hits);
        let marker = ai.clone();
        let marked = id.clone();
        ai.set_replace_probe(Some(Arc::new(move || {
            if hits_probe.fetch_add(1, Ordering::SeqCst) == 1 {
                marker.set_dirty(HashSet::from([marked.clone()]));
            }
        })));
        let conflict = ai.shared.invoke(
            &issued.token,
            "update_todo",
            &json!({ "id": id, "revision": revision, "listId": target }),
        );
        assert_eq!(conflict.unwrap_err(), format!("待办保存冲突：{id}"));
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(std::fs::read(&source_path).unwrap(), source_before);
        assert_eq!(std::fs::read(&target_path).unwrap(), target_before);
        let stored = ai.shared.todos.item(&id).unwrap();
        assert_eq!(stored.list_id, source);
        assert_eq!(stored.item.title, "本机");
        ai.set_replace_probe(None);
        ai.shutdown();
    }

    #[test]
    fn keep_external_still_writes_after_the_switch_is_off() {
        let (_temp, ai) = opened();
        ai.set_enabled_at(true, 0).unwrap();
        let issued = ai.authorize("Claude").unwrap();
        let created = ai
            .shared
            .invoke(&issued.token, "create_todo", &json!({ "title": "本机" }))
            .unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        ai.set_dirty(HashSet::from([id.clone()]));
        let conflict = ai.shared.invoke(
            &issued.token,
            "update_todo",
            &json!({ "id": id, "revision": 1, "title": "外部" }),
        );
        assert!(conflict.unwrap_err().starts_with("待办保存冲突"));
        ai.set_enabled(false).unwrap();
        ai.revoke(&issued.id).unwrap();
        ai.set_dirty(HashSet::new());
        ai.keep_external(&id).unwrap();
        assert_eq!(ai.shared.todos.item(&id).unwrap().item.title, "外部");
        assert!(ai.conflicts().is_empty());
        ai.shutdown();
    }

    #[test]
    fn saved_switch_stays_on_when_the_port_is_taken() {
        let temp = crate::storage::test_temp::TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            user_profile: temp.path().join("profile"),
            local_app_data: temp.path().join("local"),
        })
        .unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let notes = NoteCommands::open(store.clone()).unwrap();
        std::fs::create_dir_all(store.data_dir()).unwrap();
        std::fs::write(
            store.data_dir().join(CLIENTS_FILE),
            b"{\"schemaVersion\":1,\"enabled\":true,\"clients\":[]}\n",
        )
        .unwrap();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let ai = ExternalAi::open_at(store, todos, Some(notes), port);
        assert!(ai.enabled());
        assert!(ai.unavailable());
        assert!(ai.port().is_none());
        let saved = std::fs::read_to_string(ai.shared.store.data_dir().join(CLIENTS_FILE)).unwrap();
        assert!(saved.contains("\"enabled\": true") || saved.contains("\"enabled\":true"));
        drop(listener);
        ai.set_enabled_at(true, port).unwrap();
        assert!(!ai.unavailable());
        assert_eq!(ai.port(), Some(port));
        ai.shutdown();
    }

    #[test]
    fn deleting_a_session_or_letting_it_idle_clears_connected() {
        let (_temp, ai) = opened();
        ai.set_enabled_at(true, 0).unwrap();
        let issued = ai.authorize("Claude").unwrap();
        let port = ai.port().unwrap();
        let init_body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{}}}"#;
        let init = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{init_body}",
            issued.token,
            init_body.len()
        );
        let response =
            String::from_utf8(http::handle_http(&ai.shared, true, init.as_bytes())).unwrap();
        let session = response
            .split("\r\n")
            .find_map(|line| line.strip_prefix("Mcp-Session-Id: "))
            .unwrap()
            .trim()
            .to_owned();
        assert!(ai.clients()[0].connected);
        let delete = format!(
            "DELETE /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nMcp-Session-Id: {session}\r\nContent-Length: 0\r\n\r\n",
            issued.token
        );
        let deleted =
            String::from_utf8(http::handle_http(&ai.shared, true, delete.as_bytes())).unwrap();
        assert!(deleted.starts_with("HTTP/1.1 204"));
        assert!(!ai.clients()[0].connected);

        let again =
            String::from_utf8(http::handle_http(&ai.shared, true, init.as_bytes())).unwrap();
        let session = again
            .split("\r\n")
            .find_map(|line| line.strip_prefix("Mcp-Session-Id: "))
            .unwrap()
            .trim()
            .to_owned();
        assert!(ai.clients()[0].connected);
        ai.set_session_idle(Duration::ZERO);
        assert!(!ai.clients()[0].connected);
        let ping_body = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
        let ping = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nMcp-Session-Id: {session}\r\nContent-Length: {}\r\n\r\n{ping_body}",
            issued.token,
            ping_body.len()
        );
        let stale =
            String::from_utf8(http::handle_http(&ai.shared, true, ping.as_bytes())).unwrap();
        assert!(stale.contains("未授权"));
        ai.shutdown();
    }

    #[test]
    fn query_string_and_off_loopback_do_not_answer_with_the_token() {
        let (_temp, ai) = opened();
        ai.set_enabled_at(true, 0).unwrap();
        let issued = ai.authorize("Claude").unwrap();
        let port = ai.port().unwrap();
        let raw = format!(
            "POST /mcp?token={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 2\r\n\r\n{{}}",
            token = issued.token,
        );
        let response = http::handle_http(&ai.shared, true, raw.as_bytes());
        let text = String::from_utf8(response).unwrap();
        assert!(text.contains("未授权"));
        assert!(!text.contains(&issued.token));
        let silent = http::handle_http(&ai.shared, false, raw.as_bytes());
        assert!(silent.is_empty());
        ai.shutdown();
    }
}
