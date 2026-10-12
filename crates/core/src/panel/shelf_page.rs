//! 收纳页的数据整理：分组行、引用行、选中、排序换位、关联待办时的筛选。
//!
//! 输入是 [`ShelfCommands::list`](crate::shelves::ShelfCommands::list) 和
//! [`TodoCommands::lists`](crate::todos::TodoCommands::lists) 的快照。这里不读写盘，不访问用户文件，
//! 规则仍在收纳服务里。

use std::collections::HashSet;

use crate::shelves::{Shelf, ShelfRef};
use crate::todos::{TodoItem, TodoList};

/// 左侧分组一行。`todo_titles` 是各条关联待办的标题，按关联的先后；找不到的待办不在其中。
/// 多条标题怎么排版产品规格没有写，由界面决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub id: String,
    pub name: String,
    pub count: usize,
    pub todo_titles: Vec<String>,
}

/// 分组关联的一条待办，以及它现在的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedTodo {
    pub id: String,
    /// 找不到这条待办时为空。
    pub title: String,
    pub found: bool,
    pub completed: bool,
    pub in_trash: bool,
}

/// 一条待选的待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoChoice {
    pub id: String,
    pub title: String,
    pub list_name: String,
}

fn find_todo<'a>(lists: &'a [TodoList], id: &str) -> Option<&'a TodoItem> {
    lists
        .iter()
        .flat_map(|list| &list.items)
        .find(|item| item.id == id)
}

/// 分组按服务给出的顺序。关联的待办不论完成或在回收站，都显示它的标题，因为完成和进入回收站不改变分组。
#[must_use]
pub fn group_rows(shelves: &[Shelf], lists: &[TodoList]) -> Vec<GroupRow> {
    shelves
        .iter()
        .map(|shelf| GroupRow {
            id: shelf.id.clone(),
            name: shelf.name.clone(),
            count: shelf.refs.len(),
            todo_titles: shelf
                .todo_ids
                .iter()
                .filter_map(|id| find_todo(lists, id))
                .map(|item| item.title.clone())
                .collect(),
        })
        .collect()
}

/// 一个分组关联的各条待办（`Shelf::todo_ids`），按关联的先后，包括找不到的。排序产品规格没有写，这是实现选择。
#[must_use]
pub fn linked_todos(todo_ids: &[String], lists: &[TodoList]) -> Vec<LinkedTodo> {
    todo_ids
        .iter()
        .map(|id| match find_todo(lists, id) {
            Some(item) => LinkedTodo {
                id: id.clone(),
                title: item.title.clone(),
                found: true,
                completed: item.completed,
                in_trash: item.in_trash(),
            },
            None => LinkedTodo {
                id: id.clone(),
                title: String::new(),
                found: false,
                completed: false,
                in_trash: false,
            },
        })
        .collect()
}

/// 保留原来选中的分组；它不在了就选第一个；没有分组时为空。
/// 有分组时总有一个选中，这样拖入总有明确的目标。
#[must_use]
pub fn pick_selection(shelves: &[Shelf], previous: Option<&str>) -> Option<String> {
    previous
        .filter(|id| shelves.iter().any(|shelf| shelf.id == *id))
        .map(str::to_owned)
        .or_else(|| shelves.first().map(|shelf| shelf.id.clone()))
}

/// 关联了这条待办的分组里排在最前的一个。关联多个分组时选第一个。
#[must_use]
pub fn first_group_for_todo<'a>(shelves: &'a [Shelf], todo_id: &str) -> Option<&'a str> {
    shelves
        .iter()
        .find(|shelf| shelf.todo_ids.iter().any(|id| id == todo_id))
        .map(|shelf| shelf.id.as_str())
}

/// 被至少一个分组关联的待办 id，待办页据此显示标记。
#[must_use]
pub fn linked_todo_ids(shelves: &[Shelf]) -> HashSet<String> {
    shelves
        .iter()
        .flat_map(|shelf| shelf.todo_ids.iter().cloned())
        .collect()
}

