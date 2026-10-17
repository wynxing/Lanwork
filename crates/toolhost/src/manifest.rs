//! TOML 清单。未列出的字段、组件或服务拒绝安装，原因归到「清单无效」。

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::id::{imported_prefix_forbidden, valid_id};

const SERVICES: &[&str] = &[
    "timer",
    "notify",
    "storage",
    "todo.read",
    "todo.complete",
    "http",
    "secret",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    Text,
    Button,
    Timer,
    Progress,
    List,
    Input,
    Toggle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WidgetDecl {
    pub id: String,
    pub kind: WidgetKind,
    pub text: String,
    pub label: String,
    pub on: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretDecl {
    pub id: String,
    pub label: String,
    /// 这个密钥发出去时用的请求头。没有则由脚本在每次请求里写。
    pub header: Option<String>,
    /// `header` 有值时，请求头的值是此前缀加上密钥原文。可以是空字符串。
    pub prefix: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedHeader {
    pub name: String,
    pub value: String,
}

pub(crate) const HEADER_NAME_MAX: usize = 64;
pub(crate) const HEADER_VALUE_MAX: usize = 512;
pub(crate) const PREFIX_MAX: usize = 128;
pub(crate) const HEADER_LIMIT: usize = 16;

const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "cookie",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub services: BTreeSet<String>,
    pub domains: Vec<String>,
    /// 为真时可以向 `domains` 里的每一台 POST JSON。
    pub post: bool,
    pub headers: Vec<FixedHeader>,
    pub secrets: Vec<SecretDecl>,
    pub widgets: Vec<WidgetDecl>,
    /// 入口脚本相对工具文件夹的路径。不含 `..`。
    pub entry: String,
    pub script: String,
}

pub fn load_dir(dir: &Path) -> Result<Manifest, String> {
    let manifest_path = dir.join("manifest.toml");
    let text = std::fs::read(&manifest_path).map_err(|err| format!("读不到清单：{err}"))?;
    let text = std::str::from_utf8(&text).map_err(|_| "清单不是 UTF-8".to_string())?;
    let (manifest, entry) = parse(text)?;
    let script_path = entry_file(dir, &entry)?;
    let script = std::fs::read(&script_path).map_err(|err| format!("读不到入口脚本：{err}"))?;
    let script = std::str::from_utf8(&script).map_err(|_| "入口脚本不是 UTF-8".to_string())?;
    Ok(Manifest {
        script: script.to_string(),
        ..manifest
    })
}

pub fn parse(text: &str) -> Result<(Manifest, String), String> {
    let value: toml::Value = toml::from_str(text).map_err(|_| "清单无法解析".to_string())?;
    let table = value.as_table().ok_or("清单无法解析")?;
    require_keys(
        table.keys().map(String::as_str),
        &[
            "id",
            "name",
            "description",
            "entry",
            "services",
            "http",
            "secret",
            "component",
        ],
    )?;

    let id = required_str(table, "id")?;
    if !valid_id(id) {
        return Err(if id.starts_with('.') {
            "id 不能以 . 开头".to_string()
        } else {
            "id 不是单个路径分量".to_string()
        });
    }
    if imported_prefix_forbidden(id) {
        return Err("id 不能以 lanwork. 开头".to_string());
    }

    let name = trimmed_line(required_str(table, "name")?, "名称")?;
    let description = trimmed_line(required_str(table, "description")?, "短说明")?;
    let entry = required_str(table, "entry")?.to_string();
    check_entry_text(&entry)?;

    let services_value = table.get("services").ok_or("缺少 services")?;
    let services = parse_services(services_value)?;

    let http_present = table.contains_key("http");
    let secret_present = table.contains_key("secret");
    if services.contains("http") != http_present {
        return Err("http 和服务声明对不齐".to_string());
    }
    if services.contains("secret") != secret_present {
        return Err("secret 和服务声明对不齐".to_string());
    }

    let (domains, post, headers) = if services.contains("http") {
        parse_http(table.get("http").ok_or("缺少 http")?)?
    } else {
        (Vec::new(), false, Vec::new())
    };
    let secrets = if services.contains("secret") {
        parse_secrets(table.get("secret").ok_or("缺少 secret")?)?
    } else {
        Vec::new()
    };
    reject_header_collisions(&headers, &secrets)?;
    let widgets = parse_widgets(table.get("component").ok_or("缺少 component")?)?;

    Ok((
        Manifest {
            id: id.to_string(),
            name,
            description,
            services,
            domains,
            post,
            headers,
            secrets,
            widgets,
            entry: entry.clone(),
            script: String::new(),
        },
        entry,
    ))
}

fn parse_services(value: &toml::Value) -> Result<BTreeSet<String>, String> {
    let array = value.as_array().ok_or("services 不是数组")?;
    let mut set = BTreeSet::new();
    for item in array {
        let name = item.as_str().ok_or("服务名不是字符串")?;
        if !SERVICES.contains(&name) {
            return Err(format!("不认识的服务 {name}"));
        }
        set.insert(name.to_string());
    }
    Ok(set)
}

fn parse_http(value: &toml::Value) -> Result<(Vec<String>, bool, Vec<FixedHeader>), String> {
    let table = value.as_table().ok_or("http 不是表")?;
    require_keys(
        table.keys().map(String::as_str),
        &["domains", "post", "headers"],
    )?;
    let array = table
        .get("domains")
        .and_then(toml::Value::as_array)
        .ok_or("domains 不是数组")?;
    if array.is_empty() {
        return Err("声明了 http 但没有主机名".to_string());
    }
    let mut domains = Vec::new();
    for item in array {
        let host = item.as_str().ok_or("主机名不是字符串")?;
        if !valid_domain(host) {
            return Err(format!("主机名不是单独的域名：{host}"));
        }
        domains.push(host.to_string());
    }
    let post = match table.get("post") {
        None => false,
        Some(toml::Value::Boolean(value)) => *value,
        Some(_) => return Err("post 不是布尔".to_string()),
    };
    let headers = match table.get("headers") {
        None => Vec::new(),
        Some(value) => parse_headers(value)?,
    };
    Ok((domains, post, headers))
}

fn parse_headers(value: &toml::Value) -> Result<Vec<FixedHeader>, String> {
    let array = value.as_array().ok_or("headers 不是数组")?;
    if array.len() > HEADER_LIMIT {
        return Err("请求头过多".to_string());
    }
    let mut headers = Vec::new();
    for item in array {
        let table = item.as_table().ok_or("请求头不是表")?;
        require_keys(table.keys().map(String::as_str), &["name", "value"])?;
        let name = required_str(table, "name")?;
        header_name_reason(name)?;
        let value = required_str(table, "value")?;
        header_value_reason(value)?;
        if headers
            .iter()
            .any(|header: &FixedHeader| header.name.eq_ignore_ascii_case(name))
        {
            return Err("请求头重复".to_string());
        }
        headers.push(FixedHeader {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    Ok(headers)
}

pub(crate) fn header_name_reason(name: &str) -> Result<(), String> {
    if FORBIDDEN_HEADERS
        .iter()
        .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
    {
        return Err(format!("不能使用这个请求头：{name}"));
    }
    if name.is_empty()
        || name.len() > HEADER_NAME_MAX
        || name.starts_with('-')
        || name.ends_with('-')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("请求头名称无效".to_string());
    }
    Ok(())
}

pub(crate) fn header_value_reason(value: &str) -> Result<(), String> {
    if value.len() > HEADER_VALUE_MAX {
        return Err("请求头的值过长".to_string());
    }
    if value.trim().is_empty() {
        return Err("请求头的值为空".to_string());
    }
    if !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return Err("请求头的值无效".to_string());
    }
    Ok(())
}

pub(crate) fn prefix_reason(prefix: &str) -> Result<(), String> {
    if prefix.len() > PREFIX_MAX {
        return Err("密钥头的前缀过长".to_string());
    }
    if !prefix.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return Err("密钥头的前缀无效".to_string());
    }
    Ok(())
}

fn reject_header_collisions(headers: &[FixedHeader], secrets: &[SecretDecl]) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::new();
    for secret in secrets {
        let Some(name) = secret.header.as_deref() else {
            continue;
        };
        if seen.iter().any(|prior| name.eq_ignore_ascii_case(prior)) {
            return Err("请求头重复".to_string());
        }
        if headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case(name))
        {
            return Err("请求头与密钥头重名".to_string());
        }
        seen.push(name);
    }
    Ok(())
}

