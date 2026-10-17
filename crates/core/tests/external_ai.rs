//! 外部 AI 的回环 MCP。有效 token、无效 token、开关关闭都走真实 TCP。

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lanwork_core::backup::BackupCommands;
use lanwork_core::external_ai::{CLIENTS_FILE, ExternalAi};
use lanwork_core::notes::NoteCommands;
use lanwork_core::shelves::{IncomingRef, ShelfCommands};
use lanwork_core::storage::{Store, StorePaths};
use lanwork_core::todos::TodoCommands;
use serde_json::Value;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lanwork-external-ai-{nanos}-{seq}"));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct Harness {
    temp: TempDir,
    store: Store,
    todos: TodoCommands,
    notes: NoteCommands,
    ai: ExternalAi,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path.join("data"),
            cache_dir: temp.path.join("cache"),
            user_profile: temp.path.join("profile"),
            local_app_data: temp.path.join("local"),
        })
        .unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let notes = NoteCommands::open(store.clone()).unwrap();
        let ai = ExternalAi::open(store.clone(), todos.clone(), Some(notes.clone()));
        Self {
            temp,
            store,
            todos,
            notes,
            ai,
        }
    }

    fn listen(&self) -> u16 {
        self.ai.set_enabled_at(true, 0).unwrap();
        self.ai.port().unwrap()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.ai.shutdown();
    }
}

fn post(port: u16, token: &str, session: Option<&str>, body: &str) -> String {
    let session = session
        .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n{session}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("连接 127.0.0.1");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    String::from_utf8(buf).expect("响应不是 UTF-8")
}

fn status(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

fn header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    let head = response.split("\r\n\r\n").next()?;
    for line in head.lines().skip(1) {
        let (key, value) = line.split_once(':')?;
        if key.eq_ignore_ascii_case(name) {
            return Some(value.trim());
        }
    }
    None
}

fn body(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
}

fn session_of(port: u16, token: &str) -> String {
    let response = post(
        port,
        token,
        None,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    );
    assert_eq!(status(&response), 200, "initialize 没有成功");
    assert!(body(&response).contains("2025-03-26"));
    header(&response, "mcp-session-id")
        .expect("没有 Mcp-Session-Id")
        .to_owned()
}

fn tool(port: u16, token: &str, session: &str, name: &str, args: &str) -> (bool, Value) {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
    );
    let response = post(port, token, Some(session), &payload);
    assert_eq!(status(&response), 200, "工具调用没有回到 200");
    let parsed: Value = serde_json::from_str(body(&response)).expect("工具响应不是 JSON");
    let is_error = parsed["result"]["isError"].as_bool().unwrap_or(false);
    let text = parsed["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    if is_error {
        (true, Value::String(text))
    } else {
        let value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (false, value)
    }
}

fn titles(todos: &TodoCommands) -> Vec<String> {
    todos
        .lists()
        .unwrap()
        .into_iter()
        .flat_map(|list| list.items.into_iter().map(|item| item.title))
        .collect()
}

#[test]
fn loopback_accepts_a_valid_token_and_refuses_the_rest() {
    let harness = Harness::new();
    assert!(harness.ai.port().is_none());
    let port = harness.listen();
    let issued = harness.ai.authorize("Claude").unwrap();
    let session = session_of(port, &issued.token);

    let (error, created) = tool(
        port,
        &issued.token,
        &session,
        "create_todo",
        r#"{"title":"买菜"}"#,
    );
    assert!(!error);
    assert_eq!(created["title"], "买菜");
    assert_eq!(created["revision"], 1);
    assert_eq!(titles(&harness.todos), ["买菜"]);

    let wrong = post(
        port,
        "lw_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        Some(&session),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"create_todo","arguments":{"title":"不该写下"}}}"#,
    );
    assert_eq!(status(&wrong), 401);
    assert!(body(&wrong).contains("未授权"));
    assert!(!wrong.contains("0123456789abcdef0123456789abcdef"));
    assert_eq!(titles(&harness.todos), ["买菜"]);

    let naked = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(naked.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let naked = String::from_utf8(buf).unwrap();
    assert_eq!(status(&naked), 401);
    assert!(body(&naked).contains("未授权"));

    harness.ai.revoke(&issued.id).unwrap();
    let revoked = post(
        port,
        &issued.token,
        Some(&session),
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"create_todo","arguments":{"title":"撤销后"}}}"#,
    );
    assert_eq!(status(&revoked), 401);
    assert!(body(&revoked).contains("未授权"));
    assert!(!revoked.contains(&issued.token));
    assert_eq!(titles(&harness.todos), ["买菜"]);

    harness.ai.set_enabled(false).unwrap();
    assert!(harness.ai.port().is_none());
    let refused = TcpStream::connect_timeout(
        &std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(500),
    );
    assert!(refused.is_err(), "关掉开关后仍能连上");
}