/// 把分组上移或下移一格之后的全部 id。已经在端点时为空。
#[must_use]
pub fn reordered_group_ids(shelves: &[Shelf], id: &str, up: bool) -> Option<Vec<String>> {
    let at = shelves.iter().position(|shelf| shelf.id == id)?;
    let other = if up { at.checked_sub(1)? } else { at + 1 };
    if other >= shelves.len() {
        return None;
    }
    let mut ids: Vec<String> = shelves.iter().map(|shelf| shelf.id.clone()).collect();
    ids.swap(at, other);
    Some(ids)
}

/// 关联待办时可选的条目：未完成且不在回收站。关键字去掉首尾空白后，按不区分大小写的子串匹配标题；
/// 关键字为空时列出全部。待办按清单顺序、清单内按服务给出的顺序。
///
/// 可选范围和匹配方式产品规格没有写，这是实现选择。
#[must_use]
pub fn todo_choices(lists: &[TodoList], keyword: &str) -> Vec<TodoChoice> {
    let needle = keyword.trim().to_lowercase();
    lists
        .iter()
        .flat_map(|list| list.items.iter().map(move |item| (list, item)))
        .filter(|(_, item)| !item.completed && !item.in_trash())
        .filter(|(_, item)| needle.is_empty() || item.title.to_lowercase().contains(&needle))
        .map(|(list, item)| TodoChoice {
            id: item.id.clone(),
            title: item.title.clone(),
            list_name: list.name.clone(),
        })
        .collect()
}

