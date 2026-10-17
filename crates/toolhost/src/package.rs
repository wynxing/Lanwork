//! 工具包的校验。文件夹和 zip 都先读清单，确认之前不写入数据目录。
//!
//! 拒绝路径穿越、压缩炸弹、非法清单、重复路径和超出已确认范围的服务。
//! 只保留清单、入口脚本，以及内容确是 PNG 的 `icon.png`。其它文件不复制。
//! 入口以外的 `.lua` 会不会拒绝安装仍待定，这一版不因此拒绝，也不执行、不复制。
//! 压缩包过大时界面同样显示「清单无效」。规格没有另给一句。

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::manifest::{Manifest, SecretDecl, load_dir, parse};

/// 系统文件对话框里用来选中当前文件夹的占位名。不是产品界面上的句子。
pub const FOLDER_SENTINEL: &str = "Folder Selection.";

const INVALID: &str = "清单无效";
const MAX_ARCHIVE: usize = 8 * 1024 * 1024;
pub const MAX_UNCOMPRESSED: usize = 8 * 1024 * 1024;
pub const MAX_ENTRY: usize = 2 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 64;
const SCHEMA: u64 = 1;

/// 读到的工具包。文件路径相对工具根，只用 `/` 拆开。
#[derive(Clone, Debug, PartialEq)]
pub struct StagedPackage {
    pub manifest: Manifest,
    pub files: Vec<(String, Vec<u8>)>,
}

/// 用户确认过的服务、域名和密钥。运行时清单不能超出这份范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub services: BTreeSet<String>,
    pub domains: Vec<String>,
    pub secrets: Vec<SecretDecl>,
}

impl Grant {
    #[must_use]
    pub fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            services: manifest.services.clone(),
            domains: manifest.domains.clone(),
            secrets: manifest.secrets.clone(),
        }
    }

    /// 清单里的服务、域名和密钥 id 都在确认过的范围内。
    #[must_use]
    pub fn allows(&self, manifest: &Manifest) -> bool {
        if manifest
            .services
            .iter()
            .any(|service| !self.services.contains(service))
        {
            return false;
        }
        if manifest.domains.iter().any(|domain| {
            !self
                .domains
                .iter()
                .any(|granted| granted.eq_ignore_ascii_case(domain))
        }) {
            return false;
        }
        manifest
            .secrets
            .iter()
            .all(|secret| self.secrets.iter().any(|granted| granted.id == secret.id))
    }
}

#[derive(Debug)]
pub struct PackageError {
    message: &'static str,
    detail: String,
}

