//! 插件自己声明的登录描述。路径、字段、过期和请求都写在清单里。
//!
//! 宿主只读登录文件，不写、不刷新、不修改。脚本只能拿到清单点名的字段。
//! 内置 `[[usage]]` 不走这里。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::http::{BODY_MAX, Outgoing};
use crate::id::valid_id;
use crate::manifest::{
    FixedHeader, header_name_reason, optional_str, parse_headers, prefix_reason, require_keys,
    required_str, trimmed_line, valid_domain,
};
use crate::usage::{jwt_expiry_ms, origin_match, parse_rfc3339_ms};

/// 登录文件超过这个字节数就当作没有登录，不发请求。
pub const LOGIN_FILE_MAX: usize = 256 * 1024;
/// 同一个工具、同一个描述，两次真正去读文件、发请求的最短间隔。
pub const LOGIN_INTERVAL: Duration = Duration::from_secs(60);
const FIELD_LIMIT: usize = 8;
const EXTRACT_LIMIT: usize = 32;
const LOGIN_LIMIT: usize = 8;
const LABEL_CHARS: usize = 64;
const PATH_BYTES: usize = 512;
const PATH_SEGS: usize = 16;
const INDEX_MAX: usize = 4096;
const KEY_PREFIX_MAX: usize = 256;
const URL_MAX: usize = 2048;
const EXTRACT_CHARS: usize = 128;
/// 字段值以外，文件里每个不少于这么多字节的字符串都参与回显检查。
const EXTRA_SENSITIVE_MIN: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpiryFormat {
    EpochS,
    EpochMs,
    Rfc3339,
    JwtExp,
}

impl ExpiryFormat {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EpochS => "epoch_s",
            Self::EpochMs => "epoch_ms",
            Self::Rfc3339 => "rfc3339",
            Self::JwtExp => "jwt_exp",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "epoch_s" => Some(Self::EpochS),
            "epoch_ms" => Some(Self::EpochMs),
            "rfc3339" => Some(Self::Rfc3339),
            "jwt_exp" => Some(Self::JwtExp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractType {
    Number,
    String,
    Bool,
}

impl ExtractType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::String => "string",
            Self::Bool => "bool",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "number" => Some(Self::Number),
            "string" => Some(Self::String),
            "bool" => Some(Self::Bool),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginField {
    pub name: String,
    pub path: String,
    pub header: String,
    pub prefix: String,
    pub key_prefix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginExpires {
    pub format: ExpiryFormat,
    pub path: Option<String>,
    pub field: Option<String>,
    pub key_prefix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<FixedHeader>,
    pub body: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginExtract {
    pub name: String,
    pub path: String,
    pub kind: ExtractType,
}

/// 一份插件登录描述。授权按整份比较，改了任何一处都要再确认。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginDecl {
    pub id: String,
    pub label: String,
    pub file: String,
    pub fields: Vec<LoginField>,
    pub expires: Option<LoginExpires>,
    pub request: LoginRequest,
    pub extract: Vec<LoginExtract>,
}

/// 家目录。没有绝对路径时当作没有登录，不去读当前工作目录。
///
/// 插件描述不接受 `CODEX_HOME` 这一类目录变量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginRoots {
    pub home: Option<PathBuf>,
}

impl LoginRoots {
    #[must_use]
    pub fn from_env() -> Self {
        let userprofile = env_path("USERPROFILE");
        let home = env_path("HOME");
        let chosen = userprofile.or(home);
        Self {
            home: chosen.filter(|path| path.is_absolute()),
        }
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoginScalar {
    Number(f64),
    String(String),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoginValue {
    pub name: String,
    pub value: LoginScalar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginError {
    LoginExpired,
    Invalid,
    Failed,
    Unauthorized,
}

impl LoginError {
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::LoginExpired => "登录已过期",
            Self::Invalid => "数据无效",
            Self::Failed => "请求失败",
            Self::Unauthorized => "未授权",
        }
    }
}

/// 从登录文件读到、不能出现在提取结果里的一段文字。
///
/// `header` 是实际放进请求头的值（前缀加字段值）。其它文件字符串没有请求头。
#[derive(Clone)]
pub struct LoginSecret {
    value: String,
    header: Option<String>,
}

pub struct LoginCall {
    pub outgoing: Outgoing,
    /// 只留在这次请求的线程上，用来确认提取结果里没有这些字。不进脚本，也不进日志。
    pub sensitive: Vec<LoginSecret>,
}

/// 整份描述的指纹。缓存键带上它，清单改过之后不会交出上次的提取结果。
#[must_use]
pub fn login_fingerprint(login: &LoginDecl) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    login.id.hash(&mut hasher);
    login.label.hash(&mut hasher);
    login.file.hash(&mut hasher);
    for field in &login.fields {
        field.name.hash(&mut hasher);
        field.path.hash(&mut hasher);
        field.header.hash(&mut hasher);
        field.prefix.hash(&mut hasher);
        field.key_prefix.hash(&mut hasher);
    }
    match &login.expires {
        Some(expires) => {
            true.hash(&mut hasher);
            expires.format.as_str().hash(&mut hasher);
            expires.path.hash(&mut hasher);
            expires.field.hash(&mut hasher);
            expires.key_prefix.hash(&mut hasher);
        }
        None => false.hash(&mut hasher),
    }
    login.request.method.hash(&mut hasher);
    login.request.url.hash(&mut hasher);
    for header in &login.request.headers {
        header.name.hash(&mut hasher);
        header.value.hash(&mut hasher);
    }
    login.request.body.hash(&mut hasher);
    for item in &login.extract {
        item.name.hash(&mut hasher);
        item.path.hash(&mut hasher);
        item.kind.as_str().hash(&mut hasher);
    }
    hasher.finish()
}

impl std::fmt::Debug for LoginCall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginCall")
            .field("outgoing", &self.outgoing)
            .field("sensitive_len", &self.sensitive.len())
            .finish()
    }
}

/// 确认页上这一项的句子。`label` 写进第一句。
#[must_use]
pub fn confirm_lines(login: &LoginDecl) -> Vec<String> {
    let verb = if login.request.method == "POST" {
        "提交到"
    } else {
        "发往"
    };
    let mut lines = vec![format!(
        "读取本机文件 ~/{} 中的 {}，{verb} {}",
        login.file, login.label, login.request.url
    )];
    for header in &login.request.headers {
        lines.push(format!("请求头 {}: {}", header.name, header.value));
    }
    for field in &login.fields {
        if field.prefix.is_empty() {
            lines.push(format!("请求头 {}", field.header));
        } else {
            lines.push(format!("请求头 {}，前缀 {}", field.header, field.prefix));
        }
    }
    let names = login
        .extract
        .iter()
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>()
        .join("、");
    lines.push(format!("交给脚本：{names}"));
    lines
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
}

fn parse_json_path(text: &str) -> Result<Vec<Seg>, String> {
    if text.is_empty() || text.len() > PATH_BYTES {
        return Err("字段路径无效".to_string());
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut segs = Vec::new();
    if bytes[0] == b'[' {
        index = push_indexes(text, index, &mut segs)?;
    } else {
        let (key, next) = parse_key(text, index)?;
        segs.push(Seg::Key(key));
        index = push_indexes(text, next, &mut segs)?;
    }
    while index < bytes.len() {
        if bytes[index] != b'.' {
            return Err("字段路径无效".to_string());
        }
        index += 1;
        if index >= bytes.len() || bytes[index] == b'.' || bytes[index] == b'[' {
            return Err("字段路径无效".to_string());
        }
        let (key, next) = parse_key(text, index)?;
        segs.push(Seg::Key(key));
        index = push_indexes(text, next, &mut segs)?;
    }
    if segs.is_empty() || segs.len() > PATH_SEGS {
        return Err("字段路径无效".to_string());
    }
    Ok(segs)
}

fn parse_key(text: &str, start: usize) -> Result<(String, usize), String> {
    let bytes = text.as_bytes();
    let mut index = start;
    while index < bytes.len()
        && bytes[index] != b'.'
        && bytes[index] != b'['
        && bytes[index] != b']'
    {
        if bytes[index] < 0x20 || bytes[index] == 0x7f {
            return Err("字段路径无效".to_string());
        }
        index += 1;
    }
    if index == start {
        return Err("字段路径无效".to_string());
    }
    Ok((text[start..index].to_string(), index))
}

fn push_indexes(text: &str, mut index: usize, segs: &mut Vec<Seg>) -> Result<usize, String> {
    let bytes = text.as_bytes();
    while index < bytes.len() && bytes[index] == b'[' {
        index += 1;
        if index >= bytes.len() || !bytes[index].is_ascii_digit() {
            return Err("字段路径无效".to_string());
        }
        let number_at = index;
        if bytes[index] == b'0' {
            index += 1;
            if index < bytes.len() && bytes[index].is_ascii_digit() {
                return Err("字段路径无效".to_string());
            }
        } else {
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        }
        if index >= bytes.len() || bytes[index] != b']' {
            return Err("字段路径无效".to_string());
        }
        let number: usize = text[number_at..index]
            .parse()
            .map_err(|_| "字段路径无效".to_string())?;
        if number > INDEX_MAX {
            return Err("字段路径无效".to_string());
        }
        segs.push(Seg::Index(number));
        if segs.len() > PATH_SEGS {
            return Err("字段路径无效".to_string());
        }
        index += 1;
    }
    Ok(index)
}

fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let segs = parse_json_path(path).ok()?;
    let mut current = value;
    for seg in segs {
        current = match seg {
            Seg::Key(key) => current.as_object()?.get(&key)?,
            Seg::Index(index) => current.as_array()?.get(index)?,
        };
    }
    Some(current)
}

/// 把清单里的 `file` 收成相对家目录的路径。不合规则时不安装。
pub(crate) fn normalize_login_file(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("登录文件路径无效".to_string());
    }
    let text = text
        .strip_prefix("~/")
        .or_else(|| text.strip_prefix("~\\"))
        .unwrap_or(text);
    if text.is_empty() || text.starts_with('~') {
        return Err("登录文件路径无效".to_string());
    }
    if text.contains(':') || text.contains('\0') {
        return Err("登录文件路径无效".to_string());
    }
    if text.starts_with('/') || text.starts_with('\\') {
        return Err("登录文件路径无效".to_string());
    }
    let unified = text.replace('\\', "/");
    if unified.len() > PATH_BYTES || unified.starts_with("//") {
        return Err("登录文件路径无效".to_string());
    }
    let segments: Vec<&str> = unified.split('/').collect();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        return Err("登录文件路径无效".to_string());
    }
    let last = segments.last().copied().unwrap_or("");
    if !last.to_ascii_lowercase().ends_with(".json") || last.eq_ignore_ascii_case(".json") {
        return Err("登录文件必须是 JSON".to_string());
    }
    if denied_segments(&segments) {
        return Err("登录文件路径不允许".to_string());
    }
    Ok(unified)
}

