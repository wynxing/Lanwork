//! `HKCU` 和 `HKLM` 的 App Paths。64 位进程另外读 `WOW6432Node`，那是 32 位注册表里的同一项。

use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::core::PWSTR;

use super::comutil::{self, expand_env, unquote};
use crate::apps::{AppEntry, AppSource, LaunchTarget};

const ROOTS: &[(HKEY, &str)] = &[
    (
        HKEY_CURRENT_USER,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths",
    ),
    (
        HKEY_LOCAL_MACHINE,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths",
    ),
    (
        HKEY_LOCAL_MACHINE,
        r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths",
    ),
];

pub(crate) fn read_app_paths() -> Result<Vec<AppEntry>, String> {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut opened = 0u32;
    for (hive, subkey) in ROOTS {
        match read_hive(*hive, subkey) {
            Ok(list) => {
                opened += 1;
                entries.extend(list);
            }
            Err(HiveError::Missing) => {}
            Err(HiveError::Failed(message)) => errors.push(message),
        }
    }
    if opened == 0 && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(entries)
}

enum HiveError {
    Missing,
    Failed(String),
}

fn read_hive(hive: HKEY, subkey: &str) -> Result<Vec<AppEntry>, HiveError> {
    let wide = comutil::wide_null(subkey);
    let mut raw = HKEY::default();
    // SAFETY: 子键路径以 0 结尾。KEY_READ 只读。
    let status = unsafe { RegOpenKeyExW(hive, comutil::pcwstr(&wide), None, KEY_READ, &mut raw) };
    if status == ERROR_FILE_NOT_FOUND {
        return Err(HiveError::Missing);
    }
    if status != ERROR_SUCCESS {
        return Err(HiveError::Failed(format!("{subkey} 打不开: {status:?}")));
    }
    let key = OpenKey(raw);
    let mut entries = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 256];
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
            return Err(HiveError::Failed(format!("{subkey} 枚举失败: {status:?}")));
        }
        index = index.saturating_add(1);
        let child = comutil::wide_from_buf(&name[..name_len as usize]);
        if let Some(entry) = read_default(&key, &child) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn read_default(key: &OpenKey, child: &str) -> Option<AppEntry> {
    let wide = comutil::wide_null(child);
    let mut bytes = vec![0u8; 4096];
    let mut size = bytes.len() as u32;
    // SAFETY: 默认值名是空指针。缓冲区大小按字节传入，只接受字符串类型。
    let status = unsafe {
        RegGetValueW(
            key.0,
            comutil::pcwstr(&wide),
            PCWSTR_NULL,
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let chars = (size as usize / 2).saturating_sub(1);
    let units = bytes[..chars.saturating_mul(2)]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    let raw = String::from_utf16_lossy(&units);
    let text = unquote(raw.trim());
    if text.is_empty() {
        return None;
    }
    let expanded = expand_env(text);
    let file = PathBuf::from(expanded.trim());
    if !comutil::is_existing_file(&file) {
        return None;
    }
    let name = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| child.trim_end_matches(".exe").to_owned());
    let target_name = file
        .file_name()
        .map(|file_name| file_name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if crate::apps::rules::is_uninstaller(&name, &target_name, "") {
        return None;
    }
    Some(AppEntry {
        name,
        source: AppSource::AppPaths,
        target: LaunchTarget::Path {
            path: file,
            args: String::new(),
            working_directory: None,
        },
        icon_path: None,
        icon_index: 0,
        alternate_names: Vec::new(),
    })
}

struct OpenKey(HKEY);

impl Drop for OpenKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// 读默认值时值名为空。
const PCWSTR_NULL: windows::core::PCWSTR = windows::core::PCWSTR::null();
