//! 恢复和更换数据目录时，工具窗口先停住。
//!
//! 界面在进入后台线程之前关掉窗口。这里只决定停住之后要不要重开，以及能不能把 `open` 写回去。

/// 停住期间退出和关闭都不把 `open` 写回。
#[must_use]
pub fn persist_open_allowed(frozen: bool) -> bool {
    !frozen
}

/// 导入成功后不重开。失败时数据没换，重开原来开着的窗口。
#[must_use]
pub fn reopen_after_import(open: &[String], success: bool) -> Vec<String> {
    if success { Vec::new() } else { open.to_vec() }
}

/// 更换数据目录之后，从新目录重开切换前开着的窗口。
#[must_use]
pub fn reopen_after_migrate(open: &[String]) -> Vec<String> {
    open.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_does_not_reopen_and_migrate_does() {
        let open = vec!["example.quota".to_owned(), "example.other".to_owned()];
        assert!(reopen_after_import(&open, true).is_empty());
        assert_eq!(reopen_after_import(&open, false), open);
        assert_eq!(reopen_after_migrate(&open), open);
        assert!(!persist_open_allowed(true));
        assert!(persist_open_allowed(false));
    }
}
