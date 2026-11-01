//! 开始菜单入口、官方卸载和卸载后的残留清单。
//!
//! 这里只做判断：哪一项菜单能点、卸载命令怎么从登记信息里解析、
//! 哪些路径和注册表键可以删、残留清单里哪些默认勾选。
//! 读注册表、启动卸载程序和真正删除在 Windows 模块里。

use std::path::{Path, PathBuf};

use super::model::{AppEntry, AppSource, LaunchTarget, normalize_path_key};

/// 一条卸载登记。`key` 是完整注册表路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallRecord {
    pub key: String,
    pub display_name: String,
    pub uninstall_string: String,
    pub install_location: String,
    pub display_icon: String,
    pub publisher: String,
    pub system_component: bool,
    pub windows_installer: bool,
}

/// 可以交给系统启动的官方卸载命令。不含静默开关。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallCommand {
    Process {
        file: String,
        args: String,
    },
    /// 商店包的包系列名，例如 `Name_PublisherId`。
    Store {
        family_name: String,
    },
}

/// 卸载前记下的足迹，用来在卸载进程结束之后找残留。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppFootprint {
    pub name: String,
    pub publisher: String,
    pub install_dir: Option<PathBuf>,
    pub target_exe: Option<PathBuf>,
    /// 卸载进程结束时，目标 exe 是否还在。
    pub target_exe_present: bool,
    pub target_url: Option<String>,
    pub source_file: Option<PathBuf>,
    pub uninstall_key: Option<String>,
}

/// 菜单打开时算好的两项。`None` 表示那一项留在菜单里但不能点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalPlan {
    pub clean_path: Option<PathBuf>,
    pub uninstall: Option<PreparedUninstall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedUninstall {
    pub command: UninstallCommand,
    pub footprint: AppFootprint,
}

/// 本机上和删除判断有关的目录。测试用 [`MachineRoots::builtin`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineRoots {
    pub windows: PathBuf,
    pub program_files: PathBuf,
    pub program_files_x86: PathBuf,
    pub program_data: PathBuf,
    pub user_profile: PathBuf,
    pub app_data: PathBuf,
    pub local_app_data: PathBuf,
    pub desktop: PathBuf,
    pub public_desktop: PathBuf,
    pub documents: PathBuf,
    pub start_menu: Vec<PathBuf>,
    pub startup: Vec<PathBuf>,
}

