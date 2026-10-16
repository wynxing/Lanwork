//! 面板里不调用窗口 API 的决定：标签、定位目标、窗口尺寸与位置，待办页的列表、行和输入解析，
//! 便签页的列表、多选、便签编辑窗口的保存状态、便签悬浮窗的默认大小和位置，收纳页的分组、引用和拖放判定，
//! 应用页的卡片和启用状态，以及工具悬浮窗外壳的位置、大小、贴边、钉住和是否开着。
//!
//! 窗口、DWM 和 Shell 调用在 `crates/app`。

mod apps_page;
mod float_shell;
mod float_snap;
mod float_source;
mod float_stack;
mod note_edit;
mod note_page;
mod note_select;
mod settings_page;
mod shelf_dnd;
mod shelf_page;
mod shelf_select;
mod todo_page;
mod tool_install;
mod triage;

pub use apps_page::{
    ENABLED_FILE, EnabledBytes, FLOAT_LABEL, IMPORT_LABEL, ListedTool, ToolCard, decode_enabled,
    encode_enabled, installed_tools, is_enabled, is_png, menu_items, name_mark, read_enabled,
    set_disabled, should_reopen, usable_icon, visible_cards, write_enabled,
};
pub use float_shell::{
    BackupSlice, DEFAULT_LOGICAL_HEIGHT, DEFAULT_LOGICAL_WIDTH, ExportSlice, MARGIN_LOGICAL,
    RECORD_DIR, ScanSkip, ShellBytes, ShellGesture, ShellRecord, ShellSession, StoredShell,
    backup_json, backup_slice, decode, encode, export_json, export_slice, geometry_for_open,
    read_record, record_relative, scan_records, shell_default_origin, shell_logical_size,
    write_failure_text, write_record,
};
pub use float_snap::{
    ANIM_STEPS, DRAG_MOVE_LOGICAL, DockInput, DockState, HANDLE_LENGTH_LOGICAL,
    HANDLE_THICKNESS_LOGICAL, OuterEdge, PxBox, SNAP_SLOP_LOGICAL, ScreenBounds, Transition,
    dock_applies, dpi_at, edge_label, lerp_box, moved_enough, outer_edges, snap_on_release,
    snap_slop, transition, visible_box,
};
pub use float_source::{note_source, shelf_source, todo_source};
pub use float_stack::{
    DOCK_MERGE_GAP_LOGICAL, DOT_DIAMETER_LOGICAL, DOT_GAP_LOGICAL, DOT_ROW_LOGICAL_HEIGHT,
    DetachOutcome, DragOrigin, DropEffect, DropInput, FilteredPages, Flip, HANDLE_DOT_CAP,
    OtherFloat, PageStack, StackMember, StackPage, StackRestore, StartupPage, assemble_stacks,
    collapsed_handles_close, detach_page, dock_merge_gap, dot_row_size,
    drop_effect as stack_drop_effect, filter_open_pages, fit_dot_row, flip_index, handle_dot_count,
    merge_stacks, overlaps_more_than_half, pages_to_park, place_dots, plan_startup,
    restore_log_line, slots_for,
};
pub use note_edit::{AUTOSAVE_IDLE_MS, EditSession, SaveOutcome, SaveProblem};
pub use note_page::{
    FLOAT_CASCADE, FLOAT_CASCADE_STEPS, FLOAT_HEIGHT, FLOAT_MIN_HEIGHT, FLOAT_MIN_WIDTH,
    FLOAT_WIDTH, FloatPlacement, MonitorWork, NoteRow, NoteView, contain_float,
    float_default_origin, list_rows, locate_note, logical_work_limits, release_float,
    restore_float, settle_float, tag_names, trash_rows,
};
pub use note_select::{
    NoteMultiSelect, NoteRowMods, NoteRowPick, clear_note_checks, enter_note_select, note_row_mods,
    note_select_after_delete, pick_note_row, retain_note_select, select_all_notes,
};
pub use tool_install::{
    CARD_FILE, GRANT_FILE, INVALID_PACKAGE, Installed, STORAGE_DIR, TOOLS_DIR, UNAUTHORIZED,
    UNINSTALL_BODY, UPDATE_NOTE, WRONG_UPDATE, install_copy, list_installed, read_tool_storage,
    recover_previous, remove_installed, storage_path, tool_dir, write_tool_storage,
};