fn denied_segments(segments: &[&str]) -> bool {
    if segments.iter().any(|segment| {
        segment.eq_ignore_ascii_case(".ssh") || segment.eq_ignore_ascii_case(".gnupg")
    }) {
        return true;
    }
    let appdata = segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("AppData"));
    if let Some(at) = appdata
        && segments[at + 1..]
            .iter()
            .any(|segment| segment.eq_ignore_ascii_case("User Data"))
    {
        return true;
    }
    let firefox = segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("Firefox"));
    if let Some(at) = firefox
        && segments[at + 1..]
            .iter()
            .any(|segment| segment.eq_ignore_ascii_case("Profiles"))
    {
        return true;
    }
    if has_consecutive(
        segments,
        &["AppData", "Roaming", "Microsoft", "Credentials"],
    ) || has_consecutive(segments, &["AppData", "Roaming", "Microsoft", "Protect"])
        || has_consecutive(segments, &["Documents", "Lanwork"])
    {
        return true;
    }
    false
}

fn has_consecutive(segments: &[&str], needle: &[&str]) -> bool {
    segments.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle.iter())
            .all(|(segment, expected)| segment.eq_ignore_ascii_case(expected))
    })
}

pub(crate) fn parse_login_array(
    value: &toml::Value,
    domains: &[String],
) -> Result<Vec<LoginDecl>, String> {
    let array = value.as_array().ok_or("login 不是数组")?;
    if array.is_empty() {
        return Err("声明了 login 但没有描述".to_string());
    }
    if array.len() > LOGIN_LIMIT {
        return Err("登录描述过多".to_string());
    }
    let mut logins = Vec::new();
    for item in array {
        let table = item.as_table().ok_or("登录描述不是表")?;
        require_keys(
            table.keys().map(String::as_str),
            &[
                "id", "label", "file", "fields", "expires", "request", "extract",
            ],
        )?;
        let id = required_str(table, "id")?;
        if !valid_id(id) {
            return Err("登录 id 无效".to_string());
        }
        if logins.iter().any(|login: &LoginDecl| login.id == id) {
            return Err("登录 id 重复".to_string());
        }
        let label = trimmed_line(required_str(table, "label")?, "登录名称")?;
        if label.chars().count() > LABEL_CHARS {
            return Err("登录名称过长".to_string());
        }
        let file = normalize_login_file(required_str(table, "file")?)?;
        let fields = parse_fields(table.get("fields").ok_or("缺少 fields")?)?;
        let expires = match table.get("expires") {
            None => None,
            Some(value) => Some(parse_expires(value, &fields)?),
        };
        let request = parse_request(table.get("request").ok_or("缺少 request")?, domains)?;
        reject_login_headers(&request.headers, &fields)?;
        let extract = parse_extract(table.get("extract").ok_or("缺少 extract")?)?;
        logins.push(LoginDecl {
            id: id.to_string(),
            label,
            file,
            fields,
            expires,
            request,
            extract,
        });
    }
    Ok(logins)
}

fn parse_fields(value: &toml::Value) -> Result<Vec<LoginField>, String> {
    let array = value.as_array().ok_or("fields 不是数组")?;
    if array.is_empty() {
        return Err("登录描述没有字段".to_string());
    }
    if array.len() > FIELD_LIMIT {
        return Err("登录字段过多".to_string());
    }
    let mut fields = Vec::new();
    for item in array {
        let table = item.as_table().ok_or("登录字段不是表")?;
        require_keys(
            table.keys().map(String::as_str),
            &["name", "path", "header", "prefix", "key_prefix"],
        )?;
        let name = required_str(table, "name")?;
        if !valid_id(name) {
            return Err("登录字段名无效".to_string());
        }
        if fields.iter().any(|field: &LoginField| field.name == name) {
            return Err("登录字段名重复".to_string());
        }
        let path = required_str(table, "path")?;
        parse_json_path(path)?;
        let header = required_str(table, "header")?;
        header_name_reason(header)?;
        let prefix = match optional_str(table, "prefix")? {
            Some(prefix) => {
                prefix_reason(prefix)?;
                prefix.to_string()
            }
            None => String::new(),
        };
        let key_prefix = match optional_str(table, "key_prefix")? {
            Some(prefix) => Some(valid_key_prefix(prefix)?),
            None => None,
        };
        fields.push(LoginField {
            name: name.to_string(),
            path: path.to_string(),
            header: header.to_string(),
            prefix,
            key_prefix,
        });
    }
    Ok(fields)
}