#[test]
fn invalid_bearer_does_not_create_and_blank_title_does_not_write() {
    let harness = Harness::new();
    let port = harness.listen();
    let issued = harness.ai.authorize("Cursor").unwrap();
    let session = session_of(port, &issued.token);
    let (error, message) = tool(
        port,
        &issued.token,
        &session,
        "create_todo",
        r#"{"title":"   "}"#,
    );
    assert!(error);
    assert_eq!(message, "标题为空");
    assert!(titles(&harness.todos).is_empty());

    let (error, created) = tool(
        port,
        &issued.token,
        &session,
        "create_todo",
        r#"{"title":"第一","due":"2026-03-01","remindAt":"09:30","recurrence":{"rule":"daily","until":"2026-03-03"}}"#,
    );
    assert!(!error);
    let id = created["id"].as_str().unwrap();
    let revision = created["revision"].as_u64().unwrap();
    let (error, _) = tool(
        port,
        &issued.token,
        &session,
        "complete_todo",
        &format!(r#"{{"id":"{id}","revision":{revision}}}"#),
    );
    assert!(!error);
    let items = harness
        .todos
        .lists()
        .unwrap()
        .into_iter()
        .flat_map(|list| list.items)
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 2, "周期完成应生成下一次");
    assert!(
        items
            .iter()
            .any(|item| item.completed && item.title == "第一")
    );

    let live = items.iter().find(|item| item.title == "第一").unwrap();
    let (error, _) = tool(
        port,
        &issued.token,
        &session,
        "delete_todo",
        &format!(r#"{{"id":"{}","revision":{}}}"#, live.id, live.revision),
    );
    assert!(!error);
    let trashed = harness.todos.item(&live.id).unwrap();
    assert!(trashed.item.in_trash());
    let (error, message) = tool(
        port,
        &issued.token,
        &session,
        "delete_todo",
        &format!(
            r#"{{"id":"{}","revision":{}}}"#,
            live.id, trashed.item.revision
        ),
    );
    assert!(error);
    assert_eq!(message, "待办已经在回收站");

    let bad_time = tool(
        port,
        &issued.token,
        &session,
        "create_todo",
        r#"{"title":"提醒","remindAt":"25:00"}"#,
    );
    assert!(bad_time.0);
    assert_eq!(bad_time.1, "提醒时间无效");
}

#[test]
fn notes_follow_the_existing_conflict_and_trash_rules() {
    let harness = Harness::new();
    let port = harness.listen();
    let issued = harness.ai.authorize("Claude").unwrap();
    let session = session_of(port, &issued.token);
    let (error, created) = tool(
        port,
        &issued.token,
        &session,
        "create_note",
        r#"{"title":"","body":"原文","tags":[" 工作 ","工作"],"pinned":true}"#,
    );
    assert!(!error);
    assert_eq!(created["revision"], 1);
    assert_eq!(created["tags"], serde_json::json!(["工作"]));
    let id = created["id"].as_str().unwrap();
    let (error, message) = tool(
        port,
        &issued.token,
        &session,
        "update_note",
        &format!(r#"{{"id":"{id}","revision":0,"body":"覆盖"}}"#),
    );
    assert!(error);
    assert_eq!(message, format!("便签保存冲突：{id}"));
    assert_eq!(harness.notes.get(id).unwrap().body, "原文");
    let (error, saved) = tool(
        port,
        &issued.token,
        &session,
        "update_note",
        &format!(r#"{{"id":"{id}","revision":1,"body":"新正文"}}"#),
    );
    assert!(!error);
    assert_eq!(saved["body"], "新正文");
    assert_eq!(saved["title"], "");
    let revision = saved["revision"].as_u64().unwrap();
    let (error, _) = tool(
        port,
        &issued.token,
        &session,
        "delete_note",
        &format!(r#"{{"id":"{id}","revision":{revision}}}"#),
    );
    assert!(!error);
    assert!(harness.notes.get(id).unwrap().is_deleted());
    let trashed_revision = harness.notes.get(id).unwrap().revision;
    let (error, message) = tool(
        port,
        &issued.token,
        &session,
        "delete_note",
        &format!(r#"{{"id":"{id}","revision":{trashed_revision}}}"#),
    );
    assert!(error);
    assert_eq!(message, format!("便签已在回收站：{id}"));
    let (error, restored) = tool(
        port,
        &issued.token,
        &session,
        "restore_note",
        &format!(r#"{{"id":"{id}","revision":{trashed_revision}}}"#),
    );
    assert!(!error);
    assert!(restored["deletedAt"].is_null());
    assert!(!harness.notes.get(id).unwrap().is_deleted());
}

#[test]
fn export_and_backup_omit_the_token_and_shelf_bytes_stay() {
    let harness = Harness::new();
    let shelves = ShelfCommands::open(harness.store.clone()).unwrap();
    let group = shelves.create_group("盒子").unwrap();
    let marker = harness.temp.path.join("shelf-target");
    std::fs::write(&marker, b"keep").unwrap();
    let stored = marker.to_string_lossy().replace('\\', "/");
    shelves
        .add_refs(
            Some(&group.id),
            &[IncomingRef {
                path: stored,
                folder: false,
            }],
        )
        .unwrap();
    let shelf_path = harness
        .store
        .data_dir()
        .join("shelves")
        .join(format!("{}.json", group.id));
    let before = std::fs::read(&shelf_path).unwrap();

    let port = harness.listen();
    let issued = harness.ai.authorize("Claude").unwrap();
    let session = session_of(port, &issued.token);
    let (error, _) = tool(
        port,
        &issued.token,
        &session,
        "create_todo",
        r#"{"title":"导出"}"#,
    );
    assert!(!error);
    assert_eq!(std::fs::read(&shelf_path).unwrap(), before);
    assert!(marker.is_file());

    harness.store.log_info(&format!("seen {}", issued.token));
    let log = std::fs::read_to_string(harness.store.log_path()).unwrap();
    assert!(!log.contains(&issued.token), "日志留下了 token");
    assert!(log.contains("[redacted]"));

    let dest = harness.temp.path.join("export.zip");
    BackupCommands::open(harness.store.clone())
        .export(&dest, 1_700_000_000_000)
        .unwrap();
    let package = std::fs::read(&dest).unwrap();
    let package = String::from_utf8_lossy(&package);
    assert!(!package.contains(&issued.token), "导出包含有 token 原文");
    assert!(!package.contains(CLIENTS_FILE), "导出包含有授权文件");
    assert!(!package.contains("tokenHash"), "导出包含有 token 哈希");
}

#[test]
fn the_seventeenth_connection_gets_503() {
    let harness = Harness::new();
    let port = harness.listen();
    let mut held = Vec::new();
    for _ in 0..16 {
        held.push(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let response = loop {
        let response = post(
            port,
            "lw_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            None,
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        );
        if status(&response) == 503 {
            break response;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "16 个连接没有占满：{response}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(body(&response).contains("连接过多"), "{response}");
    drop(held);
}

/// 16 个连接占满之后，同时只允许 4 个 503 线程。第 5 个直接丢掉。
/// 对方一直发送时，5 秒到点这些线程退出，后来的连接又能拿到完整的 503。
#[test]
fn busy_reject_cap_drops_the_fifth_and_frees_on_deadline() {
    let harness = Harness::new();
    let port = harness.listen();
    let stop = Arc::new(AtomicBool::new(false));
    let mut held = Vec::new();
    let mut joins = JoinOnDrop {
        stop: Arc::clone(&stop),
        threads: Vec::new(),
    };
    for _ in 0..16 {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let stop = Arc::clone(&stop);
        joins.threads.push(std::thread::spawn(move || {
            let _ = writer.set_write_timeout(Some(Duration::from_secs(1)));
            while !stop.load(Ordering::Relaxed) {
                if writer.write_all(b"P").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }));
        held.push(stream);
    }
    let _held = held;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = post(
            port,
            "lw_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            None,
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        );
        if status(&response) == 503 && body(&response).contains("连接过多") {
            break;
        }
        assert!(Instant::now() < deadline, "16 个连接没有占满：{response}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // 上面这次 503 的短线程要先退出，否则它占着 4 个名额里的一个。
    std::thread::sleep(Duration::from_millis(200));

    let mut ready = Vec::new();
    for _ in 0..4 {
        let (rx, thread) = spawn_busy_peer(port);
        ready.push(rx);
        joins.threads.push(thread);
    }
    let mut full_at = None;
    for rx in &ready {
        let got = rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false);
        assert!(got, "拒绝线程没有回「连接过多」");
        full_at = Some(Instant::now());
    }
    let full_at = full_at.expect("没有占满拒绝名额");

    let (elapsed, text) = read_dropped_overflow(port);
    assert!(
        !text.contains("HTTP/1.1"),
        "超过 4 个拒绝线程时仍回了响应：{text}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "丢掉套接字用了 {elapsed:?}"
    );

    let rest = Duration::from_secs(6).saturating_sub(full_at.elapsed());
    std::thread::sleep(rest);
    let response = post(
        port,
        "lw_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        None,
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
    );
    assert_eq!(status(&response), 503, "{response}");
    assert!(body(&response).contains("连接过多"), "{response}");
}

struct JoinOnDrop {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for JoinOnDrop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// 拿到 503 之后继续发送，让排空停不下来，直到服务端的截止时间。
fn spawn_busy_peer(port: u16) -> (std::sync::mpsc::Receiver<bool>, std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let Ok(mut stream) = TcpStream::connect((Ipv4Addr::LOCALHOST, port)) else {
            let _ = tx.send(false);
            return;
        };
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if stream.write_all(request.as_bytes()).is_err() {
            let _ = tx.send(false);
            return;
        }
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);
        let _ = tx.send(text.contains("连接过多"));
        let junk = [b'a'; 1024];
        let until = Instant::now() + Duration::from_secs(12);
        while Instant::now() < until {
            if stream.write_all(&junk).is_err() {
                break;
            }
        }
    });
    (rx, thread)
}

fn read_dropped_overflow(port: u16) -> (Duration, String) {
    let start = Instant::now();
    let Ok(mut stream) = TcpStream::connect((Ipv4Addr::LOCALHOST, port)) else {
        return (start.elapsed(), String::new());
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut buf = Vec::new();
    if stream.write_all(request.as_bytes()).is_ok() {
        let _ = stream.read_to_end(&mut buf);
    }
    (start.elapsed(), String::from_utf8_lossy(&buf).into_owned())
}

#[test]
fn migrating_drops_the_in_memory_token() {
    let harness = Harness::new();
    harness.listen();
    harness.ai.authorize("Claude").unwrap();
    let old = harness.store.data_dir();
    let blocked = harness.temp.path.join("blocked");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::write(blocked.join(CLIENTS_FILE), b"{}\n").unwrap();
    let err = lanwork_core::backup::migrate_data_dir(&harness.store, &blocked, false, |_| {})
        .unwrap_err();
    assert!(matches!(
        err,
        lanwork_core::backup::MigrateError::AlreadyHasData
    ));
    assert_eq!(harness.store.data_dir(), old);
    assert!(blocked.join(CLIENTS_FILE).is_file());

    let target = harness.temp.path.join("next");
    lanwork_core::backup::migrate_data_dir(&harness.store, &target, false, |_| {}).unwrap();
    assert_eq!(harness.store.data_dir(), target);
    assert!(old.join(CLIENTS_FILE).is_file());
    assert!(!target.join(CLIENTS_FILE).exists());
    harness.ai.reload_auth();
    assert!(!harness.ai.enabled());
    assert!(harness.ai.clients().is_empty());
    assert!(harness.ai.port().is_none());
}
