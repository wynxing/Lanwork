//! 给手测用的窗口。不是收纳界面。

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DEFAULT_QUALITY, DeleteObject, FW_NORMAL,
    HBRUSH, HGDIOBJ, OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{RegisterDragDrop, RevokeDragDrop};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, GetClientRect, GetMessageW, IDC_ARROW, LoadCursorW,
    MSG, MoveWindow, PostQuitMessage, RegisterClassW, SW_SHOWNORMAL, SendMessageW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CREATE, WM_DESTROY, WM_SETFONT, WM_SIZE,
    WNDCLASSW, WS_CHILD, WS_HSCROLL, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use crate::ole::{self, DropState};
use crate::parse::session_header;

struct WindowHost {
    state: Arc<DropState>,
    font: AtomicIsize,
}

static HOST: Mutex<Option<WindowHost>> = Mutex::new(None);

pub(crate) fn run() -> Result<(), String> {
    ole::ensure_ole()?;
    let log_path = std::env::temp_dir().join("lanwork-spike-urldrop.log");
    println!("urldrop: log={}", log_path.display());
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let state = Arc::new(DropState {
        log: Mutex::new(String::new()),
        edit: AtomicIsize::new(0),
        log_path: log_path.clone(),
    });
    ole::publish(&state, &session_header(&log_path.display().to_string()));
    {
        let mut slot = lock_host();
        *slot = Some(WindowHost {
            state: state.clone(),
            font: AtomicIsize::new(0),
        });
    }
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(error) => {
            *lock_host() = None;
            return Err(error);
        }
    };
    let target = ole::drop_target(state);
    if let Err(error) = unsafe { RegisterDragDrop(hwnd, &target) } {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        }
        *lock_host() = None;
        return Err(format!("RegisterDragDrop: {error}"));
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    }
    let mut message: MSG = unsafe { std::mem::zeroed() };
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(target);
    *lock_host() = None;
    Ok(())
}

fn lock_host() -> std::sync::MutexGuard<'static, Option<WindowHost>> {
    HOST.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn create_window() -> Result<HWND, String> {
    let module = unsafe { GetModuleHandleW(None) }.map_err(|error| error.to_string())?;
    let instance = HINSTANCE(module.0);
    let class_name = w!("LanworkUrlDropSpike");
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(|error| error.to_string())?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: cursor,
        hbrBackground: HBRUSH((5 + 1) as *mut c_void),
        lpszClassName: class_name,
        ..unsafe { std::mem::zeroed() }
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let code = windows::core::Error::from_thread().code().0;
        let already = (ERROR_CLASS_ALREADY_EXISTS.0 as i32) == (code & 0xFFFF);
        if !already {
            return Err(format!("RegisterClassW: {code:#x}"));
        }
    }
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("Lanwork 网址拖放验证"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            760,
            560,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("CreateWindowExW: {error}"))
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            create_edit(hwnd);
            LRESULT(0)
        }
        WM_SIZE => {
            layout(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let font_bits = {
                let host = lock_host();
                host.as_ref()
                    .map(|host| host.font.swap(0, Ordering::AcqRel))
                    .unwrap_or(0)
            };
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                if font_bits != 0 {
                    let _ = DeleteObject(HGDIOBJ(font_bits as *mut c_void));
                }
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn create_edit(parent: HWND) {
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => HINSTANCE(module.0),
        Err(_) => return,
    };
    let style = WS_CHILD
        | WS_VISIBLE
        | WS_VSCROLL
        | WS_HSCROLL
        | WINDOW_STYLE(ES_MULTILINE as u32)
        | WINDOW_STYLE(ES_AUTOVSCROLL as u32)
        | WINDOW_STYLE(ES_AUTOHSCROLL as u32)
        | WINDOW_STYLE(ES_READONLY as u32);
    let Ok(edit) = (unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            PCWSTR::null(),
            style,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(module),
            None,
        )
    }) else {
        return;
    };
    let font = unsafe {
        CreateFontW(
            -18,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        )
    };
    if !font.is_invalid() {
        if let Some(host) = lock_host().as_ref() {
            host.font.store(font.0 as isize, Ordering::Release);
        }
        unsafe {
            SendMessageW(
                edit,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
    let state = lock_host().as_ref().map(|host| Arc::clone(&host.state));
    if let Some(state) = state {
        ole::remember_edit(&state, edit);
    }
    layout(parent);
}

fn layout(hwnd: HWND) {
    let Some(edit) = edit_hwnd() else {
        return;
    };
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let width = (rect.right - rect.left - 16).max(0);
    let height = (rect.bottom - rect.top - 16).max(0);
    unsafe {
        let _ = MoveWindow(edit, 8, 8, width, height, true);
    }
}

fn edit_hwnd() -> Option<HWND> {
    let host = lock_host();
    let bits = host.as_ref()?.state.edit.load(Ordering::Acquire);
    if bits == 0 {
        None
    } else {
        Some(HWND(bits as *mut c_void))
    }
}
