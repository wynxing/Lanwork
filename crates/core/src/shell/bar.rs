//! 搜索条里不调用窗口 API 的决定：背景、位置、按键和结果动作。
//!
//! 窗口、DWM 和 Shell 调用在 `crates/app`。

use std::path::PathBuf;

use crate::apps::{LaunchTarget, containing_folder, supports_run_as_admin};
use crate::dispatch::{FileProgress, RowDetail, SearchGroup, SearchRow, UsageKey, ViewPhase};
use crate::search::classify_prefix;

/// `DWMWA_SYSTEMBACKDROP_TYPE` 从这个 build 起可用。不是产品最低版本。
pub const MIN_BACKDROP_BUILD: u32 = 22621;

/// 搜索条的逻辑宽度。产品规格没有写宽度，这是实现选择。
pub const BAR_WIDTH: u32 = 680;

/// Windows 的 100% 缩放。
pub const BASE_DPI: u32 = 96;

/// 纯色背景。产品规格只要求与主题一致，这里取 Windows 11 窗口底色。
pub const LIGHT_SOLID_RGB: u32 = 0x00F3_F3F3;
pub const DARK_SOLID_RGB: u32 = 0x0020_2020;

/// 系统读数。读不到的项由调用方按「不能用亚克力」填。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackdropInput {
    pub build: Option<u32>,
    pub transparency_enabled: bool,
    pub battery_saver: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    Acrylic,
    Solid,
}

/// 透明效果关闭、节电模式、build 低于 22621 或读不到 build 时用纯色。
/// DWM 调用失败由调用方再退回纯色。
#[must_use]
pub fn choose_backdrop(input: BackdropInput) -> Backdrop {
    let api = input.build.is_some_and(|build| build >= MIN_BACKDROP_BUILD);
    if api && input.transparency_enabled && !input.battery_saver {
        Backdrop::Acrylic
    } else {
        Backdrop::Solid
    }
}

#[must_use]
pub fn solid_rgb(dark: bool) -> u32 {
    if dark {
        DARK_SOLID_RGB
    } else {
        LIGHT_SOLID_RGB
    }
}

/// 显示器工作区，物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl WorkArea {
    #[must_use]
    pub fn width(self) -> i32 {
        self.right.saturating_sub(self.left).max(0)
    }

    #[must_use]
    pub fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top).max(0)
    }
}

/// 逻辑长度换成该 DPI 下的物理像素，四舍五入。
#[must_use]
pub fn to_physical(logical: u32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    let scaled =
        (u64::from(logical) * u64::from(dpi) + u64::from(BASE_DPI) / 2) / u64::from(BASE_DPI);
    i32::try_from(scaled).unwrap_or(i32::MAX)
}

/// 物理像素换成逻辑长度，向下取整。
#[must_use]
pub fn to_logical(physical: i32, dpi: u32) -> u32 {
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    let physical = u64::try_from(physical.max(0)).unwrap_or(0);
    u32::try_from(physical * u64::from(BASE_DPI) / u64::from(dpi)).unwrap_or(u32::MAX)
}

/// 窗口左上角，物理像素。水平居中，上边缘在工作区高度 25% 处。
#[must_use]
pub fn bar_origin(work: WorkArea, width_px: i32) -> (i32, i32) {
    let x = work.left + (work.width() - width_px) / 2;
    let y = work.top + work.height() / 4;
    (x, y)
}

/// 结果区最多占到工作区底边之上，逻辑像素。`input_height` 是输入框一行。
#[must_use]
pub fn max_results_height(work: WorkArea, dpi: u32, input_height: u32, bottom_gap: u32) -> u32 {
    let below = work.height() - work.height() / 4;
    to_logical(below, dpi).saturating_sub(input_height.saturating_add(bottom_gap))
}

/// 结果行高、列表上下留白和组间距，逻辑像素。版式是实现选择。
pub const ROW_HEIGHT: u32 = 48;
pub const LIST_PAD: u32 = 6;
pub const GROUP_GAP: u32 = 10;

/// 结果上的类型文字。
#[must_use]
pub fn group_label(group: SearchGroup) -> &'static str {
    match group {
        SearchGroup::Browser => "浏览器",
        SearchGroup::Application => "应用",
        SearchGroup::Todo => "待办",
        SearchGroup::Note => "便签",
        SearchGroup::File => "文件",
        SearchGroup::Folder => "文件夹",
    }
}

