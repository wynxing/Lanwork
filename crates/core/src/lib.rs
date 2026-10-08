//! 模型、服务与存储。
//!
//! 不依赖 Slint，也不依赖 Win32 窗口 API，以便在 CI 上直接测试。
//!
//! 已接入的模块：[`search`]（搜索条前缀分类，以及应用、待办、便签共用的匹配引擎），待办收集的日期前缀解析（[`parse_todo_due_prefix`]），[`storage`]（数据目录、原子写入、损坏隔离与变更消息），[`notes`]（便签的模型、保存和命令），[`todos`]（待办清单与条目），[`shelves`]（收纳分组、路径引用和待办关联），[`github`]（`gh` 读取、快照、已定信号、转为待办和来源同步），以及 [`apps`]（应用索引：枚举、去重、缓存和启动）。
//! 文件索引和查询调度还没有。

pub mod apps;
pub mod github;
pub mod notes;
pub mod search;
pub mod shelves;
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
