//! 向清单里声明的主机名发 HTTPS GET，以及清单允许时的 POST JSON。
//!
//! 不跟随重定向。同一主机名之内也不跟随，已经定了。
//! 不附带密钥的明文 `http` 不发送，已经定了。Lua 看到的说明是「不允许明文 http」。
//! 附带密钥时只允许 `https`。
//! HTTP 超时和响应正文上限规格待定。这次连接和读取各最多 5 秒，响应正文最多 1 MiB，超过则「请求失败」。
//! 请求正文最多 64 KiB，必须是 JSON。
//! 套接字在另一条线程上，脚本回调不等待网络。
//! 脚本不能自带固定请求头以外的头。表里多出来的字段这次忽略，是否拒绝待定。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::ClientConnection;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore, StreamOwned};
use url::Url;

use crate::manifest::{
    FixedHeader, header_name_reason, header_value_reason, prefix_reason, valid_domain,
};

pub const BODY_CAP: usize = 1024 * 1024;
/// POST 正文的字节上限。
pub const BODY_MAX: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);
/// 一次请求最多附上的密钥个数。
pub const SECRET_LIMIT: usize = 8;

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub secrets: Vec<SecretAttach>,
    pub body: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SecretAttach {
    pub id: String,
    pub header: String,
    pub prefix: String,
    /// 脚本写了 `prefix` 时为真。没写则沿用清单里的前缀。
    pub prefix_set: bool,
}

/// 已经从凭据库取出、可以拼进请求头的密钥。原文只留在宿主。
///
/// `Debug` 不打印原文，避免日志或断言失败时把密钥打出来。
#[derive(Clone)]
pub struct ResolvedSecret {
    pub header: String,
    pub prefix: String,
    pub value: String,
}

impl std::fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedSecret")
            .field("header", &self.header)
            .field("prefix", &self.prefix)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Clone)]
