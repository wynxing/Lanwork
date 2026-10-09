//! 查询线程上的 COM。初始化后留在线程上，避免把别人的套间卸掉。

use std::cell::Cell;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn ensure_com() -> Result<(), String> {
    let mut error = None;
    COM_READY.with(|ready| {
        if ready.get() {
            return;
        }
        // SAFETY: 保留参数是空的。成功或 S_FALSE 表示这个线程可以继续用 STA。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() {
            error = Some(format!("COM 初始化失败，hresult={hr:?}"));
            return;
        }
        ready.set(true);
    });
    match error {
        Some(message) => Err(message),
        None => Ok(()),
    }
}
