//! 开机启动要写入注册表的命令行。
//!
//! 值名和键路径在外壳里。这里只决定写、删还是不动，以及带引号的 exe 路径。

use std::path::Path;

/// 注册表 Run 项上要做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunValueAction {
    Write(String),
    Delete,
    Unchanged,
}

/// `enabled` 为假时，已有值就删掉。为真时，和 `desired` 不同才写。
#[must_use]
pub fn run_value_action(enabled: bool, desired: &str, existing: Option<&str>) -> RunValueAction {
    if enabled {
        if existing == Some(desired) {
            RunValueAction::Unchanged
        } else {
            RunValueAction::Write(desired.to_owned())
        }
    } else if existing.is_some() {
        RunValueAction::Delete
    } else {
        RunValueAction::Unchanged
    }
}

/// 带引号的 exe 路径。路径含 `"` 或不是 Unicode 时返回 `None`，调用方不要写注册表。
#[must_use]
pub fn quoted_executable(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if text.is_empty() || text.contains('"') {
        return None;
    }
    Some(format!("\"{text}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_value_tracks_the_flag() {
        let desired = r#""C:\Lanwork\lanwork.exe""#;
        assert_eq!(
            run_value_action(true, desired, None),
            RunValueAction::Write(desired.to_owned())
        );
        assert_eq!(
            run_value_action(true, desired, Some(desired)),
            RunValueAction::Unchanged
        );
        assert_eq!(
            run_value_action(false, desired, Some(desired)),
            RunValueAction::Delete
        );
        assert_eq!(
            run_value_action(false, desired, None),
            RunValueAction::Unchanged
        );
    }

    #[test]
    fn quotes_the_executable_and_rejects_an_embedded_quote() {
        assert_eq!(
            quoted_executable(Path::new(r"C:\Program Files\Lanwork\lanwork.exe")).as_deref(),
            Some(r#""C:\Program Files\Lanwork\lanwork.exe""#)
        );
        assert_eq!(quoted_executable(Path::new(r#"C:\a"b.exe"#)), None);
    }
}