impl PackageError {
    fn invalid(detail: impl Into<String>) -> Self {
        Self {
            message: INVALID,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

/// 文件夹，或一个 zip 文件。
pub enum PickedSource {
    Directory(PathBuf),
    Zip(PathBuf),
}

/// 文件对话框的返回值。占位名表示用户要的是当前文件夹。
pub fn resolve_selection(path: &Path) -> Result<PickedSource, PackageError> {
    if path.is_dir() {
        return Ok(PickedSource::Directory(path.to_path_buf()));
    }
    if path.is_file() {
        if is_zip_name(path) {
            return Ok(PickedSource::Zip(path.to_path_buf()));
        }
        return Err(PackageError::invalid("不是文件夹或 zip"));
    }
    if path.file_name().and_then(|name| name.to_str()) == Some(FOLDER_SENTINEL)
        && path.parent().is_some_and(|parent| parent.is_dir())
    {
        return Ok(PickedSource::Directory(
            path.parent().unwrap_or(path).to_path_buf(),
        ));
    }
    Err(PackageError::invalid("不是文件夹或 zip"))
}

pub fn inspect_path(path: &Path) -> Result<StagedPackage, PackageError> {
    match resolve_selection(path)? {
        PickedSource::Directory(dir) => inspect_dir(&dir),
        PickedSource::Zip(file) => {
            let bytes = read_capped(&file, MAX_ARCHIVE)?;
            inspect_zip_bytes(&bytes)
        }
    }
}

pub fn inspect_zip_bytes(bytes: &[u8]) -> Result<StagedPackage, PackageError> {
    if bytes.len() > MAX_ARCHIVE {
        return Err(PackageError::invalid("压缩包过大"));
    }
    let files = read_zip(bytes)?;
    stage_from_files(files)
}

fn inspect_dir(dir: &Path) -> Result<StagedPackage, PackageError> {
    let root = dir
        .canonicalize()
        .map_err(|_| PackageError::invalid("工具文件夹打不开"))?;
    let manifest_path = contained(&root, Path::new("manifest.toml"))?;
    let manifest_bytes = read_capped(&manifest_path, MAX_ENTRY)?;
    let loaded = load_dir(&root).map_err(PackageError::invalid)?;
    let script_path = contained(&root, Path::new(&loaded.entry))?;
    let script = read_capped(&script_path, MAX_ENTRY)?;
    if std::str::from_utf8(&script).is_err() {
        return Err(PackageError::invalid("入口脚本不是 UTF-8"));
    }
    let mut files = vec![
        ("manifest.toml".to_string(), manifest_bytes),
        (loaded.entry.replace('\\', "/"), script),
    ];
    if let Some(icon) = read_icon(&root)? {
        files.push(("icon.png".to_string(), icon));
    }
    Ok(StagedPackage {
        manifest: loaded,
        files,
    })
}

fn stage_from_files(files: Vec<(String, Vec<u8>)>) -> Result<StagedPackage, PackageError> {
    let prefix = choose_prefix(&files)?;
    let manifest_name = join_prefix(&prefix, "manifest.toml");
    let manifest_bytes = files
        .iter()
        .find(|(name, _)| name == &manifest_name)
        .map(|(_, bytes)| bytes.clone())
        .ok_or_else(|| PackageError::invalid("没有清单"))?;
    let text = std::str::from_utf8(&manifest_bytes)
        .map_err(|_| PackageError::invalid("清单不是 UTF-8"))?;
    let (mut manifest, entry) = parse(text).map_err(PackageError::invalid)?;
    let script_name = join_prefix(&prefix, &entry.replace('\\', "/"));
    let script = files
        .iter()
        .find(|(name, _)| name == &script_name)
        .map(|(_, bytes)| bytes.clone())
        .ok_or_else(|| PackageError::invalid("入口脚本不存在"))?;
    let script_text = std::str::from_utf8(&script)
        .map_err(|_| PackageError::invalid("入口脚本不是 UTF-8"))?
        .to_string();
    manifest.script = script_text;
    let mut staged = vec![
        ("manifest.toml".to_string(), manifest_bytes),
        (entry.replace('\\', "/"), script),
    ];
    let icon_name = join_prefix(&prefix, "icon.png");
    if let Some((_, bytes)) = files.iter().find(|(name, _)| name == &icon_name) {
        if bytes.len() > MAX_ENTRY {
            return Err(PackageError::invalid("图标过大"));
        }
        if png_signature(bytes) {
            staged.push(("icon.png".to_string(), bytes.clone()));
        }
    }
    Ok(StagedPackage {
        manifest,
        files: staged,
    })
}

/// 已经装进数据目录的副本，按导入的规则再查一遍。
///
/// `files` 的路径相对工具根。`card.json` 和 `grant.json` 不参与清单校验，结果里也不保留它们。
/// 通过之后，文件只留清单、入口脚本和 PNG 图标。授权按这份清单重新生成，不读备份里的 `grant.json`。
#[derive(Clone, Debug, PartialEq)]
pub struct RecheckedTool {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 相对工具根，只用 `/`。不含 `card.json` 和 `grant.json`。
    pub files: Vec<(String, Vec<u8>)>,
    /// `Grant::from_manifest` 的编码。没有密钥原文。
    pub grant: Vec<u8>,
}

pub fn recheck_installed(
    dir_name: &str,
    files: &[(String, Vec<u8>)],
) -> Result<RecheckedTool, PackageError> {
    let staged = stage_copy(dir_name, files)?;
    let grant = Grant::from_manifest(&staged.manifest);
    Ok(RecheckedTool {
        id: staged.manifest.id.clone(),
        name: staged.manifest.name.clone(),
        description: staged.manifest.description.clone(),
        files: staged.files,
        grant: encode_grant(&grant),
    })
}

/// 确认页用的名称和权限句子。校验与 [`recheck_installed`] 相同，句子来自清单。
///
/// 不读 `grant.json`，也不写盘。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyPreview {
    pub name: String,
    pub lines: Vec<String>,
}

pub fn preview_copy(
    dir_name: &str,
    files: &[(String, Vec<u8>)],
) -> Result<CopyPreview, PackageError> {
    let staged = stage_copy(dir_name, files)?;
    Ok(CopyPreview {
        name: staged.manifest.name.clone(),
        lines: permission_lines(&staged.manifest),
    })
}

fn stage_copy(dir_name: &str, files: &[(String, Vec<u8>)]) -> Result<StagedPackage, PackageError> {
    if !crate::id::valid_id(dir_name) {
        return Err(PackageError::invalid(if dir_name.starts_with('.') {
            "id 不能以 . 开头"
        } else {
            "id 不是单个路径分量"
        }));
    }
    if crate::id::imported_prefix_forbidden(dir_name) {
        return Err(PackageError::invalid("导入的 id 不能以 lanwork. 开头"));
    }
    if files.len() > MAX_ENTRIES {
        return Err(PackageError::invalid("压缩包文件过多"));
    }
    let mut total = 0usize;
    let mut seen = BTreeSet::new();
    let mut staged_input = Vec::new();
    for (name, bytes) in files {
        let name = normalize_name(name)?;
        if !seen.insert(name.clone()) {
            return Err(PackageError::invalid("路径重复"));
        }
        if bytes.len() > MAX_ENTRY {
            return Err(PackageError::invalid("文件过大"));
        }
        total = total.saturating_add(bytes.len());
        if total > MAX_UNCOMPRESSED {
            return Err(PackageError::invalid("压缩包过大"));
        }
        if name == "card.json" || name == "grant.json" {
            continue;
        }
        staged_input.push((name, bytes.clone()));
    }
    let staged = stage_from_files(staged_input)?;
    if staged.manifest.id != dir_name {
        return Err(PackageError::invalid("id 与目录不一致"));
    }
    Ok(staged)
}

/// 根上有清单就用根。否则只能有一个顶层文件夹，清单在里面。
fn choose_prefix(files: &[(String, Vec<u8>)]) -> Result<String, PackageError> {
    if files.iter().any(|(name, _)| name == "manifest.toml") {
        return Ok(String::new());
    }
    let mut tops = BTreeSet::new();
    for (name, _) in files {
        let top = name.split('/').next().unwrap_or("");
        if top.is_empty() {
            return Err(PackageError::invalid("没有清单"));
        }
        tops.insert(top.to_string());
    }
    if tops.len() != 1 {
        return Err(PackageError::invalid("没有清单"));
    }
    let folder = tops.into_iter().next().unwrap_or_default();
    let manifest = format!("{folder}/manifest.toml");
    if files.iter().any(|(name, _)| name == &manifest) {
        Ok(format!("{folder}/"))
    } else {
        Err(PackageError::invalid("没有清单"))
    }
}

fn join_prefix(prefix: &str, relative: &str) -> String {
    if prefix.is_empty() {
        relative.to_string()
    } else {
        format!("{prefix}{relative}")
    }
}

/// 确认页上的白话句子。域名逐条，密钥只出现名称。
#[must_use]
pub fn permission_lines(manifest: &Manifest) -> Vec<String> {
    lines_for(&manifest.services, &manifest.domains, &manifest.secrets)
}

#[must_use]
pub fn grant_lines(grant: &Grant) -> Vec<String> {
    lines_for(&grant.services, &grant.domains, &grant.secrets)
}

fn lines_for(
    services: &BTreeSet<String>,
    domains: &[String],
    secrets: &[SecretDecl],
) -> Vec<String> {
    let mut lines = Vec::new();
    for domain in domains {
        lines.push(format!("访问 {domain}"));
    }
    for secret in secrets {
        lines.push(format!("使用你填写的 {}", secret.label));
    }
    if services.contains("notify") {
        lines.push("发送通知".to_string());
    }
    if services.contains("todo.read") {
        lines.push("读取你的待办".to_string());
    }
    if services.contains("todo.complete") {
        lines.push("完成你的待办".to_string());
    }
    if services.contains("storage") {
        lines.push("保存这个工具自己的数据".to_string());
    }
    if services.contains("timer") {
        lines.push("定时刷新".to_string());
    }
    lines
}

pub fn encode_grant(grant: &Grant) -> Vec<u8> {
    let secrets: Vec<serde_json::Value> = grant
        .secrets
        .iter()
        .map(|secret| {
            serde_json::json!({
                "id": secret.id,
                "label": secret.label,
            })
        })
        .collect();
    let value = serde_json::json!({
        "schemaVersion": SCHEMA,
        "services": grant.services,
        "domains": grant.domains,
        "secrets": secrets,
    });
    let mut bytes = serde_json::to_vec(&value).expect("确认记录可以编码");
    bytes.push(b'\n');
    bytes
}

pub fn decode_grant(bytes: &[u8]) -> Option<Grant> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let obj = value.as_object()?;
    match obj.get("schemaVersion") {
        None => {}
        Some(serde_json::Value::Number(number)) if number.as_u64() == Some(SCHEMA) => {}
        Some(_) => return None,
    }
    let services = string_set(obj.get("services")?)?;
    let domains = string_list(obj.get("domains")?)?;
    let mut secrets = Vec::new();
    for item in obj.get("secrets")?.as_array()? {
        let item = item.as_object()?;
        let id = item.get("id")?.as_str()?.to_string();
        let label = item.get("label")?.as_str()?.to_string();
        if id.is_empty() || label.is_empty() {
            return None;
        }
        secrets.push(SecretDecl { id, label });
    }
    Some(Grant {
        services,
        domains,
        secrets,
    })
}

fn string_set(value: &serde_json::Value) -> Option<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    for item in value.as_array()? {
        set.insert(item.as_str()?.to_string());
    }
    Some(set)
}

