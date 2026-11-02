//! 读卸载登记、扫描残留、启动官方卸载和删除勾选的项。
//!
//! 能不能删由 `removal` 里的纯函数决定。这里只做本机读写。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, RRF_RT_REG_DWORD,
    RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteTreeW, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartMenu, FOLDERID_CommonStartup, FOLDERID_Desktop, FOLDERID_Documents,
    FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_ProgramData, FOLDERID_ProgramFiles,
    FOLDERID_ProgramFilesX86, FOLDERID_PublicDesktop, FOLDERID_RoamingAppData, FOLDERID_StartMenu,
    FOLDERID_Startup, FOLDERID_Windows, KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath,
};
use windows::core::PWSTR;

use super::comutil::{self, expand_env, unquote};
use super::launch::{self, LaunchError};
use super::shortcut::peek_target;
use super::store::take_pwstr;
use crate::apps::removal::{
    AppFootprint, LeftoverKind, MachineRoots, ObservedLeftovers, ObservedShortcut,
    UninstallCommand, UninstallEnd, UninstallRecord, deletion_allowed, registry_candidates,
};

const UNINSTALL_ROOTS: &[(HKEY, &str, &str)] = &[
    (
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
    (
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
    (
        HKEY_CURRENT_USER,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
];

const WALK_DEPTH: u32 = 8;
const WALK_CAP: usize = 2000;

/// 本机已知文件夹。读不到的项用内置路径。结果缓存一次。
pub fn machine_roots() -> MachineRoots {
    cached_roots().clone()
}

fn cached_roots() -> &'static MachineRoots {
    static ROOTS: OnceLock<MachineRoots> = OnceLock::new();
    ROOTS.get_or_init(read_roots)
}

fn read_roots() -> MachineRoots {
    let builtin = MachineRoots::builtin();
    let mut roots = MachineRoots {
        windows: folder(&FOLDERID_Windows).unwrap_or(builtin.windows),
        program_files: folder(&FOLDERID_ProgramFiles).unwrap_or(builtin.program_files),
        program_files_x86: folder(&FOLDERID_ProgramFilesX86).unwrap_or(builtin.program_files_x86),
        program_data: folder(&FOLDERID_ProgramData).unwrap_or(builtin.program_data),
        user_profile: folder(&FOLDERID_Profile).unwrap_or(builtin.user_profile),
        app_data: folder(&FOLDERID_RoamingAppData).unwrap_or(builtin.app_data),
        local_app_data: folder(&FOLDERID_LocalAppData).unwrap_or(builtin.local_app_data),
        desktop: folder(&FOLDERID_Desktop).unwrap_or(builtin.desktop),
        public_desktop: folder(&FOLDERID_PublicDesktop).unwrap_or(builtin.public_desktop),
        documents: folder(&FOLDERID_Documents).unwrap_or(builtin.documents),
        start_menu: Vec::new(),
        startup: Vec::new(),
    };
    for id in [FOLDERID_StartMenu, FOLDERID_CommonStartMenu] {
        if let Some(path) = folder(&id) {
            roots.start_menu.push(path);
        }
    }
    if roots.start_menu.is_empty() {
        roots.start_menu = builtin.start_menu;
    }
    for id in [FOLDERID_Startup, FOLDERID_CommonStartup] {
        if let Some(path) = folder(&id) {
            roots.startup.push(path);
        }
    }
    if roots.startup.is_empty() {
        roots.startup = builtin.startup;
    }
    if let Ok(extra) = std::env::var(crate::apps::EXTRA_SHORTCUT_DIR_ENV) {
        let path = PathBuf::from(extra.trim());
        if path.is_dir() {
            roots.start_menu.push(path);
        }
    }
    roots
}

fn folder(id: &windows::core::GUID) -> Option<PathBuf> {
    // SAFETY: 已知文件夹 ID 来自本模块的常量。DONT_VERIFY 只取路径，不创建目录。
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, None) }.ok()?;
    let text = take_pwstr(raw);
    if text.is_empty() {
        None
    } else {
        Some(PathBuf::from(text))
    }
}

/// 三处卸载登记。一处都打不开时返回错误，调用方按空登记继续。
pub fn read_uninstall_records() -> Result<Vec<UninstallRecord>, String> {
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let mut opened = 0u32;
    for (hive, subkey, label) in UNINSTALL_ROOTS {
        match read_hive(*hive, subkey, label) {
            Ok(list) => {
                opened += 1;
                records.extend(list);
            }
            Err(HiveError::Missing) => {}
            Err(HiveError::Failed(message)) => errors.push(message),
        }
    }
    if opened == 0 && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(records)
}

