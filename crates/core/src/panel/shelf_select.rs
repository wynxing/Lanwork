//! 面板收纳页的多选。不读盘，不读时钟，也不调用窗口 API。
//!
//! 条目在分组内部，按资源管理器：没有「选择」按钮。分组在左侧列表，有「选择」模式，
//! 点选方式和便签列表的多选相同。悬浮窗不走这里。
//!
//! 可见行为在产品规格「收纳」的「多选」。Ctrl 与 Shift 同时按下时，条目按范围追加，
//! 分组按便签那样整段替换。这两处产品规格没有逐键写死的部分见架构文档。

use std::time::Duration;

/// 分组里的一条引用。`web` 为真时 `key` 是保存的网址，否则是保存的路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfItemId {
    pub key: String,
    pub web: bool,
}

/// 当前分组里选中的引用。`ids` 按列表顺序，不是按点击顺序。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemSelect {
    pub ids: Vec<ShelfItemId>,
    /// 范围选择连到的那一行，也是路径行和左侧色条对准的那一条。
    pub anchor: Option<ShelfItemId>,
}

/// 点在条目上时按住的键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemMods {
    /// 单击：只留下这一行。
    Plain,
    /// Ctrl+点击：这一行加入或移出选择。
    Toggle,
    /// Shift+点击：从锚点到这一行，替换选择。
    Range,
    /// Ctrl+Shift+点击：从锚点到这一行，并保留范围以外已经选中的。
    RangeAdd,
}

/// 分组列表的多选。和便签列表一样，先进入模式才出现勾选。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupSelect {
    pub active: bool,
    pub ids: Vec<String>,
    pub anchor: Option<String>,
}

/// 点分组行时按住的键。Shift 优先于 Ctrl。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMods {
    Plain,
    Toggle,
    Range,
}

/// 点分组行之后界面要做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupPick {
    /// 还没进入多选时的普通点击，打开这一组。
    Open,
    Multi(GroupSelect),
}

/// 两次单击算双击的上限。调用方传入已经过去的时间，这里不读时钟。
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// 分组行在列表里的高度，与 `shelvespage.slint` 一致。有关联待办标题时更高。
pub const GROUP_ROW_PLAIN: f32 = 40.0;
pub const GROUP_ROW_WITH_TODO: f32 = 56.0;

/// 左侧分组列表的可见区域。坐标和条目拖动回调用同一套逻辑像素。
#[derive(Debug, Clone, Copy)]
pub struct ListViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// `ListView` 的 `content-y`。向下滚动时为负。
    pub content_y: f32,
}

/// Shift 优先。Ctrl 和 Shift 一起按时，条目把范围追加到已有选择上。
#[must_use]
pub fn item_mods(ctrl: bool, shift: bool) -> ItemMods {
    match (ctrl, shift) {
        (true, true) => ItemMods::RangeAdd,
        (false, true) => ItemMods::Range,
        (true, false) => ItemMods::Toggle,
        (false, false) => ItemMods::Plain,
    }
}

/// Shift 优先于 Ctrl，和便签列表相同。同时按着时整段替换，不追加。
#[must_use]
pub fn group_mods(ctrl: bool, shift: bool) -> GroupMods {
    if shift {
        GroupMods::Range
    } else if ctrl {
        GroupMods::Toggle
    } else {
        GroupMods::Plain
    }
}

/// 这一次松开是不是双击。只认连续两次都落在同一行的普通点击。
#[must_use]
pub fn is_double_click(
    previous: Option<usize>,
    elapsed: Duration,
    index: usize,
    plain: bool,
) -> bool {
    plain && previous == Some(index) && elapsed <= DOUBLE_CLICK
}

/// 单击、Ctrl+点击或 Shift+点击。`index` 超出列表时选择不变。
#[must_use]
pub fn click_item(
    state: &ItemSelect,
    visible: &[ShelfItemId],
    index: usize,
    mods: ItemMods,
) -> ItemSelect {
    let Some(id) = visible.get(index).cloned() else {
        return state.clone();
    };
    match mods {
        ItemMods::Plain => ItemSelect {
            ids: vec![id.clone()],
            anchor: Some(id),
        },
        ItemMods::Toggle => toggle_item(state, visible, id),
        ItemMods::Range => range_items(state, visible, index, false),
        ItemMods::RangeAdd => range_items(state, visible, index, true),
    }
}