fn parse_expires(value: &toml::Value, fields: &[LoginField]) -> Result<LoginExpires, String> {
    let table = value.as_table().ok_or("expires 不是表")?;
    require_keys(
        table.keys().map(String::as_str),
        &["format", "path", "field", "key_prefix"],
    )?;
    let name = required_str(table, "format")?;
    let format = ExpiryFormat::parse(name).ok_or_else(|| format!("不认识的过期格式 {name}"))?;
    let path = match optional_str(table, "path")? {
        Some(path) => {
            parse_json_path(path)?;
            Some(path.to_string())
        }
        None => None,
    };
    let field = match optional_str(table, "field")? {
        Some(field) => {
            if !fields.iter().any(|item| item.name == field) {
                return Err("过期字段不存在".to_string());
            }
            Some(field.to_string())
        }
        None => None,
    };
    let key_prefix = match optional_str(table, "key_prefix")? {
        Some(prefix) => Some(valid_key_prefix(prefix)?),
        None => None,
    };
    match format {
        ExpiryFormat::JwtExp => {
            if field.is_none() || path.is_some() || key_prefix.is_some() {
                return Err("jwt_exp 只能写 field".to_string());
            }
        }
        ExpiryFormat::EpochS | ExpiryFormat::EpochMs | ExpiryFormat::Rfc3339 => {
            if path.is_none() || field.is_some() {
                return Err("这种过期格式要写 path".to_string());
            }
        }
    }
    Ok(LoginExpires {
        format,
        path,
        field,
        key_prefix,
    })
}

fn parse_request(value: &toml::Value, domains: &[String]) -> Result<LoginRequest, String> {
    let table = value.as_table().ok_or("request 不是表")?;
    require_keys(
        table.keys().map(String::as_str),
        &["method", "url", "headers", "body"],
    )?;
    let method = required_str(table, "method")?;
    if method != "GET" && method != "POST" {
        return Err("登录请求的方法无效".to_string());
    }
    let url = required_str(table, "url")?;
    exact_https_url(url, domains)?;
    let headers = match table.get("headers") {
        None => Vec::new(),
        Some(value) => parse_headers(value)?,
    };
    let body = match optional_str(table, "body")? {
        Some(body) => {
            if body.len() > BODY_MAX {
                return Err("登录请求的正文过大".to_string());
            }
            if serde_json::from_str::<Value>(body).is_err() {
                return Err("登录请求的正文不是 JSON".to_string());
            }
            Some(body.to_string())
        }
        None => None,
    };
    if method == "POST" && body.is_none() {
        return Err("POST 登录请求没有正文".to_string());
    }
    if method == "GET" && body.is_some() {
        return Err("GET 登录请求不能带正文".to_string());
    }
    Ok(LoginRequest {
        method: method.to_string(),
        url: url.to_string(),
        headers,
        body,
    })
}

fn exact_https_url(raw: &str, domains: &[String]) -> Result<(), String> {
    if raw.is_empty()
        || raw.len() > URL_MAX
        || raw
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte < 0x20)
    {
        return Err("登录地址无效".to_string());
    }
    let parsed = url::Url::parse(raw).map_err(|_| "登录地址无效".to_string())?;
    if parsed.scheme() != "https" {
        return Err("登录地址必须是 https".to_string());
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.fragment().is_some()
    {
        return Err("登录地址无效".to_string());
    }
    let host = parsed.host_str().ok_or("登录地址无效")?;
    if !valid_domain(host) {
        return Err("登录地址的主机名无效".to_string());
    }
    if !domains
        .iter()
        .any(|domain| domain.eq_ignore_ascii_case(host))
    {
        return Err("登录地址的主机名不在 domains 里".to_string());
    }
    if parsed
        .path()
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err("登录地址无效".to_string());
    }
    let query = match parsed.query() {
        Some(query) => format!("?{query}"),
        None => String::new(),
    };
    let rebuilt = format!("https://{host}{}{query}", parsed.path());
    if rebuilt != raw {
        return Err("登录地址无效".to_string());
    }
    Ok(())
}

fn reject_login_headers(headers: &[FixedHeader], fields: &[LoginField]) -> Result<(), String> {
    let mut seen: Vec<&str> = headers.iter().map(|header| header.name.as_str()).collect();
    for field in fields {
        if seen
            .iter()
            .any(|name| field.header.eq_ignore_ascii_case(name))
        {
            return Err("请求头重复".to_string());
        }
        seen.push(field.header.as_str());
    }
    Ok(())
}

fn parse_extract(value: &toml::Value) -> Result<Vec<LoginExtract>, String> {
    let array = value.as_array().ok_or("extract 不是数组")?;
    if array.is_empty() {
        return Err("登录描述没有提取字段".to_string());
    }
    if array.len() > EXTRACT_LIMIT {
        return Err("提取字段过多".to_string());
    }
    let mut extract = Vec::new();
    for item in array {
        let table = item.as_table().ok_or("提取字段不是表")?;
        require_keys(table.keys().map(String::as_str), &["name", "path", "type"])?;
        let name = required_str(table, "name")?;
        if !valid_id(name) {
            return Err("提取字段名无效".to_string());
        }
        if extract.iter().any(|item: &LoginExtract| item.name == name) {
            return Err("提取字段名重复".to_string());
        }
        let path = required_str(table, "path")?;
        parse_json_path(path)?;
        let kind_name = required_str(table, "type")?;
        let kind =
            ExtractType::parse(kind_name).ok_or_else(|| format!("不认识的提取类型 {kind_name}"))?;
        extract.push(LoginExtract {
            name: name.to_string(),
            path: path.to_string(),
            kind,
        });
    }
    Ok(extract)
}

fn valid_key_prefix(text: &str) -> Result<String, String> {
    if text.is_empty()
        || text.len() > KEY_PREFIX_MAX
        || text.contains('*')
        || text.contains("..")
        || !text.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err("键前缀无效".to_string());
    }
    Ok(text.to_string())
}

pub fn prepare_login(
    login: &LoginDecl,
    roots: &LoginRoots,
    data_dir: Option<&Path>,
    now_ms: i64,
) -> Result<LoginCall, LoginError> {
    let bytes = read_login_file(&login.file, roots, data_dir)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| LoginError::LoginExpired)?;
    let root: Value = serde_json::from_str(text).map_err(|_| LoginError::LoginExpired)?;
    let selected = select_prefixes(&root, login, now_ms)?;
    let mut headers = Vec::new();
    for header in &login.request.headers {
        headers.push((header.name.clone(), header.value.clone()));
    }
    let mut field_values = Vec::new();
    let mut field_pairs = Vec::new();
    for field in &login.fields {
        let base = match &field.key_prefix {
            Some(prefix) => selected
                .get(prefix)
                .copied()
                .ok_or(LoginError::LoginExpired)?,
            None => &root,
        };
        let value = lookup(base, &field.path).ok_or(LoginError::LoginExpired)?;
        let text = value.as_str().ok_or(LoginError::LoginExpired)?;
        if !header_safe(text) {
            return Err(LoginError::LoginExpired);
        }
        let header_value = format!("{}{text}", field.prefix);
        field_values.push(text.to_string());
        field_pairs.push((text.to_string(), header_value.clone()));
        headers.push((field.header.clone(), header_value));
    }
    if let Some(expires) = &login.expires {
        let exp = expiry_ms(&root, login, expires, &selected, &field_values)
            .ok_or(LoginError::LoginExpired)?;
        if now_ms >= exp {
            return Err(LoginError::LoginExpired);
        }
    }
    let body = match (&login.request.method[..], &login.request.body) {
        ("POST", Some(body)) => body.as_bytes().to_vec(),
        ("GET", None) => Vec::new(),
        _ => return Err(LoginError::LoginExpired),
    };
    Ok(LoginCall {
        outgoing: Outgoing {
            method: login.request.method.clone(),
            url: login.request.url.clone(),
            headers,
            body,
        },
        sensitive: sensitive_values(&root, &field_pairs),
    })
}

fn header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}

