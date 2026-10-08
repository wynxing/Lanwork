//! Win32：版本、透明效果、节电、DWM 背景、截图。
//!
//! 亚克力属性只在 build ≥ 22621 时设置。调用失败记 HRESULT，不当成已经套上背景。

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WAIT_FAILED,
    WAIT_OBJECT_0, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    CreateSolidBrush, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    GetSystemPowerStatus, HPOWERNOTIFY, RegisterPowerSettingNotification, SYSTEM_POWER_STATUS,
    UnregisterPowerSettingNotification,
};
const GUID_POWER_SAVING_STATUS: windows::core::GUID =
    windows::core::GUID::from_u128(0xe00958c0_c213_4ace_ac77_fecced2eeea5);
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_DWORD, REG_NOTIFY_CHANGE_LAST_SET,
    RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, RegQueryValueExW,
};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow, GetWindowRect,
    RegisterClassW, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, UnregisterClassW,
    WM_POWERBROADCAST, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::decision::Rgb;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> i32;
}

const PBT_POWERSETTINGCHANGE: u32 = 0x8013;
const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

pub struct WindowsBuild {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
}

pub struct Personalize {
    pub transparency_enabled: bool,
    pub apps_use_light_theme: bool,
}

pub struct DwmRequest {
    pub apply_backdrop: bool,
    pub acrylic: bool,
    pub dark: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct DwmResult {
    pub backdrop: i32,
    pub readback: Option<u32>,
    pub corner: i32,
    pub dark: i32,
    pub extend: i32,
}

pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Capture {
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgb> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = ((y * self.width + x) * 3) as usize;
        Some(Rgb {
            r: self.rgb[index],
            g: self.rgb[index + 1],
            b: self.rgb[index + 2],
        })
    }
}

pub struct ReferenceWindow {
    hwnd: HWND,
    class_name: Vec<u16>,
    brush: windows::Win32::Graphics::Gdi::HBRUSH,
    instance: HINSTANCE,
}

pub struct Watcher {
    shutdown: HANDLE,
    poke: HANDLE,
    thread: Option<JoinHandle<()>>,
    subclass_hwnd: Option<HWND>,
    power_notify: Option<HPOWERNOTIFY>,
    subclass_installed: bool,
}

impl Watcher {
    pub fn subclass_installed(&self) -> bool {
        self.subclass_installed
    }
}

pub fn windows_build() -> Result<WindowsBuild, String> {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: u32::try_from(std::mem::size_of::<OSVERSIONINFOW>())
            .map_err(|_| "OSVERSIONINFOW 尺寸超出 u32".to_string())?,
        ..Default::default()
    };
    let status = unsafe { RtlGetVersion(&mut info) };
    if status < 0 {
        return Err(format!("RtlGetVersion 返回 {status:#x}"));
    }
    Ok(WindowsBuild {
        major: info.dwMajorVersion,
        minor: info.dwMinorVersion,
        build: info.dwBuildNumber,
    })
}

pub fn personalize() -> Result<Personalize, String> {
    let key = open_personalize()?;
    let transparency = query_dword(key, w!("EnableTransparency"))?;
    let light = query_dword(key, w!("AppsUseLightTheme"))?;
    close_key(key);
    Ok(Personalize {
        transparency_enabled: transparency != 0,
        apps_use_light_theme: light != 0,
    })
}

pub fn battery_saver() -> Result<bool, String> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }
        .map_err(|err| format!("GetSystemPowerStatus 失败：{err}"))?;
    Ok(status.SystemStatusFlag & 1 != 0)
}

pub fn apply_dwm(hwnd: HWND, request: DwmRequest) -> DwmResult {
    let mut result = DwmResult {
        backdrop: 1,
        readback: None,
        corner: 1,
        dark: 1,
        extend: 1,
    };
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    result.extend = hresult(unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) });

    if request.apply_backdrop {
        let backdrop: DWM_SYSTEMBACKDROP_TYPE = if request.acrylic {
            DWMSBT_TRANSIENTWINDOW
        } else {
            DWMSBT_NONE
        };
        result.backdrop = hresult(unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop as *const _ as *const c_void,
                u32::try_from(std::mem::size_of_val(&backdrop)).unwrap_or(4),
            )
        });
        let mut readback = DWM_SYSTEMBACKDROP_TYPE::default();
        let read = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &mut readback as *mut _ as *mut c_void,
                u32::try_from(std::mem::size_of_val(&readback)).unwrap_or(4),
            )
        };
        if read.is_ok() {
            result.readback = Some(backdrop_value(readback));
        }
    }

    let corner = DWMWCP_ROUND;
    result.corner = hresult(unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const c_void,
            u32::try_from(std::mem::size_of_val(&corner)).unwrap_or(4),
        )
    });

    let dark: u32 = if request.dark { 1 } else { 0 };
    result.dark = hresult(unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const c_void,
            u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4),
        )
    });
    result
}

