//! 本机回环上的 MCP HTTP。只接受 `127.0.0.1`，一次连接只处理一个请求。

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Notice, Shared, fresh_id};

pub const DEFAULT_PORT: u16 = 39217;

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 16;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const SUPPORTED_PROTOCOLS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
const DEFAULT_PROTOCOL: &str = "2025-03-26";

pub(super) fn accept_loop(listener: TcpListener, shared: Arc<Shared>, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                // 监听套接字是非阻塞的。Windows 上 accept 出来的套接字会继承这一点，
                // 空读会立刻返回，连接计数马上放掉。先改回阻塞，读超时才算数。
                let _ = stream.set_nonblocking(false);
                if shared.expire_sessions() {
                    shared.notify(Notice::Clients);
                }
                if !try_acquire(&shared) {
                    let mut stream = stream;
                    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
                    let _ = stream.write_all(&http_response(
                        503,
                        "Service Unavailable",
                        &[],
                        error_body(None, "连接过多"),
                    ));
                    continue;
                }
                let task = Arc::clone(&shared);
                let spawned = std::thread::Builder::new()
                    .name("lanwork-mcp-conn".to_owned())
                    .spawn(move || {
                        let _guard = ConnectionGuard(Arc::clone(&task));
                        serve_one(&task, stream);
                    });
                if spawned.is_err() {
                    release(&shared);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(_) => {
                if shutdown.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    }
}

struct ConnectionGuard(Arc<Shared>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        release(&self.0);
    }
}

fn try_acquire(shared: &Shared) -> bool {
    let mut current = shared.connections.load(Ordering::Relaxed);
    loop {
        if current >= MAX_CONNECTIONS {
            return false;
        }
        match shared.connections.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

fn release(shared: &Shared) {
    shared.connections.fetch_sub(1, Ordering::AcqRel);
}

fn serve_one(shared: &Shared, mut stream: TcpStream) {
    let from_loopback = stream
        .peer_addr()
        .is_ok_and(|addr| matches!(addr.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST));
    if !from_loopback {
        return;
    }
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let raw = match read_message(&mut stream) {
        Ok(raw) => raw,
        Err(ReadFail::TooLarge) => {
            let _ = stream.write_all(&http_response(
                400,
                "Bad Request",
                &[],
                error_body(None, "参数无效"),
            ));
            return;
        }
        Err(ReadFail::Closed) => return,
    };
    let response = handle_http(shared, true, &raw);
    let _ = stream.write_all(&response);
}

enum ReadFail {
    Closed,
    TooLarge,
}

fn read_message(stream: &mut TcpStream) -> Result<Vec<u8>, ReadFail> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    let header_end = loop {
        if buf.len() > MAX_HEADER {
            return Err(ReadFail::TooLarge);
        }
        match stream.read(&mut tmp) {
            Ok(0) => return Err(ReadFail::Closed),
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(end) = find_sep(&buf) {
                    break end;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ReadFail::Closed),
        }
    };
    let head = std::str::from_utf8(&buf[..header_end]).map_err(|_| ReadFail::Closed)?;
    let length = content_length(head).map_err(|_| ReadFail::Closed)?;
    if length > MAX_BODY {
        return Err(ReadFail::TooLarge);
    }
    while buf.len() < header_end + 4 + length {
        match stream.read(&mut tmp) {
            Ok(0) => return Err(ReadFail::Closed),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ReadFail::Closed),
        }
        if buf.len() > header_end + 4 + MAX_BODY {
            return Err(ReadFail::TooLarge);
        }
    }
    buf.truncate(header_end + 4 + length);
    Ok(buf)
}

fn find_sep(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &str) -> Result<usize, ()> {
    let mut found = None;
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            if found.is_some() {
                return Err(());
            }
            let parsed = value.trim().parse::<usize>().map_err(|_| ())?;
            found = Some(parsed);
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(());
        }
    }
    Ok(found.unwrap_or(0))
}

