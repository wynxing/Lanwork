//! 单实例。命名互斥量选出第一个进程，信号量把后来的启动留到第一个进程来取。
//!
//! 自动复位事件会在等待之前被清掉。信号量的计数会留着，所以后来的启动不会丢。
//! 第二个进程通知之后自己退出。已有进程接着显示什么，产品规格没有写。

#[cfg(test)]
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SetLastError, WIN32_ERROR,
};
#[cfg(test)]
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::System::Threading::{CreateMutexW, CreateSemaphoreW, ReleaseSemaphore};

use crate::winutil::{pcwstr, wide};

pub(crate) const MUTEX_NAME: &str = r"Local\Lanwork.SingleInstance.Mutex";
pub(crate) const ACTIVATE_NAME: &str = r"Local\Lanwork.SingleInstance.Activate";

pub(crate) enum Claim {
    Primary(Primary),
    Secondary,
}

pub(crate) struct Primary {
    mutex: HANDLE,
    semaphore: HANDLE,
}

// SAFETY: 这两个值是内核句柄，不是 Rust 引用。创建之后只在持有者线程上等待和关闭。
unsafe impl Send for Primary {}

impl Drop for Primary {
    fn drop(&mut self) {
        // SAFETY: 句柄由本结构独占，关闭后不再使用。
        unsafe {
            let _ = CloseHandle(self.mutex);
            let _ = CloseHandle(self.semaphore);
        }
    }
}

impl Primary {
    pub(crate) fn semaphore(&self) -> HANDLE {
        self.semaphore
    }

    /// 等到一次后来的启动，或超时。超时返回 false。测试用来确认信号量计数还在。
    #[cfg(test)]
    pub(crate) fn wait_timeout(&self, timeout_ms: u32) -> bool {
        // SAFETY: 信号量在 Drop 之前有效。
        let wake = unsafe { WaitForSingleObject(self.semaphore, timeout_ms) };
        wake == WAIT_OBJECT_0
    }
}

pub(crate) fn claim(mutex_name: &str, activate_name: &str) -> Result<Claim, String> {
    let activate = wide(activate_name);
    let mutex = wide(mutex_name);
    // SAFETY: 名字以 0 结尾。先把上次的错误清掉，再读本次的 GetLastError。
    let semaphore = unsafe {
        SetLastError(WIN32_ERROR(0));
        CreateSemaphoreW(None, 0, i32::MAX, pcwstr(&activate))
            .map_err(|err| format!("单实例信号量创建失败: {err}"))?
    };
    let mutex_handle = unsafe {
        SetLastError(WIN32_ERROR(0));
        match CreateMutexW(None, true, pcwstr(&mutex)) {
            Ok(handle) => handle,
            Err(err) => {
                let _ = CloseHandle(semaphore);
                return Err(format!("单实例互斥量创建失败: {err}"));
            }
        }
    };
    let last_error = unsafe { GetLastError() };
    let already = last_error == ERROR_ALREADY_EXISTS;
    if already {
        let released = unsafe { ReleaseSemaphore(semaphore, 1, None) };
        unsafe {
            let _ = CloseHandle(mutex_handle);
            let _ = CloseHandle(semaphore);
        }
        released.map_err(|err| format!("通知已有实例失败: {err}"))?;
        return Ok(Claim::Secondary);
    }
    Ok(Claim::Primary(Primary {
        mutex: mutex_handle,
        semaphore,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn unique_names() -> (String, String) {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        (
            format!(r"Local\Lanwork.Test.{pid}.{seq}.Mutex"),
            format!(r"Local\Lanwork.Test.{pid}.{seq}.Activate"),
        )
    }

    #[test]
    fn second_claim_signals_the_first_then_a_new_primary_can_start() {
        let (mutex_name, activate_name) = unique_names();
        let primary = match claim(&mutex_name, &activate_name).unwrap() {
            Claim::Primary(primary) => primary,
            Claim::Secondary => panic!("first claim should own the mutex"),
        };
        assert!(!primary.wait_timeout(0));
        assert!(matches!(
            claim(&mutex_name, &activate_name).unwrap(),
            Claim::Secondary
        ));
        assert!(primary.wait_timeout(1_000));
        drop(primary);
        assert!(matches!(
            claim(&mutex_name, &activate_name).unwrap(),
            Claim::Primary(_)
        ));
    }
}
