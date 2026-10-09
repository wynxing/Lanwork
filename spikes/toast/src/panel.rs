use std::ffi::c_void;
use std::sync::Mutex;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DEFAULT_GUI_FONT, DeleteObject, FONT_CHARSET, FONT_CLIP_PRECISION,
    FONT_OUTPUT_PRECISION, FONT_QUALITY, GetStockObject, HFONT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FLASHW_ALL, FLASHW_TIMERNOFG,
    FLASHWINFO, FlashWindowEx, GWL_STYLE, GetMessageW, GetSystemMetrics, GetWindowLongW, HMENU,
    IDC_ARROW, IsWindow, IsWindowVisible, LoadCursorW, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOWNORMAL, SWP_FRAMECHANGED,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SendMessageW, SetForegroundWindow,
    SetWindowLongW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WNDCLASSW, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::PCWSTR;

use crate::model::{Locate, SAMPLE_TODOS, locate_result, status_line, window_title};
use crate::util::{SpikeResult, hwnd_bits, log_line, pcwstr, wide};

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_CLOSE: u32 = 0x0010;
const WM_QUIT: u32 = 0x0012;
const WM_LOCATE: u32 = 0x8000 + 20;
const WM_FORCE_SHOW: u32 = 0x8000 + 21;
const WM_SETFONT: u32 = 0x0030;
const LB_ADDSTRING: u32 = 0x0180;
const LB_SETCURSEL: u32 = 0x0186;
const LB_SETTOPINDEX: u32 = 0x0197;
const HWND_TOPMOST_VALUE: isize = -1;
const HWND_NOTOPMOST_VALUE: isize = -2;

struct Ui {
    hwnd: isize,
    list: isize,
    status: isize,
    font: isize,
    launch: Option<String>,
    title: String,
    status_text: String,
}

static UI: Mutex<Ui> = Mutex::new(Ui {
    hwnd: 0,
    list: 0,
    status: 0,
    font: 0,
    launch: None,
    title: String::new(),
    status_text: String::new(),
});

pub fn note_activation(launch: &str) {
    log_line(&format!("note_activation launch={launch}"));
    let hwnd = {
        let mut ui = lock_ui();
        ui.launch = Some(launch.to_string());
        ui.hwnd
    };
    if hwnd == 0 {
        log_line("note_activation 时面板尚未创建，先记下 launch");
        return;
    }
    let window = HWND(hwnd_bits(hwnd));
    let visible = force_show(window);
    log_line(&format!(
        "note_activation 已 ShowWindow+SetForegroundWindow IsWindowVisible={visible}"
    ));
    unsafe {
        let posted = PostMessageW(Some(window), WM_LOCATE, WPARAM(0), LPARAM(0));
        log_line(&format!("PostMessage WM_LOCATE ret={posted:?}"));
    }
}

pub fn current_launch() -> Option<String> {
    lock_ui().launch.clone()
}

pub fn current_title() -> String {
    lock_ui().title.clone()
}

pub fn current_status() -> String {
    lock_ui().status_text.clone()
}

