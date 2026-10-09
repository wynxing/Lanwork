//! 当前用户和公共开始菜单，加上测量用目录。

use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartMenu, FOLDERID_CommonStartup, FOLDERID_StartMenu, FOLDERID_Startup,
    KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath,
};

use super::shortcut::read_indexed_file;
use super::store::take_pwstr;
use crate::apps::AppEntry;
use crate::apps::rules::is_excluded_directory;

const MAX_DEPTH: u32 = 16;

pub(crate) fn read_user() -> Result<Vec<AppEntry>, String> {
    read_known(&FOLDERID_StartMenu)
}

pub(crate) fn read_common() -> Result<Vec<AppEntry>, String> {
    read_known(&FOLDERID_CommonStartMenu)
}

pub(crate) fn read_shortcut_dir_excluding(
    dir: &Path,
    excluded: &[PathBuf],
) -> Result<Vec<AppEntry>, String> {
    let mut files = Vec::new();
    collect(dir, &mut files, 0, excluded)?;
    Ok(files
        .iter()
        .filter_map(|path| read_indexed_file(path))
        .collect())
}

pub(crate) fn startup_folders() -> Vec<PathBuf> {
    [FOLDERID_Startup, FOLDERID_CommonStartup]
        .into_iter()
        .filter_map(|id| known_folder(&id).ok())
        .collect()
}

pub(crate) fn watch_directories(extra: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(path) = known_folder(&FOLDERID_StartMenu) {
        dirs.push(path);
    }
    if let Ok(path) = known_folder(&FOLDERID_CommonStartMenu) {
        dirs.push(path);
    }
    if let Some(extra) = extra
        && extra.is_dir()
    {
        dirs.push(extra.to_path_buf());
    }
    dirs
}

fn read_known(id: &windows::core::GUID) -> Result<Vec<AppEntry>, String> {
    let dir = known_folder(id)?;
    read_shortcut_dir_excluding(&dir, &startup_folders())
}

fn known_folder(id: &windows::core::GUID) -> Result<PathBuf, String> {
    // SAFETY: id 指向静态 GUID。DONT_VERIFY 避免函数去创建目录。字符串由 take_pwstr 释放。
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, None) }
        .map_err(|err| format!("开始菜单目录不可用: {err}"))?;
    let text = take_pwstr(raw);
    if text.is_empty() {
        Err("开始菜单目录为空".to_owned())
    } else {
        Ok(PathBuf::from(text))
    }
}

fn collect(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    depth: u32,
    excluded: &[PathBuf],
) -> Result<(), String> {
    if depth > MAX_DEPTH || is_excluded_directory(dir, excluded) {
        return Ok(());
    }
    let iter = std::fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for item in iter.flatten() {
        let path = item.path();
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if is_excluded_directory(&path, excluded) {
                continue;
            }
            let _ = collect(&path, out, depth + 1, excluded);
        } else if is_indexed_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_indexed_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk") || ext.eq_ignore_ascii_case("url"))
}