pub fn valid_domain(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.ends_with('.') {
        return false;
    }
    if host.chars().any(|ch| {
        matches!(ch, '/' | '\\' | '?' | '#' | '*' | ':' | '@' | ' ' | '\t') || ch.is_control()
    }) {
        return false;
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() || host.parse::<std::net::Ipv6Addr>().is_ok() {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels
        .iter()
        .any(|label| label.is_empty() || label.len() > 63)
    {
        return false;
    }
    labels.iter().all(|label| {
        let bytes = label.as_bytes();
        !bytes.first().is_some_and(|ch| *ch == b'-')
            && !bytes.last().is_some_and(|ch| *ch == b'-')
            && bytes
                .iter()
                .all(|ch| ch.is_ascii_alphanumeric() || *ch == b'-')
    })
}

fn parse_secrets(value: &toml::Value) -> Result<Vec<SecretDecl>, String> {
    let array = value.as_array().ok_or("secret 不是数组")?;
    if array.is_empty() {
        return Err("声明了 secret 但没有密钥项".to_string());
    }
    let mut secrets = Vec::new();
    let mut seen = BTreeSet::new();
    for item in array {
        let table = item.as_table().ok_or("密钥项不是表")?;
        require_keys(
            table.keys().map(String::as_str),
            &["id", "label", "header", "prefix"],
        )?;
        let id = required_str(table, "id")?;
        if !valid_id(id) {
            return Err("密钥 id 无效".to_string());
        }
        if !seen.insert(id.to_string()) {
            return Err("密钥 id 重复".to_string());
        }
        let label = trimmed_line(required_str(table, "label")?, "密钥名称")?;
        let header = optional_str(table, "header")?;
        let prefix = optional_str(table, "prefix")?;
        if prefix.is_some() && header.is_none() {
            return Err("写了前缀但没有请求头".to_string());
        }
        let header = match header {
            Some(header) => {
                header_name_reason(header)?;
                Some(header.to_string())
            }
            None => None,
        };
        let prefix = match prefix {
            Some(prefix) => {
                prefix_reason(prefix)?;
                prefix.to_string()
            }
            None => String::new(),
        };
        secrets.push(SecretDecl {
            id: id.to_string(),
            label,
            header,
            prefix,
        });
    }
    Ok(secrets)
}

fn parse_widgets(value: &toml::Value) -> Result<Vec<WidgetDecl>, String> {
    let array = value.as_array().ok_or("component 不是数组")?;
    let mut widgets = Vec::new();
    let mut seen = BTreeSet::new();
    for item in array {
        let table = item.as_table().ok_or("组件不是表")?;
        let kind_name = required_str(table, "component")?;
        let id = required_str(table, "id")?;
        if id.trim().is_empty() || id.chars().any(|ch| ch.is_control() || ch == '\n') {
            return Err("组件 id 无效".to_string());
        }
        if !seen.insert(id.to_string()) {
            return Err("组件 id 重复".to_string());
        }
        let kind = match kind_name {
            "text" => WidgetKind::Text,
            "button" => WidgetKind::Button,
            "timer" => WidgetKind::Timer,
            "progress" => WidgetKind::Progress,
            "list" => WidgetKind::List,
            "input" => WidgetKind::Input,
            "toggle" => WidgetKind::Toggle,
            other => return Err(format!("不认识的组件 {other}")),
        };
        let allowed: &[&str] = match kind {
            WidgetKind::Text => &["component", "id", "text"],
            WidgetKind::Button => &["component", "id", "label"],
            WidgetKind::Timer | WidgetKind::Progress | WidgetKind::List => &["component", "id"],
            WidgetKind::Input => &["component", "id", "text"],
            WidgetKind::Toggle => &["component", "id", "label", "on"],
        };
        require_keys(table.keys().map(String::as_str), allowed)?;
        let text = match kind {
            WidgetKind::Text | WidgetKind::Input => optional_str(table, "text")?.unwrap_or(""),
            _ => {
                if table.contains_key("text") {
                    return Err("这个组件不能写 text".to_string());
                }
                ""
            }
        };
        let label = match kind {
            WidgetKind::Button | WidgetKind::Toggle => {
                trimmed_line(required_str(table, "label")?, "按钮文字")?
            }
            _ => String::new(),
        };
        let on = if kind == WidgetKind::Toggle {
            match table.get("on") {
                None => false,
                Some(toml::Value::Boolean(value)) => *value,
                Some(_) => return Err("开关的 on 不是布尔".to_string()),
            }
        } else {
            false
        };
        widgets.push(WidgetDecl {
            id: id.to_string(),
            kind,
            text: text.to_string(),
            label,
            on,
        });
    }
    Ok(widgets)
}

fn check_entry_text(entry: &str) -> Result<(), String> {
    if entry.is_empty() || !entry.ends_with(".lua") {
        return Err("入口脚本必须以 .lua 结尾".to_string());
    }
    let path = Path::new(entry);
    if path.is_absolute() {
        return Err("入口脚本不能是绝对路径".to_string());
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("入口脚本不能离开工具文件夹".to_string());
    }
    Ok(())
}

fn entry_file(dir: &Path, entry: &str) -> Result<PathBuf, String> {
    check_entry_text(entry)?;
    let joined = dir.join(entry);
    let root = dir
        .canonicalize()
        .map_err(|err| format!("工具文件夹打不开：{err}"))?;
    let file = joined
        .canonicalize()
        .map_err(|_| "入口脚本不存在".to_string())?;
    if !file.starts_with(&root) {
        return Err("入口脚本不能离开工具文件夹".to_string());
    }
    Ok(file)
}

fn require_keys<'a>(keys: impl Iterator<Item = &'a str>, allowed: &[&str]) -> Result<(), String> {
    for key in keys {
        if !allowed.contains(&key) {
            return Err(format!("不认识的字段 {key}"));
        }
    }
    Ok(())
}

fn required_str<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    key: &str,
) -> Result<&'a str, String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("缺少 {key}"))
}

