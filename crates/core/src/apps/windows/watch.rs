//! `ReadDirectoryChangesW` 监视开始菜单目录。多次变化合并成一次回调。

use std::path::Path;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_INCOMPLETE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

use super::comutil;

/// 内核句柄可以交给另一个线程等待。同一句柄不会从两个线程同时关闭。
#[derive(Clone, Copy)]
struct SendHandle(HANDLE);
// SAFETY: HANDLE 是内核对象的数值，移动到监视线程后仍由创建线程负责关闭。
unsafe impl Send for SendHandle {}

const CAP: Duration = Duration::from_secs(2);

pub(crate) struct WatchHandle {
    stop: SendHandle,
    thread: Option<JoinHandle<()>>,
}

impl WatchHandle {
    fn signal(&self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.signal();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            let _ = CloseHandle(self.stop.0);
        }
    }
}

pub(crate) fn spawn(
    directories: Vec<std::path::PathBuf>,
    debounce: Duration,
    on_change: impl Fn() + Send + 'static,
) -> Result<WatchHandle, String> {
    let stop = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map_err(|err| format!("监视停止事件创建失败: {err}"))?;
    let stop = SendHandle(stop);
    let stop_for_thread = stop;
    let thread = std::thread::Builder::new()
        .name("lanwork-apps-watch".to_owned())
        .spawn(move || watch_loop(stop_for_thread, directories, debounce, on_change))
        .map_err(|err| {
            unsafe {
                let _ = CloseHandle(stop.0);
            }
            err.to_string()
        })?;
    Ok(WatchHandle {
        stop,
        thread: Some(thread),
    })
}

fn watch_loop(
    stop: SendHandle,
    directories: Vec<std::path::PathBuf>,
    debounce: Duration,
    on_change: impl Fn(),
) {
    let mut watches = Vec::new();
    for dir in directories {
        if let Some(watch) = DirWatch::open(&dir) {
            watches.push(watch);
        }
    }
    if watches.is_empty() {
        unsafe {
            let _ = WaitForMultipleObjects(&[stop.0], false, u32::MAX);
        }
        return;
    }
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;
    loop {
        if let (Some(started), Some(touched)) = (first, last) {
            let quiet = debounce.saturating_sub(touched.elapsed());
            let cap = CAP.saturating_sub(started.elapsed());
            if quiet.is_zero() || cap.is_zero() {
                on_change();
                first = None;
                last = None;
                continue;
            }
        }
        let timeout = match (first, last) {
            (Some(started), Some(touched)) => {
                let quiet = debounce.saturating_sub(touched.elapsed());
                let cap = CAP.saturating_sub(started.elapsed());
                millis(quiet.min(cap))
            }
            _ => u32::MAX,
        };
        let mut handles = Vec::with_capacity(watches.len() + 1);
        handles.push(stop.0);
        for watch in &watches {
            handles.push(watch.event.0);
        }
        let status = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
        if status == WAIT_OBJECT_0 {
            break;
        }
        if status == WAIT_TIMEOUT {
            if first.is_some() {
                on_change();
                first = None;
                last = None;
            }
            continue;
        }
        let code = wait_code(status);
        if code > WAIT_OBJECT_0.0 && code < WAIT_OBJECT_0.0 + handles.len() as u32 {
            let index = (code - WAIT_OBJECT_0.0) as usize - 1;
            if !watches[index].complete() {
                watches.remove(index);
                if watches.is_empty() {
                    break;
                }
            }
            let now = Instant::now();
            if first.is_none() {
                first = Some(now);
            }
            last = Some(now);
            continue;
        }
        break;
    }
}

fn millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

fn wait_code(status: windows::Win32::Foundation::WAIT_EVENT) -> u32 {
    status.0
}

/// 持有堆上的监视对象。取消后若读仍未完成，泄漏的是这一份，不是 `OVERLAPPED` 的副本。
struct OwnedWatch {
    inner: Option<Box<DirWatch>>,
}

impl Drop for OwnedWatch {
    fn drop(&mut self) {
        if let Some(watch) = self.inner.take() {
            release_watch(watch, 1_000);
        }
    }
}

impl std::ops::Deref for OwnedWatch {
    type Target = DirWatch;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("directory watch")
    }
}

impl std::ops::DerefMut for OwnedWatch {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("directory watch")
    }
}

struct DirWatch {
    handle: SendHandle,
    event: SendHandle,
    buffer: Vec<u8>,
    overlapped: OVERLAPPED,
    armed: bool,
}

