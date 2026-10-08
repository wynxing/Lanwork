//! 进程内的 Win32 小工具：错误、单调时钟、Private Bytes、COM 初始化。

use std::ffi::OsStr;
use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::PCWSTR;

#[derive(Debug)]
pub struct FileIdxError {
    pub message: String,
    pub hresult: Option<u32>,
}

impl FileIdxError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hresult: None,
        }
    }

    pub fn with_hresult(message: impl Into<String>, hresult: u32) -> Self {
        Self {
            message: message.into(),
            hresult: Some(hresult),
        }
    }
}

impl fmt::Display for FileIdxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.hresult {
            Some(code) => write!(f, "{} (HRESULT 0x{code:08X})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for FileIdxError {}

impl From<windows::core::Error> for FileIdxError {
    fn from(err: windows::core::Error) -> Self {
        let code = err.code().0 as u32;
        Self::with_hresult(err.message(), code)
    }
}

impl From<std::io::Error> for FileIdxError {
    fn from(err: std::io::Error) -> Self {
        Self::new(err.to_string())
    }
}

pub fn mono_ns() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    let elapsed = BASE.get_or_init(Instant::now).elapsed().as_nanos();
    u64::try_from(elapsed).unwrap_or(u64::MAX)
}

/// `PROCESS_MEMORY_COUNTERS_EX.PrivateUsage`，和 `lanwork-sample` 的 `private_bytes` 是同一个字段。
pub fn private_bytes() -> Result<u64, FileIdxError> {
    let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
    let bytes = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>())
        .map_err(|_| FileIdxError::new("内存计数结构过大"))?;
    memory.cb = bytes;
    // SAFETY: 结构体的 cb 已设成自身大小，句柄是当前进程的伪句柄。
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut memory as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
            bytes,
        )
    }
    .map_err(|err| FileIdxError::new(format!("读 Private Bytes 失败：{err}")))?;
    Ok(memory.PrivateUsage as u64)
}

pub struct ComInit;

impl ComInit {
    pub fn new() -> Result<Self, FileIdxError> {
        // SAFETY: 本进程在查询线程上只初始化一次 STA。没有别的线程调用 COM。
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(FileIdxError::from)?;
        Ok(Self)
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        // SAFETY: 与 `new` 成对。
        unsafe { CoUninitialize() };
    }
}

pub fn wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(wide: &[u16]) -> PCWSTR {
    PCWSTR(wide.as_ptr())
}

pub fn wide_path(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn string_from_wide_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn filetime_u64(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}
