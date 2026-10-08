//! 数据目录上的文件操作。
//!
//! Windows 上把路径转成 `\\?\` 扩展路径，这样超过 MAX_PATH 的数据目录仍能读写。
//! 这里不用 Win32 窗口 API。

use std::io;
use std::path::{Path, PathBuf};

pub fn create_dir_all(path: &Path) -> io::Result<()> {
    let path = io_path(path)?;
    if exists(&path) {
        return Ok(());
    }
    let mut pending = Vec::new();
    let mut current = path.clone();
    loop {
        if exists(&current) {
            break;
        }
        pending.push(current.clone());
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    while let Some(dir) = pending.pop() {
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(io_path(path)?)
}

pub fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(io_path(path)?) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let from = io_path(from)?;
    let to = io_path(to)?;
    #[cfg(windows)]
    {
        // 隔离损坏文件时不用 REPLACE，避免盖掉已经隔离的另一份。
        windows_move(&from, &to, 0)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }
}

pub fn exists(path: &Path) -> bool {
    match io_path(path) {
        Ok(path) => path.try_exists().unwrap_or(false),
        Err(_) => false,
    }
}

pub fn read_dir(path: &Path) -> io::Result<Vec<DirEntry>> {
    let path = io_path(path)?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        entries.push(DirEntry {
            path: logical_path(&entry.path()),
            is_file: file_type.is_file(),
        });
    }
    Ok(entries)
}

/// 去掉 Windows 扩展前缀，错误信息和隔离记录仍用普通路径。
pub(crate) fn logical_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

pub fn file_len(path: &Path) -> io::Result<u64> {
    Ok(std::fs::metadata(io_path(path)?)?.len())
}

pub fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let path = io_path(path)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(())
}

#[derive(Debug)]
pub struct DirEntry {
    pub path: PathBuf,
    pub is_file: bool,
}

pub(crate) fn io_path(path: &Path) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        windows_extended(path)
    }
    #[cfg(not(windows))]
    {
        Ok(super::paths::normalize_lexical(&make_absolute(path)?))
    }
}

#[cfg(not(windows))]
fn make_absolute(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[cfg(windows)]
fn windows_extended(path: &Path) -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let absolute = super::paths::normalize_lexical(&absolute);
    let mut wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    for unit in &mut wide {
        if *unit == b'/' as u16 {
            *unit = b'\\' as u16;
        }
    }
    const PREFIX: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    if wide.starts_with(&PREFIX) {
        return Ok(absolute);
    }
    while wide.len() > 3 && wide.last() == Some(&(b'\\' as u16)) {
        wide.pop();
    }
    let mut prefixed = Vec::with_capacity(wide.len() + 8);
    if wide.starts_with(&[b'\\' as u16, b'\\' as u16]) {
        prefixed.extend_from_slice(&PREFIX);
        prefixed.extend_from_slice(&[b'U' as u16, b'N' as u16, b'C' as u16, b'\\' as u16]);
        prefixed.extend_from_slice(&wide[2..]);
    } else {
        prefixed.extend_from_slice(&PREFIX);
        prefixed.extend_from_slice(&wide);
    }
    Ok(PathBuf::from(OsString::from_wide(&prefixed)))
}

#[cfg(windows)]
fn windows_move(from: &Path, to: &Path, flags: u32) -> io::Result<()> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let from = wide_null(from);
    let to = wide_null(to);
    // SAFETY: 两个缓冲区在调用期间有效，并且以 0 结尾。flags 只包含 MoveFileEx 的文件标志。
    let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        Err(io::Error::from_raw_os_error(code as i32))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn wide_null(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