impl MachineRoots {
    /// 单元测试和没有系统目录时的路径。正式运行改用本机已知文件夹。
    #[must_use]
    pub fn builtin() -> Self {
        let profile = PathBuf::from(r"C:\Users\User");
        let roaming = profile.join(r"AppData\Roaming");
        Self {
            windows: PathBuf::from(r"C:\Windows"),
            program_files: PathBuf::from(r"C:\Program Files"),
            program_files_x86: PathBuf::from(r"C:\Program Files (x86)"),
            program_data: PathBuf::from(r"C:\ProgramData"),
            user_profile: profile.clone(),
            app_data: roaming.clone(),
            local_app_data: profile.join(r"AppData\Local"),
            desktop: profile.join("Desktop"),
            public_desktop: PathBuf::from(r"C:\Users\Public\Desktop"),
            documents: profile.join("Documents"),
            start_menu: vec![
                PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu"),
                roaming.join(r"Microsoft\Windows\Start Menu"),
            ],
            startup: vec![
                PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup"),
                roaming.join(r"Microsoft\Windows\Start Menu\Programs\Startup"),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftoverKind {
    Directory,
    Shortcut,
    Registry,
}

/// 残留清单的一行。`checked` 按置信度给出默认值，界面可以改。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub kind: LeftoverKind,
    pub location: String,
    pub confidence: Confidence,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedShortcut {
    pub path: PathBuf,
    pub target: Option<PathBuf>,
    pub url: Option<String>,
}

/// 扫描看到的候选。规划函数再决定哪些进清单。
#[derive(Debug, Clone, Default)]
pub struct ObservedLeftovers {
    pub directories: Vec<PathBuf>,
    pub shortcuts: Vec<ObservedShortcut>,
    pub registry_keys: Vec<String>,
}

/// 卸载进程的结局。取消不显示错误，也不进入残留扫描。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallEnd {
    Cancelled,
    Finished { code: u32 },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfterUninstall {
    pub refresh: bool,
    pub scan: bool,
    pub error: Option<String>,
}

const SYSTEM_DIR_SUFFIXES: &[&str] = &[
    "Windows Defender",
    "Windows NT",
    "Internet Explorer",
    "Windows Media Player",
    "Windows Mail",
    "Windows Photo Viewer",
    "Windows Sidebar",
    "Windows Security",
    "WindowsApps",
    "ModifiableWindowsApps",
];

const GENERIC_NAMES: &[&str] = &[
    "microsoft",
    "windows",
    "common",
    "temp",
    "tmp",
    "programs",
    "packages",
    "data",
    "app",
    "apps",
    "application",
    "cache",
    "update",
    "updater",
    "install",
    "installer",
    "files",
    "local",
    "roaming",
    "locallow",
    "system",
    "software",
    "currentversion",
    "uninstall",
    "desktop",
    "documents",
    "downloads",
    "users",
    "public",
    "program files",
    "program files (x86)",
    "programdata",
    "windowsapps",
    "common files",
    "appdata",
    "start menu",
    "startup",
    "default",
    "music",
    "pictures",
    "videos",
    "favorites",
    "links",
    "contacts",
    "classes",
    "policies",
    "explorer",
    "wow6432node",
];

/// 菜单上「清理开始菜单入口」和「卸载…」是否能点。
#[must_use]
pub fn removal_plan(
    entry: &AppEntry,
    records: &[UninstallRecord],
    roots: &MachineRoots,
) -> RemovalPlan {
    let matched = match_uninstall(entry, records);
    let protected = is_protected(entry, &matched, roots);
    if protected {
        return RemovalPlan {
            clean_path: None,
            uninstall: None,
        };
    }
    let uninstall = usable_command(entry, matched, roots);
    RemovalPlan {
        clean_path: clean_path(entry, roots),
        uninstall,
    }
}

/// 卸载进程结束之后要不要刷新索引、扫残留、显示错误。
#[must_use]
pub fn after_uninstall(end: &UninstallEnd, indexed_entry_gone: bool) -> AfterUninstall {
    match end {
        UninstallEnd::Failed(message) => AfterUninstall {
            refresh: false,
            scan: false,
            error: Some(message.clone()),
        },
        UninstallEnd::Cancelled => AfterUninstall {
            refresh: true,
            scan: false,
            error: None,
        },
        UninstallEnd::Finished { code } if is_cancel_code(*code) => AfterUninstall {
            refresh: true,
            scan: false,
            error: None,
        },
        UninstallEnd::Finished { code } if !indexed_entry_gone => AfterUninstall {
            refresh: true,
            scan: false,
            error: (*code != 0 && *code != 3010).then(|| "卸载没有完成".to_owned()),
        },
        UninstallEnd::Finished { .. } => AfterUninstall {
            refresh: true,
            scan: true,
            error: None,
        },
    }
}

/// 把观察到的路径和键收成清单。关键目录和关键注册表路径不出现。
#[must_use]
pub fn plan_leftovers(
    footprint: &AppFootprint,
    observed: &ObservedLeftovers,
    roots: &MachineRoots,
) -> Vec<Leftover> {
    let mut items = Vec::new();
    for dir in &observed.directories {
        if let Some(item) = classify_dir(dir, footprint, roots) {
            push_unique(&mut items, item);
        }
    }
    for shortcut in &observed.shortcuts {
        if let Some(item) = classify_shortcut(shortcut, footprint, roots) {
            push_unique(&mut items, item);
        }
    }
    for key in &observed.registry_keys {
        if let Some(item) = classify_key(key, footprint) {
            push_unique(&mut items, item);
        }
    }
    items.sort_by(|left, right| {
        kind_rank(left.kind)
            .cmp(&kind_rank(right.kind))
            .then_with(|| {
                left.location
                    .to_lowercase()
                    .cmp(&right.location.to_lowercase())
            })
    });
    items
}

/// 删除前再查一次。规划漏掉的关键路径这里仍然拒绝。
#[must_use]
pub fn deletion_allowed(kind: LeftoverKind, location: &str, roots: &MachineRoots) -> bool {
    match kind {
        LeftoverKind::Registry => registry_delete_allowed(location),
        LeftoverKind::Shortcut => shortcut_delete_allowed(Path::new(location), roots),
        LeftoverKind::Directory => directory_delete_allowed(Path::new(location), roots),
    }
}

/// 只允许这一条卸载子键，或软件名下的一层到两层键。
#[must_use]
pub fn registry_delete_allowed(key: &str) -> bool {
    let parts = key_parts(key);
    if uninstall_product_key(&parts) {
        return true;
    }
    software_app_key(&parts)
}

/// 按名称和发布者拼出要查看的注册表键。不包含关键路径。
#[must_use]
pub fn registry_candidates(footprint: &AppFootprint) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(key) = footprint
        .uninstall_key
        .as_ref()
        .filter(|key| registry_delete_allowed(key))
    {
        keys.push(key.clone());
    }
    let name = name_token(&footprint.name);
    let publisher = name_token(&footprint.publisher);
    let Some(name) = name else {
        return keys;
    };
    for prefix in [
        r"HKCU\Software",
        r"HKLM\SOFTWARE",
        r"HKLM\SOFTWARE\WOW6432Node",
    ] {
        let leaf = format!(r"{prefix}\{name}");
        if registry_delete_allowed(&leaf) {
            keys.push(leaf);
        }
        if let Some(publisher) = publisher.as_deref() {
            let nested = format!(r"{prefix}\{publisher}\{name}");
            if registry_delete_allowed(&nested) {
                keys.push(nested);
            }
            let wide = format!(r"{prefix}\{publisher}");
            if registry_delete_allowed(&wide) {
                keys.push(wide);
            }
        }
    }
    keys
}

fn usable_command(
    entry: &AppEntry,
    matched: Option<RecordMatch<'_>>,
    roots: &MachineRoots,
) -> Option<PreparedUninstall> {
    let (command, record) = match (&entry.target, matched) {
        (LaunchTarget::Aumid { aumid }, _) => {
            let family_name = store_family(aumid)?;
            (UninstallCommand::Store { family_name }, None)
        }
        (
            _,
            Some(RecordMatch::Command {
                file, args, record, ..
            }),
        ) => (UninstallCommand::Process { file, args }, Some(record)),
        _ => return None,
    };
    Some(PreparedUninstall {
        command,
        footprint: footprint(entry, record, roots),
    })
}

fn footprint(
    entry: &AppEntry,
    record: Option<&UninstallRecord>,
    roots: &MachineRoots,
) -> AppFootprint {
    let target_exe = match &entry.target {
        LaunchTarget::Path { path, .. } => Some(path.clone()),
        _ => None,
    };
    let install_dir = record
        .map(|record| record.install_location.trim())
        .filter(|text| !text.is_empty())
        .map(PathBuf::from)
        .filter(|path| directory_delete_allowed(path, roots))
        .or_else(|| {
            target_exe
                .as_deref()
                .and_then(parent_display)
                .filter(|path| directory_delete_allowed(path, roots))
        });
    AppFootprint {
        name: entry.name.clone(),
        publisher: record
            .map(|record| record.publisher.trim().to_owned())
            .unwrap_or_default(),
        install_dir,
        target_exe_present: false,
        target_exe,
        target_url: match &entry.target {
            LaunchTarget::Url { url } => Some(url.clone()),
            _ => None,
        },
        source_file: entry.source_path.clone(),
        uninstall_key: record
            .map(|record| record.key.clone())
            .filter(|key| registry_delete_allowed(key)),
    }
}

enum RecordMatch<'a> {
    Command {
        file: String,
        args: String,
        record: &'a UninstallRecord,
    },
    SystemComponent(&'a UninstallRecord),
}

fn match_uninstall<'a>(
    entry: &AppEntry,
    records: &'a [UninstallRecord],
) -> Option<RecordMatch<'a>> {
    if matches!(entry.target, LaunchTarget::Aumid { .. }) {
        return None;
    }
    let mut best: Option<(u32, Vec<&UninstallRecord>)> = None;
    for record in records {
        let score = match_score(entry, record);
        if score == 0 {
            continue;
        }
        match &mut best {
            Some((best_score, tied)) if *best_score == score => tied.push(record),
            Some((best_score, _)) if *best_score > score => {}
            _ => best = Some((score, vec![record])),
        }
    }
    let (score, tied) = best?;
    if tied.len() != 1 || score < 50 {
        return None;
    }
    let record = tied[0];
    if record.system_component {
        return Some(RecordMatch::SystemComponent(record));
    }
    let parsed = parse_record_command(record)?;
    Some(RecordMatch::Command {
        file: parsed.file,
        args: parsed.args,
        record,
    })
}

fn match_score(entry: &AppEntry, record: &UninstallRecord) -> u32 {
    let Some(target) = target_path(&entry.target) else {
        return name_score(&entry.name, &record.display_name);
    };
    if path_eq(icon_file(&record.display_icon).as_deref(), Some(target)) {
        return 100;
    }
    if string_contains_path(&record.uninstall_string, target) {
        return 90;
    }
    if location_contains_target(&record.install_location, target) {
        return 80;
    }
    name_score(&entry.name, &record.display_name)
}

fn name_score(left: &str, right: &str) -> u32 {
    if folded(left) == folded(right) && !folded(left).is_empty() {
        50
    } else {
        0
    }
}

struct OwnedCommand {
    file: String,
    args: String,
}

fn parse_record_command(record: &UninstallRecord) -> Option<OwnedCommand> {
    if let Some(parsed) = parse_uninstall_string(&record.uninstall_string) {
        return Some(parsed);
    }
    if record.windows_installer {
        let id = record.key.rsplit('\\').next().unwrap_or("");
        let guid = brace_guid(id)?;
        return Some(OwnedCommand {
            file: "msiexec.exe".to_owned(),
            args: format!("/x {guid}"),
        });
    }
    None
}

fn parse_uninstall_string(raw: &str) -> Option<OwnedCommand> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (file, args) = split_command(raw)?;
    if is_msiexec(&file) {
        return Some(OwnedCommand {
            file: "msiexec.exe".to_owned(),
            args: strip_quiet_switches(&args),
        });
    }
    if file.trim().is_empty() {
        return None;
    }
    Some(OwnedCommand {
        file,
        args: strip_quiet_switches(&args),
    })
}