/// 右键。点在已选行上时保留整组选择，锚点改到这一行。点在未选行上时只留下这一行。
#[must_use]
pub fn context_item(state: &ItemSelect, visible: &[ShelfItemId], index: usize) -> ItemSelect {
    let Some(id) = visible.get(index).cloned() else {
        return state.clone();
    };
    if state.ids.iter().any(|existing| existing == &id) {
        return ItemSelect {
            ids: order_items(&state.ids, visible),
            anchor: Some(id),
        };
    }
    ItemSelect {
        ids: vec![id.clone()],
        anchor: Some(id),
    }
}

/// 拖动的是哪些条。按下的那一条已经在选择里时拖整组，否则只拖这一条。
#[must_use]
pub fn drag_items(state: &ItemSelect, visible: &[ShelfItemId], index: usize) -> Vec<ShelfItemId> {
    let Some(id) = visible.get(index) else {
        return Vec::new();
    };
    if state.ids.iter().any(|existing| existing == id) {
        order_items(&state.ids, visible)
    } else {
        vec![id.clone()]
    }
}

/// 选中当前分组里的每一条。锚点不动；还没有锚点时用第一行。
#[must_use]
pub fn select_all_items(state: &ItemSelect, visible: &[ShelfItemId]) -> ItemSelect {
    ItemSelect {
        ids: visible.to_vec(),
        anchor: state
            .anchor
            .clone()
            .filter(|id| visible.iter().any(|row| row == id))
            .or_else(|| visible.first().cloned()),
    }
}

/// 清空条目选择。
#[must_use]
pub fn clear_items() -> ItemSelect {
    ItemSelect::default()
}

/// 分组内容变了之后，去掉已经不在列表里的选择和锚点。
#[must_use]
pub fn retain_items(state: &ItemSelect, visible: &[ShelfItemId]) -> ItemSelect {
    ItemSelect {
        ids: order_items(&state.ids, visible),
        anchor: state
            .anchor
            .clone()
            .filter(|id| visible.iter().any(|row| row == id)),
    }
}

/// 移除之后仍留在分组里的那些继续选中。一个都不剩就清空。
#[must_use]
pub fn items_after_remove(
    failed: &[ShelfItemId],
    anchor: Option<&ShelfItemId>,
    visible: &[ShelfItemId],
) -> ItemSelect {
    retain_items(
        &ItemSelect {
            ids: failed.to_vec(),
            anchor: anchor.cloned(),
        },
        visible,
    )
}

/// 点「选择」。不预先勾上任何一组。当前打开的分组若还在列表里，当作 Shift 的锚点。
#[must_use]
pub fn enter_group_select(open_id: Option<&str>, visible: &[String]) -> GroupSelect {
    GroupSelect {
        active: true,
        ids: Vec::new(),
        anchor: open_id
            .filter(|id| visible.iter().any(|row| row == id))
            .map(str::to_owned),
    }
}

/// 点分组行。`index` 超出列表时不进入多选，也不打开。
#[must_use]
pub fn pick_group_row(
    state: &GroupSelect,
    visible: &[String],
    index: usize,
    mods: GroupMods,
    open_id: Option<&str>,
) -> GroupPick {
    let Some(id) = visible.get(index) else {
        return GroupPick::Multi(state.clone());
    };
    if mods == GroupMods::Plain && !state.active {
        return GroupPick::Open;
    }
    if mods == GroupMods::Range {
        return GroupPick::Multi(range_groups(state, visible, index, open_id));
    }
    let mut next = state.clone();
    next.active = true;
    next.anchor = Some(id.clone());
    if let Some(pos) = next.ids.iter().position(|existing| existing == id) {
        next.ids.remove(pos);
    } else {
        next.ids.push(id.clone());
    }
    next.ids = order_ids(&next.ids, visible);
    GroupPick::Multi(next)
}

/// 勾上当前列表里的每一组。锚点不动。
#[must_use]
pub fn select_all_groups(state: &GroupSelect, visible: &[String]) -> GroupSelect {
    GroupSelect {
        active: true,
        ids: visible.to_vec(),
        anchor: state.anchor.clone(),
    }
}

/// 去掉全部勾选，仍留在多选里。锚点不动。
#[must_use]
pub fn clear_group_checks(state: &GroupSelect) -> GroupSelect {
    GroupSelect {
        active: state.active,
        ids: Vec::new(),
        anchor: state.anchor.clone(),
    }
}