/// 处理一份已经读完的 HTTP 请求。`from_loopback` 为假时不回答。
pub(super) fn handle_http(shared: &Shared, from_loopback: bool, raw: &[u8]) -> Vec<u8> {
    if !from_loopback {
        return Vec::new();
    }
    let Some(end) = find_sep(raw) else {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    };
    let Ok(head) = std::str::from_utf8(&raw[..end]) else {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    };
    let body = raw.get(end + 4..).unwrap_or_default();
    dispatch(shared, head, body)
}

fn dispatch(shared: &Shared, head: &str, body: &[u8]) -> Vec<u8> {
    let mut lines = head.split("\r\n");
    let Some(request) = lines.next() else {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    };
    let mut parts = request.split(' ');
    let Some(method) = parts.next() else {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    };
    let Some(target) = parts.next() else {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    };
    if parts.next().is_none() {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/mcp" {
        return http_response(404, "Not Found", &[], error_body(None, "不在范围内"));
    }
    if !query.is_empty() {
        return http_response(401, "Unauthorized", &[], error_body(None, "未授权"));
    }
    let headers = match header_map(head) {
        Ok(headers) => headers,
        Err(()) => return http_response(400, "Bad Request", &[], error_body(None, "参数无效")),
    };
    if !host_ok(shared, header_one(&headers, "host")) {
        return http_response(401, "Unauthorized", &[], error_body(None, "未授权"));
    }
    if headers.iter().any(|(name, _)| name == "transfer-encoding") {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    }
    if !origin_ok(header_one(&headers, "origin")) {
        return http_response(401, "Unauthorized", &[], error_body(None, "未授权"));
    }
    let Some(token) = bearer(header_one(&headers, "authorization")) else {
        return http_response(401, "Unauthorized", &[], error_body(None, "未授权"));
    };
    let Some(client_id) = shared.authenticate(&token) else {
        return http_response(401, "Unauthorized", &[], error_body(None, "未授权"));
    };
    if !shared.enabled.load(Ordering::Acquire) {
        return http_response(401, "Unauthorized", &[], error_body(None, "未授权"));
    }
    match method {
        "POST" => post(shared, &client_id, &headers, body),
        "DELETE" => delete_session(shared, &client_id, &headers),
        _ => http_response(
            405,
            "Method Not Allowed",
            &[],
            error_body(None, "不在范围内"),
        ),
    }
}

fn post(shared: &Shared, client_id: &str, headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
    if body.len() > MAX_BODY {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return http_response(400, "Bad Request", &[], error_body(None, "参数无效")),
    };
    if value.is_array() {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    }
    let Some(message) = value.as_object() else {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    };
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return http_response(400, "Bad Request", &[], error_body(None, "参数无效"));
    };
    let id = message.get("id").cloned();
    if id.is_none() || method.starts_with("notifications/") {
        return http_response(202, "Accepted", &[], Vec::new());
    }
    let id = id.unwrap_or(Value::Null);
    if method == "initialize" {
        return initialize(shared, client_id, &id, message.get("params"));
    }
    let Some(session) = header_one(headers, "mcp-session-id") else {
        return http_response(401, "Unauthorized", &[], error_body(Some(&id), "未授权"));
    };
    if !shared
        .session_owner(session)
        .is_some_and(|owner| owner == client_id)
    {
        return http_response(401, "Unauthorized", &[], error_body(Some(&id), "未授权"));
    }
    let result = match method {
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": super::tools::definitions() })),
        "tools/call" => call_tool(shared, &token_from_headers(headers), message.get("params")),
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        "resources/read" | "prompts/get" | "sampling/createMessage" => Err("不在范围内".to_owned()),
        _ => Err("不在范围内".to_owned()),
    };
    match result {
        Ok(result) => http_response(
            200,
            "OK",
            &[],
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
                .unwrap_or_default(),
        ),
        Err(message) => {
            if method == "tools/call" {
                http_response(200, "OK", &[], tool_error_body(&id, &message))
            } else {
                http_response(200, "OK", &[], error_body(Some(&id), &message))
            }
        }
    }
}

