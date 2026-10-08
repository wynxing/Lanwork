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
    FLASHWINFO, FlashWindowEx, GetMessageW, GetSystemMetrics, HMENU, IDC_ARROW, LoadCursorW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SWP_NOMOVE,
    SWP_NOSIZE, SWP_SHOWWINDOW, SendMessageW, SetForegroundWindow, SetWindowPos, SetWindowTextW,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::PCWSTR;

use crate::model::{Locate, SAMPLE_TODOS, locate_result, status_line, window_title};
use crate::util::{SpikeResult, hwnd_bits, log_line, pcwstr, wide};

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_CLOSE: u32 = 0x0010;
const WM_LOCATE: u32 = 0x8000 + 20;
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
}

static UI: Mutex<Ui> = Mutex::new(Ui {
    hwnd: 0,
    list: 0,
    status: 0,
    font: 0,
    launch: None,
});

pub fn note_activation(launch: &str) {
    let hwnd = {
        let mut ui = lock_ui();
        ui.launch = Some(launch.to_string());
        ui.hwnd
    };
    if hwnd != 0 {
        let window = HWND(hwnd_bits(hwnd));
        unsafe {
            let _ = SetForegroundWindow(window);
            let _ = PostMessageW(Some(window), WM_LOCATE, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn run() -> SpikeResult<()> {
    log_line("模拟面板启动。关闭窗口即退出进程。");
    unsafe { run_window() }
}

unsafe fn run_window() -> SpikeResult<()> {
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
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
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

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    reveal(hwnd);
    if let Some(launch) = bind_main(hwnd) {
        apply_launch(Some(&launch));
    }

    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == 0 {
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
            let launch = lock_ui().launch.clone();
            apply_launch(launch.as_deref());
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
    apply_launch(lock_ui().launch.clone().as_deref());
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
    let ui = lock_ui();
    if ui.hwnd == 0 {
        return;
    }
    let window = HWND(hwnd_bits(ui.hwnd));
    let list = HWND(hwnd_bits(ui.list));
    let status = HWND(hwnd_bits(ui.status));
    let title = wide(&window_title(launch));
    let status_text = wide(&status_line(launch));
    unsafe {
        let _ = SetWindowTextW(window, pcwstr(&title));
        let _ = SetWindowTextW(status, pcwstr(&status_text));
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
    reveal(window);
}

fn reveal(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND(hwnd_bits(HWND_TOPMOST_VALUE))),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let _ = SetWindowPos(
            hwnd,
            Some(HWND(hwnd_bits(HWND_NOTOPMOST_VALUE))),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let _ = SetForegroundWindow(hwnd);
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 3,
            dwTimeout: 0,
        };
        let _ = FlashWindowEx(&info);
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
