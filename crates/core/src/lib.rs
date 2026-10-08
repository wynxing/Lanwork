//! 模型、服务与存储。
//!
//! 不依赖 Slint，也不依赖 Win32 窗口 API，以便在 CI 上直接测试。
//!
//! 已接入的模块：[`search`]（搜索条前缀分类，以及应用、待办、便签共用的匹配引擎），待办收集的日期前缀解析（[`parse_todo_due_prefix`]），[`storage`]（数据目录、原子写入、损坏隔离与变更消息），[`notes`]（便签的模型、保存和命令），以及 [`todos`]（待办清单与条目）。
//! 应用枚举、文件索引和查询调度还没有。

pub mod notes;
pub mod search;
pub mod storage;
pub mod todos;

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