fn string_list(value: &serde_json::Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_string))
        .collect()
}

fn is_zip_name(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

fn contained(root: &Path, relative: &Path) -> Result<PathBuf, PackageError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(PackageError::invalid("路径越界"));
    }
    let joined = root.join(relative);
    let file = joined
        .canonicalize()
        .map_err(|_| PackageError::invalid("入口脚本不存在"))?;
    if !file.starts_with(root) {
        return Err(PackageError::invalid("路径越界"));
    }
    Ok(file)
}

fn read_capped(path: &Path, limit: usize) -> Result<Vec<u8>, PackageError> {
    let meta = std::fs::metadata(path).map_err(|_| PackageError::invalid("文件读不到"))?;
    if meta.len() > limit as u64 {
        return Err(PackageError::invalid("文件过大"));
    }
    let bytes = std::fs::read(path).map_err(|_| PackageError::invalid("文件读不到"))?;
    if bytes.len() > limit {
        return Err(PackageError::invalid("文件过大"));
    }
    Ok(bytes)
}

fn read_icon(root: &Path) -> Result<Option<Vec<u8>>, PackageError> {
    let candidate = root.join("icon.png");
    if !candidate.exists() {
        return Ok(None);
    }
    let path = contained(root, Path::new("icon.png"))?;
    let bytes = read_capped(&path, MAX_ENTRY)?;
    if png_signature(&bytes) {
        Ok(Some(bytes))
    } else {
        Ok(None)
    }
}