enum HiveError {
    Missing,
    Failed(String),
}

fn read_hive(hive: HKEY, subkey: &str, label: &str) -> Result<Vec<UninstallRecord>, HiveError> {
    let key = open_key(hive, subkey).map_err(|missing| {
        if missing {
            HiveError::Missing
        } else {
            HiveError::Failed(format!("{label} 打不开"))
        }
    })?;
    let mut records = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut name_len = (name.len() - 1) as u32;
        // SAFETY: name 缓冲区和长度由本函数持有。
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                Some(PWSTR::from_raw(name.as_mut_ptr())),
                &mut name_len,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            return Err(HiveError::Failed(format!("{label} 枚举失败: {status:?}")));
        }
        index = index.saturating_add(1);
        let child = comutil::wide_from_buf(&name[..name_len as usize]);
        if child.is_empty() {
            continue;
        }
        records.push(read_record(&key, label, &child));
    }
    Ok(records)
}

fn read_record(key: &OpenKey, label: &str, child: &str) -> UninstallRecord {
    UninstallRecord {
        key: format!(r"{label}\{child}"),
        display_name: read_text(key, child, "DisplayName"),
        uninstall_string: read_text(key, child, "UninstallString"),
        install_location: read_text(key, child, "InstallLocation"),
        display_icon: read_text(key, child, "DisplayIcon"),
        publisher: read_text(key, child, "Publisher"),
        system_component: read_dword(key, child, "SystemComponent") != 0,
        windows_installer: read_dword(key, child, "WindowsInstaller") != 0,
    }
}

