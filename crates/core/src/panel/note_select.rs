//! 面板便签列表的多选。不读盘，不读时钟，也不调用窗口 API。
//!
//! 回收站和悬浮窗不走这里。可见行为在产品规格「便签」的「多选」。
//! 锚点、范围是替换还是追加、确认被取消的时机，产品规格没有写死的部分见架构文档，标为待定。

/// 一次多选里勾上的便签。`ids` 按当前列表的顺序，不是按点击顺序。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoteMultiSelect {
    pub active: bool,
    pub ids: Vec<String>,
    /// Shift+点击连到的那一行。不在当前列表里时为 `None`。
    pub anchor: Option<String>,
}

/// 点在一行上时按住的键。只看 Ctrl 和 Shift。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteRowMods {
    Plain,
    /// Ctrl+点击：这一行加入或移出勾选。
    Toggle,
    /// Shift+点击，以及同时按着 Ctrl 时：从锚点到这一行。
    Range,
}

/// 点一行之后界面要做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteRowPick {
    /// 还没进入多选时的普通点击，打开这一行。
    Open,
    /// 不打开编辑。多选状态以这里的值为准。
    Multi(NoteMultiSelect),
}

/// Shift 优先于 Ctrl。都没按着就是普通点击。
#[must_use]
pub fn note_row_mods(ctrl: bool, shift: bool) -> NoteRowMods {
    if shift {
        NoteRowMods::Range
    } else if ctrl {
        NoteRowMods::Toggle
    } else {
        NoteRowMods::Plain
    }
}

/// 点「选择」。不预先勾上任何一行。已经打开的那一篇若在当前列表里，当作 Shift 的锚点。
#[must_use]
pub fn enter_note_select(open_id: Option<&str>, visible: &[String]) -> NoteMultiSelect {
    NoteMultiSelect {
        active: true,
        ids: Vec::new(),
        anchor: open_id
            .filter(|id| visible.iter().any(|row| row == id))
            .map(str::to_owned),
    }
}