fn read_zip(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, PackageError> {
    let eocd = find_eocd(data)?;
    let disk = read_u16(data, eocd + 4)?;
    let cd_disk = read_u16(data, eocd + 6)?;
    let entries_here = read_u16(data, eocd + 8)?;
    let entries = read_u16(data, eocd + 10)?;
    let cd_size = read_u32(data, eocd + 12)?;
    let cd_offset = read_u32(data, eocd + 16)?;
    if disk != 0 || cd_disk != 0 || entries_here != entries {
        return Err(PackageError::invalid("压缩包格式不受支持"));
    }
    if cd_size == u32::MAX || cd_offset == u32::MAX || entries == u16::MAX {
        return Err(PackageError::invalid("压缩包格式不受支持"));
    }
    if entries as usize > MAX_ENTRIES {
        return Err(PackageError::invalid("压缩包文件过多"));
    }
    let cd_start = cd_offset as usize;
    let cd_end = cd_start
        .checked_add(cd_size as usize)
        .ok_or_else(|| PackageError::invalid("压缩包无法读取"))?;
    if cd_end > data.len() || cd_end > eocd {
        return Err(PackageError::invalid("压缩包无法读取"));
    }
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = 0usize;
    let mut cursor = cd_start;
    for _ in 0..entries {
        if read_u32(data, cursor)? != 0x0201_4b50 {
            return Err(PackageError::invalid("压缩包无法读取"));
        }
        let flags = read_u16(data, cursor + 8)?;
        let method = read_u16(data, cursor + 10)?;
        let crc = read_u32(data, cursor + 16)?;
        let compressed = read_u32(data, cursor + 20)?;
        let uncompressed = read_u32(data, cursor + 24)?;
        let name_len = read_u16(data, cursor + 28)? as usize;
        let extra_len = read_u16(data, cursor + 30)? as usize;
        let comment_len = read_u16(data, cursor + 32)? as usize;
        let local_offset = read_u32(data, cursor + 42)?;
        if compressed == u32::MAX || uncompressed == u32::MAX || local_offset == u32::MAX {
            return Err(PackageError::invalid("压缩包格式不受支持"));
        }
        if flags & 1 != 0 {
            return Err(PackageError::invalid("压缩包已加密"));
        }
        let name_at = cursor + 46;
        let raw_name = slice(data, name_at, name_len)?;
        let name = decode_name(raw_name, flags)?;
        cursor = name_at
            .checked_add(name_len + extra_len + comment_len)
            .ok_or_else(|| PackageError::invalid("压缩包无法读取"))?;
        if name.ends_with('/') || name.ends_with('\\') {
            let trimmed = name.trim_end_matches(['/', '\\']);
            if !trimmed.is_empty() {
                let _ = normalize_name(trimmed)?;
            }
            continue;
        }
        let name = normalize_name(&name)?;
        if !seen.insert(name.clone()) {
            return Err(PackageError::invalid("路径重复"));
        }
        let declared = uncompressed as usize;
        if declared > MAX_ENTRY || total.saturating_add(declared) > MAX_UNCOMPRESSED {
            return Err(PackageError::invalid("压缩包过大"));
        }
        let bytes = read_local(
            data,
            local_offset as usize,
            method,
            crc,
            compressed,
            declared,
        )?;
        total += bytes.len();
        if total > MAX_UNCOMPRESSED {
            return Err(PackageError::invalid("压缩包过大"));
        }
        files.push((name, bytes));
    }
    if cursor != cd_end {
        return Err(PackageError::invalid("压缩包无法读取"));
    }
    Ok(files)
}

fn read_local(
    data: &[u8],
    offset: usize,
    method: u16,
    crc: u32,
    compressed: u32,
    declared: usize,
) -> Result<Vec<u8>, PackageError> {
    if read_u32(data, offset)? != 0x0403_4b50 {
        return Err(PackageError::invalid("压缩包无法读取"));
    }
    let flags = read_u16(data, offset + 6)?;
    let local_method = read_u16(data, offset + 8)?;
    if flags & 1 != 0 {
        return Err(PackageError::invalid("压缩包已加密"));
    }
    if local_method != method {
        return Err(PackageError::invalid("压缩包无法读取"));
    }
    let name_len = read_u16(data, offset + 26)? as usize;
    let extra_len = read_u16(data, offset + 28)? as usize;
    let start = offset
        .checked_add(30 + name_len + extra_len)
        .ok_or_else(|| PackageError::invalid("压缩包无法读取"))?;
    let payload = slice(data, start, compressed as usize)?;
    let bytes = match method {
        0 => {
            if payload.len() != declared {
                return Err(PackageError::invalid("压缩包无法读取"));
            }
            payload.to_vec()
        }
        8 => inflate(payload, declared)?,
        _ => return Err(PackageError::invalid("压缩包格式不受支持")),
    };
    if crc32(&bytes) != crc {
        return Err(PackageError::invalid("压缩包无法读取"));
    }
    Ok(bytes)
}

fn inflate(payload: &[u8], declared: usize) -> Result<Vec<u8>, PackageError> {
    let mut decoder = flate2::read::DeflateDecoder::new(payload);
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let read = decoder
            .read(&mut buf)
            .map_err(|_| PackageError::invalid("压缩包无法读取"))?;
        if read == 0 {
            break;
        }
        if out.len().saturating_add(read) > declared {
            return Err(PackageError::invalid("压缩包过大"));
        }
        out.extend_from_slice(&buf[..read]);
    }
    if out.len() != declared {
        return Err(PackageError::invalid("压缩包无法读取"));
    }
    Ok(out)
}