pub struct Outgoing {
    pub method: String,
    pub url: String,
    /// 固定请求头在前，密钥头在后。不含 Host、Connection、Content-Length、Content-Type。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for Outgoing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, _)| (name.as_str(), "<redacted>"))
            .collect();
        formatter
            .debug_struct("Outgoing")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// 响应里能否看出密钥。只能看到清单里声明的主机返回的正文和响应头，所以这是尽力检查。
///
/// 命中时调用方应把正文清空并显示「请求失败」，而不是逐段替换。
/// 检查这些形式：原文；`Bearer` / `bearer` 加空格再加原文；实际发出的请求头值；
/// 上述每一项的标准 Base64 与 URL 安全 Base64（带填充和不带填充）。不按长度丢弃：短密钥的编码可能只有几个字符。
/// 另外把正文里的 `%XX`（十六进制忽略大小写，未保留字节也可以编码，可以只编码其中一部分）
/// 反复解码后再找上面的明文和 Base64。原文或解码结果里的 `+` 再按空格看一次。
pub fn response_exposes_secret(parts: &[&str], secret: &str, header_value: Option<&str>) -> bool {
    if secret.is_empty() {
        return false;
    }
    let mut plain = vec![
        secret.to_string(),
        format!("Bearer {secret}"),
        format!("bearer {secret}"),
    ];
    if let Some(header) = header_value
        && !header.is_empty()
    {
        plain.push(header.to_string());
    }
    plain.sort();
    plain.dedup();
    let mut encoded = Vec::new();
    for item in &plain {
        for url_safe in [false, true] {
            let b64 = base64_encode(item.as_bytes(), url_safe);
            if b64.is_empty() {
                continue;
            }
            let trimmed = b64.trim_end_matches('=').to_string();
            if !trimmed.is_empty() && trimmed != b64 {
                encoded.push(trimmed);
            }
            encoded.push(b64);
        }
    }
    encoded.sort();
    encoded.dedup();
    let hit = |bytes: &[u8]| {
        plain
            .iter()
            .any(|needle| contains_bytes(bytes, needle.as_bytes()))
            || encoded
                .iter()
                .any(|needle| contains_bytes(bytes, needle.as_bytes()))
    };
    for part in parts {
        let bytes = part.as_bytes();
        if hit(bytes) {
            return true;
        }
        if let Some(spaced) = plus_as_space(bytes)
            && hit(&spaced)
        {
            return true;
        }
        let mut decoded = bytes.to_vec();
        for _ in 0..4 {
            let next = percent_decode_loose(&decoded);
            if next == decoded {
                break;
            }
            decoded = next;
            if hit(&decoded) {
                return true;
            }
            if let Some(spaced) = plus_as_space(&decoded)
                && hit(&spaced)
            {
                return true;
            }
        }
    }
    false
}

fn plus_as_space(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.contains(&b'+') {
        return None;
    }
    let mut copy = bytes.to_vec();
    for byte in &mut copy {
        if *byte == b'+' {
            *byte = b' ';
        }
    }
    Some(copy)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn percent_decode_loose(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex_val(bytes[index + 1]), hex_val(bytes[index + 2]))
        {
            out.push((high << 4) | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn base64_encode(bytes: &[u8], url_safe: bool) -> String {
    const STD: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let alphabet = if url_safe { URL } else { STD };
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let n = ((bytes[index] as u32) << 16)
            | ((bytes[index + 1] as u32) << 8)
            | bytes[index + 2] as u32;
        out.push(alphabet[((n >> 18) & 0x3f) as usize] as char);
        out.push(alphabet[((n >> 12) & 0x3f) as usize] as char);
        out.push(alphabet[((n >> 6) & 0x3f) as usize] as char);
        out.push(alphabet[(n & 0x3f) as usize] as char);
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        let n = (bytes[index] as u32) << 16;
        out.push(alphabet[((n >> 18) & 0x3f) as usize] as char);
        out.push(alphabet[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let n = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
        out.push(alphabet[((n >> 18) & 0x3f) as usize] as char);
        out.push(alphabet[((n >> 12) & 0x3f) as usize] as char);
        out.push(alphabet[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

#[derive(Debug)]
pub enum HttpDeny {
    OutOfScope,
    BadAddress,
    HostDenied,
    SecretMissing,
    /// 明文 http 不发送，已经定了。
    PlainHttpUndecided,
    BadBody,
    BodyTooLarge,
    RequestFailed,
}

impl HttpDeny {
    pub fn text(&self) -> &'static str {
        match self {
            HttpDeny::OutOfScope => "不在范围内",
            HttpDeny::BadAddress => "地址无效",
            HttpDeny::HostDenied => "域名未允许",
            HttpDeny::SecretMissing => "未填写密钥",
            HttpDeny::PlainHttpUndecided => "不允许明文 http",
            HttpDeny::BadBody => "数据无效",
            HttpDeny::BodyTooLarge => "数据过大",
            HttpDeny::RequestFailed => "请求失败",
        }
    }
}

pub fn prepare(
    request: &HttpRequest,
    domains: &[String],
    post: bool,
    fixed: &[FixedHeader],
    secrets: &[ResolvedSecret],
) -> Result<Outgoing, HttpDeny> {
    let url = Url::parse(&request.url).map_err(|_| HttpDeny::BadAddress)?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpDeny::BadAddress);
    }
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        return Err(HttpDeny::BadAddress);
    }
    let host = url.host_str().ok_or(HttpDeny::BadAddress)?;
    if !valid_domain(host) {
        return Err(HttpDeny::BadAddress);
    }
    if !secrets.is_empty() && scheme != "https" {
        return Err(HttpDeny::BadAddress);
    }
    if secrets.is_empty() && scheme == "http" {
        return Err(HttpDeny::PlainHttpUndecided);
    }
    let allowed = domains
        .iter()
        .any(|domain| domain.eq_ignore_ascii_case(host));
    if !allowed {
        return Err(HttpDeny::HostDenied);
    }
    let body = match request.method.as_str() {
        "GET" => {
            if request.body.is_some() {
                return Err(HttpDeny::OutOfScope);
            }
            Vec::new()
        }
        "POST" => {
            if !post {
                return Err(HttpDeny::OutOfScope);
            }
            let text = request.body.as_deref().ok_or(HttpDeny::BadBody)?;
            if text.len() > BODY_MAX {
                return Err(HttpDeny::BodyTooLarge);
            }
            if serde_json::from_str::<serde_json::Value>(text).is_err() {
                return Err(HttpDeny::BadBody);
            }
            text.as_bytes().to_vec()
        }
        _ => return Err(HttpDeny::OutOfScope),
    };
    if secrets.len() > SECRET_LIMIT {
        return Err(HttpDeny::BadAddress);
    }
    let mut headers = Vec::new();
    for fixed in fixed {
        if header_name_reason(&fixed.name).is_err() || header_value_reason(&fixed.value).is_err() {
            return Err(HttpDeny::BadAddress);
        }
        if headers
            .iter()
            .any(|(name, _): &(String, String)| name.eq_ignore_ascii_case(&fixed.name))
        {
            return Err(HttpDeny::BadAddress);
        }
        headers.push((fixed.name.clone(), fixed.value.clone()));
    }
    for secret in secrets {
        if secret.value.is_empty() {
            return Err(HttpDeny::SecretMissing);
        }
        if header_name_reason(&secret.header).is_err()
            || prefix_reason(&secret.prefix).is_err()
            || secret
                .value
                .bytes()
                .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
        {
            return Err(HttpDeny::BadAddress);
        }
        if headers
            .iter()
            .any(|(name, _): &(String, String)| name.eq_ignore_ascii_case(&secret.header))
        {
            return Err(HttpDeny::BadAddress);
        }
        headers.push((
            secret.header.clone(),
            format!("{}{}", secret.prefix, secret.value),
        ));
    }
    Ok(Outgoing {
        method: request.method.clone(),
        url: request.url.clone(),
        headers,
        body,
    })
}

pub fn encode_http(outgoing: &Outgoing) -> Result<Vec<u8>, TransportError> {
    if outgoing.method != "GET" && outgoing.method != "POST" {
        return Err(TransportError);
    }
    let url = Url::parse(&outgoing.url).map_err(|_| TransportError)?;
    let host = url.host_str().ok_or(TransportError)?;
    let path = request_path(&url);
    let user_agent = header_of(&outgoing.headers, "user-agent").unwrap_or("Lanwork");
    let accept = header_of(&outgoing.headers, "accept").unwrap_or("*/*");
    if user_agent.contains(['\r', '\n']) || accept.contains(['\r', '\n']) {
        return Err(TransportError);
    }
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {user_agent}\r\nAccept: {accept}\r\nConnection: close\r\n",
        method = outgoing.method,
    );
    if outgoing.method == "POST" {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", outgoing.body.len()));
    }
    for (name, value) in &outgoing.headers {
        if name.eq_ignore_ascii_case("user-agent") || name.eq_ignore_ascii_case("accept") {
            continue;
        }
        if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            return Err(TransportError);
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    if outgoing.method == "POST" {
        bytes.extend_from_slice(&outgoing.body);
    }
    Ok(bytes)
}

fn header_of<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn request_path(url: &Url) -> String {
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

/// 传输失败。套接字和证书的原因留在宿主里，不交给脚本。
#[derive(Debug, Clone, Copy)]
pub struct TransportError;

pub trait HttpTransport: Send + Sync {
    fn send(&self, request: &Outgoing) -> Result<HttpResponse, TransportError>;
}

pub struct RustlsTransport {
    config: Arc<ClientConfig>,
}

impl RustlsTransport {
    pub fn native() -> Result<Self, String> {
        install_ring();
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self {
            config: Arc::new(config),
        })
    }

    pub fn with_config(config: ClientConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

impl HttpTransport for RustlsTransport {
    fn send(&self, request: &Outgoing) -> Result<HttpResponse, TransportError> {
        let url = Url::parse(&request.url).map_err(|_| TransportError)?;
        let host = url.host_str().ok_or(TransportError)?;
        let port = url.port_or_known_default().ok_or(TransportError)?;
        let server = ServerName::try_from(host.to_string()).map_err(|_| TransportError)?;
        let connection =
            ClientConnection::new(Arc::clone(&self.config), server).map_err(|_| TransportError)?;
        let tcp = connect_with_deadline(host, port)?;
        tcp.set_read_timeout(Some(TIMEOUT))
            .map_err(|_| TransportError)?;
        tcp.set_write_timeout(Some(TIMEOUT))
            .map_err(|_| TransportError)?;
        let mut tls = StreamOwned::new(connection, tcp);
        let message = encode_http(request)?;
        tls.write_all(&message).map_err(|_| TransportError)?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            if buf.len() >= BODY_CAP + 64 * 1024 {
                return Err(TransportError);
            }
            match tls.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
                Err(_) => {
                    if buf.is_empty() {
                        return Err(TransportError);
                    }
                    break;
                }
            }
        }
        parse_response(&buf)
    }
}

fn connect_with_deadline(host: &str, port: u16) -> Result<TcpStream, TransportError> {
    let addrs = (host, port).to_socket_addrs().map_err(|_| TransportError)?;
    for addr in addrs {
        if let Ok(tcp) = TcpStream::connect_timeout(&addr, TIMEOUT) {
            return Ok(tcp);
        }
    }
    Err(TransportError)
}

fn parse_response(bytes: &[u8]) -> Result<HttpResponse, TransportError> {
    let text = String::from_utf8_lossy(bytes);
    let (head, body) = text.split_once("\r\n\r\n").ok_or(TransportError)?;
    let status_line = head.lines().next().ok_or(TransportError)?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or(TransportError)?
        .parse()
        .map_err(|_| TransportError)?;
    let mut body = body.to_string();
    if body.len() > BODY_CAP {
        return Err(TransportError);
    }
    if let Some(length) = content_length(head) {
        if length > BODY_CAP {
            return Err(TransportError);
        }
        if body.len() > length {
            body.truncate(length);
        }
    }
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

pub fn install_ring() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains() -> Vec<String> {
        vec!["api.example.com".to_string()]
    }

    fn get(url: &str) -> HttpRequest {
        HttpRequest {
            method: "GET".to_string(),
            url: url.to_string(),
            secrets: Vec::new(),
            body: None,
        }
    }

    fn call<'a>(
        request: &HttpRequest,
        post: bool,
        fixed: &'a [FixedHeader],
        secrets: &'a [ResolvedSecret],
    ) -> Result<Outgoing, HttpDeny> {
        prepare(request, &domains(), post, fixed, secrets)
    }

    fn bearer(value: &str) -> ResolvedSecret {
        ResolvedSecret {
            header: "Authorization".to_string(),
            prefix: "Bearer ".to_string(),
            value: value.to_string(),
        }
    }

    fn header_is(outgoing: &Outgoing, name: &str, value: &str) -> bool {
        outgoing
            .headers
            .iter()
            .any(|(key, got)| key == name && got == value)
    }

    #[test]
    fn host_gate() {
        assert!(matches!(
            call(&get("https://evil.com/x"), false, &[], &[]),
            Err(HttpDeny::HostDenied)
        ));
        assert!(matches!(
            call(&get("https://api.example.com.evil.com/x"), false, &[], &[]),
            Err(HttpDeny::HostDenied)
        ));
        assert!(call(&get("https://API.Example.COM/v1/quota"), false, &[], &[]).is_ok());
        let mut post = get("https://api.example.com/v1");
        post.method = "POST".to_string();
        assert!(matches!(
            call(&post, false, &[], &[]),
            Err(HttpDeny::OutOfScope)
        ));
        assert!(matches!(
            call(&get("http://api.example.com/v1"), false, &[], &[]),
            Err(HttpDeny::PlainHttpUndecided)
        ));
        assert_eq!(HttpDeny::PlainHttpUndecided.text(), "不允许明文 http");
        let secret = bearer("raw");
        assert!(matches!(
            call(
                &get("http://api.example.com/v1"),
                false,
                &[],
                std::slice::from_ref(&secret)
            ),
            Err(HttpDeny::BadAddress)
        ));
        assert!(matches!(
            call(
                &get("https://api.example.com/v1"),
                false,
                &[],
                &[bearer("")]
            ),
            Err(HttpDeny::SecretMissing)
        ));
        let ready = call(
            &get("https://api.example.com/v1"),
            false,
            &[],
            &[bearer("raw-secret")],
        )
        .unwrap();
        assert!(header_is(&ready, "Authorization", "Bearer raw-secret"));
        assert!(!ready.url.contains("raw-secret"));
        assert!(ready.body.is_empty());
        let injected = ResolvedSecret {
            header: "Authorization\r\nX-Evil".to_string(),
            prefix: "Bearer ".to_string(),
            value: "raw".to_string(),
        };
        assert!(matches!(
            call(&get("https://api.example.com/v1"), false, &[], &[injected]),
            Err(HttpDeny::BadAddress)
        ));
        assert!(matches!(
            call(&get("https://user:raw@api.example.com/v1"), false, &[], &[]),
            Err(HttpDeny::BadAddress)
        ));
    }

    #[test]
    fn assembles_fixed_headers_and_several_secrets_without_putting_them_in_the_url() {
        let token = "tok-a-secret";
        let account = "acct-b-secret";
        let fixed = vec![
            FixedHeader {
                name: "anthropic-beta".to_string(),
                value: "oauth-2025-04-20".to_string(),
            },
            FixedHeader {
                name: "User-Agent".to_string(),
                value: "my-tool".to_string(),
            },
        ];
        let secrets = vec![
            ResolvedSecret {
                header: "Authorization".to_string(),
                prefix: "Bearer ".to_string(),
                value: token.to_string(),
            },
            ResolvedSecret {
                header: "ChatGPT-Account-Id".to_string(),
                prefix: String::new(),
                value: account.to_string(),
            },
        ];
        let ready = call(&get("https://api.example.com/v1"), false, &fixed, &secrets).unwrap();
        assert!(header_is(&ready, "anthropic-beta", "oauth-2025-04-20"));
        assert!(header_is(&ready, "User-Agent", "my-tool"));
        assert!(header_is(
            &ready,
            "Authorization",
            &format!("Bearer {token}")
        ));
        assert!(header_is(&ready, "ChatGPT-Account-Id", account));
        assert!(!ready.url.contains(token));
        assert!(!ready.url.contains(account));
        assert_eq!(ready.headers.len(), 4);
        let text = String::from_utf8(encode_http(&ready).unwrap()).unwrap();
        assert_eq!(text.matches("User-Agent:").count(), 1);
        assert!(text.contains("User-Agent: my-tool\r\n"));
        assert!(text.contains("Accept: */*\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        let without_secrets = text
            .lines()
            .filter(|line| {
                !line.starts_with("Authorization:") && !line.starts_with("ChatGPT-Account-Id:")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!without_secrets.contains(token));
        assert!(!without_secrets.contains(account));
        let default_agent = call(&get("https://api.example.com/v1"), false, &[], &[]).unwrap();
        let plain = String::from_utf8(encode_http(&default_agent).unwrap()).unwrap();
        assert!(plain.contains("User-Agent: Lanwork\r\n"));
        assert!(!plain.contains(token));
    }

    #[test]
    fn post_json_is_capped_and_content_type_is_owned_by_the_host() {
        let mut post = get("https://api.example.com/v1/usage");
        post.method = "POST".to_string();
        post.body = Some(r#"{"limit":1}"#.to_string());
        assert!(matches!(
            call(&post, false, &[], &[]),
            Err(HttpDeny::OutOfScope)
        ));
        let ready = call(&post, true, &[], &[bearer("raw-secret")]).unwrap();
        assert_eq!(ready.method, "POST");
        assert_eq!(ready.body, br#"{"limit":1}"#);
        assert!(!ready.url.contains("raw-secret"));
        let wire = String::from_utf8(encode_http(&ready).unwrap()).unwrap();
        let (head, body) = wire.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("POST /v1/usage HTTP/1.1\r\n"));
        assert!(head.contains("Content-Type: application/json\r\n"));
        assert!(head.contains(&format!("Content-Length: {}", body.len())));
        assert_eq!(body, r#"{"limit":1}"#);
        assert_eq!(head.matches("Content-Type:").count(), 1);
        assert!(!body.contains("raw-secret"));
        post.body = Some("not-json".to_string());
        assert!(matches!(
            call(&post, true, &[], &[]),
            Err(HttpDeny::BadBody)
        ));
        post.body = None;
        assert!(matches!(
            call(&post, true, &[], &[]),
            Err(HttpDeny::BadBody)
        ));
        post.body = Some(format!("{{\"pad\":\"{}\"}}", "x".repeat(BODY_MAX)));
        assert!(matches!(
            call(&post, true, &[], &[]),
            Err(HttpDeny::BodyTooLarge)
        ));
        let mut get_with_body = get("https://api.example.com/v1");
        get_with_body.body = Some("{}".to_string());
        assert!(matches!(
            call(&get_with_body, true, &[], &[]),
            Err(HttpDeny::OutOfScope)
        ));
        let mut http_post = get("http://api.example.com/v1");
        http_post.method = "POST".to_string();
        http_post.body = Some("{}".to_string());
        assert!(matches!(
            call(&http_post, true, &[], &[]),
            Err(HttpDeny::PlainHttpUndecided)
        ));
    }

    #[test]
    fn secret_header_cannot_reuse_a_fixed_name() {
        let fixed = vec![FixedHeader {
            name: "Authorization".to_string(),
            value: "fixed".to_string(),
        }];
        assert!(matches!(
            call(
                &get("https://api.example.com/v1"),
                false,
                &fixed,
                &[bearer("raw")]
            ),
            Err(HttpDeny::BadAddress)
        ));
        let lower = ResolvedSecret {
            header: "authorization".to_string(),
            prefix: String::new(),
            value: "raw".to_string(),
        };
        assert!(matches!(
            call(&get("https://api.example.com/v1"), false, &fixed, &[lower]),
            Err(HttpDeny::BadAddress)
        ));
    }

    #[test]
    fn secret_detection_covers_percent_and_base64() {
        let secret = "lw-raw-secret-9f3a";
        let full: String = secret.bytes().map(|byte| format!("%{byte:02x}")).collect();
        let partial = format!("%{:02X}{}", secret.as_bytes()[0], &secret[1..]);
        let mixed: String = secret
            .bytes()
            .enumerate()
            .map(|(index, byte)| {
                if index % 2 == 0 {
                    format!("%{byte:02X}")
                } else {
                    format!("%{byte:02x}")
                }
            })
            .collect();
        assert!(response_exposes_secret(&[&full], secret, None), "{full}");
        assert!(
            response_exposes_secret(&[&partial], secret, None),
            "{partial}"
        );
        assert!(response_exposes_secret(&[&mixed], secret, None), "{mixed}");
        let standard = base64_encode(secret.as_bytes(), false);
        let url_safe = base64_encode(secret.as_bytes(), true);
        assert!(standard.len() >= 8);
        assert!(response_exposes_secret(&[&standard], secret, None));
        assert!(response_exposes_secret(&[&url_safe], secret, None));
        assert!(!response_exposes_secret(
            &[r#"{"used":20,"limit":100}"#],
            secret,
            None
        ));
        assert!(response_exposes_secret(
            &[r#"{"used":20}"#, "X-Echo", &format!("Bearer {secret}")],
            secret,
            None,
        ));

        let short = "a b/c";
        assert!(response_exposes_secret(
            &["prefix a b/c suffix"],
            short,
            None
        ));
        assert!(response_exposes_secret(&["YSBiL2M="], short, None));
        assert!(response_exposes_secret(&["YSBiL2M"], short, None));
        assert!(response_exposes_secret(&["a%20b%2Fc"], short, None));
        assert!(response_exposes_secret(&["a%20b%2fc"], short, None));
        assert!(response_exposes_secret(&["a+b%2Fc"], short, None));
        assert!(response_exposes_secret(&["a+b/c"], short, None));
        assert_eq!(base64_encode(b"a/b", false), "YS9i");

        let short_key = "xyz";
        assert_eq!(base64_encode(short_key.as_bytes(), false), "eHl6");
        assert_eq!(base64_encode(short_key.as_bytes(), true), "eHl6");
        assert!(response_exposes_secret(&["eHl6"], short_key, None));
        assert!(response_exposes_secret(
            &["prefix eHl6 suffix"],
            short_key,
            None
        ));
        assert!(!response_exposes_secret(
            &[r#"{"ok":true}"#],
            short_key,
            None
        ));
        assert_eq!(base64_encode(b"x", false), "eA==");
        assert!(response_exposes_secret(&["eA=="], "x", None));
        assert!(response_exposes_secret(&["eA"], "x", None));
    }
}