/// 列表变了之后，去掉已经看不见的勾选和锚点。没在多选里时清空。
#[must_use]
pub fn retain_group_select(state: &GroupSelect, visible: &[String]) -> GroupSelect {
    if !state.active {
        return GroupSelect::default();
    }
    GroupSelect {
        active: true,
        ids: order_ids(&state.ids, visible),
        anchor: state
            .anchor
            .clone()
            .filter(|id| visible.iter().any(|row| row == id)),
    }
}

/// 批量删除之后的勾选。一个都没失败就退出多选。失败的仍留在列表里才继续勾着。
#[must_use]
pub fn group_select_after_delete(
    anchor: Option<&str>,
    failed: &[String],
    visible: &[String],
) -> GroupSelect {
    if failed.is_empty() {
        return GroupSelect::default();
    }
    let next = retain_group_select(
        &GroupSelect {
            active: true,
            ids: failed.to_vec(),
            anchor: anchor.map(str::to_owned),
        },
        visible,
    );
    if next.ids.is_empty() {
        GroupSelect::default()
    } else {
        next
    }
}

/// 有关联待办标题的分组行更高，与版式一致。
#[must_use]
pub fn group_row_height(has_todo_line: bool) -> f32 {
    if has_todo_line {
        GROUP_ROW_WITH_TODO
    } else {
        GROUP_ROW_PLAIN
    }
}

/// 指针在列表可见区域里的哪一行。滚出可见区域的行不算。
#[must_use]
pub fn hit_row(view: ListViewport, heights: &[f32], x: f32, y: f32) -> Option<usize> {
    if view.width <= 0.0
        || view.height <= 0.0
        || x < view.x
        || y < view.y
        || x >= view.x + view.width
        || y >= view.y + view.height
    {
        return None;
    }
    let mut top = view.y + view.content_y;
    let bottom_limit = view.y + view.height;
    for (index, height) in heights.iter().enumerate() {
        let bottom = top + height;
        let visible = bottom > view.y && top < bottom_limit;
        if visible && y >= top && y < bottom {
            return Some(index);
        }
        top = bottom;
    }
    None
}

/// 指针已经离开窗口。用来把分组内的拖动改成拖出到资源管理器。
#[must_use]
pub fn outside_window(x: f32, y: f32, width: f32, height: f32) -> bool {
    width <= 0.0 || height <= 0.0 || x < 0.0 || y < 0.0 || x >= width || y >= height
}

fn toggle_item(state: &ItemSelect, visible: &[ShelfItemId], id: ShelfItemId) -> ItemSelect {
    let mut ids = state.ids.clone();
    if let Some(pos) = ids.iter().position(|existing| existing == &id) {
        ids.remove(pos);
    } else {
        ids.push(id.clone());
    }
    ItemSelect {
        ids: order_items(&ids, visible),
        anchor: Some(id),
    }
}

fn range_items(state: &ItemSelect, visible: &[ShelfItemId], index: usize, add: bool) -> ItemSelect {
    let anchor_index = state
        .anchor
        .as_ref()
        .and_then(|id| position_item(visible, id));
    let (start, end, anchor) = match anchor_index {
        Some(anchor_index) => {
            let (start, end) = span(anchor_index, index);
            (start, end, state.anchor.clone())
        }
        None => (index, index, visible.get(index).cloned()),
    };
    let mut ids = visible[start..=end].to_vec();
    if add {
        for existing in &state.ids {
            if !ids.iter().any(|id| id == existing) {
                ids.push(existing.clone());
            }
        }
        ids = order_items(&ids, visible);
    }
    ItemSelect { ids, anchor }
}

fn range_groups(
    state: &GroupSelect,
    visible: &[String],
    index: usize,
    open_id: Option<&str>,
) -> GroupSelect {
    let anchor_index = state
        .anchor
        .as_deref()
        .and_then(|id| position_id(visible, id))
        .or_else(|| open_id.and_then(|id| position_id(visible, id)));
    let (anchor_index, anchor_id) = match anchor_index {
        Some(anchor_index) => (anchor_index, visible[anchor_index].clone()),
        None => (index, visible[index].clone()),
    };
    let (start, end) = span(anchor_index, index);
    GroupSelect {
        active: true,
        ids: visible[start..=end].to_vec(),
        anchor: Some(anchor_id),
    }
}