/// 每行的纵坐标和列表总高。换组时多留 [`GROUP_GAP`]，分隔线画在空隙中间。
#[must_use]
pub fn row_offsets(groups: &[SearchGroup]) -> (Vec<u32>, u32) {
    let mut offsets = Vec::with_capacity(groups.len());
    let mut y = LIST_PAD;
    for (index, group) in groups.iter().enumerate() {
        if index > 0 && groups[index - 1] != *group {
            y += GROUP_GAP;
        }
        offsets.push(y);
        y += ROW_HEIGHT;
    }
    if groups.is_empty() {
        return (offsets, 0);
    }
    (offsets, y + LIST_PAD)
}

/// 结果区下方那一行。错误优先，其次「文件索引不可用」。没有任何结果并且文件阶段已经结束时是「没有结果」。
#[must_use]
pub fn status_line(
    error: Option<&str>,
    phase: ViewPhase,
    rows: usize,
    file: FileProgress,
    file_unavailable: Option<&str>,
) -> String {
    if let Some(error) = error {
        return error.to_owned();
    }
    if phase != ViewPhase::Results {
        return String::new();
    }
    if let Some(text) = file_unavailable {
        return text.to_owned();
    }
    if rows == 0 && matches!(file, FileProgress::Settled | FileProgress::Idle) {
        return NO_RESULTS.to_owned();
    }
    String::new()
}

/// 空结果的一行。全局规则 1：只显示对象名。
pub const NO_RESULTS: &str = "没有结果";

/// Enter 带的修饰键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterChord {
    Plain,
    Ctrl,
    CtrlShift,
    Other,
}

impl EnterChord {
    #[must_use]
    pub fn from_modifiers(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Self {
        match (ctrl, shift, alt, meta) {
            (false, false, false, false) => Self::Plain,
            (true, false, false, false) => Self::Ctrl,
            (true, true, false, false) => Self::CtrlShift,
            _ => Self::Other,
        }
    }
}

/// 一条结果上的一次动作。待办和便签的落点属于面板和便签悬浮窗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAction {
    /// 用 Windows Shell 打开。
    Shell(LaunchTarget),
    /// 动词 `runas`。只有本地 exe 的应用。
    Elevated(LaunchTarget),
    /// 打开这个目录。
    OpenFolder(PathBuf),
    /// 打开面板、切到待办并定位该条。
    PanelTodo { list_id: String, item_id: String },
    /// 打开面板、切到便签并打开该篇。
    PanelNote { id: String },
    /// 悬浮该便签。
    FloatNote { id: String },
}

impl RowAction {
    /// 打开了这一项本身时次数加 1。打开所在目录不算打开这一项。
    #[must_use]
    pub fn counts_as_open(&self) -> bool {
        !matches!(self, Self::OpenFolder(_))
    }
}

/// 产品规格「搜索」的 Enter、Ctrl+Enter 和 Ctrl+Shift+Enter。没有动作时为 `None`。
#[must_use]
pub fn row_action(detail: &RowDetail, chord: EnterChord) -> Option<RowAction> {
    match (detail, chord) {
        (RowDetail::Browser { url }, EnterChord::Plain) => {
            Some(RowAction::Shell(LaunchTarget::Url { url: url.clone() }))
        }
        (RowDetail::App(hit), EnterChord::Plain) => {
            Some(RowAction::Shell(hit.entry.target.clone()))
        }
        (RowDetail::App(hit), EnterChord::Ctrl) => {
            containing_folder(&hit.entry.target).map(RowAction::OpenFolder)
        }
        (RowDetail::App(hit), EnterChord::CtrlShift) => supports_run_as_admin(&hit.entry.target)
            .then(|| RowAction::Elevated(hit.entry.target.clone())),
        (RowDetail::File { path } | RowDetail::Folder { path }, EnterChord::Plain) => {
            Some(RowAction::Shell(path_target(path)))
        }
        (RowDetail::File { path } | RowDetail::Folder { path }, EnterChord::Ctrl) => {
            parent_folder(path).map(RowAction::OpenFolder)
        }
        (RowDetail::Todo { list_id, item_id }, EnterChord::Plain) => Some(RowAction::PanelTodo {
            list_id: list_id.clone(),
            item_id: item_id.clone(),
        }),
        (RowDetail::Note { id }, EnterChord::Plain) => {
            Some(RowAction::PanelNote { id: id.clone() })
        }
        (RowDetail::Note { id }, EnterChord::Ctrl) => Some(RowAction::FloatNote { id: id.clone() }),
        _ => None,
    }
}