fn read_text(key: &OpenKey, child: &str, value: &str) -> String {
    let child_wide = comutil::wide_null(child);
    let value_wide = comutil::wide_null(value);
    let mut bytes = vec![0u8; 4096];
    let mut size = bytes.len() as u32;
    // SAFETY: 子键和值名以 0 结尾。缓冲区大小按字节传入，只接受字符串类型。
    let status = unsafe {
        RegGetValueW(
            key.0,
            comutil::pcwstr(&child_wide),
            comutil::pcwstr(&value_wide),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return String::new();
    }
    let chars = (size as usize / 2).saturating_sub(1);
    let units = bytes[..chars.saturating_mul(2)]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    let raw = String::from_utf16_lossy(&units);
    expand_env(unquote(raw.trim()).trim()).trim().to_owned()
}

fn read_dword(key: &OpenKey, child: &str, value: &str) -> u32 {
    let child_wide = comutil::wide_null(child);
    let value_wide = comutil::wide_null(value);
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: 子键和值名以 0 结尾。缓冲区是 4 字节的 DWORD。
    let status = unsafe {
        RegGetValueW(
            key.0,
            comutil::pcwstr(&child_wide),
            comutil::pcwstr(&value_wide),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_SUCCESS { data } else { 0 }
}

/// 启动官方卸载并等到进程结束。
pub fn launch_uninstall(command: &UninstallCommand) -> UninstallEnd {
    let (file, args) = match command {
        UninstallCommand::Process { file, args } => (file.clone(), args.clone()),
        UninstallCommand::Store { family_name } => (
            "powershell.exe".to_owned(),
            format!(
                "-NoProfile -Command \"Get-AppxPackage | Where-Object {{ $_.PackageFamilyName -eq '{family_name}' }} | Remove-AppxPackage\""
            ),
        ),
    };
    match launch::run_and_wait(&file, &args) {
        Ok(code) => UninstallEnd::Finished { code },
        Err(err) if err.cancelled => UninstallEnd::Cancelled,
        Err(LaunchError { message, .. }) => UninstallEnd::Failed(message),
    }
}

/// 按足迹收集候选。关键目录只作为边界，不往里面走。
pub fn observe_leftovers(footprint: &AppFootprint, roots: &MachineRoots) -> ObservedLeftovers {
    let mut observed = ObservedLeftovers::default();
    if let Some(dir) = &footprint.install_dir {
        push_dir(&mut observed.directories, dir);
    }
    if let Some(exe) = &footprint.target_exe
        && let Some(parent) = exe.parent()
    {
        push_dir(&mut observed.directories, parent);
    }
    let bases = [
        roots.app_data.clone(),
        roots.local_app_data.clone(),
        roots.documents.clone(),
        roots.program_files.clone(),
        roots.program_files_x86.clone(),
        roots.program_data.clone(),
        roots.desktop.clone(),
        roots.user_profile.clone(),
    ];
    for base in &bases {
        scan_children(base, &mut observed.directories);
    }
    for key in registry_candidates(footprint) {
        if let Some((hive, subkey)) = split_hive(&key)
            && key_exists(hive, &subkey)
        {
            observed.registry_keys.push(key);
        }
    }
    let mut homes = roots.start_menu.clone();
    homes.push(roots.desktop.clone());
    homes.push(roots.public_desktop.clone());
    for home in homes {
        walk_shortcuts(&home, 0, roots, &mut observed.shortcuts);
    }
    observed
}

fn push_dir(out: &mut Vec<PathBuf>, path: &Path) {
    if path.is_dir() {
        out.push(path.to_path_buf());
    }
}

fn scan_children(parent: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for item in entries.flatten() {
        let path = item.path();
        if path.is_dir() {
            out.push(path);
        }
    }
}

fn walk_shortcuts(dir: &Path, depth: u32, roots: &MachineRoots, out: &mut Vec<ObservedShortcut>) {
    if depth > WALK_DEPTH || out.len() >= WALK_CAP || under_windows(dir, roots) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for item in entries.flatten() {
        if out.len() >= WALK_CAP {
            return;
        }
        let path = item.path();
        if path.is_dir() {
            walk_shortcuts(&path, depth.saturating_add(1), roots, out);
            continue;
        }
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if !ext.eq_ignore_ascii_case("lnk") && !ext.eq_ignore_ascii_case("url") {
            continue;
        }
        let (target, url) = peek_target(&path);
        out.push(ObservedShortcut { path, target, url });
    }
}

fn under_windows(path: &Path, roots: &MachineRoots) -> bool {
    let key = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let windows = roots
        .windows
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();
    key == windows || key.starts_with(&(windows + "\\"))
}

/// 再查一次允许名单，然后删除。已经不在时算成功。
pub fn delete_leftover(
    kind: LeftoverKind,
    location: &str,
    roots: &MachineRoots,
) -> Result<(), String> {
    if !deletion_allowed(kind, location, roots) {
        return Err("这个位置不能删除".to_owned());
    }
    match kind {
        LeftoverKind::Registry => delete_key(location),
        LeftoverKind::Shortcut => remove_file(Path::new(location)),
        LeftoverKind::Directory => remove_dir(Path::new(location)),
    }
}

fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn remove_dir(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn delete_key(location: &str) -> Result<(), String> {
    let Some((hive, subkey)) = split_hive(location) else {
        return Err("这个注册表键不能删除".to_owned());
    };
    let wide = comutil::wide_null(&subkey);
    // SAFETY: 子键以 0 结尾，并且已经通过 deletion_allowed。
    let status = unsafe { RegDeleteTreeW(hive, comutil::pcwstr(&wide)) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("注册表删除失败: {status:?}"))
    }
}

fn key_exists(hive: HKEY, subkey: &str) -> bool {
    open_key(hive, subkey).is_ok()
}

fn split_hive(key: &str) -> Option<(HKEY, String)> {
    let key = key.trim().trim_start_matches('\\');
    if let Some(rest) = key.strip_prefix(r"HKCU\") {
        Some((HKEY_CURRENT_USER, rest.to_owned()))
    } else {
        key.strip_prefix(r"HKLM\")
            .map(|rest| (HKEY_LOCAL_MACHINE, rest.to_owned()))
    }
}

fn open_key(hive: HKEY, subkey: &str) -> Result<OpenKey, bool> {
    let wide = comutil::wide_null(subkey);
    let mut raw = HKEY::default();
    // SAFETY: 子键以 0 结尾。KEY_READ 只读，并固定读 64 位视图。
    let status = unsafe {
        RegOpenKeyExW(
            hive,
            comutil::pcwstr(&wide),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut raw,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Err(true);
    }
    if status != ERROR_SUCCESS {
        return Err(false);
    }
    Ok(OpenKey(raw))
}

struct OpenKey(HKEY);

impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: 这个句柄由 RegOpenKeyExW 打开，只关闭一次。
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
