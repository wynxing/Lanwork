use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FillRect, GetDC, HDC, HGDIOBJ, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, CreateWindowExW, DestroyIcon, DestroyWindow, DispatchMessageW,
    HWND_MESSAGE, ICONINFO, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE,
};

use crate::util::{SpikeError, SpikeResult, pcwstr, wide, write_utf16_field};

const WM_TRAY: u32 = 0x8000 + 21;

pub fn show_badge_for(seconds: u64) -> SpikeResult<()> {
    let icon = badge_icon()?;
    let window = tray_window()?;
    let added = notify(NIM_ADD, window, icon)?;
    if !added {
        let _ = destroy_icon(icon);
        unsafe {
            let _ = DestroyWindow(window);
        }
        return Err(SpikeError::new("Shell_NotifyIcon(NIM_ADD) 返回 FALSE"));
    }
    let modified = notify(NIM_MODIFY, window, icon)?;
    if !modified {
        let _ = notify(NIM_DELETE, window, icon);
        let _ = destroy_icon(icon);
        unsafe {
            let _ = DestroyWindow(window);
        }
        return Err(SpikeError::new("Shell_NotifyIcon(NIM_MODIFY) 返回 FALSE"));
    }
    pump_for(Duration::from_secs(seconds));
    let deleted = notify(NIM_DELETE, window, icon)?;
    let _ = destroy_icon(icon);
    unsafe {
        let _ = DestroyWindow(window);
    }
    if deleted {
        Ok(())
    } else {
        Err(SpikeError::new("Shell_NotifyIcon(NIM_DELETE) 返回 FALSE"))
    }
}

pub fn roundtrip() -> SpikeResult<()> {
    show_badge_for(0)
}

fn pump_for(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let deadline = Instant::now() + duration;
    let mut message = unsafe { std::mem::zeroed::<MSG>() };
    while Instant::now() < deadline {
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn tray_window() -> SpikeResult<HWND> {
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|err| crate::util::win_err("GetModuleHandleW", err))?;
    let instance = windows::Win32::Foundation::HINSTANCE(module.0);
    let title = wide("lanwork-spike-tray");
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("STATIC"),
            pcwstr(&title),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|err| crate::util::win_err("创建托盘消息窗口", err))
}

fn notify(
    message: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE,
    window: HWND,
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> SpikeResult<bool> {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = window;
    data.uID = 1;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    write_utf16_field(&mut data.szTip, "逾期 1");
    let ok = unsafe { Shell_NotifyIconW(message, &data) };
    Ok(ok.as_bool())
}

fn badge_icon() -> SpikeResult<windows::Win32::UI::WindowsAndMessaging::HICON> {
    unsafe { draw_badge() }
}

unsafe fn draw_badge() -> SpikeResult<windows::Win32::UI::WindowsAndMessaging::HICON> {
    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        return Err(SpikeError::new("GetDC 失败"));
    }
    let memory = unsafe { CreateCompatibleDC(Some(screen)) };
    let color = unsafe { CreateCompatibleBitmap(screen, 32, 32) };
    let mask = unsafe { CreateCompatibleBitmap(screen, 32, 32) };
    if memory.is_invalid() || color.is_invalid() || mask.is_invalid() {
        unsafe {
            cleanup_dc(screen, memory, color.into(), mask.into());
        }
        return Err(SpikeError::new("创建托盘位图失败"));
    }
    let previous = unsafe { SelectObject(memory, color.into()) };
    let brush = unsafe { CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00C06020)) };
    let mut rect = windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 32,
        bottom: 32,
    };
    unsafe {
        let _ = FillRect(memory, &rect, brush);
        let _ = SetBkMode(memory, TRANSPARENT);
        let _ = SetTextColor(memory, windows::Win32::Foundation::COLORREF(0x00FFFFFF));
        let mut text = wide("1");
        let _ = DrawTextW(
            memory,
            &mut text,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(memory, previous);
        let _ = DeleteObject(brush.into());
    }
    let icon = unsafe {
        CreateIconIndirect(&ICONINFO {
            fIcon: windows::core::BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        })
    };
    unsafe {
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    icon.map_err(|err| crate::util::win_err("CreateIconIndirect", err))
}

unsafe fn cleanup_dc(screen: HDC, memory: HDC, color: HGDIOBJ, mask: HGDIOBJ) {
    unsafe {
        if !color.is_invalid() {
            let _ = DeleteObject(color);
        }
        if !mask.is_invalid() {
            let _ = DeleteObject(mask);
        }
        if !memory.is_invalid() {
            let _ = DeleteDC(memory);
        }
        if !screen.is_invalid() {
            ReleaseDC(None, screen);
        }
    }
}

fn destroy_icon(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> SpikeResult<()> {
    unsafe { DestroyIcon(icon) }.map_err(|err| crate::util::win_err("DestroyIcon", err))
}