fn split_command(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        let end = rest.find('"')?;
        let file = rest[..end].trim().to_owned();
        let args = rest[end + 1..].trim().to_owned();
        return Some((file, args));
    }
    let lower = raw.to_ascii_lowercase();
    if let Some(index) = lower.find(".exe") {
        let end = index + 4;
        let file = raw[..end].trim().trim_matches('"').to_owned();
        let args = raw[end..].trim().to_owned();
        return Some((file, args));
    }
    if lower.contains("msiexec") {
        return Some(("msiexec.exe".to_owned(), raw.to_owned()));
    }
    Some((raw.to_owned(), String::new()))
}

fn strip_quiet_switches(args: &str) -> String {
    args.split_whitespace()
        .filter(|token| !is_quiet_switch(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_quiet_switch(token: &str) -> bool {
    let token = token.trim_matches('"').to_ascii_lowercase();
    matches!(
        token.as_str(),
        "/q" | "/qn"
            | "/qb"
            | "/qb!"
            | "/qb-"
            | "/qr"
            | "/quiet"
            | "/passive"
            | "/s"
            | "/silent"
            | "/verysilent"
            | "/suppressmsgboxes"
            | "/norestart"
            | "-s"
            | "-silent"
            | "-verysilent"
    )
}

fn is_msiexec(file: &str) -> bool {
    Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("msiexec.exe"))
}

fn brace_guid(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
        .unwrap_or(trimmed);
    let parts: Vec<&str> = inner.split('-').collect();
    if parts.len() != 5
        || parts[0].len() != 8
        || parts[1].len() != 4
        || parts[2].len() != 4
        || parts[3].len() != 4
        || parts[4].len() != 12
        || !parts
            .iter()
            .all(|part| part.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(format!("{{{}}}", inner.to_ascii_uppercase()))
}

fn is_protected(entry: &AppEntry, matched: &Option<RecordMatch<'_>>, roots: &MachineRoots) -> bool {
    if protected_package(&entry.target) {
        return true;
    }
    if let Some(RecordMatch::SystemComponent(record)) = matched
        && record.system_component
    {
        return true;
    }
    if let Some(path) = target_path(&entry.target)
        && is_system_dir(path, roots)
    {
        return true;
    }
    let has_command = matches!(matched, Some(RecordMatch::Command { .. }))
        || matches!(entry.target, LaunchTarget::Aumid { .. });
    if has_command {
        return false;
    }
    let paths = local_paths(entry);
    !paths.is_empty() && paths.iter().all(|path| is_system_dir(path, roots))
}

fn protected_package(target: &LaunchTarget) -> bool {
    let LaunchTarget::Aumid { aumid } = target else {
        return false;
    };
    let Some(family) = aumid.split('!').next() else {
        return false;
    };
    let lower = family.to_ascii_lowercase();
    lower.starts_with("windows.")
        || lower.starts_with("microsoft.windows.")
        || lower.starts_with("microsoft.aad")
        || lower.starts_with("microsoft.accountscontrol")
        || lower.starts_with("microsoft.asynctextservice")
        || lower.starts_with("microsoft.bioenrollment")
        || lower.starts_with("microsoft.creddialoghost")
        || lower.starts_with("microsoft.lockapp")
        || lower.starts_with("microsoft.win32webviewhost")
        || lower.starts_with("microsoft.xboxgamecallableui")
}

fn store_family(aumid: &str) -> Option<String> {
    let (family, app) = aumid.split_once('!')?;
    if app.is_empty()
        || protected_package(&LaunchTarget::Aumid {
            aumid: aumid.to_owned(),
        })
    {
        return None;
    }
    let mut chars = family.chars();
    let first = chars.next()?;
    let ok = first.is_ascii_alphanumeric()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-');
    ok.then(|| family.to_owned())
}

fn clean_path(entry: &AppEntry, roots: &MachineRoots) -> Option<PathBuf> {
    if entry.source != AppSource::StartMenu {
        return None;
    }
    let path = entry.source_path.as_ref()?;
    shortcut_delete_allowed(path, roots).then(|| path.clone())
}

fn local_paths(entry: &AppEntry) -> Vec<&Path> {
    let mut paths = Vec::new();
    if let Some(path) = target_path(&entry.target) {
        paths.push(path);
    }
    if let Some(path) = entry.source_path.as_deref() {
        paths.push(path);
    }
    paths
}

fn target_path(target: &LaunchTarget) -> Option<&Path> {
    match target {
        LaunchTarget::Path { path, .. } if !path.as_os_str().is_empty() => Some(path.as_path()),
        _ => None,
    }
}

fn is_system_dir(path: &Path, roots: &MachineRoots) -> bool {
    if under(path, &roots.windows) {
        return true;
    }
    for base in [&roots.program_files, &roots.program_files_x86] {
        for suffix in SYSTEM_DIR_SUFFIXES {
            if under(path, &base.join(suffix)) {
                return true;
            }
        }
    }
    false
}

fn shortcut_delete_allowed(path: &Path, roots: &MachineRoots) -> bool {
    if !is_shortcut_file(path) || is_blocked_path(path, roots) {
        return false;
    }
    let in_menu = roots.start_menu.iter().any(|root| under(path, root))
        && !roots.startup.iter().any(|root| under(path, root));
    let on_desktop = under(path, &roots.desktop) || under(path, &roots.public_desktop);
    (in_menu || on_desktop)
        && !equals_path(path, &roots.desktop)
        && !equals_path(path, &roots.public_desktop)
}

fn directory_delete_allowed(path: &Path, roots: &MachineRoots) -> bool {
    let key = normalize_path_key(path);
    if key.is_empty() || is_blocked_path(path, roots) || is_generic_root(path, roots) {
        return false;
    }
    key.contains('\\')
}

fn is_blocked_path(path: &Path, roots: &MachineRoots) -> bool {
    let key = normalize_path_key(path);
    if key.len() <= 3 || is_system_dir(path, roots) {
        return true;
    }
    // 开始菜单和桌面的快捷方式落在 Microsoft 目录下面，删除快捷方式本身仍然允许。
    if is_shortcut_file(path) && shortcut_home(path, roots) {
        return false;
    }
    let blocked = [
        roots.local_app_data.join("Temp"),
        roots.local_app_data.join("Packages"),
        roots.local_app_data.join("Microsoft"),
        roots.app_data.join("Microsoft"),
        roots.program_data.join("Microsoft"),
        roots.program_files.join("Common Files"),
        roots.program_files_x86.join("Common Files"),
    ];
    blocked.iter().any(|root| under(path, root))
}

fn shortcut_home(path: &Path, roots: &MachineRoots) -> bool {
    roots.start_menu.iter().any(|root| under(path, root))
        || under(path, &roots.desktop)
        || under(path, &roots.public_desktop)
}

fn is_generic_root(path: &Path, roots: &MachineRoots) -> bool {
    let extras = [
        roots.user_profile.join("Downloads"),
        roots.user_profile.join("Music"),
        roots.user_profile.join("Pictures"),
        roots.user_profile.join("Videos"),
        roots.user_profile.join("AppData"),
        roots.local_app_data.join("Programs"),
        PathBuf::from(r"C:\Users"),
        PathBuf::from(r"C:\Users\Public"),
        PathBuf::from(r"C:\Users\Default"),
    ];
    let fixed = [
        &roots.windows,
        &roots.program_files,
        &roots.program_files_x86,
        &roots.program_data,
        &roots.user_profile,
        &roots.app_data,
        &roots.local_app_data,
        &roots.desktop,
        &roots.public_desktop,
        &roots.documents,
    ];
    fixed.iter().any(|root| equals_path(path, root))
        || extras.iter().any(|root| equals_path(path, root))
        || roots.start_menu.iter().any(|root| equals_path(path, root))
        || roots.startup.iter().any(|root| equals_path(path, root))
        || roots
            .start_menu
            .iter()
            .any(|root| equals_path(path, &root.join("Programs")))
}

fn classify_dir(path: &Path, footprint: &AppFootprint, roots: &MachineRoots) -> Option<Leftover> {
    if !directory_delete_allowed(path, roots) {
        return None;
    }
    let leaf = file_leaf(path);
    let display = name_token(&footprint.name);
    let publisher = name_token(&footprint.publisher);
    let stem = exe_stem(footprint).filter(|token| display.as_ref() != Some(token));
    let leaf_is_display = tokens_equal(leaf.as_deref(), display.as_deref());
    let leaf_is_publisher = tokens_equal(leaf.as_deref(), publisher.as_deref());
    let leaf_is_stem = tokens_equal(leaf.as_deref(), stem.as_deref());
    let is_install = footprint
        .install_dir
        .as_ref()
        .is_some_and(|dir| equals_path(path, dir));
    let is_exe_home = footprint.target_exe.as_ref().is_some_and(|exe| {
        parent_key(exe).is_some_and(|parent| parent == normalize_path_key(path))
    });
    if !leaf_is_display && !leaf_is_publisher && !leaf_is_stem && !is_install && !is_exe_home {
        return None;
    }
    // 主文件名和厂商目录只作参考。高置信度限于登记里的安装目录，
    // 以及与显示名相同、父目录不是公共根的文件夹。
    let publisher_folder = leaf_is_publisher && !is_install;
    let stem_only = leaf_is_stem && !leaf_is_display && !is_install;
    let confidence = if !footprint.target_exe_present
        && !low_confidence_parent(path, roots)
        && !publisher_folder
        && !stem_only
        && (is_install || (leaf_is_display && !public_root_parent(path, roots)))
    {
        Confidence::High
    } else {
        Confidence::Low
    };
    Some(leftover(
        LeftoverKind::Directory,
        path.to_string_lossy().replace('/', "\\"),
        confidence,
    ))
}

fn classify_shortcut(
    shortcut: &ObservedShortcut,
    footprint: &AppFootprint,
    roots: &MachineRoots,
) -> Option<Leftover> {
    if !shortcut_delete_allowed(&shortcut.path, roots) {
        return None;
    }
    let target_hit = shortcut.target.as_ref().is_some_and(|target| {
        footprint
            .target_exe
            .as_ref()
            .is_some_and(|exe| equals_path(target, exe))
            || footprint
                .install_dir
                .as_ref()
                .is_some_and(|dir| under(target, dir))
    });
    let url_hit = shortcut.url.as_ref().is_some_and(|url| {
        footprint
            .target_url
            .as_ref()
            .is_some_and(|expected| folded(url) == folded(expected))
    });
    let name_hit = shortcut_name_hit(&shortcut.path, footprint);
    let confidence = if target_hit || url_hit {
        Confidence::High
    } else if name_hit {
        Confidence::Low
    } else {
        return None;
    };
    Some(leftover(
        LeftoverKind::Shortcut,
        shortcut.path.to_string_lossy().replace('/', "\\"),
        confidence,
    ))
}

fn classify_key(key: &str, footprint: &AppFootprint) -> Option<Leftover> {
    if !registry_delete_allowed(key) {
        return None;
    }
    let parts = key_parts(key);
    let leaf = parts.last()?.as_str();
    let display = name_token(&footprint.name);
    let publisher = name_token(&footprint.publisher);
    let stem = exe_stem(footprint).filter(|token| display.as_ref() != Some(token));
    let exact_uninstall = footprint
        .uninstall_key
        .as_ref()
        .is_some_and(|saved| keys_equal(saved, key));
    let leaf_is_display = display
        .as_deref()
        .is_some_and(|token| keys_equal(token, leaf));
    let leaf_is_publisher = publisher
        .as_deref()
        .is_some_and(|token| keys_equal(token, leaf));
    let leaf_is_stem = stem.as_deref().is_some_and(|token| keys_equal(token, leaf));
    if !exact_uninstall && !leaf_is_display && !leaf_is_publisher && !leaf_is_stem {
        return None;
    }
    // 厂商键是 `Software\<厂商>` 这一层。显示名和厂商相同时也不默认勾选。
    let publisher_root = publisher
        .as_deref()
        .is_some_and(|token| publisher_root_key(&parts, token));
    let confidence = if exact_uninstall || (leaf_is_display && !publisher_root && !leaf_is_stem) {
        Confidence::High
    } else {
        Confidence::Low
    };
    Some(leftover(LeftoverKind::Registry, key.to_owned(), confidence))
}

fn leftover(kind: LeftoverKind, location: String, confidence: Confidence) -> Leftover {
    Leftover {
        kind,
        checked: confidence == Confidence::High,
        location,
        confidence,
    }
}

fn push_unique(items: &mut Vec<Leftover>, item: Leftover) {
    let key = item.location.to_lowercase();
    if let Some(existing) = items
        .iter_mut()
        .find(|found| found.location.to_lowercase() == key)
    {
        if confidence_rank(item.confidence) > confidence_rank(existing.confidence) {
            *existing = item;
        }
        return;
    }
    items.push(item);
}

fn exe_stem(footprint: &AppFootprint) -> Option<String> {
    footprint.target_exe.as_deref().and_then(file_stem)
}

fn tokens_equal(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn shortcut_name_hit(path: &Path, footprint: &AppFootprint) -> bool {
    let Some(stem) = file_stem(path) else {
        return false;
    };
    name_token(&footprint.name).as_deref() == Some(stem.as_str())
        || exe_stem(footprint).as_deref() == Some(stem.as_str())
}

/// `HKCU\Software\<厂商>`、`HKLM\SOFTWARE\<厂商>` 和 WOW6432Node 下的同一层。
fn publisher_root_key(parts: &[String], publisher: &str) -> bool {
    let view: Vec<&str> = parts.iter().map(String::as_str).collect();
    let leaf_ok = view.last().is_some_and(|leaf| keys_equal(leaf, publisher));
    leaf_ok
        && matches!(
            view.as_slice(),
            ["HKLM", "SOFTWARE", _]
                | ["HKCU", "SOFTWARE", _]
                | ["HKLM", "SOFTWARE", "WOW6432NODE", _]
        )
}

fn public_root_parent(path: &Path, roots: &MachineRoots) -> bool {
    let Some(parent) = parent_key(path) else {
        return true;
    };
    [
        &roots.program_files,
        &roots.program_files_x86,
        &roots.program_data,
        &roots.app_data,
        &roots.local_app_data,
    ]
    .iter()
    .any(|root| parent == normalize_path_key(root))
}

fn name_token(text: &str) -> Option<String> {
    let token = folded(text);
    if token.chars().count() < 2 || GENERIC_NAMES.contains(&token.as_str()) {
        None
    } else {
        Some(token)
    }
}

fn low_confidence_parent(path: &Path, roots: &MachineRoots) -> bool {
    let Some(parent) = parent_key(path) else {
        return true;
    };
    let extras = [
        roots.user_profile.join("Downloads"),
        roots.user_profile.join("Music"),
        roots.user_profile.join("Pictures"),
        roots.user_profile.join("Videos"),
    ];
    parent == normalize_path_key(&roots.documents)
        || parent == normalize_path_key(&roots.desktop)
        || parent == normalize_path_key(&roots.public_desktop)
        || parent == normalize_path_key(&roots.user_profile)
        || extras
            .iter()
            .any(|extra| parent == normalize_path_key(extra))
}

/// 按 `\` 取最后一段。测试在 Linux 上跑，不能用 `Path::file_name` 切 Windows 路径。
fn file_leaf(path: &Path) -> Option<String> {
    let key = normalize_path_key(path);
    let leaf = key.rsplit('\\').next().unwrap_or("");
    (!leaf.is_empty()).then(|| leaf.to_owned())
}

fn file_stem(path: &Path) -> Option<String> {
    let leaf = file_leaf(path)?;
    let stem = leaf
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(leaf.as_str());
    (!stem.is_empty()).then(|| stem.to_owned())
}

/// 父目录的原始写法。按 `\` 切开，供 Linux 上的测试和 Windows 共用。
fn parent_display(path: &Path) -> Option<PathBuf> {
    let text = path.to_string_lossy().replace('/', "\\");
    let trimmed = text.trim_end_matches('\\');
    let (parent, leaf) = trimmed.rsplit_once('\\')?;
    if parent.is_empty() || leaf.is_empty() {
        None
    } else {
        Some(PathBuf::from(parent))
    }
}

fn parent_key(path: &Path) -> Option<String> {
    let key = normalize_path_key(path);
    let (parent, leaf) = key.rsplit_once('\\')?;
    if parent.is_empty() || leaf.is_empty() {
        None
    } else {
        Some(parent.to_owned())
    }
}

fn icon_file(raw: &str) -> Option<PathBuf> {
    let text = raw.trim().trim_matches('"');
    let cut = text
        .split(',')
        .next()
        .unwrap_or(text)
        .trim()
        .trim_matches('"');
    if cut.is_empty() {
        None
    } else {
        Some(PathBuf::from(cut))
    }
}

/// 卸载字符串里出现目标路径，并且前后都是命令行边界。
/// 保留 `\`，所以 `My\App.exe` 不会配上 `MyApp.exe`。
fn string_contains_path(hay: &str, path: &Path) -> bool {
    let needle: Vec<char> = normalize_path_key(path).chars().collect();
    if needle.len() < 4 {
        return false;
    }
    let hay: Vec<char> = hay.replace('/', "\\").to_lowercase().chars().collect();
    if hay.len() < needle.len() {
        return false;
    }
    for start in 0..=hay.len() - needle.len() {
        if hay[start..start + needle.len()] != needle[..] {
            continue;
        }
        let before = start.checked_sub(1).map(|index| hay[index]);
        let after = hay.get(start + needle.len()).copied();
        if path_token_edge(before) && path_token_edge(after) {
            return true;
        }
    }
    false
}

fn path_token_edge(ch: Option<char>) -> bool {
    match ch {
        None => true,
        Some(ch) => ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | '='),
    }
}

fn location_contains_target(location: &str, target: &Path) -> bool {
    let location = location.trim();
    if location.is_empty() {
        return false;
    }
    let loc = normalize_path_key(Path::new(location));
    let target = normalize_path_key(target);
    if loc.len() < 4 || generic_location(&loc) {
        return false;
    }
    target.starts_with(&(loc + "\\"))
}

fn generic_location(key: &str) -> bool {
    matches!(
        key,
        r"c:\program files"
            | r"c:\program files (x86)"
            | r"c:\programdata"
            | r"c:\windows"
            | r"c:\users"
    ) || key.ends_with(r"\appdata")
        || key.ends_with(r"\appdata\roaming")
        || key.ends_with(r"\appdata\local")
        || key.ends_with(r"\desktop")
        || key.ends_with(r"\documents")
}

fn path_eq(left: Option<&Path>, right: Option<&Path>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => equals_path(left, right),
        _ => false,
    }
}

fn equals_path(left: &Path, right: &Path) -> bool {
    let left = normalize_path_key(left);
    let right = normalize_path_key(right);
    !left.is_empty() && left == right
}

fn under(path: &Path, root: &Path) -> bool {
    let path = normalize_path_key(path);
    let root = normalize_path_key(root);
    if path.is_empty() || root.is_empty() {
        return false;
    }
    path == root || path.starts_with(&(root + "\\"))
}

fn is_shortcut_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk") || ext.eq_ignore_ascii_case("url"))
}

fn folded(text: &str) -> String {
    text.trim().chars().flat_map(char::to_lowercase).collect()
}

fn key_parts(key: &str) -> Vec<String> {
    key.replace('/', "\\")
        .split('\\')
        .map(|part| part.trim().to_uppercase())
        .filter(|part| !part.is_empty())
        .collect()
}

fn keys_equal(left: &str, right: &str) -> bool {
    folded(left) == folded(right)
}

fn uninstall_product_key(parts: &[String]) -> bool {
    let view: Vec<&str> = parts.iter().map(String::as_str).collect();
    match view.as_slice() {
        [
            "HKLM",
            "SOFTWARE",
            "MICROSOFT",
            "WINDOWS",
            "CURRENTVERSION",
            "UNINSTALL",
            id,
        ]
        | [
            "HKCU",
            "SOFTWARE",
            "MICROSOFT",
            "WINDOWS",
            "CURRENTVERSION",
            "UNINSTALL",
            id,
        ]
        | [
            "HKLM",
            "SOFTWARE",
            "WOW6432NODE",
            "MICROSOFT",
            "WINDOWS",
            "CURRENTVERSION",
            "UNINSTALL",
            id,
        ] => ok_product_id(id),
        _ => false,
    }
}

fn software_app_key(parts: &[String]) -> bool {
    let view: Vec<&str> = parts.iter().map(String::as_str).collect();
    match view.as_slice() {
        ["HKLM", "SOFTWARE", "WOW6432NODE", name] => ok_leaf(name),
        ["HKLM", "SOFTWARE", "WOW6432NODE", publisher, name] => ok_leaf(publisher) && ok_leaf(name),
        ["HKCU", "SOFTWARE", name] | ["HKLM", "SOFTWARE", name] => ok_leaf(name),
        ["HKCU", "SOFTWARE", publisher, name] | ["HKLM", "SOFTWARE", publisher, name] => {
            ok_leaf(publisher) && ok_leaf(name)
        }
        _ => false,
    }
}

fn ok_product_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !blocked_leaf(id)
}