pub fn is_main_visible() -> bool {
    let hwnd = main_hwnd();
    if hwnd.0.is_null() {
        return false;
    }
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

pub fn startup() -> SpikeResult<HWND> {
    log_line("模拟面板启动。关闭窗口即退出进程。");
    let hwnd = unsafe { create_window() }?;
    let visible = force_show(hwnd);
    log_line(&format!(
        "模拟面板创建完成 hwnd={} IsWindowVisible={visible}",
        hwnd.0 as isize
    ));
    unsafe {
        let posted = PostMessageW(Some(hwnd), WM_FORCE_SHOW, WPARAM(0), LPARAM(0));
        log_line(&format!("已投递 WM_FORCE_SHOW ret={posted:?}"));
    }
    Ok(hwnd)
}

pub fn message_loop() -> SpikeResult<()> {
    log_line("进入 STA 消息循环");
    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == 0 {
            log_line("GetMessageW 返回 0，消息循环结束");
            break;
        }
        if status.0 < 0 {
            return Err(crate::util::SpikeError::new("GetMessageW 失败"));
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

pub fn pump_until(
    timeout: std::time::Duration,
    mut done: impl FnMut() -> bool,
) -> SpikeResult<bool> {
    let start = std::time::Instant::now();
    loop {
        drain_messages()?;
        if done() {
            return Ok(true);
        }
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

pub fn destroy_main() {
    let hwnd = main_hwnd();
    if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return;
    }
    log_line("销毁模拟面板");
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    let _ = drain_messages();
}

fn main_hwnd() -> HWND {
    HWND(hwnd_bits(lock_ui().hwnd))
}

fn drain_messages() -> SpikeResult<()> {
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            if message.message == WM_QUIT {
                log_line("PeekMessage 取到 WM_QUIT");
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

unsafe fn create_window() -> SpikeResult<HWND> {
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|err| crate::util::win_err("GetModuleHandleW", err))?;
    let instance = HINSTANCE(module.0);
    let class_name = wide("LanworkToastSpikePanel");
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
        .map_err(|err| crate::util::win_err("LoadCursorW", err))?;
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: cursor,
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH((5 + 1) as *mut c_void),
        lpszClassName: pcwstr(&class_name),
        ..unsafe { std::mem::zeroed() }
    };
    let atom = unsafe { RegisterClassW(&window_class) };
    if atom == 0 {
        return Err(crate::util::SpikeError::new("RegisterClassW 失败"));
    }

    let dpi = unsafe { GetDpiForSystem() }.max(96) as i32;
    let width = scale(460, dpi);
    let height = scale(380, dpi);
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let x = (screen_w - width).max(0) / 2;
    let y = (screen_h - height).max(0) / 3;
    let title = wide(&window_title(None));
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            pcwstr(&class_name),
            pcwstr(&title),
            WS_OVERLAPPEDWINDOW,
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
    .map_err(|err| crate::util::win_err("CreateWindowExW", err))?;
    log_line(&format!(
        "CreateWindowExW hwnd={} 未带 WS_VISIBLE，改由 force_show 显示",
        hwnd.0 as isize
    ));
    if let Some(launch) = bind_main(hwnd) {
        apply_launch(Some(&launch));
    }
    Ok(hwnd)
}

fn bind_main(hwnd: HWND) -> Option<String> {
    let mut ui = lock_ui();
    ui.hwnd = hwnd.0 as isize;
    ui.launch.clone()
}

fn lock_ui() -> std::sync::MutexGuard<'static, Ui> {
    UI.lock().unwrap_or_else(|poison| poison.into_inner())
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            unsafe { create_children(hwnd) };
            LRESULT(0)
        }
        WM_SIZE => {
            let packed = lparam.0 as u32;
            layout((packed & 0xFFFF) as i32, (packed >> 16) as i32);
            LRESULT(0)
        }
        WM_LOCATE => {
            log_line("窗口过程收到 WM_LOCATE");
            let launch = lock_ui().launch.clone();
            apply_launch(launch.as_deref());
            LRESULT(0)
        }
        WM_FORCE_SHOW => {
            log_line("窗口过程收到 WM_FORCE_SHOW");
            force_show(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            release_font();
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn release_font() {
    let font = {
        let mut ui = lock_ui();
        ui.hwnd = 0;
        let font = ui.font;
        ui.font = 0;
        font
    };
    if font != 0 {
        unsafe {
            let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(hwnd_bits(font)));
        }
    }
}

unsafe fn create_children(parent: HWND) {
    let module = unsafe { GetModuleHandleW(None) }
        .unwrap_or(windows::Win32::Foundation::HMODULE(std::ptr::null_mut()));
    let instance = HINSTANCE(module.0);
    let status_style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | 0x0080);
    let list_style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_VSCROLL.0 | WS_BORDER.0 | 0x0001);
    let status = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w_static(),
            pcwstr(&wide("")),
            status_style,
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(100 as *mut c_void)),
            Some(instance),
            None,
        )
    };
    let list = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w_list(),
            PCWSTR::null(),
            list_style,
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(101 as *mut c_void)),
            Some(instance),
            None,
        )
    };
    let status_hwnd = status.unwrap_or_default();
    let list_hwnd = list.unwrap_or_default();
    let font = make_font();
    if let Some((font, _)) = &font {
        send(status_hwnd, WM_SETFONT, font.0 as usize, 1);
        send(list_hwnd, WM_SETFONT, font.0 as usize, 1);
    }
    for item in SAMPLE_TODOS {
        let label = wide(&format!("{}  {}", item.id, item.title));
        send(list_hwnd, LB_ADDSTRING, 0, label.as_ptr() as isize);
    }
    {
        let mut ui = lock_ui();
        ui.hwnd = parent.0 as isize;
        ui.status = status_hwnd.0 as isize;
        ui.list = list_hwnd.0 as isize;
        if let Some((font, true)) = font {
            ui.font = font.0 as isize;
        }
    }
    unsafe {
        let mut rect = windows::Win32::Foundation::RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(parent, &mut rect);
        layout(rect.right - rect.left, rect.bottom - rect.top);
    }
}

fn make_font() -> Option<(HFONT, bool)> {
    let dpi = unsafe { GetDpiForSystem() }.max(96) as i32;
    let font = unsafe {
        CreateFontW(
            -scale(18, dpi),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            FONT_CHARSET(1),
            FONT_OUTPUT_PRECISION(0),
            FONT_CLIP_PRECISION(0),
            FONT_QUALITY(5),
            0,
            pcwstr(&wide("Microsoft YaHei UI")),
        )
    };
    if font.is_invalid() {
        let stock = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
        if stock.is_invalid() {
            None
        } else {
            Some((HFONT(stock.0), false))
        }
    } else {
        Some((font, true))
    }
}