fn token_from_headers(headers: &[(String, String)]) -> String {
    bearer(header_one(headers, "authorization")).unwrap_or_default()
}

fn call_tool(shared: &Shared, token: &str, params: Option<&Value>) -> Result<Value, String> {
    let params = params.unwrap_or(&Value::Null);
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err("参数无效".to_owned());
    };
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => return Err("参数无效".to_owned()),
    };
    match shared.invoke(token, name, &arguments) {
        Ok(value) => {
            let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned());
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            }))
        }
        Err(message) => Err(message),
    }
}

fn initialize(shared: &Shared, client_id: &str, id: &Value, params: Option<&Value>) -> Vec<u8> {
    let requested = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL);
    let protocol = if SUPPORTED_PROTOCOLS.contains(&requested) {
        requested
    } else {
        DEFAULT_PROTOCOL
    };
    let session = fresh_id();
    shared.remember_session(session.clone(), client_id.to_owned());
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": protocol,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "Lanwork", "version": env!("CARGO_PKG_VERSION") },
        }
    });
    http_response(
        200,
        "OK",
        &[("Mcp-Session-Id", &session)],
        serde_json::to_vec(&body).unwrap_or_default(),
    )
}

fn delete_session(shared: &Shared, client_id: &str, headers: &[(String, String)]) -> Vec<u8> {
    let Some(session) = header_one(headers, "mcp-session-id") else {
        return http_response(401, "Unauthorized", &[], error_body(None, "未授权"));
    };
    if shared.end_session(session, client_id) {
        http_response(204, "No Content", &[], Vec::new())
    } else {
        http_response(401, "Unauthorized", &[], error_body(None, "未授权"))
    }
}

fn header_map(head: &str) -> Result<Vec<(String, String)>, ()> {
    let mut headers = Vec::new();
    for line in head.split("\r\n").skip(1) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(());
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(());
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    Ok(headers)
}

fn header_one<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let mut found = None;
    for (key, value) in headers {
        if key == name {
            if found.is_some() {
                return None;
            }
            found = Some(value.as_str());
        }
    }
    found
}

fn host_ok(shared: &Shared, host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let port = shared.port.load(Ordering::Acquire);
    let (name, given) = split_host(host);
    if !name.eq_ignore_ascii_case("127.0.0.1") && !name.eq_ignore_ascii_case("localhost") {
        return false;
    }
    match given {
        None => true,
        Some(given) => port != 0 && given == port.to_string(),
    }
}

fn origin_ok(origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    if origin.eq_ignore_ascii_case("null") {
        return true;
    }
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("HTTP://"))
    else {
        return false;
    };
    if rest.contains('/') || rest.is_empty() {
        return false;
    }
    let (name, _) = split_host(rest);
    name.eq_ignore_ascii_case("127.0.0.1") || name.eq_ignore_ascii_case("localhost")
}

fn split_host(value: &str) -> (&str, Option<&str>) {
    match value.rsplit_once(':') {
        Some((host, port))
            if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (host, Some(port))
        }
        _ => (value, None),
    }
}

fn bearer(header: Option<&str>) -> Option<String> {
    let header = header?;
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() || token.contains(' ') {
        return None;
    }
    Some(token.to_owned())
}

fn error_body(id: Option<&Value>, message: &str) -> Vec<u8> {
    let id = id.cloned().unwrap_or(Value::Null);
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32001, "message": message }
    }))
    .unwrap_or_default()
}

fn tool_error_body(id: &Value, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "isError": true
        }
    }))
    .unwrap_or_default()
}

fn http_response(status: u16, reason: &str, extra: &[(&str, &str)], body: Vec<u8>) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        if !value.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend(body);
    bytes
}