fn optional_str<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{key} 不是字符串")),
    }
}

fn trimmed_line(value: &str, label: &str) -> Result<String, String> {
    if value.contains('\n') || value.contains('\r') {
        return Err(format!("{label}不能含换行"));
    }
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label}为空"));
    }
    Ok(trimmed.to_string())
}

pub fn materialize(dir: &Path, manifest: &str, script: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("建目录失败：{err}"))?;
    std::fs::write(dir.join("manifest.toml"), manifest)
        .map_err(|err| format!("写清单失败：{err}"))?;
    std::fs::write(dir.join("main.lua"), script).map_err(|err| format!("写脚本失败：{err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_text(http: &str, secret: &str) -> String {
        format!(
            "id = \"example.headers\"\nname = \"示例\"\ndescription = \"说明\"\nentry = \"main.lua\"\nservices = [\"http\", \"secret\"]\ncomponent = []\n\n[http]\ndomains = [\"api.example.com\"]\n{http}\n\n{secret}\n"
        )
    }

    #[test]
    fn headers_and_post_parse_or_reject() {
        let secret = "[[secret]]\nid = \"token\"\nlabel = \"访问令牌\"\nheader = \"Authorization\"\nprefix = \"Bearer \"\n\n[[secret]]\nid = \"account\"\nlabel = \"账号\"\nheader = \"ChatGPT-Account-Id\"\n";
        let ok = tool_text(
            "post = true\nheaders = [\n  { name = \"anthropic-beta\", value = \"oauth-2025-04-20\" },\n  { name = \"anthropic-version\", value = \"2023-06-01\" },\n  { name = \"User-Agent\", value = \"my-tool\" },\n]\n",
            secret,
        );
        let (parsed, _) = parse(&ok).expect("清单");
        assert!(parsed.post);
        assert_eq!(parsed.headers.len(), 3);
        assert_eq!(parsed.secrets[0].prefix, "Bearer ");
        assert_eq!(
            parsed.secrets[1].header.as_deref(),
            Some("ChatGPT-Account-Id")
        );

        let cases = [
            (
                "headers = [{ name = \"Host\", value = \"evil.example\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "不能使用这个请求头：Host",
            ),
            (
                "headers = [{ name = \"cookie\", value = \"a\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "不能使用这个请求头：cookie",
            ),
            (
                "headers = [{ name = \"Content-Type\", value = \"text/plain\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "不能使用这个请求头：Content-Type",
            ),
            (
                "headers = [{ name = \"Transfer-Encoding\", value = \"chunked\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "不能使用这个请求头",
            ),
            (
                "headers = [{ name = \"Connection\", value = \"keep-alive\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "不能使用这个请求头",
            ),
            (
                "headers = [\n  { name = \"User-Agent\", value = \"a\" },\n  { name = \"user-agent\", value = \"b\" },\n]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "请求头重复",
            ),
            (
                "headers = [{ name = \"X-Bad\", value = \"a\\r\\nb\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "请求头的值无效",
            ),
            (
                "headers = [{ name = \"Authorization\", value = \"fixed\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\nheader = \"authorization\"\nprefix = \"Bearer \"\n",
                "请求头与密钥头重名",
            ),
            (
                "",
                "[[secret]]\nid = \"a\"\nlabel = \"甲\"\nheader = \"X-One\"\n\n[[secret]]\nid = \"b\"\nlabel = \"乙\"\nheader = \"x-one\"\n",
                "请求头重复",
            ),
            (
                "",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\nprefix = \"Bearer \"\n",
                "写了前缀但没有请求头",
            ),
            (
                "headers = [{ name = \"-bad\", value = \"a\" }]\n",
                "[[secret]]\nid = \"token\"\nlabel = \"令牌\"\n",
                "请求头名称无效",
            ),
        ];
        for (http, secret, expect) in cases {
            let err = parse(&tool_text(http, secret)).expect_err(expect);
            assert!(err.contains(expect), "{err} 不含 {expect}");
        }
    }

    #[test]
    fn domain_rules() {
        assert!(valid_domain("api.example.com"));
        assert!(valid_domain("localhost"));
        assert!(valid_domain("API.Example.COM"));
        assert!(!valid_domain("127.0.0.1"));
        assert!(!valid_domain("::1"));
        assert!(!valid_domain("*.example.com"));
        assert!(!valid_domain("api.example.com:443"));
        assert!(!valid_domain("https://api.example.com"));
        assert!(!valid_domain("api.example.com/v1"));
        assert!(!valid_domain(""));
    }
}
