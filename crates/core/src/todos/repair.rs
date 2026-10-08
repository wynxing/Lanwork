//! `movedAt` 与 `currentSince` 的加载修复。
//!
//! 纯函数只改内存中的清单。调用方按返回的清单 id 写回，并用批次在全部写完后发布变更。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use super::model::TodoList;

/// 同一条目 id 出现多次时，保留 `movedAt` 较新的一条。
///
/// 时间相同，或都没有 `movedAt` 时，保留清单 id 较小的一条；
/// 仍相同则保留该清单里靠前的一条。其余从清单中移除。
pub(crate) fn repair_moved_at(lists: &mut [TodoList]) -> BTreeSet<String> {
    let mut locations: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for (list_index, list) in lists.iter().enumerate() {
        for (item_index, item) in list.items.iter().enumerate() {
            locations
                .entry(item.id.clone())
                .or_default()
                .push((list_index, item_index));
        }
    }
    let mut remove: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for group in locations.values() {
        if group.len() < 2 {
            continue;
        }
        let winner = group
            .iter()
            .copied()
            .max_by(|&left, &right| compare_moved(lists, left, right))
            .expect("duplicate group is non-empty");
        for &pos in group {
            if pos != winner {
                remove.entry(pos.0).or_default().insert(pos.1);
            }
        }
    }
    remove_items(lists, remove)
}

/// 多条当前标记时，保留 `currentSince` 最新的一条，其余清除。
///
/// 没有 `currentSince` 的视为更旧。时间相同则保留清单 id、条目 id 较小的一条。
/// 只有一条时不改写，即使它没有 `currentSince`。
pub(crate) fn repair_current_since(lists: &mut [TodoList]) -> BTreeSet<String> {
    let mut currents = Vec::new();
    for (list_index, list) in lists.iter().enumerate() {
        for (item_index, item) in list.items.iter().enumerate() {
            if item.current {
                currents.push((list_index, item_index));
            }
        }
    }
    if currents.len() <= 1 {
        return BTreeSet::new();
    }
    let winner = currents
        .iter()
        .copied()
        .max_by(|&left, &right| compare_current(lists, left, right))
        .expect("more than one current item");
    let mut changed = BTreeSet::new();
    for (list_index, item_index) in currents {
        if (list_index, item_index) == winner {
            continue;
        }
        let item = &mut lists[list_index].items[item_index];
        item.current = false;
        item.current_since = None;
        changed.insert(lists[list_index].id.clone());
    }
    changed
}

fn remove_items(
    lists: &mut [TodoList],
    remove: BTreeMap<usize, BTreeSet<usize>>,
) -> BTreeSet<String> {
    let mut changed = BTreeSet::new();
    for (list_index, indexes) in remove {
        let mut ordered: Vec<_> = indexes.into_iter().collect();
        ordered.sort_unstable_by(|left, right| right.cmp(left));
        let list = &mut lists[list_index];
        for item_index in ordered {
            list.items.remove(item_index);
        }
        changed.insert(list.id.clone());
    }
    changed
}

fn compare_moved(lists: &[TodoList], left: (usize, usize), right: (usize, usize)) -> Ordering {
    let left_item = &lists[left.0].items[left.1];
    let right_item = &lists[right.0].items[right.1];
    compare_stamp(left_item.moved_at, right_item.moved_at)
        .then_with(|| lists[right.0].id.cmp(&lists[left.0].id))
        .then_with(|| right.1.cmp(&left.1))
}

fn compare_current(lists: &[TodoList], left: (usize, usize), right: (usize, usize)) -> Ordering {
    let left_item = &lists[left.0].items[left.1];
    let right_item = &lists[right.0].items[right.1];
    compare_stamp(left_item.current_since, right_item.current_since)
        .then_with(|| lists[right.0].id.cmp(&lists[left.0].id))
        .then_with(|| right_item.id.cmp(&left_item.id))
}