fn select_prefixes<'a>(
    root: &'a Value,
    login: &LoginDecl,
    now_ms: i64,
) -> Result<HashMap<String, &'a Value>, LoginError> {
    let mut prefixes = Vec::new();
    for field in &login.fields {
        if let Some(prefix) = &field.key_prefix
            && !prefixes.iter().any(|item: &String| item == prefix)
        {
            prefixes.push(prefix.clone());
        }
    }
    if let Some(expires) = &login.expires
        && let Some(prefix) = &expires.key_prefix
        && !prefixes.iter().any(|item| item == prefix)
    {
        prefixes.push(prefix.clone());
    }
    if prefixes.is_empty() {
        return Ok(HashMap::new());
    }
    let map = root.as_object().ok_or(LoginError::LoginExpired)?;
    let mut selected = HashMap::new();
    for prefix in &prefixes {
        let mut keys: Vec<&String> = map.keys().filter(|key| origin_match(key, prefix)).collect();
        keys.sort();
        let mut chosen = None;
        for key in keys {
            let entry = &map[key];
            if !prefix_fields_ready(entry, login, prefix) {
                continue;
            }
            if !prefix_expiry_ready(entry, login, prefix, now_ms) {
                continue;
            }
            chosen = Some(entry);
            break;
        }
        let Some(entry) = chosen else {
            return Err(LoginError::LoginExpired);
        };
        selected.insert(prefix.clone(), entry);
    }
    Ok(selected)
}

fn prefix_fields_ready(entry: &Value, login: &LoginDecl, prefix: &str) -> bool {
    login
        .fields
        .iter()
        .filter(|field| field.key_prefix.as_deref() == Some(prefix))
        .all(|field| {
            lookup(entry, &field.path)
                .and_then(Value::as_str)
                .is_some_and(header_safe)
        })
}

fn prefix_expiry_ready(entry: &Value, login: &LoginDecl, prefix: &str, now_ms: i64) -> bool {
    let Some(expires) = &login.expires else {
        return true;
    };
    if expires.key_prefix.as_deref() == Some(prefix) {
        let Some(path) = &expires.path else {
            return false;
        };
        let Some(found) = lookup(entry, path) else {
            return false;
        };
        return expiry_from_value(expires.format, found).is_some_and(|exp| now_ms < exp);
    }
    if expires.format == ExpiryFormat::JwtExp
        && let Some(name) = &expires.field
        && login
            .fields
            .iter()
            .any(|field| field.name == *name && field.key_prefix.as_deref() == Some(prefix))
    {
        let Some(field) = login.fields.iter().find(|field| field.name == *name) else {
            return false;
        };
        let Some(token) = lookup(entry, &field.path).and_then(Value::as_str) else {
            return false;
        };
        return jwt_expiry_ms(token).is_some_and(|exp| now_ms < exp);
    }
    true
}

fn expiry_ms(
    root: &Value,
    login: &LoginDecl,
    expires: &LoginExpires,
    selected: &HashMap<String, &Value>,
    field_values: &[String],
) -> Option<i64> {
    match expires.format {
        ExpiryFormat::JwtExp => {
            let name = expires.field.as_deref()?;
            let index = login.fields.iter().position(|field| field.name == name)?;
            jwt_expiry_ms(field_values.get(index)?)
        }
        ExpiryFormat::EpochS | ExpiryFormat::EpochMs | ExpiryFormat::Rfc3339 => {
            let path = expires.path.as_deref()?;
            let base = match &expires.key_prefix {
                Some(prefix) => *selected.get(prefix)?,
                None => root,
            };
            expiry_from_value(expires.format, lookup(base, path)?)
        }
    }
}

fn expiry_from_value(format: ExpiryFormat, value: &Value) -> Option<i64> {
    match format {
        ExpiryFormat::EpochS => integer_ms(value, 1000),
        ExpiryFormat::EpochMs => integer_ms(value, 1),
        ExpiryFormat::Rfc3339 => {
            let text = value.as_str()?.trim();
            parse_rfc3339_ms(text)
        }
        ExpiryFormat::JwtExp => None,
    }
}

fn integer_ms(value: &Value, scale: i64) -> Option<i64> {
    let raw = match value {
        Value::Number(number) => number.as_i64()?,
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() || text.starts_with('+') {
                return None;
            }
            text.parse().ok()?
        }
        _ => return None,
    };
    raw.checked_mul(scale)
}

fn sensitive_values(root: &Value, field_values: &[(String, String)]) -> Vec<LoginSecret> {
    let mut out = Vec::new();
    for (value, header) in field_values {
        push_secret(&mut out, value, Some(header.clone()));
    }
    let mut extra = Vec::new();
    collect_long_strings(root, &mut extra);
    for value in extra {
        push_secret(&mut out, &value, None);
    }
    out
}

fn push_secret(out: &mut Vec<LoginSecret>, value: &str, header: Option<String>) {
    if value.is_empty() {
        return;
    }
    if let Some(existing) = out.iter_mut().find(|item| item.value == value) {
        if existing.header.is_none() {
            existing.header = header;
        }
        return;
    }
    out.push(LoginSecret {
        value: value.to_string(),
        header,
    });
}

fn collect_long_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if text.len() >= EXTRA_SENSITIVE_MIN {
                out.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_long_strings(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_long_strings(item, out);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub fn interpret_login(
    login: &LoginDecl,
    status: u16,
    body: &str,
    sensitive: &[LoginSecret],
) -> Result<Vec<LoginValue>, LoginError> {
    if status == 401 || status == 403 {
        return Err(LoginError::LoginExpired);
    }
    if !(200..300).contains(&status) {
        return Err(LoginError::Failed);
    }
    let root: Value = serde_json::from_str(body).map_err(|_| LoginError::Invalid)?;
    let mut out = Vec::new();
    let mut strings = Vec::new();
    for extract in &login.extract {
        let Some(found) = lookup(&root, &extract.path) else {
            continue;
        };
        if found.is_null() {
            continue;
        }
        match extract.kind {
            ExtractType::Number => {
                let number = found
                    .as_f64()
                    .filter(|number| number.is_finite())
                    .ok_or(LoginError::Invalid)?;
                out.push(LoginValue {
                    name: extract.name.clone(),
                    value: LoginScalar::Number(number),
                });
            }
            ExtractType::Bool => {
                out.push(LoginValue {
                    name: extract.name.clone(),
                    value: LoginScalar::Bool(found.as_bool().ok_or(LoginError::Invalid)?),
                });
            }
            ExtractType::String => {
                let raw = found.as_str().ok_or(LoginError::Invalid)?.to_string();
                strings.push(raw);
                out.push(LoginValue {
                    name: extract.name.clone(),
                    value: LoginScalar::String(String::new()),
                });
            }
        }
    }
    if out.is_empty() {
        return Err(LoginError::Invalid);
    }
    if exposes_login(&strings, sensitive) {
        return Err(LoginError::Invalid);
    }
    let mut strings = strings.into_iter();
    for item in &mut out {
        if let LoginScalar::String(text) = &mut item.value {
            *text = clean_string(&strings.next().unwrap_or_default());
        }
    }
    Ok(out)
}

/// 字符串提取对上登录文件里的值。单个字段、以及按声明顺序拼起来的全部字符串都查。
///
/// 形式与密钥回显相同，另加十六进制。比对前去掉格式字符和空白。数字和布尔不在这里。
/// 这是尽力检查。
fn exposes_login(raw_strings: &[String], sensitive: &[LoginSecret]) -> bool {
    if raw_strings.is_empty() || sensitive.iter().all(|item| item.value.is_empty()) {
        return false;
    }
    let stripped: Vec<String> = raw_strings
        .iter()
        .map(|text| strip_format_and_space(text))
        .collect();
    let cleaned: Vec<String> = raw_strings.iter().map(|text| clean_string(text)).collect();
    let cleaned_stripped: Vec<String> = cleaned
        .iter()
        .map(|text| strip_format_and_space(text))
        .collect();
    let mut parts = Vec::new();
    parts.extend(raw_strings.iter().cloned());
    parts.extend(stripped.iter().cloned());
    parts.extend(cleaned.iter().cloned());
    parts.extend(cleaned_stripped.iter().cloned());
    if raw_strings.len() > 1 {
        parts.push(raw_strings.concat());
        parts.push(stripped.concat());
        parts.push(cleaned.concat());
        parts.push(cleaned_stripped.concat());
    }
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    sensitive.iter().any(|item| {
        crate::http::response_exposes_secret(&refs, &item.value, item.header.as_deref())
    })
}

fn strip_format_and_space(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_format_char(*ch))
        .collect()
}

fn is_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

fn clean_string(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control())
        .take(EXTRACT_CHARS)
        .collect()
}