impl DirWatch {
    fn open(dir: &Path) -> Option<OwnedWatch> {
        let wide = comutil::wide_path(dir);
        let handle = unsafe {
            CreateFileW(
                comutil::pcwstr(&wide),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                windows::Win32::Storage::FileSystem::OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .ok()?;
        let event = match unsafe { CreateEventW(None, false, false, PCWSTR::null()) } {
            Ok(event) => event,
            Err(_) => {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return None;
            }
        };
        let mut watch = Box::new(Self {
            handle: SendHandle(handle),
            event: SendHandle(event),
            buffer: vec![0u8; 16 * 1024],
            overlapped: unsafe { std::mem::zeroed() },
            armed: false,
        });
        if !watch.arm() {
            release_watch(watch, 0);
            return None;
        }
        Some(OwnedWatch { inner: Some(watch) })
    }

    fn arm(&mut self) -> bool {
        let event = self.event.0;
        self.overlapped = unsafe { std::mem::zeroed() };
        self.overlapped.hEvent = event;
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_ATTRIBUTES
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_SIZE;
        // SAFETY: 缓冲区和 OVERLAPPED 在这个 Box 里。完成或泄漏之前不释放这份分配。
        let ok = unsafe {
            ReadDirectoryChangesW(
                self.handle.0,
                self.buffer.as_mut_ptr().cast(),
                self.buffer.len() as u32,
                true,
                filter,
                None,
                Some(&mut self.overlapped),
                None,
            )
        };
        self.armed = ok.is_ok();
        self.armed
    }

    fn complete(&mut self) -> bool {
        let mut bytes = 0u32;
        match unsafe { GetOverlappedResult(self.handle.0, &self.overlapped, &mut bytes, false) } {
            // 事件先于完成被唤醒时，原来的读仍然挂着，不能再发一次。
            Err(err) if io_incomplete(&err) => true,
            Ok(()) | Err(_) => {
                self.armed = false;
                self.arm()
            }
        }
    }
}

fn io_incomplete(err: &windows::core::Error) -> bool {
    let code = err.code().0 as u32;
    code == ERROR_IO_INCOMPLETE.0 || code == 0x8007_03E4
}

enum Release {
    Finished,
    /// 内核仍握着原来的 `OVERLAPPED`。整份 `DirWatch` 留在堆上，然后再关目录句柄。
    Leaked,
}

fn release_watch(mut watch: Box<DirWatch>, wait_ms: u32) -> Release {
    if watch.armed {
        unsafe {
            let _ = CancelIoEx(watch.handle.0, Some(&watch.overlapped));
            if wait_ms > 0 {
                let _ = WaitForSingleObject(watch.event.0, wait_ms);
            }
        }
        match detach_if_incomplete(watch) {
            Ok(finished) => watch = finished,
            Err(()) => return Release::Leaked,
        }
        watch.armed = false;
    }
    unsafe {
        let _ = CloseHandle(watch.handle.0);
        let _ = CloseHandle(watch.event.0);
    }
    Release::Finished
}

/// 读仍未完成时，内核持有的是这份 `OVERLAPPED` 的地址。泄漏整个 `DirWatch` 后再关目录句柄。
fn detach_if_incomplete(watch: Box<DirWatch>) -> Result<Box<DirWatch>, ()> {
    let mut bytes = 0u32;
    let done = unsafe { GetOverlappedResult(watch.handle.0, &watch.overlapped, &mut bytes, false) };
    if matches!(done, Err(err) if io_incomplete(&err)) {
        let directory = watch.handle.0;
        let pinned = Box::leak(watch);
        unsafe {
            let _ = CloseHandle(directory);
        }
        // 关掉句柄后内核仍会写回这份 OVERLAPPED。留着地址，避免写进已释放的副本。
        let _ = pinned.overlapped.Internal;
        return Err(());
    }
    Ok(watch)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DirWatch, detach_if_incomplete};
    use crate::storage::test_temp::TempDir;

    #[test]
    fn incomplete_read_leaks_the_original_overlapped_before_close() {
        let temp = TempDir::new();
        let mut owned = DirWatch::open(temp.path()).expect("watch");
        let watch = owned.inner.take().expect("inner");
        let detached = detach_if_incomplete(watch);
        assert!(detached.is_err(), "刚挂上的读应该仍是 ERROR_IO_INCOMPLETE");
        std::fs::write(temp.path().join("touch.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(300));
    }
}