/// 当前分组里第 `first` 行起的至多 `count` 条引用的路径。只给可见的行用。
#[must_use]
pub fn visible_paths(refs: &[ShelfRef], first: usize, count: usize) -> Vec<String> {
    refs.iter()
        .skip(first)
        .take(count)
        .map(|item| item.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::{ListKind, TodoItem};

    fn shelf(id: &str, todos: &[&str], paths: &[&str]) -> Shelf {
        Shelf {
            id: id.to_owned(),
            name: format!("分组 {id}"),
            order: 0,
            todo_ids: todos.iter().map(|id| (*id).to_owned()).collect(),
            refs: paths
                .iter()
                .map(|path| ShelfRef {
                    path: (*path).to_owned(),
                    name: (*path).to_owned(),
                    folder: false,
                    added_at: 0,
                })
                .collect(),
            float: None,
            float_dock: None,
        }
    }

    fn todo(id: &str, title: &str) -> TodoItem {
        TodoItem::new(id.to_owned(), title.to_owned(), 0)
    }

    fn list(id: &str, name: &str, items: Vec<TodoItem>) -> TodoList {
        let mut list = TodoList::new(id.to_owned(), name.to_owned(), ListKind::Normal, 0);
        list.items = items;
        list
    }

    #[test]
    fn group_rows_show_count_and_the_linked_todo_title_even_when_it_is_done_or_trashed() {
        let mut done = todo("t-done", "已完成的");
        done.completed = true;
        let mut trashed = todo("t-trash", "回收站里的");
        trashed.deleted_at = Some(1);
        let lists = [list(
            "l",
            "清单",
            vec![todo("t-open", "写周报"), done, trashed],
        )];
        let shelves = [
            shelf("a", &["t-open"], &["C:\\a", "C:\\b"]),
            shelf("b", &["t-done"], &[]),
            shelf("c", &["t-trash"], &[]),
            shelf("d", &["t-gone"], &[]),
            shelf("e", &[], &["C:\\x"]),
            shelf("f", &["t-done", "t-gone", "t-open"], &[]),
        ];
        let rows = group_rows(&shelves, &lists);
        assert_eq!(
            rows.iter()
                .map(|row| (row.count, row.todo_titles.join("|")))
                .collect::<Vec<_>>(),
            [
                (2, "写周报".to_owned()),
                (0, "已完成的".to_owned()),
                (0, "回收站里的".to_owned()),
                (0, String::new()),
                (1, String::new()),
                (0, "已完成的|写周报".to_owned()),
            ]
        );
    }

    #[test]
    fn linked_todos_keep_the_link_order_and_say_what_state_each_is_in() {
        let mut done = todo("t-done", "已完成的");
        done.completed = true;
        let mut trashed = todo("t-trash", "回收站里的");
        trashed.deleted_at = Some(1);
        let lists = [list(
            "l",
            "清单",
            vec![todo("t-open", "写周报"), done, trashed],
        )];
        let group = shelf("a", &["t-trash", "t-gone", "t-open", "t-done"], &[]);
        let rows = linked_todos(&group.todo_ids, &lists);
        assert_eq!(
            rows.iter()
                .map(|row| (
                    row.id.as_str(),
                    row.title.as_str(),
                    row.found,
                    row.completed,
                    row.in_trash
                ))
                .collect::<Vec<_>>(),
            [
                ("t-trash", "回收站里的", true, false, true),
                ("t-gone", "", false, false, false),
                ("t-open", "写周报", true, false, false),
                ("t-done", "已完成的", true, true, false),
            ]
        );
    }

    #[test]
    fn selection_keeps_the_previous_group_or_falls_back_to_the_first() {
        let shelves = [shelf("a", &[], &[]), shelf("b", &[], &[])];
        assert_eq!(pick_selection(&shelves, Some("b")).as_deref(), Some("b"));
        assert_eq!(pick_selection(&shelves, Some("zzz")).as_deref(), Some("a"));
        assert_eq!(pick_selection(&shelves, None).as_deref(), Some("a"));
        assert_eq!(pick_selection(&[], Some("a")), None);
    }

    #[test]
    fn open_linked_shelf_picks_the_first_group_that_links_the_todo() {
        let shelves = [
            shelf("a", &[], &[]),
            shelf("b", &["t1"], &[]),
            shelf("c", &["t1"], &[]),
            shelf("d", &["t2"], &[]),
            shelf("e", &["t3", "t1"], &[]),
        ];
        assert_eq!(first_group_for_todo(&shelves, "t1"), Some("b"));
        assert_eq!(first_group_for_todo(&shelves, "t3"), Some("e"));
        assert_eq!(first_group_for_todo(&shelves, "t2"), Some("d"));
        assert_eq!(first_group_for_todo(&shelves, "t4"), None);
        let linked = linked_todo_ids(&shelves);
        assert!(linked.contains("t1") && linked.contains("t2") && linked.contains("t3"));
        assert_eq!(linked.len(), 3);
    }

    #[test]
    fn reorder_swaps_neighbours_and_stops_at_the_ends() {
        let shelves = [
            shelf("a", &[], &[]),
            shelf("b", &[], &[]),
            shelf("c", &[], &[]),
        ];
        assert_eq!(
            reordered_group_ids(&shelves, "b", true).unwrap(),
            ["b", "a", "c"]
        );
        assert_eq!(
            reordered_group_ids(&shelves, "b", false).unwrap(),
            ["a", "c", "b"]
        );
        assert_eq!(reordered_group_ids(&shelves, "a", true), None);
        assert_eq!(reordered_group_ids(&shelves, "c", false), None);
        assert_eq!(reordered_group_ids(&shelves, "zzz", true), None);
    }

    #[test]
    fn choices_are_open_todos_filtered_by_a_case_insensitive_substring() {
        let mut done = todo("t3", "Weekly report done");
        done.completed = true;
        let mut trashed = todo("t4", "Weekly report trashed");
        trashed.deleted_at = Some(1);
        let lists = [
            list(
                "l1",
                "收件箱",
                vec![todo("t1", "Weekly Report"), todo("t2", "买牛奶"), done],
            ),
            list("l2", "工作", vec![trashed, todo("t5", "report 草稿")]),
        ];
        let all = todo_choices(&lists, "  ");
        assert_eq!(
            all.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["t1", "t2", "t5"]
        );
        let hit = todo_choices(&lists, " REPORT ");
        assert_eq!(
            hit.iter()
                .map(|c| (c.id.as_str(), c.list_name.as_str()))
                .collect::<Vec<_>>(),
            [("t1", "收件箱"), ("t5", "工作")]
        );
        assert!(todo_choices(&lists, "不存在").is_empty());
    }

    #[test]
    fn visible_paths_take_only_the_visible_window() {
        let shelf = shelf("a", &[], &["1", "2", "3", "4"]);
        assert_eq!(visible_paths(&shelf.refs, 1, 2), ["2", "3"]);
        assert_eq!(visible_paths(&shelf.refs, 3, 10), ["4"]);
        assert!(visible_paths(&shelf.refs, 9, 2).is_empty());
    }
}
