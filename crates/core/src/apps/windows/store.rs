//! `shell:AppsFolder` 里的 AUMID。显示名用 Shell 的普通显示名。

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::{PWSTR, w};

use super::comutil;
use crate::apps::{AppEntry, AppSource, LaunchTarget};

pub(crate) fn read_store() -> Result<Vec<AppEntry>, String> {
    comutil::ensure_com()?;
    // SAFETY: 解析名是常量。绑定枚举器后逐项读取显示名，字符串由 CoTaskMemFree 释放。
    let root: IShellItem = unsafe { SHCreateItemFromParsingName(w!("shell:AppsFolder"), None) }
        .map_err(|err| format!("商店应用文件夹打不开: {err}"))?;
    let enumerator: IEnumShellItems = unsafe { root.BindToHandler(None, &BHID_EnumItems) }
        .map_err(|err| format!("商店应用枚举失败: {err}"))?;
    let mut entries = Vec::new();
    loop {
        let mut fetched = 0u32;
        let mut items = [None];
        let next = unsafe { enumerator.Next(&mut items, Some(&mut fetched)) };
        if next.is_err() || fetched == 0 {
            break;
        }
        let Some(item) = items[0].take() else {
            continue;
        };
        let Some(entry) = read_item(&item) else {
            continue;
        };
        entries.push(entry);
    }
    Ok(entries)
}

fn read_item(item: &IShellItem) -> Option<AppEntry> {
    let aumid = display_name(item, SIGDN_PARENTRELATIVEPARSING)?;
    let aumid = aumid.trim().to_owned();
    if aumid.is_empty() {
        return None;
    }
    let name = display_name(item, SIGDN_NORMALDISPLAY)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| aumid.clone());
    Some(AppEntry {
        name,
        source: AppSource::Store,
        target: LaunchTarget::Aumid { aumid },
        icon_path: None,
        icon_index: 0,
    })
}

fn display_name(item: &IShellItem, kind: windows::Win32::UI::Shell::SIGDN) -> Option<String> {
    let raw = unsafe { item.GetDisplayName(kind) }.ok()?;
    let text = take_pwstr(raw);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(crate) fn take_pwstr(ptr: PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let text = unsafe { ptr.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(ptr.0.cast())) };
    text
}
