//! 模型、服务与存储。
//!
//! 不依赖 Slint，也不依赖 Win32 窗口 API，以便在 CI 上直接测试。
//!
//! 已接入的模块：[`search`]（搜索条前缀分类，以及应用、待办、便签共用的匹配引擎），待办收集的日期前缀解析（[`parse_todo_due_prefix`]），[`storage`]（数据目录、原子写入、损坏隔离与变更消息），[`notes`]（便签的模型、保存和命令），[`todos`]（待办清单与条目），[`shelves`]（收纳分组、路径引用和待办关联），[`github`]（`gh` 读取、快照、已定信号、转为待办和来源同步），[`apps`]（应用索引：枚举、用户目录、去重、缓存和启动），[`files`]（文件来源：Everything SDK 与 Windows Search 回退），[`dispatch`]（查询调度：待办与便签索引、结果合并、图标缓存），[`config`]（`config.json`），以及 [`shell`]（托盘菜单、热键修改顺序、图标像素、主题解析和开机启动命令行）。

pub mod apps;
pub mod config;
pub mod dispatch;
pub mod files;
pub mod github;
pub mod notes;
pub mod search;
pub mod shell;
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