fn layout(width: i32, height: i32) {
    let ui = lock_ui();
    if ui.status == 0 || ui.list == 0 {
        return;
    }
    let margin = 12;
    let status_height = 56;
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::MoveWindow(
            HWND(hwnd_bits(ui.status)),
            margin,
            margin,
            (width - margin * 2).max(10),
            status_height,
            true,
        );
        let _ = windows::Win32::UI::WindowsAndMessaging::MoveWindow(
            HWND(hwnd_bits(ui.list)),
            margin,
            margin + status_height + 8,
            (width - margin * 2).max(10),
            (height - status_height - margin * 2 - 8).max(10),
            true,
        );
    }
}

fn apply_launch(launch: Option<&str>) {
    let title_text = window_title(launch);
    let status_text = status_line(launch);
    let ui = {
        let mut ui = lock_ui();
        ui.title = title_text.clone();
        ui.status_text = status_text.clone();
        if ui.hwnd == 0 {
            return;
        }
        (ui.hwnd, ui.list, ui.status)
    };
    let window = HWND(hwnd_bits(ui.0));
    let list = HWND(hwnd_bits(ui.1));
    let status = HWND(hwnd_bits(ui.2));
    let title = wide(&title_text);
    let status_wide = wide(&status_text);
    unsafe {
        let _ = SetWindowTextW(window, pcwstr(&title));
        let _ = SetWindowTextW(status, pcwstr(&status_wide));
        match locate_result(launch) {
            Locate::Selected(index) => {
                send(list, LB_SETCURSEL, index, 0);
                send(list, LB_SETTOPINDEX, index, 0);
                log_line(&format!("模拟面板已定位 index={index}"));
            }
            Locate::Missing => {
                send(list, LB_SETCURSEL, usize::MAX, 0);
                log_line("模拟面板未选中任何行");
            }
            Locate::Waiting => {
                send(list, LB_SETCURSEL, usize::MAX, 0);
            }
        }
    }
    force_show(window);
}

pub fn force_show(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        log_line("force_show: hwnd 为空");
        return false;
    }
    unsafe {
        let first = ShowWindow(hwnd, SW_SHOWNORMAL);
        let second = ShowWindow(hwnd, SW_SHOWNORMAL);
        let style_after_show = GetWindowLongW(hwnd, GWL_STYLE);
        log_line(&format!(
            "ShowWindow SW_SHOWNORMAL 两次 ret={},{} style=0x{:X} IsWindowVisible={}",
            first.0,
            second.0,
            style_after_show as u32,
            IsWindowVisible(hwnd).as_bool()
        ));
        let top = SetWindowPos(
            hwnd,
            Some(HWND(hwnd_bits(HWND_TOPMOST_VALUE))),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let drop = SetWindowPos(
            hwnd,
            Some(HWND(hwnd_bits(HWND_NOTOPMOST_VALUE))),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        log_line(&format!(
            "SetWindowPos SWP_SHOWWINDOW topmost={top:?} notopmost={drop:?} IsWindowVisible={}",
            IsWindowVisible(hwnd).as_bool()
        ));
        if !IsWindowVisible(hwnd).as_bool() {
            let style = GetWindowLongW(hwnd, GWL_STYLE);
            let updated = style | WS_VISIBLE.0 as i32;
            let previous = SetWindowLongW(hwnd, GWL_STYLE, updated);
            let again = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
            );
            log_line(&format!(
                "补 WS_VISIBLE style=0x{:X}->0x{:X} SetWindowLong 原值=0x{:X} SetWindowPos={again:?} IsWindowVisible={}",
                style as u32,
                updated as u32,
                previous as u32,
                IsWindowVisible(hwnd).as_bool()
            ));
        }
        let foreground = SetForegroundWindow(hwnd);
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 3,
            dwTimeout: 0,
        };
        let flashed = FlashWindowEx(&info);
        let visible = IsWindowVisible(hwnd).as_bool();
        log_line(&format!(
            "SetForegroundWindow ret={} FlashWindowEx={flashed:?} IsWindowVisible={visible}",
            foreground.as_bool()
        ));
        visible
    }
}

fn scale(value: i32, dpi: i32) -> i32 {
    value * dpi / 96
}

fn w_static() -> PCWSTR {
    windows::core::w!("STATIC")
}

fn w_list() -> PCWSTR {
    windows::core::w!("LISTBOX")
}

fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) {
    unsafe {
        SendMessageW(hwnd, message, Some(WPARAM(wparam)), Some(LPARAM(lparam)));
    }
}