fn span(anchor: usize, index: usize) -> (usize, usize) {
    if anchor <= index {
        (anchor, index)
    } else {
        (index, anchor)
    }
}

fn order_items(ids: &[ShelfItemId], visible: &[ShelfItemId]) -> Vec<ShelfItemId> {
    visible
        .iter()
        .filter(|id| ids.iter().any(|selected| selected == *id))
        .cloned()
        .collect()
}

fn order_ids(ids: &[String], visible: &[String]) -> Vec<String> {
    visible
        .iter()
        .filter(|id| ids.iter().any(|selected| selected == *id))
        .cloned()
        .collect()
}

fn position_item(visible: &[ShelfItemId], id: &ShelfItemId) -> Option<usize> {
    visible.iter().position(|row| row == id)
}

fn position_id(visible: &[String], id: &str) -> Option<usize> {
    visible.iter().position(|row| row == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, web: bool) -> ShelfItemId {
        ShelfItemId {
            key: key.to_owned(),
            web,
        }
    }

    fn visible() -> Vec<ShelfItemId> {
        vec![
            item("a", false),
            item("b", false),
            item("c", true),
            item("d", false),
        ]
    }

    fn ids(state: &ItemSelect) -> Vec<&str> {
        state.ids.iter().map(|id| id.key.as_str()).collect()
    }

    #[test]
    fn plain_click_keeps_only_that_row_and_ctrl_toggles() {
        let rows = visible();
        let once = click_item(&ItemSelect::default(), &rows, 0, ItemMods::Plain);
        assert_eq!(ids(&once), ["a"]);
        let toggled = click_item(&once, &rows, 2, ItemMods::Toggle);
        assert_eq!(ids(&toggled), ["a", "c"]);
        assert_eq!(toggled.anchor.as_ref().map(|id| id.key.as_str()), Some("c"));
        let off = click_item(&toggled, &rows, 0, ItemMods::Toggle);
        assert_eq!(ids(&off), ["c"]);
        let replaced = click_item(&off, &rows, 3, ItemMods::Plain);
        assert_eq!(ids(&replaced), ["d"]);
    }

    #[test]
    fn shift_replaces_with_the_range_and_ctrl_shift_keeps_the_rest() {
        let rows = visible();
        let anchor = click_item(&ItemSelect::default(), &rows, 0, ItemMods::Plain);
        let range = click_item(&anchor, &rows, 2, ItemMods::Range);
        assert_eq!(ids(&range), ["a", "b", "c"]);
        assert_eq!(range.anchor.as_ref().map(|id| id.key.as_str()), Some("a"));
        let with_tail = ItemSelect {
            ids: vec![item("a", false), item("d", false)],
            anchor: Some(item("a", false)),
        };
        let added = click_item(&with_tail, &rows, 1, ItemMods::RangeAdd);
        assert_eq!(ids(&added), ["a", "b", "d"]);
        assert_eq!(added.anchor.as_ref().map(|id| id.key.as_str()), Some("a"));
    }

    #[test]
    fn shift_without_an_anchor_selects_only_the_clicked_row() {
        let rows = visible();
        let range = click_item(&ItemSelect::default(), &rows, 2, ItemMods::Range);
        assert_eq!(ids(&range), ["c"]);
        assert!(range.anchor.as_ref().is_some_and(|id| id.web));
    }

    #[test]
    fn right_click_keeps_a_selected_row_and_replaces_an_unselected_one() {
        let rows = visible();
        let many = select_all_items(&ItemSelect::default(), &rows);
        let kept = context_item(&many, &rows, 1);
        assert_eq!(ids(&kept), ["a", "b", "c", "d"]);
        assert_eq!(kept.anchor.as_ref().map(|id| id.key.as_str()), Some("b"));
        let only = context_item(&ItemSelect::default(), &rows, 3);
        assert_eq!(ids(&only), ["d"]);
    }

    #[test]
    fn dragging_a_selected_row_takes_the_selection_and_an_unselected_row_does_not() {
        let rows = visible();
        let two = click_item(
            &click_item(&ItemSelect::default(), &rows, 0, ItemMods::Plain),
            &rows,
            2,
            ItemMods::Toggle,
        );
        let keys = |items: Vec<ShelfItemId>| -> Vec<String> {
            items.into_iter().map(|id| id.key).collect()
        };
        assert_eq!(keys(drag_items(&two, &rows, 2)), ["a", "c"]);
        assert_eq!(keys(drag_items(&two, &rows, 1)), ["b"]);
    }

    #[test]
    fn select_all_and_retain_follow_the_current_list() {
        let rows = visible();
        let all = select_all_items(&ItemSelect::default(), &rows);
        assert_eq!(all.ids.len(), 4);
        let shorter = vec![item("b", false), item("d", false)];
        let kept = retain_items(&all, &shorter);
        assert_eq!(ids(&kept), ["b", "d"]);
        let after = items_after_remove(&[item("b", false)], kept.anchor.as_ref(), &shorter);
        assert_eq!(ids(&after), ["b"]);
        assert!(items_after_remove(&[], None, &shorter).ids.is_empty());
    }

    #[test]
    fn a_second_plain_click_on_the_same_row_is_a_double_click() {
        assert!(is_double_click(
            Some(1),
            Duration::from_millis(400),
            1,
            true
        ));
        assert!(!is_double_click(
            Some(1),
            Duration::from_millis(400),
            1,
            false
        ));
        assert!(!is_double_click(
            Some(1),
            Duration::from_millis(501),
            1,
            true
        ));
        assert!(!is_double_click(
            Some(0),
            Duration::from_millis(10),
            1,
            true
        ));
        assert_eq!(DOUBLE_CLICK, Duration::from_millis(500));
    }

    #[test]
    fn ctrl_and_shift_map_the_way_each_list_expects() {
        assert_eq!(item_mods(false, false), ItemMods::Plain);
        assert_eq!(item_mods(true, false), ItemMods::Toggle);
        assert_eq!(item_mods(false, true), ItemMods::Range);
        assert_eq!(item_mods(true, true), ItemMods::RangeAdd);
        assert_eq!(group_mods(true, true), GroupMods::Range);
        assert_eq!(group_mods(true, false), GroupMods::Toggle);
    }

    #[test]
    fn group_clicks_match_the_note_list() {
        let rows = ["a", "b", "c", "d"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(matches!(
            pick_group_row(&GroupSelect::default(), &rows, 1, GroupMods::Plain, None),
            GroupPick::Open
        ));
        let GroupPick::Multi(entered) = pick_group_row(
            &GroupSelect::default(),
            &rows,
            1,
            GroupMods::Toggle,
            Some("a"),
        ) else {
            panic!("ctrl enters multi-select");
        };
        assert!(entered.active);
        assert_eq!(entered.ids, ["b"]);
        let from_button = enter_group_select(Some("a"), &rows);
        assert!(from_button.ids.is_empty());
        assert_eq!(from_button.anchor.as_deref(), Some("a"));
        let GroupPick::Multi(range) =
            pick_group_row(&from_button, &rows, 2, GroupMods::Range, Some("a"))
        else {
            panic!("shift selects a range");
        };
        assert_eq!(range.ids, ["a", "b", "c"]);
        let all = select_all_groups(&range, &rows);
        assert_eq!(all.ids, rows);
        let cleared = clear_group_checks(&all);
        assert!(cleared.active && cleared.ids.is_empty());
        let after = group_select_after_delete(Some("a"), &["c".to_owned()], &rows);
        assert_eq!(after.ids, ["c"]);
        assert!(!group_select_after_delete(None, &[], &rows).active);
    }

    #[test]
    fn hit_testing_uses_row_heights_and_ignores_rows_scrolled_away() {
        assert_eq!(group_row_height(false), 40.0);
        assert_eq!(group_row_height(true), 56.0);
        let view = ListViewport {
            x: 8.0,
            y: 80.0,
            width: 200.0,
            height: 80.0,
            content_y: 0.0,
        };
        let heights = [40.0, 56.0, 40.0];
        assert_eq!(hit_row(view, &heights, 20.0, 90.0), Some(0));
        assert_eq!(hit_row(view, &heights, 20.0, 130.0), Some(1));
        assert_eq!(hit_row(view, &heights, 20.0, 170.0), None);
        let scrolled = ListViewport {
            content_y: -40.0,
            ..view
        };
        assert_eq!(hit_row(scrolled, &heights, 20.0, 90.0), Some(1));
        assert_eq!(hit_row(view, &heights, 4.0, 90.0), None);
        assert!(outside_window(-1.0, 10.0, 100.0, 100.0));
        assert!(!outside_window(10.0, 10.0, 100.0, 100.0));
    }
}
