//! 把文本或文件放进系统剪贴板。
//!
//! 「复制文件」用资源管理器「复制」的三份数据：`CF_HDROP`、`Preferred DropEffect`
//!（`DROPEFFECT_COPY`）和路径的 `CF_UNICODETEXT`。字节由 `lanwork_core::shell` 排好。

use lanwork_core::shell::{
    PREFERRED_DROP_EFFECT_FORMAT, explorer_file_copy, unicode_clipboard_text,
};
use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};

use crate::winutil::{pcwstr, wide};

/// 把一段文本放进剪贴板，格式是 `CF_UNICODETEXT`。
pub(crate) fn copy_text(text: &str) -> Result<(), String> {
    let bytes = unicode_clipboard_text(text).ok_or_else(|| "复制失败：文本无效".to_owned())?;
    write_clipboard(&[(u32::from(CF_UNICODETEXT.0), &bytes)])
}

/// 按资源管理器「复制」的格式放入这一条路径。粘贴到资源管理器会复制文件或文件夹。
pub(crate) fn copy_explorer_file(path: &str) -> Result<(), String> {
    let copy = explorer_file_copy(path).ok_or_else(|| "复制失败：路径无效".to_owned())?;
    let effect = register_drop_effect()?;
    write_clipboard(&[
        (u32::from(CF_HDROP.0), &copy.hdrop),
        (effect, &copy.preferred_drop_effect),
        (u32::from(CF_UNICODETEXT.0), &copy.unicode_text),
    ])
}

fn register_drop_effect() -> Result<u32, String> {
    let wide = wide(PREFERRED_DROP_EFFECT_FORMAT);
    let id = unsafe { RegisterClipboardFormatW(pcwstr(&wide)) };
    if id == 0 {
        Err("复制失败：剪贴板格式没有注册上".to_owned())
    } else {
        Ok(id)
    }
}

fn write_clipboard(items: &[(u32, &[u8])]) -> Result<(), String> {
    unsafe { OpenClipboard(None) }.map_err(|err| format!("复制失败：{err}"))?;
    let cleared = unsafe { EmptyClipboard() };
    if let Err(err) = cleared {
        unsafe {
            let _ = CloseClipboard();
        }
        return Err(format!("复制失败：{err}"));
    }
    for (format, bytes) in items {
        if let Err(err) = set_global(*format, bytes) {
            unsafe {
                let _ = CloseClipboard();
            }
            return Err(err);
        }
    }
    unsafe { CloseClipboard() }.map_err(|err| format!("复制失败：{err}"))
}

fn set_global(format: u32, bytes: &[u8]) -> Result<(), String> {
    // SAFETY: 长度是这份字节的大小。失败时由调用方释放；成功后剪贴板接管。
    let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
        .map_err(|err| format!("复制失败：{err}"))?;
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        unsafe {
            let _ = GlobalFree(Some(global));
        }
        return Err("复制失败：剪贴板内存没有锁上".to_owned());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
        let _ = GlobalUnlock(global);
        if let Err(err) = SetClipboardData(format, Some(HANDLE(global.0))) {
            let _ = GlobalFree(Some(global));
            return Err(format!("复制失败：{err}"));
        }
    }
    Ok(())
}