/// 打开文件或目录本身的 Shell 目标。
#[must_use]
pub fn path_target(path: &str) -> LaunchTarget {
    LaunchTarget::Path {
        path: PathBuf::from(path),
        args: String::new(),
        working_directory: None,
    }
}

fn parent_folder(path: &str) -> Option<PathBuf> {
    let parent = std::path::Path::new(path).parent()?;
    (!parent.as_os_str().is_empty()).then(|| parent.to_path_buf())
}

/// Esc 收起还是只清空。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeAction {
    Hide,
    Clear,
}

/// 输入框为空时收起。收集时收起并保留文本。搜索时只清空。
#[must_use]
pub fn escape_action(text: &str) -> EscapeAction {
    if text.is_empty() || classify_prefix(text).is_capture() {
        EscapeAction::Hide
    } else {
        EscapeAction::Clear
    }
}

/// 收起后输入框里留下的文本。只有未提交的收集文本保留。
#[must_use]
pub fn text_after_hide(text: &str) -> &str {
    if classify_prefix(text).is_capture() {
        text
    } else {
        ""
    }
}

/// 上下键移动选择，不回绕。
#[must_use]
pub fn move_selection(current: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if down {
        (current + 1).min(len - 1)
    } else {
        current.saturating_sub(1).min(len - 1)
    }
}

/// 鼠标悬停或单击的行号。界面传来的行号可能来自已经被替换的列表，越界时不选。
#[must_use]
pub fn pointed_row(index: i32, len: usize) -> Option<usize> {
    usize::try_from(index).ok().filter(|index| *index < len)
}