/// 只打开一次。最终路径和硬链接数都从这次的句柄上取，读也读这个句柄。
fn read_login_file(
    relative: &str,
    roots: &LoginRoots,
    data_dir: Option<&Path>,
) -> Result<Vec<u8>, LoginError> {
    let home = roots
        .home
        .as_deref()
        .filter(|path| path.is_absolute())
        .ok_or(LoginError::LoginExpired)?;
    // 清单阶段已经拒绝绝对路径和 `..`。这里再拦一次，避免调用方绕过解析。
    let normalized = normalize_login_file(relative).map_err(|_| LoginError::LoginExpired)?;
    let joined = home.join(normalized.replace('/', std::path::MAIN_SEPARATOR_STR));
    let mut file = open_login_handle(&joined)?;
    if !is_single_regular_file(&file) {
        return Err(LoginError::LoginExpired);
    }
    let final_path = final_path_from_handle(&file)?;
    let home_canon = std::fs::canonicalize(home).map_err(|_| LoginError::LoginExpired)?;
    let relative_parts =
        components_under(&home_canon, &final_path).ok_or(LoginError::LoginExpired)?;
    let borrowed: Vec<&str> = relative_parts.iter().map(String::as_str).collect();
    if denied_segments(&borrowed) {
        return Err(LoginError::LoginExpired);
    }
    if let Some(data_dir) = data_dir.filter(|path| path.is_absolute())
        && path_inside(data_dir, &final_path)
    {
        return Err(LoginError::LoginExpired);
    }
    let name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.to_ascii_lowercase().ends_with(".json") {
        return Err(LoginError::LoginExpired);
    }
    read_capped_handle(&mut file)
}

fn is_single_regular_file(file: &std::fs::File) -> bool {
    #[cfg(unix)]
    {
        let Ok(meta) = file.metadata() else {
            return false;
        };
        if !meta.is_file() {
            return false;
        }
        use std::os::unix::fs::MetadataExt;
        meta.nlink() <= 1
    }
    #[cfg(windows)]
    {
        windows_single_regular_file(file)
    }
}

/// 硬链接数、目录和重解析点都从这次的句柄上读，不用尚未稳定的 `MetadataExt`。
#[cfg(windows)]
fn windows_single_regular_file(file: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let handle = HANDLE(file.as_raw_handle());
    if unsafe { GetFileInformationByHandle(handle, &raw mut info) }.is_err() {
        return false;
    }
    let attr = info.dwFileAttributes;
    info.nNumberOfLinks <= 1
        && attr & FILE_ATTRIBUTE_DIRECTORY.0 == 0
        && attr & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
}

/// 打开登录文件。Windows 上允许同时读和写，不打开重解析点标志：
/// 当时已经存在的符号链接和 junction 会跟着走，句柄绑定到目标文件。
/// 之后换到路径上的重解析点不会再被这次读取跟着走。
fn open_login_handle(path: &Path) -> Result<std::fs::File, LoginError> {
    #[cfg(unix)]
    {
        std::fs::File::open(path).map_err(|_| LoginError::LoginExpired)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;
        use windows::Win32::Foundation::GENERIC_READ;
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows::core::PCWSTR;
        let wide = wide_extended(path);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|_| LoginError::LoginExpired)?;
        if handle.is_invalid() {
            return Err(LoginError::LoginExpired);
        }
        Ok(unsafe { std::fs::File::from_raw_handle(handle.0) })
    }
}

#[cfg(windows)]
fn wide_extended(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide = Vec::new();
    let text = path.as_os_str();
    let lossy = text.to_string_lossy();
    if path.is_absolute() && !lossy.starts_with(r"\\?\") {
        if lossy.starts_with(r"\\") {
            wide.extend(r"\\?\UNC\".encode_utf16());
            wide.extend(text.encode_wide().skip(2));
        } else {
            wide.extend(r"\\?\".encode_utf16());
            wide.extend(text.encode_wide());
        }
    } else {
        wide.extend(text.encode_wide());
    }
    wide.push(0);
    wide
}

fn final_path_from_handle(file: &std::fs::File) -> Result<PathBuf, LoginError> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let link = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .map_err(|_| LoginError::LoginExpired)?;
        if !link.is_absolute() {
            return Err(LoginError::LoginExpired);
        }
        let text = link.to_string_lossy();
        if text.ends_with(" (deleted)") {
            return Err(LoginError::LoginExpired);
        }
        Ok(link)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW,
        };
        let handle = HANDLE(file.as_raw_handle());
        let mut buf = vec![0u16; 512];
        loop {
            let n = unsafe { GetFinalPathNameByHandleW(handle, &mut buf, FILE_NAME_NORMALIZED) };
            if n == 0 {
                return Err(LoginError::LoginExpired);
            }
            let n = n as usize;
            if n < buf.len() {
                let path = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
                if path.as_os_str().is_empty() {
                    return Err(LoginError::LoginExpired);
                }
                return Ok(path);
            }
            if n > 32 * 1024 {
                return Err(LoginError::LoginExpired);
            }
            buf.resize(n, 0);
        }
    }
}

fn components_under(home: &Path, file: &Path) -> Option<Vec<String>> {
    let home = strip_verbatim(home);
    let file = strip_verbatim(file);
    let home_parts = component_keys(&home);
    let file_parts = component_keys(&file);
    if file_parts.len() <= home_parts.len() {
        return None;
    }
    if home_parts
        .iter()
        .zip(file_parts.iter())
        .any(|(left, right)| left != right)
    {
        return None;
    }
    Some(file_parts[home_parts.len()..].to_vec())
}

fn path_inside(root: &Path, file: &Path) -> bool {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let root_parts = component_keys(&strip_verbatim(&root));
    let file_parts = component_keys(&strip_verbatim(file));
    if root_parts.is_empty() || file_parts.len() <= root_parts.len() {
        return false;
    }
    root_parts
        .iter()
        .zip(file_parts.iter())
        .all(|(left, right)| left == right)
}

fn component_keys(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_ascii_lowercase()),
            Component::Prefix(prefix) => {
                Some(prefix.as_os_str().to_string_lossy().to_ascii_lowercase())
            }
            Component::RootDir => Some(String::new()),
            Component::CurDir | Component::ParentDir => None,
        })
        .collect()
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

fn read_capped_handle(file: &mut std::fs::File) -> Result<Vec<u8>, LoginError> {
    let mut reader = file.take(u64::try_from(LOGIN_FILE_MAX).unwrap_or(u64::MAX) + 1);
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|_| LoginError::LoginExpired)?;
    if buf.len() > LOGIN_FILE_MAX {
        return Err(LoginError::LoginExpired);
    }
    Ok(buf)
}

struct CacheSlot {
    at: Instant,
    outcome: Result<Vec<LoginValue>, LoginError>,
}

thread_local! {
    static LOGIN_CACHE: std::cell::RefCell<HashMap<(String, String, u64), CacheSlot>> =
        std::cell::RefCell::new(HashMap::new());
}

