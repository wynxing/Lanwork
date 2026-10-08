//! 用 `IShellLinkW` 读出目标、参数、工作目录和图标。写快捷方式只给测试和测量用。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    GPS_DEFAULT, IPropertyStore, SHGetPropertyStoreFromParsingName,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{GUID, Interface};

use super::comutil::{self, expand_env, unquote};
use super::store::take_pwstr;
use crate::apps::{AppEntry, AppSource, LaunchTarget};

pub(crate) fn read_shortcut(path: &Path) -> Option<AppEntry> {
    comutil::ensure_com().ok()?;
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let wide = comutil::wide_path(path);
    let link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let persist: IPersistFile = link.cast().ok()?;
    // SAFETY: 路径在调用期间有效。只读打开，不解析、不弹界面。
    unsafe { persist.Load(comutil::pcwstr(&wide), STGM_READ) }.ok()?;

    let target = read_text(&link, LinkField::Path);
    let args = read_text(&link, LinkField::Arguments).trim().to_owned();
    let work = read_text(&link, LinkField::WorkingDirectory);
    let (icon_path, icon_index) = read_icon(&link);

    let working_directory = {
        let expanded = expand_env(work.trim());
        let trimmed = expanded.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    };

    let launch = if let Some(file) = existing_target(&target) {
        LaunchTarget::Path {
            path: file,
            args,
            working_directory,
        }
    } else {
        let aumid = app_user_model_id(&wide)?;
        LaunchTarget::Aumid { aumid }
    };

    Some(AppEntry {
        name,
        source: AppSource::StartMenu,
        target: launch,
        icon_path,
        icon_index,
    })
}

#[cfg(test)]
pub(crate) fn save_shortcut(
    path: &Path,
    target: &Path,
    args: &str,
    working_directory: Option<&Path>,
) -> Result<(), String> {
    comutil::ensure_com()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|err| format!("创建快捷方式失败: {err}"))?;
    let target_wide = comutil::wide_path(target);
    unsafe { link.SetPath(comutil::pcwstr(&target_wide)) }
        .map_err(|err| format!("写入快捷方式目标失败: {err}"))?;
    if !args.is_empty() {
        let args_wide = comutil::wide_null(args);
        unsafe { link.SetArguments(comutil::pcwstr(&args_wide)) }
            .map_err(|err| format!("写入快捷方式参数失败: {err}"))?;
    }
    let work_wide;
    if let Some(dir) = working_directory {
        work_wide = comutil::wide_path(dir);
        unsafe { link.SetWorkingDirectory(comutil::pcwstr(&work_wide)) }
            .map_err(|err| format!("写入快捷方式工作目录失败: {err}"))?;
    }
    let persist: IPersistFile = link
        .cast()
        .map_err(|err| format!("快捷方式不能保存: {err}"))?;
    let file_wide = comutil::wide_path(path);
    unsafe { persist.Save(comutil::pcwstr(&file_wide), true) }
        .map_err(|err| format!("保存快捷方式失败: {err}"))?;
    Ok(())
}

enum LinkField {
    Path,
    Arguments,
    WorkingDirectory,
}

fn read_text(link: &IShellLinkW, field: LinkField) -> String {
    let mut buf = vec![0u16; 32768];
    let result = match field {
        // SAFETY: 缓冲区由本函数持有。pfd 为空，不取 WIN32_FIND_DATA。
        LinkField::Path => unsafe { link.GetPath(&mut buf, std::ptr::null_mut(), 0) },
        LinkField::Arguments => unsafe { link.GetArguments(&mut buf) },
        LinkField::WorkingDirectory => unsafe { link.GetWorkingDirectory(&mut buf) },
    };
    if result.is_err() {
        return String::new();
    }
    comutil::wide_from_buf(&buf)
}

fn read_icon(link: &IShellLinkW) -> (Option<PathBuf>, i32) {
    let mut buf = vec![0u16; 32768];
    let mut index = 0i32;
    // SAFETY: 缓冲区和索引都由本函数持有。
    if unsafe { link.GetIconLocation(&mut buf, &mut index) }.is_err() {
        return (None, 0);
    }
    let text = expand_env(comutil::wide_from_buf(&buf).trim());
    let text = text.trim();
    if text.is_empty() {
        (None, index)
    } else {
        (Some(PathBuf::from(text)), index)
    }
}

fn existing_target(raw: &str) -> Option<PathBuf> {
    let text = unquote(raw);
    if text.is_empty() {
        return None;
    }
    let expanded = expand_env(text);
    let path = PathBuf::from(expanded.trim());
    if comutil::is_existing_file(&path) {
        Some(path)
    } else {
        None
    }
}

fn app_user_model_id(lnk: &[u16]) -> Option<String> {
    // SAFETY: lnk 以 0 结尾。只读属性，不改快捷方式。读完释放 PROPVARIANT。
    let store: IPropertyStore =
        unsafe { SHGetPropertyStoreFromParsingName(comutil::pcwstr(lnk), None, GPS_DEFAULT) }
            .ok()?;
    let mut value = unsafe { store.GetValue(&PKEY_APP_USER_MODEL_ID) }.ok()?;
    let text = unsafe { PropVariantToStringAlloc(&value) }.ok();
    unsafe {
        let _ = PropVariantClear(&mut value);
    }
    let owned = take_pwstr(text?);
    let trimmed = owned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// `PKEY_AppUserModel_ID`。不额外打开 EnhancedStorage 模块。
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