/// 新结果到达后的选中行。同一段输入的后续结果（例如文件结果合并进来）保留原来选中的那一条；
/// 输入变了，或那一条已经不在，回到第一条。
#[must_use]
pub fn reselect(previous: Option<&UsageKey>, same_text: bool, rows: &[SearchRow]) -> usize {
    if !same_text {
        return 0;
    }
    previous
        .and_then(|key| rows.iter().position(|row| &row.usage == key))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{AppEntry, AppHit, AppSource};
    use crate::search::HitKind;

    #[test]
    fn labels_cover_every_group() {
        assert_eq!(
            [
                SearchGroup::Browser,
                SearchGroup::Application,
                SearchGroup::Todo,
                SearchGroup::Note,
                SearchGroup::File,
                SearchGroup::Folder,
            ]
            .map(group_label),
            ["浏览器", "应用", "待办", "便签", "文件", "文件夹"]
        );
    }

    #[test]
    fn offsets_leave_a_gap_between_groups() {
        assert_eq!(row_offsets(&[]), (Vec::new(), 0));
        let (offsets, height) = row_offsets(&[
            SearchGroup::Application,
            SearchGroup::Application,
            SearchGroup::Todo,
            SearchGroup::File,
        ]);
        assert_eq!(offsets, vec![6, 54, 112, 170]);
        assert_eq!(height, 170 + 48 + 6);
    }

    #[test]
    fn status_line_order() {
        use FileProgress::{InFlight, Settled, Waiting};
        assert_eq!(
            status_line(Some("启动失败"), ViewPhase::Results, 3, Settled, Some("x")),
            "启动失败"
        );
        assert_eq!(
            status_line(None, ViewPhase::Results, 3, Settled, Some("文件索引不可用")),
            "文件索引不可用"
        );
        assert_eq!(
            status_line(None, ViewPhase::Results, 0, Settled, None),
            NO_RESULTS
        );
        assert_eq!(status_line(None, ViewPhase::Results, 0, Waiting, None), "");
        assert_eq!(status_line(None, ViewPhase::Results, 0, InFlight, None), "");
        assert_eq!(status_line(None, ViewPhase::Results, 2, Settled, None), "");
        assert_eq!(status_line(None, ViewPhase::Capture, 0, Settled, None), "");
        assert_eq!(status_line(None, ViewPhase::Empty, 0, Settled, None), "");
    }

    fn app(target: LaunchTarget) -> RowDetail {
        RowDetail::App(AppHit {
            entry: AppEntry {
                name: "Demo".to_owned(),
                source: AppSource::StartMenu,
                target,
                icon_path: None,
                icon_index: 0,
                alternate_names: Vec::new(),
            },
            kind: HitKind::Exact,
            score: 0,
        })
    }

    fn exe() -> LaunchTarget {
        LaunchTarget::Path {
            path: PathBuf::from("C:/Tools/demo.exe"),
            args: "--x".to_owned(),
            working_directory: None,
        }
    }

    fn row(key: &str) -> SearchRow {
        SearchRow {
            group: SearchGroup::File,
            kind: None,
            usage: UsageKey::File(key.to_owned()),
            label: key.to_owned(),
            location: String::new(),
            icon: None,
            detail: RowDetail::File {
                path: key.to_owned(),
            },
        }
    }

    #[test]
    fn backdrop_falls_back_to_solid() {
        let ok = BackdropInput {
            build: Some(22621),
            transparency_enabled: true,
            battery_saver: false,
        };
        assert_eq!(choose_backdrop(ok), Backdrop::Acrylic);
        for input in [
            BackdropInput {
                build: Some(22000),
                ..ok
            },
            BackdropInput { build: None, ..ok },
            BackdropInput {
                transparency_enabled: false,
                ..ok
            },
            BackdropInput {
                battery_saver: true,
                ..ok
            },
        ] {
            assert_eq!(choose_backdrop(input), Backdrop::Solid, "{input:?}");
        }
        assert_eq!(solid_rgb(false), 0xF3F3F3);
        assert_eq!(solid_rgb(true), 0x202020);
    }

    #[test]
    fn origin_is_centered_at_quarter_height() {
        let work = WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(bar_origin(work, 680), (620, 260));
        let second = WorkArea {
            left: 1920,
            top: 0,
            right: 1920 + 2560,
            bottom: 1392,
        };
        let width = to_physical(BAR_WIDTH, 144);
        assert_eq!(width, 1020);
        assert_eq!(bar_origin(second, width), (1920 + 770, 348));
        let negative = WorkArea {
            left: -1280,
            top: -200,
            right: 0,
            bottom: 800,
        };
        assert_eq!(bar_origin(negative, 680), (-1280 + 300, 50));
    }

    #[test]
    fn dpi_conversion_rounds() {
        assert_eq!(to_physical(680, 96), 680);
        assert_eq!(to_physical(680, 120), 850);
        assert_eq!(to_physical(1, 144), 2);
        assert_eq!(to_physical(10, 0), 10);
        assert_eq!(to_logical(1020, 144), 680);
        assert_eq!(to_logical(-5, 144), 0);
        let work = WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(max_results_height(work, 144, 56, 24), 540 - 80);
    }

    #[test]
    fn enter_chords() {
        assert_eq!(
            EnterChord::from_modifiers(false, false, false, false),
            EnterChord::Plain
        );
        assert_eq!(
            EnterChord::from_modifiers(true, false, false, false),
            EnterChord::Ctrl
        );
        assert_eq!(
            EnterChord::from_modifiers(true, true, false, false),
            EnterChord::CtrlShift
        );
        assert_eq!(
            EnterChord::from_modifiers(false, true, false, false),
            EnterChord::Other
        );
        assert_eq!(
            EnterChord::from_modifiers(true, false, true, false),
            EnterChord::Other
        );
    }

    #[test]
    fn app_with_local_file_has_all_three_actions() {
        let detail = app(exe());
        assert_eq!(
            row_action(&detail, EnterChord::Plain),
            Some(RowAction::Shell(exe()))
        );
        assert_eq!(
            row_action(&detail, EnterChord::Ctrl),
            Some(RowAction::OpenFolder(PathBuf::from("C:/Tools")))
        );
        assert_eq!(
            row_action(&detail, EnterChord::CtrlShift),
            Some(RowAction::Elevated(exe()))
        );
        assert_eq!(row_action(&detail, EnterChord::Other), None);
    }

    #[test]
    fn store_app_and_game_link_only_open() {
        for target in [
            LaunchTarget::Aumid {
                aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned(),
            },
            LaunchTarget::Url {
                url: "steam://rungameid/570".to_owned(),
            },
        ] {
            let detail = app(target.clone());
            assert_eq!(
                row_action(&detail, EnterChord::Plain),
                Some(RowAction::Shell(target))
            );
            assert_eq!(row_action(&detail, EnterChord::Ctrl), None);
            assert_eq!(row_action(&detail, EnterChord::CtrlShift), None);
        }
    }

    #[test]
    fn files_folders_browser_todo_note() {
        let file = RowDetail::File {
            path: "C:/Docs/a.txt".to_owned(),
        };
        assert_eq!(
            row_action(&file, EnterChord::Plain),
            Some(RowAction::Shell(path_target("C:/Docs/a.txt")))
        );
        assert_eq!(
            row_action(&file, EnterChord::Ctrl),
            Some(RowAction::OpenFolder(PathBuf::from("C:/Docs")))
        );
        assert_eq!(row_action(&file, EnterChord::CtrlShift), None);

        let folder = RowDetail::Folder {
            path: "C:/Docs/Sub".to_owned(),
        };
        assert_eq!(
            row_action(&folder, EnterChord::Ctrl),
            Some(RowAction::OpenFolder(PathBuf::from("C:/Docs")))
        );
        let root = RowDetail::Folder {
            path: "Docs".to_owned(),
        };
        assert_eq!(row_action(&root, EnterChord::Ctrl), None);

        let browser = RowDetail::Browser {
            url: "https://example.com".to_owned(),
        };
        assert_eq!(
            row_action(&browser, EnterChord::Plain),
            Some(RowAction::Shell(LaunchTarget::Url {
                url: "https://example.com".to_owned()
            }))
        );
        assert_eq!(row_action(&browser, EnterChord::Ctrl), None);

        let todo = RowDetail::Todo {
            list_id: "l".to_owned(),
            item_id: "i".to_owned(),
        };
        assert_eq!(
            row_action(&todo, EnterChord::Plain),
            Some(RowAction::PanelTodo {
                list_id: "l".to_owned(),
                item_id: "i".to_owned()
            })
        );
        assert_eq!(row_action(&todo, EnterChord::Ctrl), None);

        let note = RowDetail::Note { id: "n".to_owned() };
        assert_eq!(
            row_action(&note, EnterChord::Plain),
            Some(RowAction::PanelNote { id: "n".to_owned() })
        );
        assert_eq!(
            row_action(&note, EnterChord::Ctrl),
            Some(RowAction::FloatNote { id: "n".to_owned() })
        );
        assert_eq!(row_action(&note, EnterChord::CtrlShift), None);
    }

    #[test]
    fn open_folder_is_not_an_open() {
        assert!(!RowAction::OpenFolder(PathBuf::from("C:\\")).counts_as_open());
        assert!(RowAction::Shell(exe()).counts_as_open());
        assert!(RowAction::Elevated(exe()).counts_as_open());
    }

    #[test]
    fn escape_and_hide_keep_only_capture_text() {
        assert_eq!(escape_action(""), EscapeAction::Hide);
        assert_eq!(escape_action("abc"), EscapeAction::Clear);
        assert_eq!(escape_action(" "), EscapeAction::Clear);
        assert_eq!(escape_action("+ 买菜"), EscapeAction::Hide);
        assert_eq!(escape_action("＋明天"), EscapeAction::Hide);
        assert_eq!(escape_action("/note 标题"), EscapeAction::Hide);
        assert_eq!(escape_action("/notes"), EscapeAction::Clear);
        assert_eq!(text_after_hide("abc"), "");
        assert_eq!(text_after_hide("+ 买菜"), "+ 买菜");
        assert_eq!(text_after_hide("/note"), "/note");
        assert_eq!(text_after_hide(""), "");
    }

    #[test]
    fn selection_does_not_wrap() {
        assert_eq!(move_selection(0, 3, false), 0);
        assert_eq!(move_selection(0, 3, true), 1);
        assert_eq!(move_selection(2, 3, true), 2);
        assert_eq!(move_selection(5, 3, false), 2);
        assert_eq!(move_selection(0, 0, true), 0);
    }

    #[test]
    fn pointed_row_ignores_stale_indexes() {
        assert_eq!(pointed_row(0, 3), Some(0));
        assert_eq!(pointed_row(2, 3), Some(2));
        assert_eq!(pointed_row(3, 3), None);
        assert_eq!(pointed_row(-1, 3), None);
        assert_eq!(pointed_row(0, 0), None);
    }

    #[test]
    fn reselect_keeps_key_for_same_text() {
        let rows = vec![row("a"), row("b"), row("c")];
        let key = UsageKey::File("b".to_owned());
        assert_eq!(reselect(Some(&key), true, &rows), 1);
        assert_eq!(reselect(Some(&key), false, &rows), 0);
        let gone = UsageKey::File("z".to_owned());
        assert_eq!(reselect(Some(&gone), true, &rows), 0);
        assert_eq!(reselect(None, true, &rows), 0);
    }
}