fn ok_leaf(name: &str) -> bool {
    name.chars().count() >= 2 && !name.contains(':') && !blocked_leaf(name)
}

fn blocked_leaf(name: &str) -> bool {
    matches!(
        name,
        "MICROSOFT"
            | "WINDOWS"
            | "CLASSES"
            | "SYSTEM"
            | "POLICIES"
            | "EXPLORER"
            | "CURRENTVERSION"
            | "UNINSTALL"
            | "SOFTWARE"
            | "WOW6432NODE"
            | "APP PATHS"
            | "WINDOWS NT"
            | "COMMAND PROCESSOR"
            | "WINDOWS DEFENDER"
            | "INTERNET EXPLORER"
            | "WINDOWSAPPS"
    )
}

fn is_cancel_code(code: u32) -> bool {
    matches!(code, 2 | 1223 | 1602)
}

fn kind_rank(kind: LeftoverKind) -> u8 {
    match kind {
        LeftoverKind::Directory => 0,
        LeftoverKind::Shortcut => 1,
        LeftoverKind::Registry => 2,
    }
}

fn confidence_rank(confidence: Confidence) -> u8 {
    match confidence {
        Confidence::Low => 0,
        Confidence::High => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> MachineRoots {
        MachineRoots::builtin()
    }

    fn entry(
        name: &str,
        source: AppSource,
        target: LaunchTarget,
        source_path: Option<&str>,
    ) -> AppEntry {
        AppEntry {
            name: name.to_owned(),
            source,
            target,
            icon_path: None,
            icon_index: 0,
            source_path: source_path.map(PathBuf::from),
            alternate_names: Vec::new(),
        }
    }

    fn exe(path: &str) -> LaunchTarget {
        LaunchTarget::Path {
            path: PathBuf::from(path),
            args: String::new(),
            working_directory: None,
        }
    }

    fn record(name: &str, uninstall: &str, location: &str) -> UninstallRecord {
        UninstallRecord {
            key: format!(r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{name}"),
            display_name: name.to_owned(),
            uninstall_string: uninstall.to_owned(),
            install_location: location.to_owned(),
            display_icon: String::new(),
            publisher: "Demo Co".to_owned(),
            system_component: false,
            windows_installer: false,
        }
    }

    #[test]
    fn start_menu_shortcut_can_be_cleaned_without_an_uninstaller() {
        let item = entry(
            "Demo",
            AppSource::StartMenu,
            exe(r"C:\Program Files\Demo\demo.exe"),
            Some(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Demo.lnk"),
        );
        let plan = removal_plan(&item, &[], &roots());
        assert_eq!(
            plan.clean_path.as_deref(),
            Some(Path::new(
                r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Demo.lnk"
            ))
        );
        assert!(plan.uninstall.is_none());
    }

    #[test]
    fn system_notepad_keeps_clean_and_uninstall_disabled() {
        let item = entry(
            "记事本",
            AppSource::StartMenu,
            exe(r"C:\Windows\System32\notepad.exe"),
            Some(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Accessories\Notepad.lnk"),
        );
        let plan = removal_plan(&item, &[], &roots());
        assert!(plan.clean_path.is_none());
        assert!(plan.uninstall.is_none());
    }

    #[test]
    fn uninstall_string_matches_the_install_directory_and_drops_quiet_switches() {
        let item = entry(
            "Demo",
            AppSource::StartMenu,
            exe(r"C:\Program Files\Demo\demo.exe"),
            Some(r"C:\Users\User\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Demo.lnk"),
        );
        let records = [record(
            "Demo",
            r#""C:\Program Files\Demo\uninstall.exe" /allusers"#,
            r"C:\Program Files\Demo",
        )];
        let plan = removal_plan(&item, &records, &roots());
        assert!(plan.clean_path.is_some());
        let command = plan.uninstall.expect("uninstall").command;
        assert_eq!(
            command,
            UninstallCommand::Process {
                file: r"C:\Program Files\Demo\uninstall.exe".to_owned(),
                args: "/allusers".to_owned(),
            }
        );
    }

    #[test]
    fn msiexec_quiet_switches_are_removed_and_ambiguous_names_do_not_match() {
        let parsed =
            parse_uninstall_string(r"MsiExec.exe /I{11111111-2222-3333-4444-555555555555} /qn")
                .expect("msi");
        assert_eq!(parsed.file, "msiexec.exe");
        assert_eq!(parsed.args, "/I{11111111-2222-3333-4444-555555555555}");

        let item = entry("Demo", AppSource::Path, exe(r"D:\Tools\demo.exe"), None);
        let records = [
            record("Demo", r"C:\One\uninstall.exe", r"C:\One"),
            record("Demo", r"C:\Two\uninstall.exe", r"C:\Two"),
        ];
        assert!(removal_plan(&item, &records, &roots()).uninstall.is_none());
    }

    #[test]
    fn msi_product_code_without_a_string_uses_interactive_msiexec() {
        let mut saved = record("Demo", "", "");
        saved.windows_installer = true;
        saved.key = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{AABBCCDD-1111-2222-3333-444444444444}".to_owned();
        saved.display_icon = r"C:\Program Files\Demo\demo.exe,0".to_owned();
        let item = entry(
            "别的名字",
            AppSource::StartMenu,
            exe(r"C:\Program Files\Demo\demo.exe"),
            Some(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Demo.lnk"),
        );
        let command = removal_plan(&item, &[saved], &roots())
            .uninstall
            .expect("msi")
            .command;
        assert_eq!(
            command,
            UninstallCommand::Process {
                file: "msiexec.exe".to_owned(),
                args: "/x {AABBCCDD-1111-2222-3333-444444444444}".to_owned(),
            }
        );
    }

    #[test]
    fn store_and_inbox_packages_follow_the_family_name() {
        let calc = entry(
            "计算器",
            AppSource::Store,
            LaunchTarget::Aumid {
                aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned(),
            },
            None,
        );
        let plan = removal_plan(&calc, &[], &roots());
        assert!(plan.clean_path.is_none());
        assert_eq!(
            plan.uninstall.expect("store").command,
            UninstallCommand::Store {
                family_name: "Microsoft.WindowsCalculator_8wekyb3d8bbwe".to_owned(),
            }
        );
        let settings = entry(
            "设置",
            AppSource::Store,
            LaunchTarget::Aumid {
                aumid: "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel"
                    .to_owned(),
            },
            None,
        );
        let plan = removal_plan(&settings, &[], &roots());
        assert!(plan.clean_path.is_none());
        assert!(plan.uninstall.is_none());
        let dotted = entry(
            "点",
            AppSource::Store,
            LaunchTarget::Aumid {
                aumid: ".NotAPackage_publisher!App".to_owned(),
            },
            None,
        );
        assert!(removal_plan(&dotted, &[], &roots()).uninstall.is_none());
    }

    #[test]
    fn system_component_flag_disables_both_actions() {
        let mut saved = record(
            "Demo",
            r"C:\Program Files\Demo\uninstall.exe",
            r"C:\Program Files\Demo",
        );
        saved.system_component = true;
        let item = entry(
            "Demo",
            AppSource::StartMenu,
            exe(r"C:\Program Files\Demo\demo.exe"),
            Some(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Demo.lnk"),
        );
        let plan = removal_plan(&item, &[saved], &roots());
        assert!(plan.clean_path.is_none());
        assert!(plan.uninstall.is_none());
    }

    #[test]
    fn game_url_in_the_start_menu_can_be_cleaned() {
        let item = entry(
            "Dota 2",
            AppSource::StartMenu,
            LaunchTarget::Url {
                url: "steam://rungameid/570".to_owned(),
            },
            Some(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Dota 2.url"),
        );
        let plan = removal_plan(&item, &[], &roots());
        assert!(plan.clean_path.is_some());
        assert!(plan.uninstall.is_none());
    }

    #[test]
    fn leftovers_check_exact_items_and_skip_critical_paths() {
        let footprint = AppFootprint {
            name: "Demo".to_owned(),
            publisher: "Demo Co".to_owned(),
            install_dir: Some(PathBuf::from(r"C:\Program Files\Demo")),
            target_exe: Some(PathBuf::from(r"C:\Program Files\Demo\demo.exe")),
            target_exe_present: false,
            target_url: None,
            source_file: None,
            uninstall_key: Some(
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Demo".to_owned(),
            ),
        };
        let observed = ObservedLeftovers {
            directories: vec![
                PathBuf::from(r"C:\Program Files\Demo"),
                PathBuf::from(r"C:\Users\User\AppData\Roaming\Demo Co"),
                PathBuf::from(r"C:\Users\User\Documents\Demo"),
                PathBuf::from(r"C:\Windows\System32"),
                PathBuf::from(r"C:\Program Files"),
                PathBuf::from(r"C:\Program Files\WindowsApps\Demo"),
            ],
            shortcuts: vec![
                ObservedShortcut {
                    path: PathBuf::from(r"C:\Users\User\Desktop\Demo.lnk"),
                    target: Some(PathBuf::from(r"C:\Program Files\Demo\demo.exe")),
                    url: None,
                },
                ObservedShortcut {
                    path: PathBuf::from(
                        r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Demo.lnk",
                    ),
                    target: Some(PathBuf::from(r"D:\Other\other.exe")),
                    url: None,
                },
            ],
            registry_keys: vec![
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Demo".to_owned(),
                r"HKCU\Software\Demo".to_owned(),
                r"HKCU\Software\Demo Co".to_owned(),
                r"HKLM\SYSTEM\CurrentControlSet\Services\Demo".to_owned(),
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall".to_owned(),
                r"HKCU\Software\Microsoft".to_owned(),
            ],
        };
        let items = plan_leftovers(&footprint, &observed, &roots());
        let checked: Vec<_> = items
            .iter()
            .filter(|item| item.checked)
            .map(|item| item.location.as_str())
            .collect();
        assert!(
            checked
                .iter()
                .any(|path| path.ends_with(r"Program Files\Demo"))
        );
        assert!(
            checked
                .iter()
                .any(|path| path.ends_with(r"Desktop\Demo.lnk"))
        );
        assert!(checked.iter().any(|path| path.ends_with(r"Uninstall\Demo")));
        assert!(checked.contains(&r"HKCU\Software\Demo"));
        assert!(
            items
                .iter()
                .any(|item| { item.location.ends_with(r"Roaming\Demo Co") && !item.checked })
        );
        assert!(
            items
                .iter()
                .any(|item| { item.location.ends_with(r"Documents\Demo") && !item.checked })
        );
        assert!(
            items
                .iter()
                .any(|item| { item.location.ends_with(r"Programs\Demo.lnk") && !item.checked })
        );
        assert!(items.iter().all(|item| {
            !item.location.contains(r"System32")
                && !item.location.contains("WindowsApps")
                && item.location != r"C:\Program Files"
                && !item.location.contains(r"HKLM\SYSTEM")
                && item.location != r"HKCU\Software\Microsoft"
                && item.location != r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"
        }));
    }

    #[test]
    fn an_install_dir_stays_unchecked_while_the_exe_remains() {
        let footprint = AppFootprint {
            name: "Demo".to_owned(),
            publisher: String::new(),
            install_dir: Some(PathBuf::from(r"C:\Program Files\Demo")),
            target_exe: Some(PathBuf::from(r"C:\Program Files\Demo\demo.exe")),
            target_exe_present: true,
            target_url: None,
            source_file: None,
            uninstall_key: None,
        };
        let observed = ObservedLeftovers {
            directories: vec![PathBuf::from(r"C:\Program Files\Demo")],
            ..ObservedLeftovers::default()
        };
        let items = plan_leftovers(&footprint, &observed, &roots());
        assert_eq!(items.len(), 1);
        assert!(!items[0].checked);
    }

    #[test]
    fn cancel_does_not_scan_and_a_removed_entry_does() {
        let cancel = after_uninstall(&UninstallEnd::Cancelled, true);
        assert!(cancel.refresh);
        assert!(!cancel.scan);
        assert!(cancel.error.is_none());
        let gone = after_uninstall(&UninstallEnd::Finished { code: 0 }, true);
        assert!(gone.scan);
        assert!(gone.error.is_none());
        let stayed = after_uninstall(&UninstallEnd::Finished { code: 1 }, false);
        assert!(!stayed.scan);
        assert_eq!(stayed.error.as_deref(), Some("卸载没有完成"));
        let user_exit = after_uninstall(&UninstallEnd::Finished { code: 1602 }, true);
        assert!(!user_exit.scan);
        assert!(user_exit.error.is_none());
    }

    #[test]
    fn startup_shortcuts_and_plain_executables_are_not_deletable_entries() {
        let startup = entry(
            "Demo",
            AppSource::StartMenu,
            exe(r"C:\Program Files\Demo\demo.exe"),
            Some(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup\Demo.lnk"),
        );
        assert!(removal_plan(&startup, &[], &roots()).clean_path.is_none());
        let direct = entry(
            "Demo",
            AppSource::Path,
            exe(r"C:\Program Files\Demo\demo.exe"),
            Some(r"C:\Program Files\Demo\demo.exe"),
        );
        assert!(removal_plan(&direct, &[], &roots()).clean_path.is_none());
        assert!(!deletion_allowed(
            LeftoverKind::Directory,
            r"C:\Windows\System32",
            &roots()
        ));
        assert!(!registry_delete_allowed(r"HKLM\SYSTEM\Demo"));
        assert!(registry_delete_allowed(
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Demo"
        ));
    }

    #[test]
    fn uninstall_string_path_match_keeps_separators() {
        let item = entry(
            "My App",
            AppSource::StartMenu,
            exe(r"C:\Program Files\My\App.exe"),
            Some(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\My App.lnk"),
        );
        let sibling = record("MyApp", r#""C:\Program Files\MyApp.exe" /uninstall"#, "");
        assert!(
            removal_plan(&item, &[sibling], &roots())
                .uninstall
                .is_none()
        );

        let exact = record("Other", r#""C:\Program Files\My\App.exe" /uninstall"#, "");
        let command = removal_plan(&item, &[exact], &roots())
            .uninstall
            .expect("exact path")
            .command;
        assert_eq!(
            command,
            UninstallCommand::Process {
                file: r"C:\Program Files\My\App.exe".to_owned(),
                args: "/uninstall".to_owned(),
            }
        );
    }

    #[test]
    fn non_msi_uninstallers_drop_silent_switches() {
        let very = parse_uninstall_string(r#""C:\Program Files\App\unins000.exe" /VERYSILENT"#)
            .expect("verysilent");
        assert_eq!(very.file, r"C:\Program Files\App\unins000.exe");
        assert_eq!(very.args, "");

        let silent =
            parse_uninstall_string(r#""C:\Program Files\App\unins000.exe" /SILENT /allusers"#)
                .expect("silent");
        assert_eq!(silent.args, "/allusers");

        let nsis = parse_uninstall_string(r"C:\Program Files\App\uninstall.exe /S").expect("nsis");
        assert_eq!(nsis.file, r"C:\Program Files\App\uninstall.exe");
        assert_eq!(nsis.args, "");
    }

    #[test]
    fn exe_stem_folders_and_publisher_keys_stay_unchecked() {
        let footprint = AppFootprint {
            name: "Widget".to_owned(),
            publisher: "Widget".to_owned(),
            install_dir: Some(PathBuf::from(r"C:\Program Files\Widget")),
            target_exe: Some(PathBuf::from(r"C:\Program Files\Widget\player.exe")),
            target_exe_present: false,
            target_url: None,
            source_file: None,
            uninstall_key: Some(
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Widget".to_owned(),
            ),
        };
        let observed = ObservedLeftovers {
            directories: vec![
                PathBuf::from(r"C:\Program Files\Widget"),
                PathBuf::from(r"C:\Program Files\Player"),
                PathBuf::from(r"C:\ProgramData\Player"),
                PathBuf::from(r"C:\Users\User\AppData\Roaming\Player"),
                PathBuf::from(r"C:\ProgramData\Widget"),
            ],
            shortcuts: Vec::new(),
            registry_keys: vec![
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Widget".to_owned(),
                r"HKLM\SOFTWARE\Widget".to_owned(),
                r"HKCU\Software\Widget".to_owned(),
                r"HKLM\SOFTWARE\WOW6432Node\Widget".to_owned(),
            ],
        };
        let items = plan_leftovers(&footprint, &observed, &roots());
        let checked: Vec<_> = items
            .iter()
            .filter(|item| item.checked)
            .map(|item| item.location.as_str())
            .collect();
        assert!(
            checked
                .iter()
                .any(|path| path.ends_with(r"Program Files\Widget"))
        );
        assert!(
            checked
                .iter()
                .any(|path| path.ends_with(r"Uninstall\Widget"))
        );
        for path in [
            r"Program Files\Player",
            r"ProgramData\Player",
            r"Roaming\Player",
            r"ProgramData\Widget",
        ] {
            assert!(
                items
                    .iter()
                    .any(|item| item.location.ends_with(path) && !item.checked),
                "{path}"
            );
        }
        for key in [
            r"HKLM\SOFTWARE\Widget",
            r"HKCU\Software\Widget",
            r"HKLM\SOFTWARE\WOW6432Node\Widget",
        ] {
            assert!(
                items
                    .iter()
                    .any(|item| item.location == key && !item.checked),
                "{key}"
            );
        }
    }
}