pub fn capture_window(hwnd: HWND) -> Result<Capture, String> {
    let rect = window_rect(hwnd)?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return Err(format!("窗口尺寸无效：{width}x{height}"));
    }
    capture_screen_rect(rect.left, rect.top, width, height)
}

pub fn capture_screen_pixel(x: i32, y: i32) -> Result<Rgb, String> {
    let capture = capture_screen_rect(x, y, 1, 1)?;
    capture
        .pixel(0, 0)
        .ok_or_else(|| "屏幕像素为空".to_string())
}

pub fn hwnd_from_isize(value: isize) -> HWND {
    HWND(value as *mut c_void)
}

impl ReferenceWindow {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Result<Self, String> {
        let instance = unsafe { GetModuleHandleW(None) }
            .map_err(|err| format!("GetModuleHandleW 失败：{err}"))?;
        let instance = HINSTANCE(instance.0);
        let class_name: Vec<u16> =
            std::ffi::OsStr::new(&format!("LanworkRenderRef{}", std::process::id()))
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
        let brush = unsafe { CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FF_00FF)) };
        if brush.is_invalid() {
            return Err("CreateSolidBrush 失败".to_string());
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(reference_wndproc),
            hInstance: instance,
            hbrBackground: brush,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            let _ = unsafe { DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(brush.0)) };
            return Err("RegisterClassW 失败".to_string());
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class_name.as_ptr()),
                w!(""),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                width,
                height,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|err| format!("参考窗创建失败：{err}"))?;
        Ok(Self {
            hwnd,
            class_name,
            brush,
            instance,
        })
    }

    pub fn place_behind(&self, target: HWND) -> Result<(), String> {
        let rect = window_rect(target)?;
        unsafe {
            // hWndInsertAfter 先于本窗口，所以参考窗紧贴在测量窗后面，而不是沉到桌面最底。
            SetWindowPos(
                self.hwnd,
                Some(target),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|err| format!("参考窗置后失败：{err}"))?;
        }
        Ok(())
    }
}

impl Drop for ReferenceWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), Some(self.instance));
            let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(self.brush.0));
        }
    }
}

impl Watcher {
    pub fn start(hwnd: HWND, on_change: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let shutdown = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|err| format!("创建退出事件失败：{err}"))?;
        let poke = unsafe { CreateEventW(None, false, false, None) }
            .map_err(|err| format!("创建电源事件失败：{err}"))?;
        let registry = unsafe { CreateEventW(None, false, false, None) }
            .map_err(|err| format!("创建注册表事件失败：{err}"))?;
        let key = open_personalize()?;

        let subclass_installed =
            unsafe { SetWindowSubclass(hwnd, Some(power_subclass), 1, poke.0 as usize).as_bool() };
        let power_notify = if subclass_installed {
            unsafe {
                RegisterPowerSettingNotification(
                    HANDLE(hwnd.0),
                    &GUID_POWER_SAVING_STATUS,
                    DEVICE_NOTIFY_WINDOW_HANDLE,
                )
                .ok()
            }
        } else {
            None
        };

        let on_change = Arc::new(on_change);
        let thread_change = Arc::clone(&on_change);
        let shutdown_bits = shutdown.0 as isize;
        let poke_bits = poke.0 as isize;
        let registry_bits = registry.0 as isize;
        let key_bits = key.0 as isize;
        let thread = std::thread::Builder::new()
            .name("render-spike-watch".to_string())
            .spawn(move || {
                watch_loop(
                    HKEY(key_bits as *mut c_void),
                    HANDLE(shutdown_bits as *mut c_void),
                    HANDLE(poke_bits as *mut c_void),
                    HANDLE(registry_bits as *mut c_void),
                    thread_change,
                )
            })
            .map_err(|err| format!("监听线程启动失败：{err}"))?;

        Ok(Self {
            shutdown,
            poke,
            thread: Some(thread),
            subclass_hwnd: subclass_installed.then_some(hwnd),
            power_notify,
            subclass_installed,
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.shutdown);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            if let Some(hwnd) = self.subclass_hwnd.take() {
                let _ = RemoveWindowSubclass(hwnd, Some(power_subclass), 1);
            }
            if let Some(cookie) = self.power_notify.take() {
                let _ = UnregisterPowerSettingNotification(cookie);
            }
            let _ = CloseHandle(self.poke);
            let _ = CloseHandle(self.shutdown);
        }
    }
}

