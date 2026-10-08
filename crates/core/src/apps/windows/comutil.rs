//! COM 初始化和路径文本。不创建窗口。

use std::cell::Cell;
use std::path::Path;

use windows::Win32::Foundation::GetLastError;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::core::PCWSTR;

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn ensure_com() -> Result<(), String> {
    let mut error = None;
    COM_READY.with(|ready| {
        if ready.get() {
            return;
        }
        // SAFETY: 保留参数是空的。成功或 S_FALSE 都表示这个线程可以继续用 STA。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() {
            error = Some(format!("COM 初始化失败: {hr:?}"));
            return;
        }
        ready.set(true);
    });
    match error {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

pub(crate) fn wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(crate) fn pcwstr(buf: &[u16]) -> PCWSTR {
    PCWSTR::from_raw(buf.as_ptr())
}

pub(crate) fn wide_from_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub(crate) fn expand_env(input: &str) -> String {
    if !input.contains('%') {
        return input.to_owned();
    }
    let src = wide_null(input);
    let mut buf = vec![0u16; src.len().saturating_add(32).max(64)];
    loop {
        // SAFETY: 两个缓冲区都以 0 结尾，且在调用期间有效。
        let written = unsafe { ExpandEnvironmentStringsW(pcwstr(&src), Some(&mut buf)) };
        if written == 0 {
            return input.to_owned();
        }
        let needed = written as usize;
        if needed <= buf.len() {
            return wide_from_buf(&buf[..needed]);
        }
        buf.resize(needed, 0);
    }
}

pub(crate) fn unquote(text: &str) -> &str {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('"')
        && let Some((inside, _)) = rest.split_once('"')
    {
        return inside;
    }
    text
}

pub(crate) fn is_existing_file(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    let wide = wide_path(path);
    // SAFETY: 路径缓冲区以 0 结尾。失败时用 GetLastError 区分，属性值本身只用来判断文件。
    let attributes = unsafe { GetFileAttributesW(pcwstr(&wide)) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        let _ = unsafe { GetLastError() };
        return false;
    }
    attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
}
