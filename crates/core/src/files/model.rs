//! 文件来源的状态和结果。
//!
//! 可见说明只有规格里的「文件索引不可用」。日志里的错误码不进这句文字。

/// 一次查询最多接收的条数。界面最多显示 20 条，那是查询调度的事。
pub const MAX_FILE_RESULTS: usize = 50;

/// 两者都不可用时，结果区显示这一句。
pub const FILE_INDEX_UNAVAILABLE: &str = "文件索引不可用";

/// Everything 的三种状态。探测失败和 DLL 缺失都算未运行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EverythingStatus {
    Ready,
    NotReady,
    NotRunning,
    /// 空白输入没有做状态检查。不是第四种产品状态。
    NotChecked,
}

/// Windows Search 的两种状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsSearchStatus {
    Available,
    Unavailable,
    /// 空白输入没有做状态检查。
    NotChecked,
}

/// 这一次结果从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSource {
    Everything,
    WindowsSearch,
    /// 两者都不可用。说明文字是 [`FILE_INDEX_UNAVAILABLE`]。
    Unavailable,
    /// 输入里没有非空白字符。没有查询。
    Blank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileQueryResult {
    pub sequence: u64,
    pub everything: EverythingStatus,
    pub windows_search: WindowsSearchStatus,
    pub source: FileSource,
    pub hits: Vec<FileHit>,
}

impl FileQueryResult {
    /// 界面只在两者都不可用时使用这句。空白输入和其他来源都没有这句。
    #[must_use]
    pub fn unavailable_label(&self) -> Option<&'static str> {
        if self.source == FileSource::Unavailable {
            Some(FILE_INDEX_UNAVAILABLE)
        } else {
            None
        }
    }
}

pub(crate) fn is_blank_query(text: &str) -> bool {
    !text.chars().any(|ch| !ch.is_whitespace())
}

/// `System.ItemType` 为 `Directory` 时标成文件夹。其余标成文件。
///
/// 这是当前实现选择。产品规格只要求标成文件或文件夹，没有写用哪一列。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn kind_from_item_type(item_type: &str) -> FileKind {
    if item_type.eq_ignore_ascii_case("Directory") {
        FileKind::Folder
    } else {
        FileKind::File
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn clamp_results(limit: usize) -> usize {
    limit.min(MAX_FILE_RESULTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_label_is_only_the_specified_sentence() {
        let result = FileQueryResult {
            sequence: 3,
            everything: EverythingStatus::NotRunning,
            windows_search: WindowsSearchStatus::Unavailable,
            source: FileSource::Unavailable,
            hits: Vec::new(),
        };
        assert_eq!(result.unavailable_label(), Some("文件索引不可用"));
        assert_eq!(FILE_INDEX_UNAVAILABLE, "文件索引不可用");
    }

    #[test]
    fn blank_and_successful_sources_have_no_unavailable_label() {
        for source in [
            FileSource::Blank,
            FileSource::Everything,
            FileSource::WindowsSearch,
        ] {
            let result = FileQueryResult {
                sequence: 1,
                everything: EverythingStatus::NotChecked,
                windows_search: WindowsSearchStatus::NotChecked,
                source,
                hits: Vec::new(),
            };
            assert_eq!(result.unavailable_label(), None);
        }
    }

    #[test]
    fn directory_item_type_is_a_folder() {
        assert_eq!(kind_from_item_type("Directory"), FileKind::Folder);
        assert_eq!(kind_from_item_type("directory"), FileKind::Folder);
        assert_eq!(kind_from_item_type(".txt"), FileKind::File);
        assert_eq!(kind_from_item_type(""), FileKind::File);
    }

    #[test]
    fn result_cap_never_exceeds_50() {
        assert_eq!(clamp_results(50), 50);
        assert_eq!(clamp_results(80), 50);
        assert_eq!(clamp_results(0), 0);
        assert_eq!(MAX_FILE_RESULTS, 50);
    }

    #[test]
    fn whitespace_only_is_blank_and_inner_spaces_are_not() {
        assert!(is_blank_query(""));
        assert!(is_blank_query(" \t\n"));
        assert!(!is_blank_query(" 季度 报告 "));
    }
}
