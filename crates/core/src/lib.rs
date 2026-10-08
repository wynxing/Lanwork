//! 模型、服务与存储。
//!
//! 不依赖 Slint，也不依赖 Win32 窗口 API，以便在 CI 上直接测试。
//! 已有搜索条前缀分类（[`search::classify_prefix`]）和待办收集的日期前缀解析（[`parse_todo_due_prefix`]）。

pub mod search;

mod capture;

pub use capture::{CivilDate, TodoDue, TodoDuePrefix, parse_todo_due_prefix};

#[cfg(test)]
mod tests {
    #[test]
    fn package_name() {
        let name = env!("CARGO_PKG_NAME").to_owned();
        assert_eq!(name, "lanwork-core");
    }
}
