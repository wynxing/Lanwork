//! SDK3 结果里的文件名和完整路径。不加载 DLL。
//!
//! `GetResultNameW` 读 `NAME`（0）。`GetResultFullPathNameW` 读 `PATH_AND_NAME`（240）。
//! 没有请求任何属性时，结果自带 `PATH_AND_NAME`。请求了别的属性，这项默认值就没了。
//! 所以两项都要请求。路径为空的命中不收下。

use super::model::{FileHit, FileKind};

/// `EVERYTHING3_PROPERTY_ID_NAME`。SDK 3.0.0.9。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const SDK3_PROPERTY_NAME: u32 = 0;

/// `EVERYTHING3_PROPERTY_ID_PATH_AND_NAME`。SDK 3.0.0.9。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const SDK3_PROPERTY_PATH_AND_NAME: u32 = 240;

/// 同时请求完整路径和文件名。只请求 [`SDK3_PROPERTY_NAME`] 时，完整路径为空。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn sdk3_property_requests() -> &'static [u32] {
    &[SDK3_PROPERTY_PATH_AND_NAME, SDK3_PROPERTY_NAME]
}

/// 路径为空则不收下。文件名为空时，从完整路径的最后一段取。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn everything_hit(name: String, path: String, folder: bool) -> Option<FileHit> {
    if path.is_empty() {
        return None;
    }
    let name = if name.is_empty() {
        path.rsplit(['\\', '/']).next().unwrap_or("").to_owned()
    } else {
        name
    };
    if name.is_empty() {
        return None;
    }
    Some(FileHit {
        name,
        path,
        kind: if folder {
            FileKind::Folder
        } else {
            FileKind::File
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk3_requests_both_name_and_full_path() {
        let ids = sdk3_property_requests();
        assert!(ids.contains(&SDK3_PROPERTY_PATH_AND_NAME));
        assert!(ids.contains(&SDK3_PROPERTY_NAME));
        assert_ne!(ids, [SDK3_PROPERTY_NAME].as_slice());
    }

    #[test]
    fn name_only_does_not_cover_the_full_path() {
        assert!(!covers_name_and_full_path(&[SDK3_PROPERTY_NAME]));
        assert!(covers_name_and_full_path(sdk3_property_requests()));
    }

    #[test]
    fn empty_path_is_dropped_even_when_the_name_is_present() {
        assert!(everything_hit("note.txt".to_owned(), String::new(), false).is_none());
    }

    #[test]
    fn name_and_full_path_are_both_kept() {
        let hit = everything_hit("note.txt".to_owned(), r"C:\work\note.txt".to_owned(), false)
            .expect("hit");
        assert_eq!(hit.name, "note.txt");
        assert_eq!(hit.path, r"C:\work\note.txt");
        assert_eq!(hit.kind, FileKind::File);
    }

    #[test]
    fn missing_name_comes_from_the_full_path() {
        let hit = everything_hit(String::new(), r"C:\work\note.txt".to_owned(), true).expect("hit");
        assert_eq!(hit.name, "note.txt");
        assert_eq!(hit.path, r"C:\work\note.txt");
        assert_eq!(hit.kind, FileKind::Folder);
    }

    #[test]
    fn empty_name_and_empty_path_are_dropped() {
        assert!(everything_hit(String::new(), String::new(), false).is_none());
    }

    fn covers_name_and_full_path(ids: &[u32]) -> bool {
        ids.contains(&SDK3_PROPERTY_NAME) && ids.contains(&SDK3_PROPERTY_PATH_AND_NAME)
    }
}
