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
pub enum Origin {
    Builtin,
    Imported,
}

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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub services: BTreeSet<String>,
    pub domains: Vec<String>,
    pub secrets: Vec<SecretDecl>,
    pub widgets: Vec<WidgetDecl>,
    /// 入口脚本相对工具文件夹的路径。不含 `..`。
    pub entry: String,
    pub script: String,
}

pub fn load_dir(dir: &Path, origin: Origin) -> Result<Manifest, String> {
    let manifest_path = dir.join("manifest.toml");
    let text = std::fs::read(&manifest_path).map_err(|err| format!("读不到清单：{err}"))?;
    let text = std::str::from_utf8(&text).map_err(|_| "清单不是 UTF-8".to_string())?;
    let (manifest, entry) = parse(text, origin)?;
    let script_path = entry_file(dir, &entry)?;
    let script = std::fs::read(&script_path).map_err(|err| format!("读不到入口脚本：{err}"))?;
    let script = std::str::from_utf8(&script).map_err(|_| "入口脚本不是 UTF-8".to_string())?;
    Ok(Manifest {
        script: script.to_string(),
        ..manifest
    })
}

pub fn parse(text: &str, origin: Origin) -> Result<(Manifest, String), String> {
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
        return Err("id 不是单个路径分量".to_string());
    }
    if origin == Origin::Imported && imported_prefix_forbidden(id) {
        return Err("导入的 id 不能以 lanwork. 开头".to_string());
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

    let domains = if services.contains("http") {
        parse_domains(table.get("http").ok_or("缺少 http")?)?
    } else {
        Vec::new()
    };
    let secrets = if services.contains("secret") {
        parse_secrets(table.get("secret").ok_or("缺少 secret")?)?
    } else {
        Vec::new()
    };
    let widgets = parse_widgets(table.get("component").ok_or("缺少 component")?)?;

    Ok((
        Manifest {
            id: id.to_string(),
            name,
            description,
            services,
            domains,
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

fn parse_domains(value: &toml::Value) -> Result<Vec<String>, String> {
    let table = value.as_table().ok_or("http 不是表")?;
    require_keys(table.keys().map(String::as_str), &["domains"])?;
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
    Ok(domains)
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
        require_keys(table.keys().map(String::as_str), &["id", "label"])?;
        let id = required_str(table, "id")?;
        if !valid_id(id) {
            return Err("密钥 id 无效".to_string());
        }
        if !seen.insert(id.to_string()) {
            return Err("密钥 id 重复".to_string());
        }
        let label = trimmed_line(required_str(table, "label")?, "密钥名称")?;
        secrets.push(SecretDecl {
            id: id.to_string(),
            label,
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
