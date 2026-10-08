//! 同目录临时文件 + 刷盘 + 替换。
//!
//! 目标已存在时用 `ReplaceFileW(REPLACEFILE_WRITE_THROUGH)`。
//! 目标不存在时用 `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`。
//! 替换失败时删除临时文件，不改原文件。

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::fsutil;

pub fn atomic_write(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let dest = fsutil::io_path(dest)?;
    let tmp = sibling_tmp(&dest)?;
    let write_result = write_temp(&tmp, bytes);
    if let Err(err) = write_result {
        let _ = fsutil::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = replace_file(&tmp, &dest) {
        let _ = fsutil::remove_file(&tmp);
        return Err(err);
    }
    let _ = fsutil::remove_file(&tmp);
    Ok(())
}

fn write_temp(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = tmp.parent() {
        fsutil::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(fsutil::io_path(tmp)?)?;
    file.write_all(bytes)?;
    flush_file_buffers(&file)?;
    Ok(())
}

fn sibling_tmp(dest: &Path) -> io::Result<PathBuf> {
    let name = dest
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    let mut tmp_name = std::ffi::OsString::from(name);
    tmp_name.push(".tmp");
    let parent = dest
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))?;
    Ok(parent.join(tmp_name))
}

#[cfg(windows)]
fn flush_file_buffers(file: &std::fs::File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;

    let handle = file.as_raw_handle() as HANDLE;
    // SAFETY: handle 属于这个仍打开的文件，FlushFileBuffers 只要求可写的文件句柄。
    let ok = unsafe { FlushFileBuffers(handle) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        Err(io::Error::from_raw_os_error(code as i32))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn flush_file_buffers(file: &std::fs::File) -> io::Result<()> {
    // 产品路径在 Windows 上调用 FlushFileBuffers。这里只让非 Windows 测试把数据落到盘上。
    file.sync_all()
}

#[cfg(windows)]
fn replace_file(tmp: &Path, dest: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
        ReplaceFileW,
    };

    const ERROR_FILE_NOT_FOUND: u32 = 2;
    let dest_w = fsutil::wide_null(dest);
    let tmp_w = fsutil::wide_null(tmp);
    if fsutil::exists(dest) {
        // SAFETY: 路径缓冲区以 0 结尾且在调用期间有效。备份文件名为空。
        // 不在共享冲突时改试 MoveFileEx：占用中的原文件必须保持不变。
        let ok = unsafe {
            ReplaceFileW(
                dest_w.as_ptr(),
                tmp_w.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok != 0 {
            return Ok(());
        }
        let code = unsafe { GetLastError() };
        if code != ERROR_FILE_NOT_FOUND {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
    }
    // SAFETY: 同上。目标不存在（或刚被删掉）时把临时文件移成正式文件。
    let ok = unsafe {
        MoveFileExW(
            tmp_w.as_ptr(),
            dest_w.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        Err(io::Error::from_raw_os_error(code as i32))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(tmp: &Path, dest: &Path) -> io::Result<()> {
    std::fs::rename(tmp, dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_temp::TempDir;

    #[test]
    fn replace_keeps_previous_bytes_readable_as_one_value() {
        let temp = TempDir::new();
        let dest = temp.path().join("item.json");
        atomic_write(&dest, b"one").unwrap();
        atomic_write(&dest, b"two").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"two");
        assert!(!temp.path().join("item.json.tmp").exists());
    }
}
