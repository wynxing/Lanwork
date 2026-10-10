//! 面板里不调用窗口 API 的决定：标签、定位目标、窗口尺寸与位置，待办页的列表、行和输入解析，
//! 便签页的列表、便签编辑窗口的保存状态，以及便签悬浮窗的默认大小和位置。
//!
//! 窗口、DWM 和 Shell 调用在 `crates/app`。

mod note_edit;
mod note_page;
mod todo_page;

pub use note_edit::{AUTOSAVE_IDLE_MS, EditSession, SaveOutcome, SaveProblem};
pub use note_page::{
    FLOAT_CASCADE, FLOAT_CASCADE_STEPS, FLOAT_HEIGHT, FLOAT_MIN_HEIGHT, FLOAT_MIN_WIDTH,
    FLOAT_WIDTH, FloatPlacement, NoteRow, NoteView, float_default_origin, list_rows, locate_note,
    restore_float, tag_names, trash_rows,
};

pub use todo_page::{
    InputError, ListEntry, RECURRENCE_CHOICES, Row, TodoView, default_view, format_date,
    list_entries, locate, parse_due, parse_remind, recurrence_from_choice, recurrence_index,
    reordered_ids, rows, trash_count, trash_days_left, view_exists,
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
    Github,
    Settings,
}

impl PanelTab {
    pub const ALL: [Self; 5] = [
        Self::Todo,
        Self::Notes,
        Self::Shelves,
        Self::Github,
        Self::Settings,
    ];

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Todo => "待办",
            Self::Notes => "便签",
            Self::Shelves => "收纳",
            Self::Github => "GitHub",
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

    /// 还没有内容的标签只显示一行对象名。这些标签的内容由各自的界面实现。
    #[must_use]
    pub fn empty_label(self) -> &'static str {
        match self {
            Self::Todo => "没有待办",
            Self::Notes => "没有便签",
            Self::Shelves => "没有分组",
            Self::Github => "没有条目",
            Self::Settings => "没有设置项",
        }
    }
}

/// 打开面板时要定位的对象。收纳分组等它的标签实现后再加。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelTarget {
    Todo { item_id: String },
    Note { note_id: String },
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
            ["待办", "便签", "收纳", "GitHub", "设置"]
        );
        assert_eq!(PanelTab::default(), PanelTab::Todo);
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
    fn empty_state_is_one_object_name() {
        assert_eq!(PanelTab::Todo.empty_label(), "没有待办");
        assert_eq!(PanelTab::Shelves.empty_label(), "没有分组");
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
