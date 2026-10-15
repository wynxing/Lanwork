//! HTTPS GET。只向清单里声明的主机名发送。
//!
//! 不跟随重定向。同一主机名之内是否跟随，待定。
//! 不附带密钥的明文 `http` 是否允许，待定；这次不发送，Lua 看到的说明写明仍待定。
//! 附带密钥时只允许 `https`。
//! HTTP 超时和正文上限规格待定。这次连接 5 秒，正文最多 1 MiB，超过则「请求失败」。
//! 脚本不能自带其它请求头。表里多出来的字段这次忽略，是否拒绝待定。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::ClientConnection;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore, StreamOwned};
use url::Url;

use crate::manifest::valid_domain;

pub const BODY_CAP: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub secret: Option<SecretAttach>,
}

#[derive(Clone, Debug)]
pub struct SecretAttach {
    pub id: String,
    pub header: String,
    pub prefix: String,
}

#[derive(Clone, Debug)]
pub struct Outgoing {
    pub url: String,
    pub header_name: Option<String>,
    pub header_value: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

#[derive(Debug)]
pub enum HttpDeny {
    OutOfScope,
    BadAddress,
    HostDenied,
    SecretMissing,
    /// 明文 http 这次不发送。产品是否允许，待定。
    PlainHttpUndecided,
    RequestFailed,
}

impl HttpDeny {
    pub fn text(&self) -> &'static str {
        match self {
            HttpDeny::OutOfScope => "不在范围内",
            HttpDeny::BadAddress => "地址无效",
            HttpDeny::HostDenied => "域名未允许",
            HttpDeny::SecretMissing => "未填写密钥",
            HttpDeny::PlainHttpUndecided => "明文 http（是否允许仍待定）",
            HttpDeny::RequestFailed => "请求失败",
        }
    }
}

pub fn prepare(
    request: &HttpRequest,
    domains: &[String],
    secret_value: Option<&str>,
    declared_secret: bool,
) -> Result<Outgoing, HttpDeny> {
    if request.method != "GET" {
        return Err(HttpDeny::OutOfScope);
    }
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
    if request.secret.is_some() && scheme != "https" {
        return Err(HttpDeny::BadAddress);
    }
    if request.secret.is_none() && scheme == "http" {
        return Err(HttpDeny::PlainHttpUndecided);
    }
    let allowed = domains
        .iter()
        .any(|domain| domain.eq_ignore_ascii_case(host));
    if !allowed {
        return Err(HttpDeny::HostDenied);
    }
    let (header_name, header_value) = if let Some(secret) = &request.secret {
        if !declared_secret {
            return Err(HttpDeny::SecretMissing);
        }
        let value = secret_value.ok_or(HttpDeny::SecretMissing)?;
        if value.is_empty() {
            return Err(HttpDeny::SecretMissing);
        }
        if !header_token(&secret.header)
            || secret.prefix.contains(['\r', '\n'])
            || value.contains(['\r', '\n'])
        {
            return Err(HttpDeny::BadAddress);
        }
        (
            Some(secret.header.clone()),
            Some(format!("{}{value}", secret.prefix)),
        )
    } else {
        (None, None)
    };
    Ok(Outgoing {
        url: request.url.clone(),
        header_name,
        header_value,
    })
}