fn decode_name(bytes: &[u8], flags: u16) -> Result<String, PackageError> {
    if flags & 0x0800 == 0 && !bytes.is_ascii() {
        return Err(PackageError::invalid("文件名不是 UTF-8"));
    }
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| PackageError::invalid("文件名不是 UTF-8"))
}

fn normalize_name(name: &str) -> Result<String, PackageError> {
    let name = name.replace('\\', "/");
    if name.is_empty() || name.starts_with('/') || name.contains('\0') || name.contains(':') {
        return Err(PackageError::invalid("路径越界"));
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') {
            return Err(PackageError::invalid("路径越界"));
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(PackageError::invalid("路径越界"));
    }
    Ok(parts.join("/"))
}

fn find_eocd(data: &[u8]) -> Result<usize, PackageError> {
    if data.len() < 22 {
        return Err(PackageError::invalid("压缩包无法读取"));
    }
    let start = data.len().saturating_sub(22 + 65_535);
    let mut index = data.len() - 22;
    loop {
        if data[index..index + 4] == [0x50, 0x4b, 0x05, 0x06] {
            return Ok(index);
        }
        if index == start {
            break;
        }
        index -= 1;
    }
    Err(PackageError::invalid("压缩包无法读取"))
}

fn slice(data: &[u8], start: usize, len: usize) -> Result<&[u8], PackageError> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| PackageError::invalid("压缩包无法读取"))?;
    data.get(start..end)
        .ok_or_else(|| PackageError::invalid("压缩包无法读取"))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, PackageError> {
    let bytes = slice(data, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, PackageError> {
    let bytes = slice(data, offset, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// PNG 文件头。清单模块不暴露时，包校验自己认这个签名。
pub(crate) fn png_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::{countdown, quota};
    use crate::manifest::materialize;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lanwork-pkg-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn quota_dir() -> (PathBuf, Manifest) {
        let dir = scratch("quota");
        let (manifest, script) = quota().unwrap();
        materialize(&dir, &manifest, &script).unwrap();
        let staged = inspect_path(&dir).unwrap();
        (dir, staged.manifest)
    }

    #[test]
    fn shipped_example_matches_the_quota_guide() {
        let (manifest, script) = quota().unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/api-quota");
        let on_disk_manifest = std::fs::read_to_string(root.join("manifest.toml"))
            .unwrap()
            .replace("\r\n", "\n");
        let on_disk_script = std::fs::read_to_string(root.join("main.lua"))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(on_disk_manifest.trim_end(), manifest.trim_end());
        assert_eq!(on_disk_script.trim_end(), script.trim_end());
        let staged = inspect_path(&root).unwrap();
        assert_eq!(staged.manifest.id, "example.api-quota");
        assert!(
            staged
                .files
                .iter()
                .any(|(name, bytes)| { name == "icon.png" && png_signature(bytes) })
        );
    }

    #[test]
    fn quota_folder_lists_domains_without_the_secret_value() {
        let (dir, manifest) = quota_dir();
        assert_eq!(manifest.id, "example.api-quota");
        let lines = permission_lines(&manifest);
        assert!(lines.contains(&"访问 api.example.com".to_string()));
        assert!(lines.contains(&"使用你填写的 API 密钥".to_string()));
        assert!(lines.contains(&"定时刷新".to_string()));
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("sk-") || line.contains("密钥值"))
        );
        assert!(!lines.iter().any(|line| line.contains("联网")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shipped_countdown_passes_import_without_permissions() {
        let (manifest, script) = countdown().unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/countdown");
        let on_disk_manifest = std::fs::read_to_string(root.join("manifest.toml"))
            .unwrap()
            .replace("\r\n", "\n");
        let on_disk_script = std::fs::read_to_string(root.join("main.lua"))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(on_disk_manifest.trim_end(), manifest.trim_end());
        assert_eq!(on_disk_script.trim_end(), script.trim_end());
        let staged = inspect_path(&root).unwrap();
        assert_eq!(staged.manifest.id, "example.countdown");
        assert!(staged.manifest.services.is_empty());
        assert!(permission_lines(&staged.manifest).is_empty());
        let names: Vec<&str> = staged.files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["manifest.toml", "main.lua"]);
    }

    #[test]
    fn zip_root_and_single_folder_both_install_the_same_files() {
        let (dir, _) = quota_dir();
        let root = zip_of(&dir, "");
        let nested = zip_of(&dir, "my-tool");
        let from_root = inspect_zip_bytes(&root).unwrap();
        let from_nested = inspect_zip_bytes(&nested).unwrap();
        assert_eq!(from_root.manifest.id, from_nested.manifest.id);
        assert_eq!(from_root.manifest.script, from_nested.manifest.script);
        assert!(from_root.files.iter().any(|(name, _)| name == "main.lua"));
        assert!(!from_root.files.iter().any(|(name, _)| name.contains("..")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_traversal_duplicate_names_and_unknown_services() {
        let err = inspect_zip_bytes(&zip_bytes(&[("../secret.txt", b"x".as_slice())])).unwrap_err();
        assert_eq!(err.to_string(), "清单无效");
        assert!(err.detail().contains("路径"));
        let err = inspect_zip_bytes(&zip_bytes(&[
            ("manifest.toml", b"id = \"a\"".as_slice()),
            ("manifest.toml", b"id = \"b\"".as_slice()),
        ]));
        assert!(err.is_err());
        let err = inspect_zip_bytes(&zip_bytes(&[("/etc/passwd", b"x".as_slice())])).unwrap_err();
        assert_eq!(err.to_string(), "清单无效");
        let (manifest, script) = quota().unwrap();
        let dir = scratch("bad-service");
        let text = manifest.replace(
            "services = [\"timer\", \"http\", \"secret\"]",
            "services = [\"timer\", \"http\", \"secret\", \"shell\"]",
        );
        std::fs::write(dir.join("manifest.toml"), text).unwrap();
        std::fs::write(dir.join("main.lua"), script).unwrap();
        let err = inspect_path(&dir).unwrap_err();
        assert_eq!(err.to_string(), "清单无效");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_lanwork_prefix_duplicate_ids_and_a_declared_bomb() {
        let (manifest, script) = quota().unwrap();
        let dir = scratch("prefix");
        std::fs::write(
            dir.join("manifest.toml"),
            manifest.replace("example.api-quota", "lanwork.quota"),
        )
        .unwrap();
        std::fs::write(dir.join("main.lua"), &script).unwrap();
        assert_eq!(inspect_path(&dir).unwrap_err().to_string(), "清单无效");
        let dup = manifest.replace(
            "id = \"summary\"\ncomponent = \"text\"",
            "id = \"summary\"\ncomponent = \"text\"\n\n[[component]]\nid = \"summary\"\ncomponent = \"text\"",
        );
        std::fs::write(dir.join("manifest.toml"), dup).unwrap();
        assert_eq!(inspect_path(&dir).unwrap_err().to_string(), "清单无效");
        let huge = zip_with_declared_size("main.lua", b"hi", 50_000_000);
        assert_eq!(
            inspect_zip_bytes(&huge).unwrap_err().to_string(),
            "清单无效"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn inflate_cannot_exceed_the_declared_size() {
        let payload = vec![0u8; 4096];
        let compressed = deflate_bytes(&payload);
        // 声明 100 字节，实际流会解出更多。校验和按整段明文计算，先被长度上限拦住。
        let bomb = zip_deflated("pad.bin", &compressed, &payload, 100);
        assert_eq!(
            inspect_zip_bytes(&bomb).unwrap_err().to_string(),
            "清单无效"
        );
    }

    #[test]
    fn grant_rejects_a_service_or_domain_added_after_confirm() {
        let (_dir, manifest) = quota_dir();
        let grant = Grant::from_manifest(&manifest);
        assert!(grant.allows(&manifest));
        let bytes = encode_grant(&grant);
        assert_eq!(decode_grant(&bytes).unwrap(), grant);
        let mut widened = manifest.clone();
        widened.services.insert("notify".to_string());
        assert!(!grant.allows(&widened));
        widened.services.remove("notify");
        widened.domains.push("evil.example".to_string());
        assert!(!grant.allows(&widened));
        assert!(decode_grant(b"{\"schemaVersion\":99}").is_none());
    }

    #[test]
    fn extra_files_are_not_copied_and_a_non_png_icon_is_skipped() {
        let (dir, _) = quota_dir();
        std::fs::write(dir.join("helper.lua"), b"os.execute('no')").unwrap();
        std::fs::write(dir.join("run.exe"), b"MZ").unwrap();
        std::fs::write(dir.join("icon.png"), b"not-a-png").unwrap();
        let staged = inspect_path(&dir).unwrap();
        let names: Vec<&str> = staged.files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["manifest.toml", "main.lua"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn two_top_level_folders_and_a_symlink_escape_are_rejected() {
        let zip = zip_bytes(&[
            ("a/manifest.toml", b"x".as_slice()),
            ("b/manifest.toml", b"y".as_slice()),
        ]);
        assert_eq!(inspect_zip_bytes(&zip).unwrap_err().to_string(), "清单无效");
        let dir = scratch("link");
        let outside = scratch("outside");
        std::fs::write(outside.join("manifest.toml"), b"id = \"x\"").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("manifest.toml"), dir.join("manifest.toml"))
                .unwrap();
            std::fs::write(dir.join("main.lua"), b"function on_load() end").unwrap();
            assert_eq!(inspect_path(&dir).unwrap_err().to_string(), "清单无效");
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn sentinel_selects_the_parent_folder() {
        let dir = scratch("pick");
        let sentinel = dir.join(FOLDER_SENTINEL);
        match resolve_selection(&sentinel).unwrap() {
            PickedSource::Directory(path) => assert_eq!(path, dir),
            PickedSource::Zip(_) => panic!("应是文件夹"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn zip_of(dir: &Path, folder: &str) -> Vec<u8> {
        let mut files = Vec::new();
        for name in ["manifest.toml", "main.lua"] {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            let path = if folder.is_empty() {
                name.to_string()
            } else {
                format!("{folder}/{name}")
            };
            files.push((path, bytes));
        }
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        zip_bytes(&refs)
    }

    fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut stored = Vec::new();
        for (name, bytes) in files {
            stored.push((*name, bytes.to_vec(), 0u16, bytes.to_vec()));
        }
        write_zip(&stored)
    }

    fn zip_with_declared_size(name: &str, bytes: &[u8], declared: u32) -> Vec<u8> {
        write_zip_raw(&[ZipPart {
            name,
            compressed: bytes,
            method: 0,
            plain: bytes,
            declared,
        }])
    }

    fn zip_deflated(name: &str, compressed: &[u8], plain_for_crc: &[u8], declared: u32) -> Vec<u8> {
        write_zip_raw(&[ZipPart {
            name,
            compressed,
            method: 8,
            plain: plain_for_crc,
            declared,
        }])
    }

    fn deflate_bytes(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    struct ZipPart<'a> {
        name: &'a str,
        compressed: &'a [u8],
        method: u16,
        plain: &'a [u8],
        declared: u32,
    }

    fn write_zip(files: &[(&str, Vec<u8>, u16, Vec<u8>)]) -> Vec<u8> {
        let raw: Vec<ZipPart<'_>> = files
            .iter()
            .map(|(name, compressed, method, plain)| ZipPart {
                name,
                compressed: compressed.as_slice(),
                method: *method,
                plain: plain.as_slice(),
                declared: u32::try_from(plain.len()).unwrap_or(u32::MAX),
            })
            .collect();
        write_zip_raw(&raw)
    }

    fn write_zip_raw(files: &[ZipPart<'_>]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut central = Vec::new();
        for part in files {
            let ZipPart {
                name,
                compressed,
                method,
                plain,
                declared,
            } = part;
            let name_bytes = name.as_bytes();
            let crc = crc32(plain);
            let offset = u32::try_from(local.len()).unwrap_or(0);
            push_u32(&mut local, 0x0403_4b50);
            push_u16(&mut local, 20);
            push_u16(&mut local, 0x0800);
            push_u16(&mut local, *method);
            push_u16(&mut local, 0);
            push_u16(&mut local, 0);
            push_u32(&mut local, crc);
            push_u32(&mut local, u32::try_from(compressed.len()).unwrap_or(0));
            push_u32(&mut local, *declared);
            push_u16(&mut local, u16::try_from(name_bytes.len()).unwrap_or(0));
            push_u16(&mut local, 0);
            local.extend_from_slice(name_bytes);
            local.extend_from_slice(compressed);

            push_u32(&mut central, 0x0201_4b50);
            push_u16(&mut central, 20);
            push_u16(&mut central, 20);
            push_u16(&mut central, 0x0800);
            push_u16(&mut central, *method);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, crc);
            push_u32(&mut central, u32::try_from(compressed.len()).unwrap_or(0));
            push_u32(&mut central, *declared);
            push_u16(&mut central, u16::try_from(name_bytes.len()).unwrap_or(0));
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, offset);
            central.extend_from_slice(name_bytes);
        }
        let cd_offset = u32::try_from(local.len()).unwrap_or(0);
        let cd_size = u32::try_from(central.len()).unwrap_or(0);
        let count = u16::try_from(files.len()).unwrap_or(0);
        let mut out = local;
        out.extend_from_slice(&central);
        push_u32(&mut out, 0x0605_4b50);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, count);
        push_u16(&mut out, count);
        push_u32(&mut out, cd_size);
        push_u32(&mut out, cd_offset);
        push_u16(&mut out, 0);
        out
    }

    fn push_u16(buf: &mut Vec<u8>, value: u16) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn sample_manifest() -> String {
        "\
id = \"example.quota\"
name = \"API 额度\"
description = \"剩余额度\"
entry = \"main.lua\"
services = [\"secret\", \"storage\"]

[[secret]]
id = \"api_key\"
label = \"API 密钥\"

[[component]]
id = \"summary\"
component = \"text\"
text = \"没有数据\"
"
        .to_string()
    }

    #[test]
    fn recheck_drops_the_backup_grant_and_files_import_would_drop() {
        let manifest = sample_manifest();
        let files = vec![
            ("manifest.toml".to_string(), manifest.into_bytes()),
            ("main.lua".to_string(), b"function on_load() end\n".to_vec()),
            ("helper.lua".to_string(), b"not-copied".to_vec()),
            (
                "icon.png".to_string(),
                b"\x89PNG\r\n\x1a\nicon".to_vec(),
            ),
            ("card.json".to_string(), br#"{"name":"tampered"}"#.to_vec()),
            (
                "grant.json".to_string(),
                br#"{"schemaVersion":1,"services":["notify","secret","storage"],"domains":[],"secrets":[],"value":"secret-marker"}"#
                    .to_vec(),
            ),
        ];
        let checked = recheck_installed("example.quota", &files).unwrap();
        assert_eq!(checked.id, "example.quota");
        assert_eq!(checked.name, "API 额度");
        let names: Vec<_> = checked
            .files
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["manifest.toml", "main.lua", "icon.png"]);
        let grant = decode_grant(&checked.grant).unwrap();
        assert!(grant.services.contains("secret"));
        assert!(grant.services.contains("storage"));
        assert!(!grant.services.contains("notify"));
        assert_eq!(grant.secrets.len(), 1);
        assert_eq!(grant.secrets[0].id, "api_key");
        assert!(
            !checked
                .grant
                .windows(125)
                .any(|window| window == b"secret-marker")
        );
        for id in ["lanwork.quota", "Lanwork.tool", "LANWORK.x"] {
            let err = recheck_installed(id, &files).unwrap_err();
            assert_eq!(err.to_string(), "清单无效");
            assert_eq!(err.detail(), "导入的 id 不能以 lanwork. 开头");
        }
        assert!(recheck_installed(".incoming-example.quota", &files).is_err());
        let mut crossed = files.clone();
        crossed.push(("../../notes/pwn.json".to_string(), b"{}".to_vec()));
        assert!(recheck_installed("example.quota", &crossed).is_err());
        let mut renamed = files.clone();
        renamed[0].1 = sample_manifest()
            .replace("example.quota", "example.other")
            .into_bytes();
        assert!(recheck_installed("example.quota", &renamed).is_err());
    }
}
