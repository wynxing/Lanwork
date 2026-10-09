//! 开机启动和系统主题。只写当前用户，不写 HKLM。

use std::path::Path;

use lanwork_core::shell::{
    RunValueAction, SystemLight, quoted_executable, run_value_action, system_light_from_dword,
};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW,
    RegSetValueExW,
};
use windows::core::PCWSTR;

use crate::winutil::{from_wide, pcwstr, wide};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Lanwork";
const THEME_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const THEME_VALUE: &str = "AppsUseLightTheme";

pub(crate) fn read_system_theme() -> SystemLight {
    match read_dword(THEME_KEY, THEME_VALUE) {
        Ok(value) => system_light_from_dword(value),
        Err(_) => SystemLight::Unknown,
    }
}

pub(crate) fn apply_startup(enabled: bool, executable: &Path) -> Result<(), String> {
    let existing = read_sz(RUN_KEY, RUN_VALUE)?;
    if !enabled {
        return match run_value_action(false, "", existing.as_deref()) {
            RunValueAction::Delete => delete_value(RUN_KEY, RUN_VALUE),
            RunValueAction::Unchanged | RunValueAction::Write(_) => Ok(()),
        };
    }
    let desired = quoted_executable(executable).ok_or("可执行文件路径无法写入开机启动项")?;
    match run_value_action(true, &desired, existing.as_deref()) {
        RunValueAction::Unchanged => Ok(()),
        RunValueAction::Write(value) => write_sz(RUN_KEY, RUN_VALUE, &value),
        RunValueAction::Delete => delete_value(RUN_KEY, RUN_VALUE),
    }
}

fn read_dword(subkey: &str, name: &str) -> Result<Option<u32>, String> {
    let subkey = wide(subkey);
    let name = wide(name);
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: 子键和值名以 0 结尾。缓冲区长度是 4。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&subkey),
            pcwstr(&name),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(format!("注册表读取失败: {status:?}"));
    }
    Ok(Some(data))
}

fn read_sz(subkey: &str, name: &str) -> Result<Option<String>, String> {
    let subkey = wide(subkey);
    let name = wide(name);
    let mut size = 0u32;
    // SAFETY: 先问长度。数据指针为空。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&subkey),
            pcwstr(&name),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS || size < 2 {
        return Err(format!("注册表读取失败: {status:?}"));
    }
    let mut buf = vec![0u16; (size as usize) / 2 + 1];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: buf 可写，size 是字节数。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&subkey),
            pcwstr(&name),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(format!("注册表读取失败: {status:?}"));
    }
    Ok(Some(from_wide(&buf)))
}

fn write_sz(subkey: &str, name: &str, value: &str) -> Result<(), String> {
    let key = create_key(subkey)?;
    let name = wide(name);
    let mut data = wide(value);
    // SAFETY: data 活过这次调用。指针指向这块缓冲区，长度是它的字节数，含结尾的 0。
    let bytes =
        unsafe { std::slice::from_raw_parts(data.as_mut_ptr().cast::<u8>(), data.len() * 2) };
    // SAFETY: 值名和数据在调用期间有效，数据含结尾 0。
    let status = unsafe { RegSetValueExW(key.0, pcwstr(&name), None, REG_SZ, Some(bytes)) };
    if status != ERROR_SUCCESS {
        return Err(format!("注册表写入失败: {status:?}"));
    }
    Ok(())
}

fn delete_value(subkey: &str, name: &str) -> Result<(), String> {
    let key = create_key(subkey)?;
    let name = wide(name);
    // SAFETY: 值名以 0 结尾。不存在时返回 FILE_NOT_FOUND，视为已经删掉。
    let status = unsafe { RegDeleteValueW(key.0, pcwstr(&name)) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("注册表删除失败: {status:?}"))
    }
}

struct OpenKey(HKEY);

impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: 句柄由本结构独占。
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(subkey: &str) -> Result<OpenKey, String> {
    let subkey = wide(subkey);
    let mut raw = HKEY::default();
    // SAFETY: 子键以 0 结尾。只打开当前用户，权限是读和写这个项的值。
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            pcwstr(&subkey),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            None,
            &mut raw,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("注册表项打开失败: {status:?}"));
    }
    Ok(OpenKey(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personalize_value_is_read_without_guessing_a_missing_key() {
        let theme = read_system_theme();
        let _ = theme;
        let missing = read_dword(r"Software\Lanwork\MissingShellKey", "AppsUseLightTheme");
        assert_eq!(missing.unwrap(), None);
    }
}
