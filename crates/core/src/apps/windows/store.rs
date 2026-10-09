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
        let step = match next {
            Err(err) => Err(format!("商店应用枚举中断: {err}")),
            Ok(()) if fetched == 0 => Ok(ShellNext::End),
            Ok(()) => match items[0].take().and_then(|item| read_item(&item)) {
                Some(entry) => Ok(ShellNext::Item(entry)),
                None => continue,
            },
        };
        if !push_store_item(&mut entries, step)? {
            break;
        }
    }
    Ok(entries)
}

/// `Next` 的一次结果。中途的错误必须整次失败，不能把已经拿到的条目当成完整结果。
enum ShellNext<T> {
    Item(T),
    End,
}

/// 返回 `Ok(false)` 表示枚举结束。`Err` 时调用方丢掉 `entries`。
fn push_store_item<T>(
    entries: &mut Vec<T>,
    step: Result<ShellNext<T>, String>,
) -> Result<bool, String> {
    match step? {
        ShellNext::End => Ok(false),
        ShellNext::Item(item) => {
            entries.push(item);
            Ok(true)
        }
    }
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
        alternate_names: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::{ShellNext, push_store_item};
    use crate::apps::model::{SourceAttempt, SourceSnapshots, apply_source_results};
    use crate::apps::{AppEntry, AppSource, LaunchTarget};

    fn entry(name: &str) -> AppEntry {
        AppEntry {
            name: name.to_owned(),
            source: AppSource::Store,
            target: LaunchTarget::Aumid {
                aumid: format!("Test.{name}_8wekyb3d8bbwe!App"),
            },
            icon_path: None,
            icon_index: 0,
            alternate_names: Vec::new(),
        }
    }

    #[test]
    fn mid_enumeration_error_keeps_the_previous_store_snapshot() {
        let previous = entry("上次");
        let mut partial = Vec::new();
        push_store_item(&mut partial, Ok(ShellNext::Item(entry("半次")))).unwrap();
        let failed = push_store_item::<AppEntry>(&mut partial, Err("商店应用枚举中断".to_owned()));
        assert!(failed.is_err());
        let mut snapshots = SourceSnapshots::from_entries(std::slice::from_ref(&previous));
        let outcome = apply_source_results(
            &mut snapshots,
            &[SourceAttempt {
                source: AppSource::Store,
                result: Err(failed.unwrap_err()),
            }],
        );
        assert_eq!(snapshots.store, vec![previous]);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].source, AppSource::Store);
        assert!(outcome.entries.iter().any(|item| item.name == "上次"));
        assert!(outcome.entries.iter().all(|item| item.name != "半次"));
    }
}