/// 点一行。`index` 超出列表时不进入多选，也不打开。
#[must_use]
pub fn pick_note_row(
    state: &NoteMultiSelect,
    visible: &[String],
    index: usize,
    mods: NoteRowMods,
    open_id: Option<&str>,
) -> NoteRowPick {
    let Some(id) = visible.get(index) else {
        return NoteRowPick::Multi(state.clone());
    };
    if mods == NoteRowMods::Plain && !state.active {
        return NoteRowPick::Open;
    }
    if mods == NoteRowMods::Range {
        return NoteRowPick::Multi(range_select(state, visible, index, open_id));
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
    NoteRowPick::Multi(next)
}

/// 勾上当前列表里的每一行。锚点不动。
#[must_use]
pub fn select_all_notes(state: &NoteMultiSelect, visible: &[String]) -> NoteMultiSelect {
    NoteMultiSelect {
        active: true,
        ids: visible.to_vec(),
        anchor: state.anchor.clone(),
    }
}

/// 去掉全部勾选，仍留在多选里。锚点不动。
#[must_use]
pub fn clear_note_checks(state: &NoteMultiSelect) -> NoteMultiSelect {
    NoteMultiSelect {
        active: state.active,
        ids: Vec::new(),
        anchor: state.anchor.clone(),
    }
}

/// 列表变了之后，去掉已经看不见的勾选和锚点。没在多选里时清空。
#[must_use]
pub fn retain_note_select(state: &NoteMultiSelect, visible: &[String]) -> NoteMultiSelect {
    if !state.active {
        return NoteMultiSelect::default();
    }
    NoteMultiSelect {
        active: true,
        ids: order_ids(&state.ids, visible),
        anchor: state
            .anchor
            .clone()
            .filter(|id| visible.iter().any(|row| row == id)),
    }
}

/// 批量删除之后的勾选。一个都没失败就退出多选。失败的仍留在列表里才继续勾着；都不在列表里也退出。
#[must_use]
pub fn note_select_after_delete(
    state: &NoteMultiSelect,
    failed: &[String],
    visible: &[String],
) -> NoteMultiSelect {
    if failed.is_empty() {
        return NoteMultiSelect::default();
    }
    let next = retain_note_select(
        &NoteMultiSelect {
            active: true,
            ids: failed.to_vec(),
            anchor: state.anchor.clone(),
        },
        visible,
    );
    if next.ids.is_empty() {
        NoteMultiSelect::default()
    } else {
        next
    }
}

fn range_select(
    state: &NoteMultiSelect,
    visible: &[String],
    index: usize,
    open_id: Option<&str>,
) -> NoteMultiSelect {
    let anchor_index = state
        .anchor
        .as_deref()
        .and_then(|id| position(visible, id))
        .or_else(|| open_id.and_then(|id| position(visible, id)));
    let (anchor_index, anchor_id) = match anchor_index {
        Some(anchor_index) => (anchor_index, visible[anchor_index].clone()),
        None => (index, visible[index].clone()),
    };
    let (start, end) = if anchor_index <= index {
        (anchor_index, index)
    } else {
        (index, anchor_index)
    };
    NoteMultiSelect {
        active: true,
        ids: visible[start..=end].to_vec(),
        anchor: Some(anchor_id),
    }
}

fn order_ids(ids: &[String], visible: &[String]) -> Vec<String> {
    visible
        .iter()
        .filter(|id| ids.iter().any(|selected| selected == *id))
        .cloned()
        .collect()
}

fn position(visible: &[String], id: &str) -> Option<usize> {
    visible.iter().position(|row| row == id)
}

#[cfg(test)]
mod tests {
    use super::{
        NoteMultiSelect, NoteRowMods, NoteRowPick, clear_note_checks, enter_note_select,
        note_row_mods, note_select_after_delete, pick_note_row, retain_note_select,
        select_all_notes,
    };

    fn visible() -> Vec<String> {
        ["a", "b", "c", "d"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn active(ids: &[&str], anchor: Option<&str>) -> NoteMultiSelect {
        NoteMultiSelect {
            active: true,
            ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            anchor: anchor.map(str::to_owned),
        }
    }

    fn picked(pick: NoteRowPick) -> NoteMultiSelect {
        match pick {
            NoteRowPick::Multi(state) => state,
            NoteRowPick::Open => panic!("应留在多选，不打开编辑"),
        }
    }

    #[test]
    fn shift_wins_over_ctrl() {
        assert_eq!(note_row_mods(false, false), NoteRowMods::Plain);
        assert_eq!(note_row_mods(true, false), NoteRowMods::Toggle);
        assert_eq!(note_row_mods(false, true), NoteRowMods::Range);
        assert_eq!(note_row_mods(true, true), NoteRowMods::Range);
    }

    #[test]
    fn plain_click_opens_until_select_mode() {
        let pick = pick_note_row(
            &NoteMultiSelect::default(),
            &visible(),
            1,
            NoteRowMods::Plain,
            None,
        );
        assert_eq!(pick, NoteRowPick::Open);
    }

    #[test]
    fn ctrl_click_toggles_without_opening_and_keeps_list_order() {
        let first = picked(pick_note_row(
            &NoteMultiSelect::default(),
            &visible(),
            2,
            NoteRowMods::Toggle,
            None,
        ));
        assert!(first.active);
        assert_eq!(first.ids, ["c"]);
        assert_eq!(first.anchor.as_deref(), Some("c"));

        let second = picked(pick_note_row(
            &first,
            &visible(),
            0,
            NoteRowMods::Toggle,
            None,
        ));
        assert_eq!(second.ids, ["a", "c"]);
        assert_eq!(second.anchor.as_deref(), Some("a"));

        let third = picked(pick_note_row(
            &second,
            &visible(),
            2,
            NoteRowMods::Toggle,
            None,
        ));
        assert_eq!(third.ids, ["a"]);
        assert_eq!(third.anchor.as_deref(), Some("c"));
    }

    #[test]
    fn shift_click_uses_the_open_note_then_replaces_with_the_range() {
        let pick = picked(pick_note_row(
            &NoteMultiSelect::default(),
            &visible(),
            2,
            NoteRowMods::Range,
            Some("b"),
        ));
        assert_eq!(pick.ids, ["b", "c"]);
        assert_eq!(pick.anchor.as_deref(), Some("b"));

        let downward = picked(pick_note_row(
            &pick,
            &visible(),
            0,
            NoteRowMods::Range,
            Some("d"),
        ));
        assert_eq!(downward.ids, ["a", "b"], "锚点仍是 b，不改去打开的那一篇");
        assert_eq!(downward.anchor.as_deref(), Some("b"));
    }

    #[test]
    fn shift_click_with_no_anchor_selects_only_that_row() {
        let pick = picked(pick_note_row(
            &NoteMultiSelect::default(),
            &visible(),
            3,
            NoteRowMods::Range,
            Some("missing"),
        ));
        assert_eq!(pick.ids, ["d"]);
        assert_eq!(pick.anchor.as_deref(), Some("d"));
    }

    #[test]
    fn choosing_select_anchors_on_the_open_note_without_checking_it() {
        let entered = enter_note_select(Some("c"), &visible());
        assert!(entered.active);
        assert!(entered.ids.is_empty());
        assert_eq!(entered.anchor.as_deref(), Some("c"));

        let missing = enter_note_select(Some("gone"), &visible());
        assert_eq!(missing.anchor, None);

        let ranged = picked(pick_note_row(
            &entered,
            &visible(),
            1,
            NoteRowMods::Range,
            None,
        ));
        assert_eq!(ranged.ids, ["b", "c"]);
    }

    #[test]
    fn plain_click_in_select_mode_toggles_and_does_not_open() {
        let state = active(&["a"], Some("a"));
        let pick = picked(pick_note_row(
            &state,
            &visible(),
            1,
            NoteRowMods::Plain,
            Some("a"),
        ));
        assert_eq!(pick.ids, ["a", "b"]);
        assert_eq!(pick.anchor.as_deref(), Some("b"));
    }

    #[test]
    fn select_all_and_clear_keep_the_anchor() {
        let state = active(&["b"], Some("b"));
        let all = select_all_notes(&state, &visible());
        assert_eq!(all.ids, visible());
        assert_eq!(all.anchor.as_deref(), Some("b"));
        let cleared = clear_note_checks(&all);
        assert!(cleared.active);
        assert!(cleared.ids.is_empty());
        assert_eq!(cleared.anchor.as_deref(), Some("b"));
    }

    #[test]
    fn filtering_drops_hidden_checks_and_the_anchor() {
        let state = active(&["a", "c"], Some("c"));
        let visible = ["a", "b"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let kept = retain_note_select(&state, &visible);
        assert_eq!(kept.ids, ["a"]);
        assert_eq!(kept.anchor, None);
        assert!(kept.active);

        let idle = retain_note_select(&NoteMultiSelect::default(), &visible);
        assert_eq!(idle, NoteMultiSelect::default());
    }

    #[test]
    fn a_click_outside_the_list_does_not_enter_select_mode() {
        let pick = picked(pick_note_row(
            &NoteMultiSelect::default(),
            &visible(),
            9,
            NoteRowMods::Toggle,
            None,
        ));
        assert_eq!(pick, NoteMultiSelect::default());
    }

    #[test]
    fn delete_keeps_only_the_failures_that_are_still_listed() {
        let state = active(&["a", "b", "c"], Some("b"));
        let visible = visible();
        assert_eq!(
            note_select_after_delete(&state, &[], &visible),
            NoteMultiSelect::default()
        );

        let failed = ["c", "a"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let kept = note_select_after_delete(&state, &failed, &visible);
        assert_eq!(kept.ids, ["a", "c"]);
        assert_eq!(kept.anchor.as_deref(), Some("b"));

        let gone = ["missing"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            note_select_after_delete(&state, &gone, &visible),
            NoteMultiSelect::default()
        );
    }
}
