use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::{Write, stdout};
use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, S_OK, WIN32_ERROR};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::core::{Error, PCWSTR};

#[derive(Debug)]
pub struct SpikeError {
    pub message: String,
}

impl SpikeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SpikeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SpikeError {}

pub type SpikeResult<T> = Result<T, SpikeError>;

pub fn win_err(context: &str, err: Error) -> SpikeError {
    SpikeError::new(format!("{context}: {err} (0x{:08X})", err.code().0 as u32))
}

pub fn win32_to_result(context: &str, error: WIN32_ERROR) -> SpikeResult<()> {
    if error == ERROR_SUCCESS {
        Ok(())
    } else if error == ERROR_FILE_NOT_FOUND {
        Err(SpikeError::new(format!("{context}: 找不到（Win32 2）")))
    } else {
        Err(SpikeError::new(format!("{context}: Win32 {}", error.0)))
    }
}

pub fn win32_missing_ok(error: WIN32_ERROR) -> SpikeResult<bool> {
    if error == ERROR_SUCCESS {
        Ok(true)
    } else if error == ERROR_FILE_NOT_FOUND {
        Ok(false)
    } else {
        Err(SpikeError::new(format!(
            "读取注册表失败: Win32 {}",
            error.0
        )))
    }
}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(buf: &[u16]) -> PCWSTR {
    PCWSTR(buf.as_ptr())
}

pub fn string_from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn string_from_pcwstr(value: PCWSTR) -> String {
    if value.0.is_null() {
        return String::new();
    }
    unsafe { value.to_string() }.unwrap_or_default()
}

pub fn write_utf16_field(dest: &mut [u16], text: &str) {
    dest.fill(0);
    for (index, unit) in text.encode_utf16().enumerate() {
        if index + 1 >= dest.len() {
            break;
        }
        dest[index] = unit;
    }
}

pub struct ComApartment {
    // S_OK 才是这次调用初始化的。S_FALSE 表示线程上已经有套间，不能再 CoUninitialize。
    uninit: bool,
}

impl ComApartment {
    pub fn new() -> SpikeResult<Self> {
        Self::enter(COINIT_APARTMENTTHREADED, "STA")
    }

    pub fn mta() -> SpikeResult<Self> {
        Self::enter(COINIT_MULTITHREADED, "MTA")
    }

    fn enter(model: windows::Win32::System::Com::COINIT, label: &str) -> SpikeResult<Self> {
        let hr = unsafe { CoInitializeEx(None, model) };
        log_line(&format!("CoInitializeEx {label} hr=0x{:08X}", hr.0 as u32));
        if hr.is_ok() {
            Ok(Self { uninit: hr == S_OK })
        } else {
            Err(win_err("CoInitializeEx", Error::from(hr)))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() };
            log_line("CoUninitialize");
        }
    }
}

pub fn enable_utf8_console() {
    unsafe {
        let _ = SetConsoleOutputCP(65001);
        let _ = SetConsoleCP(65001);
    }
}

pub fn log_path() -> std::path::PathBuf {
    std::env::temp_dir().join("lanwork-spike-toast.log")
}

pub fn log_line(message: &str) {
    let stamp = local_stamp();
    let line = format!("{stamp} {message}");
    println!("{line}");
    let _ = stdout().flush();
    let path = log_path();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

fn local_stamp() -> String {
    let time = unsafe { GetLocalTime() };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        time.wHour, time.wMinute, time.wSecond, time.wMilliseconds
    )
}

pub fn exe_path() -> SpikeResult<std::path::PathBuf> {
    std::env::current_exe().map_err(|err| SpikeError::new(format!("无法取得当前程序路径: {err}")))
}

pub fn shortcut_path() -> SpikeResult<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| SpikeError::new("环境变量 APPDATA 不存在，无法定位开始菜单目录"))?;
    Ok(Path::new(&appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(crate::model::SHORTCUT_FILE_NAME))
}

pub fn hwnd_bits(value: isize) -> *mut c_void {
    value as *mut c_void
}