fn watch_loop(
    key: HKEY,
    shutdown: HANDLE,
    poke: HANDLE,
    registry: HANDLE,
    on_change: Arc<dyn Fn() + Send + Sync>,
) {
    let handles = [shutdown, poke, registry];
    let mut last_transparency = None;
    let mut last_light = None;
    let mut last_battery = None;
    loop {
        let current = personalize().ok();
        let battery = battery_saver().ok();
        let changed = current.as_ref().map(|item| item.transparency_enabled) != last_transparency
            || current.as_ref().map(|item| item.apps_use_light_theme) != last_light
            || battery != last_battery;
        if changed {
            last_transparency = current.as_ref().map(|item| item.transparency_enabled);
            last_light = current.as_ref().map(|item| item.apps_use_light_theme);
            last_battery = battery;
            on_change();
        }
        if arm_notify(key, registry).is_err() {
            break;
        }
        let wait = unsafe { WaitForMultipleObjects(&handles, false, 5_000) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_FAILED {
            break;
        }
    }
    close_key(key);
    unsafe {
        let _ = CloseHandle(registry);
    }
}

unsafe extern "system" fn power_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_POWERBROADCAST && wparam.0 == PBT_POWERSETTINGCHANGE as usize {
        unsafe {
            let _ = SetEvent(HANDLE(data as *mut c_void));
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn reference_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn capture_screen_rect(x: i32, y: i32, width: i32, height: i32) -> Result<Capture, String> {
    let width_u = u32::try_from(width).map_err(|_| format!("宽度为负：{width}"))?;
    let height_u = u32::try_from(height).map_err(|_| format!("高度为负：{height}"))?;
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("GetDC 失败".to_string());
        }
        let memory = CreateCompatibleDC(Some(screen));
        if memory.is_invalid() {
            ReleaseDC(None, screen);
            return Err("CreateCompatibleDC 失败".to_string());
        }
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            return Err("CreateCompatibleBitmap 失败".to_string());
        }
        let previous = SelectObject(memory, windows::Win32::Graphics::Gdi::HGDIOBJ(bitmap.0));
        // CAPTUREBLT 把分层窗口和部分 DWM 效果算进屏幕 DC。只有 SRCCOPY 时，亚克力区域可能被拍成纯色。
        let rop = windows::Win32::Graphics::Gdi::ROP_CODE(SRCCOPY.0 | 0x4000_0000);
        if let Err(err) = BitBlt(memory, 0, 0, width, height, Some(screen), x, y, rop) {
            if !previous.is_invalid() {
                SelectObject(memory, previous);
            }
            let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            return Err(format!("BitBlt 失败：{err}"));
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).unwrap_or(0),
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (width_u * height_u * 4) as usize];
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height_u,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        if !previous.is_invalid() {
            SelectObject(memory, previous);
        }
        let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        if rows == 0 {
            return Err("GetDIBits 失败".to_string());
        }
        let mut rgb = Vec::with_capacity((width_u * height_u * 3) as usize);
        for pixel in bgra.as_chunks::<4>().0 {
            rgb.push(pixel[2]);
            rgb.push(pixel[1]);
            rgb.push(pixel[0]);
        }
        Ok(Capture {
            width: width_u,
            height: height_u,
            rgb,
        })
    }
}

pub fn write_png(capture: &Capture, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建截图目录失败：{err}"))?;
    }
    let file = File::create(path).map_err(|err| format!("创建截图失败：{err}"))?;
    let mut encoder = png::Encoder::new(file, capture.width, capture.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|err| format!("PNG 头失败：{err}"))?;
    writer
        .write_image_data(&capture.rgb)
        .map_err(|err| format!("PNG 数据失败：{err}"))?;
    Ok(())
}

fn window_rect(hwnd: HWND) -> Result<RECT, String> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|err| format!("GetWindowRect 失败：{err}"))?;
    Ok(rect)
}

fn open_personalize() -> Result<HKEY, String> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PERSONALIZE,
            None,
            KEY_READ | KEY_NOTIFY,
            &mut key,
        )
    };
    win32(status)?;
    Ok(key)
}

fn query_dword(key: HKEY, name: PCWSTR) -> Result<u32, String> {
    let mut kind = REG_DWORD;
    let mut data = 0u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);
    let status = unsafe {
        RegQueryValueExW(
            key,
            name,
            None,
            Some(&mut kind),
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    win32(status).map_err(|err| format!("读取注册表值失败：{err}"))?;
    if kind != REG_DWORD {
        return Err("注册表值不是 DWORD".to_string());
    }
    Ok(data)
}

fn arm_notify(key: HKEY, event: HANDLE) -> Result<(), String> {
    let status = unsafe {
        RegNotifyChangeKeyValue(key, false, REG_NOTIFY_CHANGE_LAST_SET, Some(event), true)
    };
    win32(status).map_err(|err| format!("RegNotifyChangeKeyValue 失败：{err}"))
}

fn win32(status: WIN32_ERROR) -> Result<(), String> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("Win32 {}", status.0))
    }
}

fn close_key(key: HKEY) {
    unsafe {
        let _ = RegCloseKey(key);
    }
}

fn hresult(result: windows::core::Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => err.code().0,
    }
}

fn backdrop_value(value: DWM_SYSTEMBACKDROP_TYPE) -> u32 {
    value.0 as u32
}