#[must_use]
pub fn cached_login(
    tool: &str,
    id: &str,
    fingerprint: u64,
) -> Option<Result<Vec<LoginValue>, LoginError>> {
    LOGIN_CACHE.with(|slot| {
        let cache = slot.borrow();
        let entry = cache.get(&(tool.to_string(), id.to_string(), fingerprint))?;
        if entry.at.elapsed() >= LOGIN_INTERVAL {
            None
        } else {
            Some(entry.outcome.clone())
        }
    })
}

pub fn store_login(
    tool: &str,
    id: &str,
    fingerprint: u64,
    outcome: Result<Vec<LoginValue>, LoginError>,
) {
    LOGIN_CACHE.with(|slot| {
        slot.borrow_mut().insert(
            (tool.to_string(), id.to_string(), fingerprint),
            CacheSlot {
                at: Instant::now(),
                outcome,
            },
        );
    });
}

#[cfg(test)]
pub(crate) fn reset_login_cache_for_test() {
    LOGIN_CACHE.with(|slot| slot.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn age_login_cache_for_test(by: Duration) {
    LOGIN_CACHE.with(|slot| {
        for entry in slot.borrow_mut().values_mut() {
            entry.at = entry.at.checked_sub(by).unwrap_or(entry.at);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lanwork-login-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("临时目录");
        path
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            Self(scratch(name))
        }
    }

    impl std::ops::Deref for TempDir {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn link_dir(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("符号链接");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // `cmd /C` 会剥掉首尾引号。raw_arg 不再让 Rust 包一层，mklink 才能看见路径上的引号。
            let output = std::process::Command::new("cmd")
                .raw_arg(format!(
                    "/C mklink /J \"{}\" \"{}\"",
                    link.display(),
                    target.display()
                ))
                .output()
                .expect("cmd");
            assert!(
                output.status.success(),
                "junction {} -> {}: {}{}",
                link.display(),
                target.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn sample(file: &str) -> LoginDecl {
        LoginDecl {
            id: "chatgpt".to_string(),
            label: "ChatGPT 登录".to_string(),
            file: file.to_string(),
            fields: vec![LoginField {
                name: "token".to_string(),
                path: "tokens.access_token".to_string(),
                header: "Authorization".to_string(),
                prefix: "Bearer ".to_string(),
                key_prefix: None,
            }],
            expires: Some(LoginExpires {
                format: ExpiryFormat::JwtExp,
                path: None,
                field: Some("token".to_string()),
                key_prefix: None,
            }),
            request: LoginRequest {
                method: "GET".to_string(),
                url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
                headers: vec![FixedHeader {
                    name: "Accept".to_string(),
                    value: "application/json".to_string(),
                }],
                body: None,
            },
            extract: vec![LoginExtract {
                name: "used".to_string(),
                path: "used".to_string(),
                kind: ExtractType::Number,
            }],
        }
    }

    fn b64url(bytes: &[u8]) -> String {
        const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        let mut index = 0;
        while index + 3 <= bytes.len() {
            let n = (u32::from(bytes[index]) << 16)
                | (u32::from(bytes[index + 1]) << 8)
                | u32::from(bytes[index + 2]);
            out.push(ALPHA[((n >> 18) & 63) as usize] as char);
            out.push(ALPHA[((n >> 12) & 63) as usize] as char);
            out.push(ALPHA[((n >> 6) & 63) as usize] as char);
            out.push(ALPHA[(n & 63) as usize] as char);
            index += 3;
        }
        if index < bytes.len() {
            let mut n = u32::from(bytes[index]) << 16;
            out.push(ALPHA[((n >> 18) & 63) as usize] as char);
            if index + 1 < bytes.len() {
                n |= u32::from(bytes[index + 1]) << 8;
                out.push(ALPHA[((n >> 12) & 63) as usize] as char);
                out.push(ALPHA[((n >> 6) & 63) as usize] as char);
            } else {
                out.push(ALPHA[((n >> 12) & 63) as usize] as char);
            }
        }
        out
    }

    fn secret(value: &str) -> LoginSecret {
        LoginSecret {
            value: value.to_string(),
            header: None,
        }
    }

    fn jwt(exp: i64) -> String {
        let header = b64url(br#"{"alg":"none"}"#);
        let payload = b64url(format!(r#"{{"exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn path_syntax_accepts_dots_and_indexes() {
        let segs = parse_json_path("tokens.access_token").unwrap();
        assert_eq!(
            segs,
            vec![
                Seg::Key("tokens".to_string()),
                Seg::Key("access_token".to_string())
            ]
        );
        let segs = parse_json_path("items[0].name").unwrap();
        assert_eq!(
            segs,
            vec![
                Seg::Key("items".to_string()),
                Seg::Index(0),
                Seg::Key("name".to_string())
            ]
        );
        let segs = parse_json_path("[2][10].a").unwrap();
        assert_eq!(
            segs,
            vec![Seg::Index(2), Seg::Index(10), Seg::Key("a".to_string())]
        );
        let value = serde_json::json!({"items":[{"name":"n"}]});
        assert_eq!(
            lookup(&value, "items[0].name").and_then(Value::as_str),
            Some("n")
        );
        for bad in [
            "", ".", "..", "a.", ".a", "a..b", "a[", "a[01]", "a[-1]", "a[4097]", "a]", "[ ]",
        ] {
            assert!(parse_json_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_paths_reject_escape_and_sensitive_locations() {
        assert_eq!(
            normalize_login_file("~/.codex/auth.json").unwrap(),
            ".codex/auth.json"
        );
        assert_eq!(
            normalize_login_file(".codex\\auth.json").unwrap(),
            ".codex/auth.json"
        );
        let rejected = [
            "../auth.json",
            ".codex/../../auth.json",
            "/etc/auth.json",
            "\\Windows\\auth.json",
            "C:/auth.json",
            "C:\\auth.json",
            "//server/share/auth.json",
            "\\\\server\\share\\auth.json",
            ".codex/auth.json:stream",
            "notes/auth.txt",
            ".ssh/id.json",
            "foo/.gnupg/keys.json",
            "AppData/Local/Google/Chrome/User Data/Default/Login.json",
            "AppData/Roaming/Mozilla/Firefox/Profiles/abc.json",
            "AppData/Roaming/Microsoft/Credentials/x.json",
            "AppData/Roaming/Microsoft/Protect/x.json",
            "Documents/Lanwork/secret.json",
            "~",
            "",
        ];
        for path in rejected {
            assert!(normalize_login_file(path).is_err(), "{path}");
        }
    }

    #[test]
    fn prefix_skips_expired_and_rejects_lookalike_hosts() {
        let mut login = sample("notes/auth.json");
        login.fields[0].path = "key".to_string();
        login.fields[0].key_prefix = Some("https://auth.x.ai".to_string());
        login.expires = Some(LoginExpires {
            format: ExpiryFormat::Rfc3339,
            path: Some("expires_at".to_string()),
            field: None,
            key_prefix: Some("https://auth.x.ai".to_string()),
        });
        let root = serde_json::json!({
            "https://auth.x.ai.evil.example": {
                "key": "evil-token-value",
                "expires_at": "2099-01-01T00:00:00Z"
            },
            "https://auth.x.ai": {
                "key": "older-token-value",
                "expires_at": "2000-01-01T00:00:00Z"
            },
            "https://auth.x.ai/oauth": {
                "key": "good-token-value",
                "expires_at": "2099-01-01T00:00:00Z"
            }
        });
        let selected = select_prefixes(&root, &login, 1_700_000_000_000).unwrap();
        let entry = selected.get("https://auth.x.ai").unwrap();
        assert_eq!(
            lookup(entry, "key").and_then(Value::as_str),
            Some("good-token-value")
        );
        assert!(!origin_match(
            "https://auth.x.ai.evil.example",
            "https://auth.x.ai"
        ));
        assert!(origin_match("https://auth.x.ai/oauth", "https://auth.x.ai"));
        assert!(origin_match("https://auth.x.ai:443", "https://auth.x.ai"));
    }

    #[test]
    fn expiry_formats_and_malformed_jwt_do_not_panic() {
        let future = serde_json::json!(4_102_444_800i64);
        let past = serde_json::json!(1_577_836_800i64);
        assert!(expiry_from_value(ExpiryFormat::EpochS, &future).unwrap() > 1_700_000_000_000);
        assert!(expiry_from_value(ExpiryFormat::EpochS, &past).unwrap() < 1_700_000_000_000);
        assert_eq!(
            expiry_from_value(
                ExpiryFormat::EpochMs,
                &serde_json::json!(1_700_000_000_000i64)
            ),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            expiry_from_value(ExpiryFormat::EpochS, &serde_json::json!("4102444800")),
            expiry_from_value(ExpiryFormat::EpochS, &future)
        );
        assert!(expiry_from_value(ExpiryFormat::EpochS, &serde_json::json!("nope")).is_none());
        assert!(expiry_from_value(ExpiryFormat::EpochS, &serde_json::json!(1.5)).is_none());
        assert!(expiry_from_value(ExpiryFormat::Rfc3339, &serde_json::json!(1)).is_none());
        assert!(
            expiry_from_value(
                ExpiryFormat::Rfc3339,
                &serde_json::json!("2099-01-01T00:00:00Z")
            )
            .is_some()
        );
        assert!(
            expiry_from_value(ExpiryFormat::Rfc3339, &serde_json::json!("not-a-date")).is_none()
        );
        let good = jwt(4_102_444_800);
        assert!(jwt_expiry_ms(&good).unwrap() > 1_700_000_000_000);
        let bad = [
            String::new(),
            "abc".to_string(),
            "a.b".to_string(),
            "a.b.c.d".to_string(),
            "a.!!!.c".to_string(),
            "a..c".to_string(),
            ".e30.c".to_string(),
            format!("a.{}.c", b64url(b"{}")),
            format!("a.{}.c", b64url(br#"{"exp":"soon"}"#)),
            format!("a.{}.c", b64url(br#"{"exp":null}"#)),
            format!("a.{}.c", b64url(br#"{"exp":true}"#)),
            format!("a.{}.c", b64url(br#"{"exp":-5}"#)),
            format!("a.{}.c", b64url(b"not-json")),
            jwt(1),
        ];
        for token in &bad {
            let exp = jwt_expiry_ms(token);
            assert!(
                exp.is_none() || exp.is_some_and(|ms| ms <= 1_700_000_000_000),
                "{token}"
            );
        }
    }

    #[test]
    fn request_url_is_exact_and_host_must_be_declared() {
        let domains = ["chatgpt.com".to_string()];
        exact_https_url("https://chatgpt.com/backend-api/wham/usage", &domains).unwrap();
        exact_https_url(
            "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
            &["cli-chat-proxy.grok.com".to_string()],
        )
        .unwrap();
        let rejected = [
            "http://chatgpt.com/usage",
            "https://user:pw@chatgpt.com/usage",
            "https://chatgpt.com:443/usage",
            "https://chatgpt.com/a/../b",
            "https://evil.example/usage",
            "https://ChatGPT.com/usage",
            "https://chatgpt.com/usage#x",
        ];
        for url in rejected {
            assert!(exact_https_url(url, &domains).is_err(), "{url}");
        }
    }

    #[test]
    fn extract_drops_undeclared_fields_and_rejects_secret_echo() {
        let mut login = sample("notes/auth.json");
        login.extract = vec![
            LoginExtract {
                name: "used".to_string(),
                path: "used".to_string(),
                kind: ExtractType::Number,
            },
            LoginExtract {
                name: "note".to_string(),
                path: "note".to_string(),
                kind: ExtractType::String,
            },
            LoginExtract {
                name: "ok".to_string(),
                path: "missing".to_string(),
                kind: ExtractType::Bool,
            },
        ];
        let sensitive = vec![secret("secret-token-value")];
        let values = interpret_login(
            &login,
            200,
            r#"{"used":1.5,"note":"hello\nthere","plan_type":"plus","raw":"secret"}"#,
            &sensitive,
        )
        .unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].name, "used");
        assert_eq!(
            values[1].value,
            LoginScalar::String("hellothere".to_string())
        );
        let long = "字".repeat(200);
        let capped = interpret_login(
            &login,
            200,
            &format!(r#"{{"used":1,"note":"{long}"}}"#),
            &[],
        )
        .unwrap();
        match &capped[1].value {
            LoginScalar::String(text) => assert_eq!(text.chars().count(), 128),
            other => panic!("{other:?}"),
        }
        let echoed = interpret_login(
            &login,
            200,
            r#"{"used":1,"note":"prefix secret-token-value suffix"}"#,
            &sensitive,
        );
        assert_eq!(echoed.unwrap_err(), LoginError::Invalid);
        assert_eq!(
            interpret_login(&login, 401, r#"{"note":"secret-token-value"}"#, &sensitive)
                .unwrap_err(),
            LoginError::LoginExpired
        );
        assert_eq!(
            interpret_login(&login, 302, r#"{"used":1,"note":"x"}"#, &sensitive).unwrap_err(),
            LoginError::Failed
        );
        assert_eq!(
            interpret_login(&login, 200, "not-json", &sensitive).unwrap_err(),
            LoginError::Invalid
        );
    }

    #[test]
    fn symlink_escape_missing_home_and_size_cap() {
        let dir = TempDir::new("escape");
        let home = dir.join("home");
        let outside = dir.join("outside");
        std::fs::create_dir_all(home.join("notes")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let token = jwt(4_102_444_800);
        let body = format!(r#"{{"tokens":{{"access_token":"{token}"}}}}"#);
        std::fs::write(outside.join("auth.json"), &body).unwrap();
        link_dir(&home.join("notes").join("linked"), &outside);
        let login = sample("notes/linked/auth.json");
        let roots = LoginRoots {
            home: Some(home.clone()),
        };
        let before = std::fs::read(outside.join("auth.json")).unwrap();
        assert_eq!(
            prepare_login(&login, &roots, None, 1_700_000_000_000).unwrap_err(),
            LoginError::LoginExpired
        );
        assert_eq!(std::fs::read(outside.join("auth.json")).unwrap(), before);

        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh").join("id.json"), &body).unwrap();
        link_dir(&home.join("alias"), &home.join(".ssh"));
        let aliased = sample("alias/id.json");
        assert_eq!(
            prepare_login(&aliased, &roots, None, 1_700_000_000_000).unwrap_err(),
            LoginError::LoginExpired
        );

        std::fs::create_dir_all(home.join("keep")).unwrap();
        std::fs::write(home.join("keep").join("auth.json"), &body).unwrap();
        let inside = sample("keep/auth.json");
        let call = prepare_login(&inside, &roots, None, 1_700_000_000_000).unwrap();
        assert_eq!(call.outgoing.url, inside.request.url);
        assert_eq!(call.outgoing.method, "GET");
        assert!(call.outgoing.body.is_empty());
        assert!(
            call.outgoing
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == &format!("Bearer {token}"))
        );
        let debug = format!("{call:?}");
        assert!(!debug.contains(&token), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
        assert_eq!(
            std::fs::read(home.join("keep").join("auth.json")).unwrap(),
            body.as_bytes()
        );

        let huge = "a".repeat(LOGIN_FILE_MAX + 1);
        std::fs::write(home.join("keep").join("big.json"), huge).unwrap();
        let big = sample("keep/big.json");
        assert_eq!(
            prepare_login(&big, &roots, None, 1_700_000_000_000).unwrap_err(),
            LoginError::LoginExpired
        );

        let data = home.join("work").join("lanwork");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("secret.json"), &body).unwrap();
        link_dir(&home.join("via"), &data);
        let via = sample("via/secret.json");
        assert_eq!(
            prepare_login(&via, &roots, Some(&data), 1_700_000_000_000).unwrap_err(),
            LoginError::LoginExpired
        );

        assert_eq!(
            prepare_login(&inside, &LoginRoots { home: None }, None, 1_700_000_000_000)
                .unwrap_err(),
            LoginError::LoginExpired
        );
        assert_eq!(
            prepare_login(
                &inside,
                &LoginRoots {
                    home: Some(PathBuf::from("relative-home")),
                },
                None,
                1_700_000_000_000
            )
            .unwrap_err(),
            LoginError::LoginExpired
        );
    }

    #[test]
    fn hard_link_is_refused_from_the_open_handle() {
        let dir = TempDir::new("hard");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("keep")).unwrap();
        let token = jwt(4_102_444_800);
        let body = format!(r#"{{"tokens":{{"access_token":"{token}"}}}}"#);
        let original = home.join("keep").join("auth.json");
        std::fs::write(&original, &body).unwrap();
        std::fs::hard_link(&original, home.join("keep").join("linked.json")).unwrap();
        let roots = LoginRoots {
            home: Some(home.clone()),
        };
        assert_eq!(
            prepare_login(&sample("keep/auth.json"), &roots, None, 1_700_000_000_000).unwrap_err(),
            LoginError::LoginExpired
        );
        assert_eq!(
            prepare_login(&sample("keep/linked.json"), &roots, None, 1_700_000_000_000)
                .unwrap_err(),
            LoginError::LoginExpired
        );
        assert_eq!(std::fs::read(&original).unwrap(), body.as_bytes());
    }

    #[test]
    fn echo_covers_encodings_splits_and_every_long_string() {
        let mut login = sample("notes/auth.json");
        login.extract = vec![
            LoginExtract {
                name: "left".to_string(),
                path: "left".to_string(),
                kind: ExtractType::String,
            },
            LoginExtract {
                name: "used".to_string(),
                path: "used".to_string(),
                kind: ExtractType::Number,
            },
            LoginExtract {
                name: "right".to_string(),
                path: "right".to_string(),
                kind: ExtractType::String,
            },
        ];
        let token = "login-token??";
        let sensitive = vec![LoginSecret {
            value: token.to_string(),
            header: Some(format!("Bearer {token}")),
        }];
        let ok = interpret_login(
            &login,
            200,
            r#"{"left":"plain","used":12.5,"right":"text"}"#,
            &sensitive,
        )
        .unwrap();
        assert_eq!(ok[1].value, LoginScalar::Number(12.5));
        let rejected = [
            r#"{"left":"bG9naW4tdG9rZW4/Pw==","used":1,"right":"x"}"#,
            r#"{"left":"bG9naW4tdG9rZW4_Pw==","used":1,"right":"x"}"#,
            r#"{"left":"bG9naW4tdG9rZW4/Pw","used":1,"right":"x"}"#,
            r#"{"left":"bG9naW4tdG9rZW4_Pw","used":1,"right":"x"}"#,
            r#"{"left":"%6c%6f%67%69%6e%2d%74%6f%6b%65%6e%3f%3f","used":1,"right":"x"}"#,
            r#"{"left":"6c6f67696e2d746f6b656e3f3f","used":1,"right":"x"}"#,
            r#"{"left":"6C6F67696E2D746F6B656E3F3F","used":1,"right":"x"}"#,
            "{\"left\":\"login-\u{200b}token??\",\"used\":1,\"right\":\"x\"}",
            "{\"left\":\"login- token??\",\"used\":1,\"right\":\"x\"}",
            "{\"left\":\"login-tok\",\"used\":1,\"right\":\"en??\"}",
            "{\"left\":\"login-\u{200b}\",\"used\":1,\"right\":\"token??\"}",
        ];
        for body in &rejected {
            assert_eq!(
                interpret_login(&login, 200, body, &sensitive).unwrap_err(),
                LoginError::Invalid,
                "{body}"
            );
        }

        let dir = TempDir::new("echo-all");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("keep")).unwrap();
        let access = jwt(4_102_444_800);
        let mut map = serde_json::Map::new();
        map.insert(
            "tokens".to_string(),
            serde_json::json!({ "access_token": access }),
        );
        for index in 0..70 {
            map.insert(
                format!("k{index:02}"),
                serde_json::json!(format!("extra-string-{index:02}-xxxx")),
            );
        }
        std::fs::write(
            home.join("keep").join("auth.json"),
            serde_json::Value::Object(map).to_string(),
        )
        .unwrap();
        let call = prepare_login(
            &sample("keep/auth.json"),
            &LoginRoots {
                home: Some(home.clone()),
            },
            None,
            1_700_000_000_000,
        )
        .unwrap();
        assert!(call.sensitive.len() > 70, "{}", call.sensitive.len());
        let mut noted = sample("keep/auth.json");
        noted.extract = vec![LoginExtract {
            name: "note".to_string(),
            path: "note".to_string(),
            kind: ExtractType::String,
        }];
        assert_eq!(
            interpret_login(
                &noted,
                200,
                r#"{"note":"extra-string-69-xxxx"}"#,
                &call.sensitive
            )
            .unwrap_err(),
            LoginError::Invalid
        );
    }

    #[test]
    fn cache_misses_when_the_descriptor_changes() {
        reset_login_cache_for_test();
        let login = sample("keep/auth.json");
        let mut other = login.clone();
        other.request.url = "https://chatgpt.com/backend-api/wham/other".to_string();
        let fingerprint = login_fingerprint(&login);
        let other_fingerprint = login_fingerprint(&other);
        assert_ne!(fingerprint, other_fingerprint);
        store_login(
            "example.tool",
            "chatgpt",
            fingerprint,
            Ok(vec![LoginValue {
                name: "used".to_string(),
                value: LoginScalar::Number(12.5),
            }]),
        );
        assert!(cached_login("example.tool", "chatgpt", fingerprint).is_some());
        assert!(cached_login("example.tool", "chatgpt", other_fingerprint).is_none());
        reset_login_cache_for_test();
    }

    #[cfg(windows)]
    #[test]
    fn short_name_resolves_to_the_denied_long_path() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        use windows::core::PCWSTR;
        let dir = TempDir::new("short");
        let home = dir.join("home");
        let nested = home.join("AppData").join("User Data").join("profile");
        std::fs::create_dir_all(&nested).unwrap();
        let token = jwt(4_102_444_800);
        let body = format!(r#"{{"tokens":{{"access_token":"{token}"}}}}"#);
        let file = nested.join("auth.json");
        std::fs::write(&file, &body).unwrap();
        let Some(short_file) = windows_short_path(&file) else {
            return;
        };
        let Some(short_home) = windows_short_path(&home) else {
            return;
        };
        let file_text = short_file.to_string_lossy();
        let home_text = short_home.to_string_lossy();
        let file_folded = file_text.to_ascii_lowercase();
        let home_folded = home_text.to_ascii_lowercase();
        let Some(rest) = file_folded.strip_prefix(&home_folded) else {
            return;
        };
        if !rest.starts_with('\\') && !rest.starts_with('/') {
            return;
        }
        let declared = rest.trim_start_matches(['\\', '/']).replace('\\', "/");
        assert!(
            !declared.to_ascii_lowercase().contains("user data"),
            "{declared}"
        );
        assert_eq!(
            prepare_login(
                &sample(&declared),
                &LoginRoots { home: Some(home) },
                None,
                1_700_000_000_000
            )
            .unwrap_err(),
            LoginError::LoginExpired
        );
        assert_eq!(std::fs::read(&file).unwrap(), body.as_bytes());

        fn windows_short_path(path: &Path) -> Option<PathBuf> {
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut buf = vec![0u16; 1024];
            let n = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf)) };
            if n == 0 || n as usize >= buf.len() {
                return None;
            }
            let short = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n as usize]));
            (short != path).then_some(short)
        }
    }
}
