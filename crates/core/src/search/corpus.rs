//! 基准和测试用的固定语料。不是产品入口。

use super::prepare::{FieldInput, FieldRole, MatchIndex};

const ZH: &[&str] = &[
    "微信",
    "钉钉",
    "网易云音乐",
    "计算器",
    "记事本",
    "浏览器",
    "回收站",
    "控制面板",
    "重庆",
    "银行",
    "重要",
    "行程",
    "邮件",
    "日历",
    "相机",
    "设置",
];

const EN: &[&str] = &[
    "Visual Studio Code",
    "Google Chrome",
    "Notepad",
    "Windows Terminal",
    "Microsoft Edge",
    "Task Manager",
    "File Explorer",
    "Paint",
];

const TODOS: &[&str] = &[
    "给微信回复",
    "提交 Visual Studio Code 补丁",
    "重写说明",
    "银行对账",
    "买牛奶",
    "整理行程",
];

/// 生成 `app_count` 个应用名和 `todo_count` 条待办标题。
///
/// 序号 0 固定是「微信」，序号 1 固定是 `Visual Studio Code`。
/// 待办从 `app_count` 起编号，第一条固定是「给微信回复」。
#[doc(hidden)]
#[must_use]
pub fn benchmark_corpus(app_count: usize, todo_count: usize) -> MatchIndex {
    let mut index = MatchIndex::new();
    for index_id in 0..app_count {
        let name = app_name(index_id);
        index.insert(
            index_id as u64,
            &[FieldInput {
                role: FieldRole::Name,
                text: &name,
            }],
        );
    }
    for index_id in 0..todo_count {
        let title = todo_title(index_id);
        index.insert(
            (app_count + index_id) as u64,
            &[FieldInput {
                role: FieldRole::Name,
                text: &title,
            }],
        );
    }
    index
}

fn app_name(index: usize) -> String {
    match index {
        0 => "微信".to_owned(),
        1 => "Visual Studio Code".to_owned(),
        _ => match index % 4 {
            0 => format!("{}{index}", ZH[index % ZH.len()]),
            1 => format!("{} {index}", EN[index % EN.len()]),
            2 => format!("{} {}", ZH[index % ZH.len()], EN[index % EN.len()]),
            _ => ZH[index % ZH.len()].to_owned(),
        },
    }
}

fn todo_title(index: usize) -> String {
    let base = TODOS[index % TODOS.len()];
    if index == 0 {
        base.to_owned()
    } else if index.is_multiple_of(5) {
        format!("{base}{index}")
    } else {
        base.to_owned()
    }
}