fn compare_stamp(left: Option<i64>, right: Option<i64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::model::{ListKind, TodoItem, TodoList};

    fn list(id: &str, items: Vec<TodoItem>) -> TodoList {
        let mut list = TodoList::new(id.into(), id.into(), ListKind::Normal, 0);
        list.items = items;
        list
    }

    fn item(id: &str) -> TodoItem {
        TodoItem::new(id.into(), id.into(), 0)
    }

    #[test]
    fn newer_moved_at_wins_and_missing_stamp_loses() {
        let mut older = item("same");
        older.moved_at = Some(1);
        older.title = "旧".into();
        let mut newer = item("same");
        newer.moved_at = Some(2);
        newer.title = "新".into();
        let mut lists = vec![list("a", vec![older]), list("b", vec![newer])];
        let changed = repair_moved_at(&mut lists);
        assert_eq!(changed.iter().collect::<Vec<_>>(), vec!["a"]);
        assert!(lists[0].items.is_empty());
        assert_eq!(lists[1].items[0].title, "新");

        let mut stamped = item("same");
        stamped.moved_at = Some(1);
        let plain = item("same");
        let mut lists = vec![list("b", vec![plain]), list("a", vec![stamped])];
        repair_moved_at(&mut lists);
        assert!(lists[0].items.is_empty());
        assert_eq!(lists[1].items.len(), 1);
    }

    #[test]
    fn equal_moved_at_keeps_the_smaller_list_id() {
        let mut left = item("same");
        left.moved_at = Some(5);
        left.title = "甲".into();
        let mut right = item("same");
        right.moved_at = Some(5);
        right.title = "乙".into();
        let mut lists = vec![list("b", vec![right]), list("a", vec![left])];
        repair_moved_at(&mut lists);
        assert!(lists[0].items.is_empty());
        assert_eq!(lists[1].items[0].title, "甲");
    }

    #[test]
    fn newest_current_since_wins_and_a_single_flag_is_kept() {
        let mut older = item("a");
        older.current = true;
        older.current_since = Some(1);
        let mut newer = item("b");
        newer.current = true;
        newer.current_since = Some(9);
        let mut bare = item("c");
        bare.current = true;
        let mut lists = vec![list("l", vec![older, newer, bare])];
        let changed = repair_current_since(&mut lists);
        assert!(changed.contains("l"));
        assert!(!lists[0].items[0].current);
        assert!(lists[0].items[0].current_since.is_none());
        assert!(lists[0].items[1].current);
        assert_eq!(lists[0].items[1].current_since, Some(9));
        assert!(!lists[0].items[2].current);

        let mut only = item("a");
        only.current = true;
        let mut lists = vec![list("l", vec![only])];
        assert!(repair_current_since(&mut lists).is_empty());
        assert!(lists[0].items[0].current);
        assert!(lists[0].items[0].current_since.is_none());
    }

    #[test]
    fn equal_current_since_keeps_the_smaller_list_id_then_the_smaller_item_id() {
        let mut on_a = item("m");
        on_a.current = true;
        on_a.current_since = Some(4);
        let mut on_b = item("a");
        on_b.current = true;
        on_b.current_since = Some(4);
        let mut lists = vec![list("b", vec![on_b]), list("a", vec![on_a])];
        repair_current_since(&mut lists);
        assert!(lists[1].items[0].current);
        assert_eq!(lists[1].id, "a");
        assert!(!lists[0].items[0].current);
        assert!(lists[0].items[0].current_since.is_none());

        let mut later_id = item("b");
        later_id.current = true;
        later_id.current_since = Some(4);
        let mut earlier_id = item("a");
        earlier_id.current = true;
        earlier_id.current_since = Some(4);
        let mut lists = vec![list("l", vec![later_id, earlier_id])];
        repair_current_since(&mut lists);
        assert!(!lists[0].items[0].current);
        assert!(lists[0].items[1].current);
        assert_eq!(lists[0].items[1].id, "a");
    }
}