pub use settings_page::{
    HOT_CORNER_CHOICES, SettingsRows, StartupChange, THEME_CHOICES, WEB_ENGINE_CHOICES,
    change_hot_corner, change_launch_at_startup, change_quiet_hours, change_snooze_days,
    change_theme, change_web_search_engine, hot_corner_at, hot_corner_index, hot_corner_title,
    hot_corner_titles, parse_snooze_days, settings_rows, startup_error_text, theme_at, theme_index,
    theme_title, theme_titles, web_engine_at, web_engine_index, web_engine_titles,
};
pub use shelf_dnd::{
    DragContinue, DropZone, EFFECT_COPY, EFFECT_LINK, EFFECT_MOVE, EFFECT_NONE, OUTGOING_EFFECTS,
    continue_drag, drop_effect, passed_drag_threshold,
};
pub use shelf_page::{
    GroupRow, LinkedTodo, ShelfLine, TodoChoice, first_group_for_todo, group_rows, linked_todo_ids,
    linked_todos, pick_selection, reordered_group_ids, shelf_lines, todo_choices, visible_paths,
};
pub use shelf_select::{
    DOUBLE_CLICK, GROUP_ROW_PLAIN, GROUP_ROW_WITH_TODO, GroupMods, GroupPick, GroupSelect,
    ItemMods, ItemSelect, LeftWindow, ListViewport, ShelfItemId, clear_group_checks, clear_items,
    click_item, context_item, drag_items, enter_group_select, group_mods, group_row_height,
    group_select_after_delete, hit_row, is_double_click, item_mods, items_after_remove,
    left_window, outside_window, pick_group_row, release_moves, retain_group_select, retain_items,
    select_all_groups, select_all_items,
};
pub use todo_page::{
    FloatRow, InputError, ListEntry, RECURRENCE_CHOICES, Row, TodoView, default_view, float_rows,
    format_date, list_entries, locate, parse_due, parse_remind, recurrence_from_choice,
    recurrence_index, reordered_ids, rows, trash_count, trash_days_left, view_exists,
};
pub use triage::{
    Action, CLEARED_HOLD_MS, CLEARED_TEXT, CardText, EMPTY_TEXT, KEY_COMPLETE, KEY_DEFER,
    KEY_DELETE, KEY_DOWN, KEY_ENTER, KEY_ESCAPE, KEY_MOVE, KEY_TODAY, KEY_UP, KeyHit, KeyInput,
    NO_OTHER_LIST, NO_TODAY, OpenMove, Session, action_labels, card_text, entry_label,
    entry_visible, inbox_queue, key_hit, progress_label,
};

use crate::shell::WorkArea;

/// 面板的逻辑宽高上限。产品规格没有写尺寸，这是实现选择；工作区放不下时由 [`panel_size`] 收小。
pub const PANEL_WIDTH: u32 = 760;
pub const PANEL_HEIGHT: u32 = 560;
/// 面板与工作区边缘至少留出的逻辑像素。
pub const PANEL_MARGIN: u32 = 24;

/// 面板的五个标签，顺序固定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelTab {
    #[default]
    Todo,
    Notes,
    Shelves,
    Apps,
    Settings,
}

impl PanelTab {
    pub const ALL: [Self; 5] = [
        Self::Todo,
        Self::Notes,
        Self::Shelves,
        Self::Apps,
        Self::Settings,
    ];

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Todo => "待办",
            Self::Notes => "便签",
            Self::Shelves => "收纳",
            Self::Apps => "应用",
            Self::Settings => "设置",
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0)
    }

    #[must_use]
    pub fn from_index(index: i32) -> Option<Self> {
        usize::try_from(index)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
    }
}

/// 打开面板时要定位的对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelTarget {
    Todo { item_id: String },
    Note { note_id: String },
    Shelf { group_id: String },
}

/// 面板的逻辑尺寸。工作区放不下默认尺寸时，每边留出 [`PANEL_MARGIN`] 后收小。
#[must_use]
pub fn panel_size(work: WorkArea, dpi: u32) -> (u32, u32) {
    let margin = 2 * PANEL_MARGIN;
    let width = crate::shell::to_logical(work.width(), dpi).saturating_sub(margin);
    let height = crate::shell::to_logical(work.height(), dpi).saturating_sub(margin);
    (PANEL_WIDTH.min(width), PANEL_HEIGHT.min(height))
}

/// 面板左上角，物理像素。在工作区里水平和垂直居中。产品规格没有写面板的位置，这是实现选择。
#[must_use]
pub fn panel_origin(work: WorkArea, width_px: i32, height_px: i32) -> (i32, i32) {
    (
        work.left + (work.width() - width_px) / 2,
        work.top + (work.height() - height_px) / 2,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_in_the_fixed_order_with_todo_as_default() {
        assert_eq!(
            PanelTab::ALL.map(PanelTab::title),
            ["待办", "便签", "收纳", "应用", "设置"]
        );
        assert_eq!(PanelTab::default(), PanelTab::Todo);
        assert!(PanelTab::ALL.iter().all(|tab| tab.title() != "扩展"));
        for (index, tab) in PanelTab::ALL.iter().enumerate() {
            assert_eq!(tab.index(), index);
            assert_eq!(
                PanelTab::from_index(i32::try_from(index).unwrap()),
                Some(*tab)
            );
        }
        assert_eq!(PanelTab::from_index(-1), None);
        assert_eq!(PanelTab::from_index(5), None);
    }

    #[test]
    fn size_shrinks_to_small_work_areas_and_keeps_the_default_on_large_ones() {
        let large = WorkArea {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1400,
        };
        assert_eq!(panel_size(large, 96), (PANEL_WIDTH, PANEL_HEIGHT));
        let small = WorkArea {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 752,
        };
        let (width, height) = panel_size(small, 144);
        assert_eq!(width, PANEL_WIDTH.min(1280 * 96 / 144 - 48));
        assert_eq!(height, 752 * 96 / 144 - 48);
        assert!(height < PANEL_HEIGHT);
    }

    #[test]
    fn origin_centers_in_the_work_area() {
        let work = WorkArea {
            left: 100,
            top: 50,
            right: 1100,
            bottom: 850,
        };
        assert_eq!(panel_origin(work, 400, 200), (400, 350));
    }
}