fn header_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub trait HttpTransport {
    fn get(&self, request: &Outgoing) -> Result<HttpResponse, ()>;
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
    fn get(&self, request: &Outgoing) -> Result<HttpResponse, ()> {
        let url = Url::parse(&request.url).map_err(|_| ())?;
        let host = url.host_str().ok_or(())?;
        let port = url.port_or_known_default().ok_or(())?;
        let server = ServerName::try_from(host.to_string()).map_err(|_| ())?;
        let connection = ClientConnection::new(Arc::clone(&self.config), server).map_err(|_| ())?;
        let tcp = TcpStream::connect((host, port)).map_err(|_| ())?;
        tcp.set_read_timeout(Some(TIMEOUT)).map_err(|_| ())?;
        tcp.set_write_timeout(Some(TIMEOUT)).map_err(|_| ())?;
        let mut tls = StreamOwned::new(connection, tcp);
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        let path = if path.is_empty() {
            "/".to_string()
        } else {
            path
        };
        let mut head = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Lanwork-toolhost-spike\r\nAccept: */*\r\nConnection: close\r\n"
        );
        if let (Some(name), Some(value)) = (&request.header_name, &request.header_value) {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        tls.write_all(head.as_bytes()).map_err(|_| ())?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            if buf.len() >= BODY_CAP + 64 * 1024 {
                return Err(());
            }
            match tls.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
                Err(_) => {
                    if buf.is_empty() {
                        return Err(());
                    }
                    break;
                }
            }
        }
        parse_response(&buf)
    }
}

fn parse_response(bytes: &[u8]) -> Result<HttpResponse, ()> {
    let text = String::from_utf8_lossy(bytes);
    let (head, body) = text.split_once("\r\n\r\n").ok_or(())?;
    let status_line = head.lines().next().ok_or(())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or(())?
        .parse()
        .map_err(|_| ())?;
    let mut body = body.to_string();
    if body.len() > BODY_CAP {
        return Err(());
    }
    if let Some(length) = content_length(head) {
        if length > BODY_CAP {
            return Err(());
        }
        if body.len() > length {
            body.truncate(length);
        }
    }
    Ok(HttpResponse { status, body })
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
            secret: None,
        }
    }

    #[test]
    fn host_gate() {
        assert!(matches!(
            prepare(&get("https://evil.com/x"), &domains(), None, false),
            Err(HttpDeny::HostDenied)
        ));
        assert!(matches!(
            prepare(
                &get("https://api.example.com.evil.com/x"),
                &domains(),
                None,
                false
            ),
            Err(HttpDeny::HostDenied)
        ));
        assert!(
            prepare(
                &get("https://API.Example.COM/v1/quota"),
                &domains(),
                None,
                false
            )
            .is_ok()
        );
        let mut post = get("https://api.example.com/v1");
        post.method = "POST".to_string();
        assert!(matches!(
            prepare(&post, &domains(), None, false),
            Err(HttpDeny::OutOfScope)
        ));
        assert!(matches!(
            prepare(&get("http://api.example.com/v1"), &domains(), None, false),
            Err(HttpDeny::PlainHttpUndecided)
        ));
        let mut with_secret = get("http://api.example.com/v1");
        with_secret.secret = Some(SecretAttach {
            id: "api_key".to_string(),
            header: "Authorization".to_string(),
            prefix: "Bearer ".to_string(),
        });
        assert!(matches!(
            prepare(&with_secret, &domains(), Some("raw"), true),
            Err(HttpDeny::BadAddress)
        ));
        let mut https = get("https://api.example.com/v1");
        https.secret = with_secret.secret.clone();
        assert!(matches!(
            prepare(&https, &domains(), None, true),
            Err(HttpDeny::SecretMissing)
        ));
        let ready = prepare(&https, &domains(), Some("raw-secret"), true).unwrap();
        assert_eq!(ready.header_name.as_deref(), Some("Authorization"));
        assert_eq!(ready.header_value.as_deref(), Some("Bearer raw-secret"));
        assert!(!ready.url.contains("raw-secret"));
        let mut injected = get("https://api.example.com/v1");
        injected.secret = Some(SecretAttach {
            id: "api_key".to_string(),
            header: "Authorization\r\nX-Evil".to_string(),
            prefix: "Bearer ".to_string(),
        });
        assert!(matches!(
            prepare(&injected, &domains(), Some("raw"), true),
            Err(HttpDeny::BadAddress)
        ));
        assert!(matches!(
            prepare(
                &get("https://user:raw@api.example.com/v1"),
                &domains(),
                None,
                false
            ),
            Err(HttpDeny::BadAddress)
        ));
    }
}
