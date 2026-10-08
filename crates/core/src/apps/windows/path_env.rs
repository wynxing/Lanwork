//! PATH 里的 `.exe`。打不开的目录跳过。UNC 路径跳过，避免一个断开的网络目录挡住其它来源。

use std::path::Path;

use super::comutil;
use crate::apps::{AppEntry, AppSource, LaunchTarget};

pub(crate) fn read_path_entries() -> Result<Vec<AppEntry>, String> {
    let Some(raw) = std::env::var_os("PATH") else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    let mut ok_dirs = 0u32;
    let mut failed_dirs = 0u32;
    for dir in std::env::split_paths(&raw) {
        if dir.as_os_str().is_empty() || is_unc(&dir) {
            continue;
        }
        match read_dir_exes(&dir) {
            Ok(found) => {
                ok_dirs += 1;
                entries.extend(found);
            }
            Err(_) => failed_dirs += 1,
        }
    }
    if ok_dirs == 0 && failed_dirs > 0 {
        return Err("PATH 里的目录都没有读到".to_owned());
    }
    Ok(entries)
}

fn read_dir_exes(dir: &Path) -> Result<Vec<AppEntry>, ()> {
    let iter = std::fs::read_dir(dir).map_err(|_| ())?;
    let mut entries = Vec::new();
    for item in iter.flatten() {
        let path = item.path();
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if !file_type.is_file() || !is_exe(&path) || !comutil::is_existing_file(&path) {
            continue;
        }
        let Some(name) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
        else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        entries.push(AppEntry {
            name,
            source: AppSource::Path,
            target: LaunchTarget::Path {
                path,
                args: String::new(),
                working_directory: None,
            },
            icon_path: None,
            icon_index: 0,
        });
    }
    Ok(entries)
}

fn is_exe(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

fn is_unc(dir: &Path) -> bool {
    let lower = dir
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    lower.starts_with(r"\\?\unc\") || (lower.starts_with(r"\\") && !lower.starts_with(r"\\?\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unc_paths_are_skipped_and_local_device_paths_are_not() {
        assert!(is_unc(Path::new(r"\\server\share\bin")));
        assert!(is_unc(Path::new(r"\\?\UNC\server\share")));
        assert!(!is_unc(Path::new(r"C:\Windows\System32")));
        assert!(!is_unc(Path::new(r"\\?\C:\Windows\System32")));
    }
}
