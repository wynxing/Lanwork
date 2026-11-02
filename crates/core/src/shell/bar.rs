//! 搜索条里不调用窗口 API 的决定：背景、位置、按键和结果动作。
//!
//! 窗口、DWM 和 Shell 调用在 `crates/app`。

use std::path::PathBuf;

use crate::apps::{
    AppEntry, LaunchTarget, MachineRoots, RemovalPlan, file_folder, launch_key, removal_plan,
    shortcut_target_folder, supports_run_as_admin,
};
use crate::dispatch::{FileProgress, RowDetail, SearchGroup, SearchRow, UsageKey, ViewPhase};
use crate::search::classify_prefix;
use crate::shelves::DEFAULT_GROUP_NAME;

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

/// 展开的悬浮窗在 [`Backdrop::Acrylic`] 时实际用的材料。
///
/// `DWMSBT_TRANSIENTWINDOW` 是背景亚克力。窗口变成非活动时，桌面窗口管理器按策略把它换成不透明纯色。
/// 再设一次同一种类型取消不了。悬浮窗失焦后仍留在屏幕上，所以不用这一种。
/// 搜索条和面板失焦就收起，仍用那一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatBackdrop {
    /// 纯色。不画系统亚克力，也不画失焦后仍在的强调策略亚克力。
    Solid,
    /// 强调策略里的亚克力。有焦点和没有焦点是同一层。
    PersistentAcrylic,
}

/// 收起，或这次本来就要纯色时，悬浮窗用纯色。否则用失焦后仍在的亚克力。
#[must_use]
pub fn float_backdrop(wanted: Backdrop, collapsed: bool) -> FloatBackdrop {
    if !collapsed && wanted == Backdrop::Acrylic {
        FloatBackdrop::PersistentAcrylic
    } else {
        FloatBackdrop::Solid
    }
}

/// 强调策略亚克力的着色，ABGR。
///
/// 高位是透明度。剩下的三字节与纯色回退相同（浅 `#F3F3F3`、暗 `#202020`），只是更透。
/// 产品规格没有写这个比例，这是实现选择。
#[must_use]
pub fn persistent_acrylic_abgr(dark: bool) -> u32 {
    if dark { 0x4820_2020 } else { 0x48F3_F3F3 }
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

/// 一块显示器。`bounds` 是显示器矩形，`work` 是工作区。物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorArea {
    pub bounds: WorkArea,
    pub work: WorkArea,
}

/// 这次显示搜索条用的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarPlacement {
    pub origin: (i32, i32),
    /// 用来限制结果区高度的工作区。
    pub work: WorkArea,
    /// 记住的位置落在 `monitors` 的这个下标上。用默认位置时为 `None`。
    pub monitor: Option<usize>,
}

/// 搜索条这次显示的左上角。
///
/// `saved` 为 `None`，或该点不在任何 `monitors.bounds` 内时，用 [`bar_origin`]
/// （光标所在显示器的工作区水平居中、上边缘 25%）。点在某块显示器内时原样使用，不拉回。
/// 判断是左闭右开、上闭下开。多块重叠时用切片里的第一块。
/// 这是待定项的临时做法，不是产品规则。
#[must_use]
pub fn place_bar(
    saved: Option<(i32, i32)>,
    monitors: &[MonitorArea],
    cursor_work: WorkArea,
    width_px: i32,
) -> BarPlacement {
    if let Some((x, y)) = saved
        && let Some(index) = monitor_at(monitors, x, y)
    {
        return BarPlacement {
            origin: (x, y),
            work: monitors[index].work,
            monitor: Some(index),
        };
    }
    BarPlacement {
        origin: bar_origin(cursor_work, width_px),
        work: cursor_work,
        monitor: None,
    }
}

fn point_inside(area: WorkArea, x: i32, y: i32) -> bool {
    x >= area.left && x < area.right && y >= area.top && y < area.bottom
}

/// 左上角落在哪一块显示器的 `bounds` 上。左闭右开、上闭下开。重叠时取第一块。
#[must_use]
pub fn monitor_at(monitors: &[MonitorArea], x: i32, y: i32) -> Option<usize> {
    monitors
        .iter()
        .position(|monitor| point_inside(monitor.bounds, x, y))
}

/// 结果列表在窗口纵坐标 `origin_y` 下的最大逻辑高度。
///
/// 距离是从 `origin_y` 到 `work.bottom` 的物理像素，按 `dpi` 换成逻辑像素，再减去输入行和底边留白。
/// 换算四舍五入后若会多出物理像素，就减掉对应的逻辑像素，使输入行、列表和留白不超过这段距离。
/// 放不下输入行加留白时返回 0。约 9 行的上限由调用方再取较小值。
/// 空间不够时列表高度为 0、窗口留在原地，是待定项的临时做法。
#[must_use]
pub fn max_results_height(
    work: WorkArea,
    origin_y: i32,
    dpi: u32,
    input_height: u32,
    bottom_gap: u32,
) -> u32 {
    let available = i64::from(work.bottom)
        .saturating_sub(i64::from(origin_y))
        .max(0);
    logical_within(available, dpi).saturating_sub(input_height.saturating_add(bottom_gap))
}

/// 最大的逻辑长度，按 [`to_physical`] 换算后不超过 `available` 个物理像素。
fn logical_within(available: i64, dpi: u32) -> u32 {
    let dpi = u64::from(if dpi == 0 { BASE_DPI } else { dpi });
    let Ok(available) = u64::try_from(available) else {
        return 0;
    };
    // (logical * dpi + BASE_DPI/2) / BASE_DPI <= available
    // logical <= (available * BASE_DPI + BASE_DPI/2 - 1) / dpi
    let base = u64::from(BASE_DPI);
    let Some(span) = available.checked_mul(base) else {
        return u32::MAX;
    };
    let Some(limit) = span.checked_add(base / 2 - 1) else {
        return u32::MAX;
    };
    u32::try_from(limit / dpi).unwrap_or(u32::MAX)
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
        SearchGroup::Shelf => "收纳",
        SearchGroup::File => "文件",
        SearchGroup::Folder => "文件夹",
        SearchGroup::WebSearch => "网页搜索",
    }
}

/// 没有壳层图标或网站图标时，是否画类型图形。收纳里的网页引用留空，不画地球。
#[must_use]
pub fn draws_type_glyph(detail: &RowDetail) -> bool {
    !matches!(detail, RowDetail::Web { .. } | RowDetail::PathTimeout)
}

/// 文件和文件夹在名称下方总是显示路径。收纳在名称下方总是显示所属分组。其它结果选中后才显示。
#[must_use]
pub fn location_always_shown(group: SearchGroup) -> bool {
    matches!(
        group,
        SearchGroup::File | SearchGroup::Folder | SearchGroup::Shelf
    )
}

/// 名称里与输入相同的第一段，返回字节范围。逐字符比较，不区分大小写；
/// 输入去掉首尾空白。名称里没有这段原文时（拼音、首字母、模糊或备用名命中）返回 `None`。
#[must_use]
pub fn match_span(label: &str, input: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = input.trim().chars().map(fold_case).collect();
    if needle.is_empty() {
        return None;
    }
    'start: for (start, _) in label.char_indices() {
        let mut end = start;
        let mut rest = label[start..].chars();
        for wanted in &needle {
            match rest.next() {
                Some(ch) if fold_case(ch) == *wanted => end += ch.len_utf8(),
                _ => continue 'start,
            }
        }
        return Some((start, end));
    }
    None
}

/// 小写恰好是一个字符时取小写，否则保留原字符，字节长度对应关系不变。
fn fold_case(ch: char) -> char {
    let mut lower = ch.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => ch,
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
    /// 动词 `runasuser`。只有本地 exe 的应用。
    RunAsUser(LaunchTarget),
    /// 打开这个目录。
    OpenFolder(PathBuf),
    /// 从搜索结果里隐藏这个应用。键是启动目标的去重键。
    HideApp { key: String },
    /// 打开面板、切到待办并定位该条。
    PanelTodo { list_id: String, item_id: String },
    /// 打开面板、切到便签并打开该篇。
    PanelNote { id: String },
    /// 悬浮该便签。
    FloatNote { id: String },
    /// 用默认浏览器打开搜索页。不计使用次数。
    WebSearch { url: String },
    /// 删除这条开始菜单快捷方式或游戏链接。
    CleanStartMenu { path: PathBuf },
    /// 启动官方卸载。命令在打开菜单时算好的 [`RemovalPlan`] 里。
    UninstallApp,
}

impl RowAction {
    /// 打开了这一项本身时次数加 1。打开目录和隐藏不算打开这一项。
    #[must_use]
    pub fn counts_as_open(&self) -> bool {
        !matches!(
            self,
            Self::OpenFolder(_)
                | Self::HideApp { .. }
                | Self::WebSearch { .. }
                | Self::CleanStartMenu { .. }
                | Self::UninstallApp
        )
    }
}

/// 应用结果右键菜单的一项。顺序和 Alt 数字是产品规格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMenuAction {
    RunAsUser,
    RunAsAdmin,
    OpenFileFolder,
    OpenTargetFolder,
    Hide,
    CleanStartMenu,
    Uninstall,
}

/// 菜单上看得见的一行。不可用的项仍占着原来的 Alt 数字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppMenuItem {
    pub action: AppMenuAction,
    pub label: &'static str,
    pub hint: &'static str,
    pub enabled: bool,
}

const APP_MENU: [(AppMenuAction, &str, &str); 7] = [
    (AppMenuAction::RunAsUser, "以其他用户身份运行", "Alt+1"),
    (AppMenuAction::RunAsAdmin, "以管理员身份运行", "Alt+2"),
    (AppMenuAction::OpenFileFolder, "打开文件所在文件夹", "Alt+3"),
    (AppMenuAction::OpenTargetFolder, "打开目标文件夹", "Alt+4"),
    (AppMenuAction::Hide, "隐藏", "Alt+5"),
    (AppMenuAction::CleanStartMenu, "清理开始菜单入口", "Alt+6"),
    (AppMenuAction::Uninstall, "卸载…", "Alt+7"),
];

/// 应用结果的右键菜单。七项都在，做不到的那一项 `enabled` 为假。
///
/// 清理和卸载按空的卸载登记和内置目录判断。界面打开菜单时改用 [`app_menu_with`]。
#[must_use]
pub fn app_menu(entry: &AppEntry) -> [AppMenuItem; 7] {
    app_menu_with(entry, &removal_plan(entry, &[], &MachineRoots::builtin()))
}

/// 用已经算好的清理和卸载计划填充菜单。
#[must_use]
pub fn app_menu_with(entry: &AppEntry, plan: &RemovalPlan) -> [AppMenuItem; 7] {
    APP_MENU.map(|(action, label, hint)| AppMenuItem {
        action,
        label,
        hint,
        enabled: menu_enabled(entry, plan, action),
    })
}

/// 搜索结果里的应用菜单。顺序与 [`app_menu_with`] 相同。
#[must_use]
pub fn app_result_menu(entry: &AppEntry, plan: &RemovalPlan) -> Vec<ResultMenuItem> {
    app_menu_with(entry, plan)
        .into_iter()
        .map(app_menu_item)
        .collect()
}

fn menu_enabled(entry: &AppEntry, plan: &RemovalPlan, action: AppMenuAction) -> bool {
    match action {
        AppMenuAction::RunAsUser | AppMenuAction::RunAsAdmin => {
            supports_run_as_admin(&entry.target)
        }
        AppMenuAction::OpenFileFolder => file_folder(entry).is_some(),
        AppMenuAction::OpenTargetFolder => shortcut_target_folder(entry).is_some(),
        AppMenuAction::Hide => !launch_key(&entry.target).is_empty(),
        AppMenuAction::CleanStartMenu => plan.clean_path.is_some(),
        AppMenuAction::Uninstall => plan.uninstall.is_some(),
    }
}

/// 菜单项对应的动作。不可用时为 `None`。清理和卸载按空的卸载登记判断。
#[must_use]
pub fn app_menu_command(entry: &AppEntry, action: AppMenuAction) -> Option<RowAction> {
    let plan = removal_plan(entry, &[], &MachineRoots::builtin());
    app_menu_command_with(entry, &plan, action)
}

/// 菜单项对应的动作。不可用时为 `None`。
#[must_use]
pub fn app_menu_command_with(
    entry: &AppEntry,
    plan: &RemovalPlan,
    action: AppMenuAction,
) -> Option<RowAction> {
    if !menu_enabled(entry, plan, action) {
        return None;
    }
    match action {
        AppMenuAction::RunAsUser => Some(RowAction::RunAsUser(entry.target.clone())),
        AppMenuAction::RunAsAdmin => Some(RowAction::Elevated(entry.target.clone())),
        AppMenuAction::OpenFileFolder => file_folder(entry).map(RowAction::OpenFolder),
        AppMenuAction::OpenTargetFolder => shortcut_target_folder(entry).map(RowAction::OpenFolder),
        AppMenuAction::Hide => Some(RowAction::HideApp {
            key: launch_key(&entry.target),
        }),
        AppMenuAction::CleanStartMenu => plan
            .clean_path
            .clone()
            .map(|path| RowAction::CleanStartMenu { path }),
        AppMenuAction::Uninstall => Some(RowAction::UninstallApp),
    }
}

/// Alt+数字在菜单开着和关着时去向不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltRoute {
    /// 菜单开着。`None` 表示这个数字没有对应的菜单项。
    Menu(Option<AppMenuAction>),
    /// 菜单关着。数字原样交给结果列表。
    Result(u32),
}

#[must_use]
pub fn alt_route(menu_open: bool, digit: u32) -> AltRoute {
    if !menu_open {
        return AltRoute::Result(digit);
    }
    let action = match digit {
        1 => Some(AppMenuAction::RunAsUser),
        2 => Some(AppMenuAction::RunAsAdmin),
        3 => Some(AppMenuAction::OpenFileFolder),
        4 => Some(AppMenuAction::OpenTargetFolder),
        5 => Some(AppMenuAction::Hide),
        6 => Some(AppMenuAction::CleanStartMenu),
        7 => Some(AppMenuAction::Uninstall),
        _ => None,
    };
    AltRoute::Menu(action)
}

/// 搜索结果右键菜单的一项。应用七项仍由 [`app_menu`] 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultMenuAction {
    RunAsUser,
    RunAsAdmin,
    OpenFileFolder,
    OpenTargetFolder,
    Hide,
    CleanStartMenu,
    Uninstall,
    /// 文件或文件夹的上一级。与 Ctrl+Enter 相同。
    OpenContainingFolder,
    CopyPath,
    CopyFile,
    /// 换成收纳分组列表，由用户选一个分组。
    AddToShelf,
    FloatNote,
    CopyUrl,
}

/// 菜单上看得见的一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultMenuItem {
    pub action: ResultMenuAction,
    pub label: &'static str,
    pub hint: &'static str,
    pub enabled: bool,
}

/// 加入收纳时列出的一个分组。`group_id` 为空表示还没有分组，选中后由服务创建「收纳」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfPickItem {
    pub group_id: Option<String>,
    pub label: String,
    pub hint: &'static str,
}

const PATH_MENU: [(ResultMenuAction, &str, &str); 4] = [
    (
        ResultMenuAction::OpenContainingFolder,
        "打开所在文件夹",
        "Alt+1",
    ),
    (ResultMenuAction::CopyPath, "复制路径", "Alt+2"),
    (ResultMenuAction::CopyFile, "复制文件", "Alt+3"),
    (ResultMenuAction::AddToShelf, "加入收纳…", "Alt+4"),
];

/// 第 `index` 项的 Alt 提示。超过 9 项时为空字符串，数字没有对应项。
#[must_use]
pub fn alt_hint(index: usize) -> &'static str {
    match index {
        0 => "Alt+1",
        1 => "Alt+2",
        2 => "Alt+3",
        3 => "Alt+4",
        4 => "Alt+5",
        5 => "Alt+6",
        6 => "Alt+7",
        7 => "Alt+8",
        8 => "Alt+9",
        _ => "",
    }
}

/// 菜单开着时，Alt+数字对应的项下标。超出项数或超过 9 时为 `None`。
#[must_use]
pub fn menu_index(digit: u32, len: usize) -> Option<usize> {
    let index = usize::try_from(digit.checked_sub(1)?).ok()?;
    (index < SHORTCUT_COUNT && index < len).then_some(index)
}

/// 光标的 UTF-8 字节偏移落在已提交文本末尾。
#[must_use]
pub fn caret_at_end(text: &str, byte_offset: i32) -> bool {
    usize::try_from(byte_offset).is_ok_and(|offset| offset == text.len())
}

/// 这一条结果的右键菜单。没有菜单的结果为 `None`。
#[must_use]
pub fn result_menu(detail: &RowDetail) -> Option<Vec<ResultMenuItem>> {
    match detail {
        RowDetail::App(hit) => Some(
            app_menu(&hit.entry)
                .into_iter()
                .map(app_menu_item)
                .collect(),
        ),
        RowDetail::File { path } | RowDetail::Folder { path } => Some(path_menu(path).to_vec()),
        RowDetail::Note { id } => Some(vec![ResultMenuItem {
            action: ResultMenuAction::FloatNote,
            label: "悬浮",
            hint: "Alt+1",
            enabled: !id.is_empty(),
        }]),
        RowDetail::Web { url } => Some(vec![ResultMenuItem {
            action: ResultMenuAction::CopyUrl,
            label: "复制网址",
            hint: "Alt+1",
            enabled: text_usable(url),
        }]),
        RowDetail::Browser { .. }
        | RowDetail::Todo { .. }
        | RowDetail::WebSearch { .. }
        | RowDetail::PathTimeout => None,
    }
}

fn app_menu_item(item: AppMenuItem) -> ResultMenuItem {
    let action = match item.action {
        AppMenuAction::RunAsUser => ResultMenuAction::RunAsUser,
        AppMenuAction::RunAsAdmin => ResultMenuAction::RunAsAdmin,
        AppMenuAction::OpenFileFolder => ResultMenuAction::OpenFileFolder,
        AppMenuAction::OpenTargetFolder => ResultMenuAction::OpenTargetFolder,
        AppMenuAction::Hide => ResultMenuAction::Hide,
        AppMenuAction::CleanStartMenu => ResultMenuAction::CleanStartMenu,
        AppMenuAction::Uninstall => ResultMenuAction::Uninstall,
    };
    ResultMenuItem {
        action,
        label: item.label,
        hint: item.hint,
        enabled: item.enabled,
    }
}

fn path_menu(path: &str) -> [ResultMenuItem; 4] {
    let usable = text_usable(path);
    PATH_MENU.map(|(action, label, hint)| ResultMenuItem {
        action,
        label,
        hint,
        enabled: match action {
            ResultMenuAction::OpenContainingFolder => usable && parent_folder(path).is_some(),
            _ => usable,
        },
    })
}

fn text_usable(text: &str) -> bool {
    !text.is_empty() && !text.contains('\0')
}

/// 加入收纳时的分组列表。还没有分组时只有一行「收纳」，`group_id` 为空。
#[must_use]
pub fn shelf_pick_menu(groups: &[(&str, &str)]) -> Vec<ShelfPickItem> {
    if groups.is_empty() {
        return vec![ShelfPickItem {
            group_id: None,
            label: DEFAULT_GROUP_NAME.to_owned(),
            hint: alt_hint(0),
        }];
    }
    groups
        .iter()
        .enumerate()
        .map(|(index, (id, name))| ShelfPickItem {
            group_id: Some((*id).to_owned()),
            label: (*name).to_owned(),
            hint: alt_hint(index),
        })
        .collect()
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
        (RowDetail::App(hit), EnterChord::Ctrl | EnterChord::CtrlShift) => {
            supports_run_as_admin(&hit.entry.target)
                .then(|| RowAction::Elevated(hit.entry.target.clone()))
        }
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
        (RowDetail::Web { url }, EnterChord::Plain) => {
            Some(RowAction::Shell(LaunchTarget::Url { url: url.clone() }))
        }
        (RowDetail::WebSearch { url }, EnterChord::Plain) => {
            Some(RowAction::WebSearch { url: url.clone() })
        }
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

/// Alt+1 至 Alt+9 的个数。
pub const SHORTCUT_COUNT: usize = 9;

/// 结果区里完整可见的前几行，最多 [`SHORTCUT_COUNT`] 行，返回 `start..end`。
/// `offsets` 来自 [`row_offsets`]；`scroll_top` 是列表向上滚过的逻辑像素，`viewport` 是结果区高度。
#[must_use]
pub fn shortcut_rows(offsets: &[u32], scroll_top: f32, viewport: u32) -> std::ops::Range<usize> {
    let top = f64::from(scroll_top.max(0.0).round());
    let bottom = top + f64::from(viewport);
    let visible = |y: u32| f64::from(y) >= top && f64::from(y) + f64::from(ROW_HEIGHT) <= bottom;
    let Some(start) = offsets.iter().position(|y| visible(*y)) else {
        return 0..0;
    };
    let count = offsets[start..]
        .iter()
        .take(SHORTCUT_COUNT)
        .take_while(|y| visible(**y))
        .count();
    start..start + count
}

/// Alt+数字对应的行号。`digit` 是 1 至 9；超出完整可见的行时没有动作。
#[must_use]
pub fn shortcut_target(rows: std::ops::Range<usize>, digit: u32) -> Option<usize> {
    let offset = usize::try_from(digit.checked_sub(1)?).ok()?;
    if offset >= SHORTCUT_COUNT {
        return None;
    }
    let index = rows.start.checked_add(offset)?;
    rows.contains(&index).then_some(index)
}

/// 鼠标悬停或单击的行号。界面传来的行号可能来自已经被替换的列表，越界时不选。
#[must_use]
pub fn pointed_row(index: i32, len: usize) -> Option<usize> {
    usize::try_from(index).ok().filter(|index| *index < len)
}

/// 锚点对齐之后，相对锚点至少差这么多物理像素，才算指针又动了。
pub const HOVER_MOVE_PX: u32 = 2;

/// 打开或结果刷新时采到的光标，和界面第一次报上来的坐标，差不到这么多物理像素
/// 仍视为同一个点。用来吃掉客户区换算在高 DPI 下大约 1 个逻辑像素的误差。
pub const HOVER_SNAP_PX: u32 = 12;

/// 物理像素上的指针位置。逻辑坐标先乘窗口缩放，再四舍五入到这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerPos {
    x: i32,
    y: i32,
}

impl PointerPos {
    #[must_use]
    pub fn from_physical(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// 窗口内的逻辑坐标。`scale` 不是有限正数，或乘出来不是有限数时返回 `None`。
    #[must_use]
    pub fn from_logical(x: f32, y: f32, scale: f32) -> Option<Self> {
        Some(Self {
            x: logical_to_physical(x, scale)?,
            y: logical_to_physical(y, scale)?,
        })
    }

    #[must_use]
    pub fn x(self) -> i32 {
        self.x
    }

    #[must_use]
    pub fn y(self) -> i32 {
        self.y
    }
}

fn logical_to_physical(logical: f32, scale: f32) -> Option<i32> {
    if !logical.is_finite() || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let physical = logical * scale;
    if !physical.is_finite() {
        return None;
    }
    let rounded = physical.round();
    if rounded > i32::MAX as f32 || rounded < i32::MIN as f32 {
        return None;
    }
    Some(rounded as i32)
}

fn pointer_apart(a: PointerPos, b: PointerPos, px: u32) -> bool {
    a.x.abs_diff(b.x) >= px || a.y.abs_diff(b.y) >= px
}

/// 悬停能不能改选中项。
///
/// 搜索条打开时，以及每次结果刷新时，记下当时的指针位置。界面第一次报上来的坐标如果还在
/// 这个点附近，只把锚点对齐过去，不改选中。之后指针再移动，悬停才改选中项。
/// 指针停在原处，或结果在静止的指针下面出现，都不改。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HoverArm {
    anchor: Option<PointerPos>,
    /// 已经用界面坐标对齐过锚点。对齐前，靠近采样点的事件只对齐。
    snapped: bool,
}

impl HoverArm {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `cursor` 是这一刻的屏幕坐标。还不知道时传 `None`，下一次采样只记下位置。
    pub fn reset(&mut self, cursor: Option<PointerPos>) {
        self.anchor = cursor;
        self.snapped = false;
    }

    /// 指针到了 `pos`。相对锚点真的移动过才返回 `true`，并把锚点改到这个位置。
    #[must_use]
    pub fn moved_to(&mut self, pos: PointerPos) -> bool {
        let Some(anchor) = self.anchor else {
            self.anchor = Some(pos);
            self.snapped = true;
            return false;
        };
        if !self.snapped && !pointer_apart(anchor, pos, HOVER_SNAP_PX) {
            self.anchor = Some(pos);
            self.snapped = true;
            return false;
        }
        self.snapped = true;
        if pointer_apart(anchor, pos, HOVER_MOVE_PX) {
            self.anchor = Some(pos);
            true
        } else {
            false
        }
    }
}

/// 悬停改选中项。指针还没相对锚点移动，或行号越界时，保持原来的选中项。
#[must_use]
pub fn hover_row(arm: &mut HoverArm, pos: PointerPos, index: i32, len: usize) -> Option<usize> {
    if !arm.moved_to(pos) {
        return None;
    }
    pointed_row(index, len)
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

/// 同一段输入的后续结果沿用 [`reselect`]。`path_prepended` 为真时，路径这一组刚接到最前面，选中新的第一条。
#[must_use]
pub fn selection_after_refresh(
    previous: Option<&UsageKey>,
    same_text: bool,
    rows: &[SearchRow],
    path_prepended: bool,
) -> usize {
    if path_prepended {
        return 0;
    }
    reselect(previous, same_text, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{AppEntry, AppHit, AppSource};
    use crate::search::HitKind;

    #[test]
    fn shortcuts_number_the_fully_visible_rows_up_to_nine() {
        let groups = vec![SearchGroup::Application; 12];
        let (offsets, height) = row_offsets(&groups);
        assert_eq!(shortcut_rows(&offsets, 0.0, height), 0..9);
        assert_eq!(shortcut_rows(&offsets[..3], 0.0, height), 0..3);
        let viewport = 5 * ROW_HEIGHT + 2 * LIST_PAD;
        assert_eq!(shortcut_rows(&offsets, 0.0, viewport), 0..5);
        // 滚过一行半：第 0、1 行不完整可见，从第 2 行起编号。
        let half = f32::from(u16::try_from(LIST_PAD + ROW_HEIGHT + ROW_HEIGHT / 2).unwrap());
        assert_eq!(shortcut_rows(&offsets, half, viewport), 2..6);
        let one_row = f32::from(u16::try_from(LIST_PAD + ROW_HEIGHT).unwrap());
        assert_eq!(shortcut_rows(&offsets, one_row, viewport), 1..6);
        assert_eq!(shortcut_rows(&[], 0.0, viewport), 0..0);
    }

    #[test]
    fn alt_digit_maps_into_the_numbered_rows_only() {
        assert_eq!(shortcut_target(2..6, 1), Some(2));
        assert_eq!(shortcut_target(2..6, 4), Some(5));
        assert_eq!(shortcut_target(2..6, 5), None);
        assert_eq!(shortcut_target(0..9, 9), Some(8));
        assert_eq!(shortcut_target(0..12, 10), None);
        assert_eq!(shortcut_target(0..9, 0), None);
        assert_eq!(shortcut_target(0..0, 1), None);
    }

    #[test]
    fn web_search_opens_only_with_plain_enter_and_does_not_count() {
        let detail = RowDetail::WebSearch {
            url: "https://www.google.com/search?q=chat".to_owned(),
        };
        let action = row_action(&detail, EnterChord::Plain).unwrap();
        assert_eq!(
            action,
            RowAction::WebSearch {
                url: "https://www.google.com/search?q=chat".to_owned()
            }
        );
        assert!(!action.counts_as_open());
        assert_eq!(row_action(&detail, EnterChord::Ctrl), None);
        assert_eq!(row_action(&detail, EnterChord::CtrlShift), None);
        assert_eq!(group_label(SearchGroup::WebSearch), "网页搜索");
        assert!(!location_always_shown(SearchGroup::WebSearch));
    }

    #[test]
    fn match_span_is_the_first_case_insensitive_occurrence() {
        assert_eq!(match_span("ChatGPT", "chat"), Some((0, 4)));
        assert_eq!(match_span("my chats", " CHAT "), Some((3, 7)));
        assert_eq!(match_span("abab", "ab"), Some((0, 2)));
        assert_eq!(match_span("微信", "信"), Some((3, 6)));
        assert_eq!(match_span("Google Chrome", "e c"), Some((5, 8)));
        assert_eq!(match_span("微信", "wx"), None);
        assert_eq!(match_span("Visual Studio Code", "vsc"), None);
        assert_eq!(match_span("ChatGPT", "   "), None);
        assert_eq!(match_span("Chat", "chatgpt"), None);
        let (start, end) = match_span("İstanbul", "stan").unwrap();
        assert_eq!(&"İstanbul"[start..end], "stan");
    }

    #[test]
    fn files_folders_and_shelf_always_show_the_second_line() {
        assert!(location_always_shown(SearchGroup::File));
        assert!(location_always_shown(SearchGroup::Folder));
        assert!(location_always_shown(SearchGroup::Shelf));
        for group in [
            SearchGroup::Browser,
            SearchGroup::Application,
            SearchGroup::Todo,
            SearchGroup::Note,
        ] {
            assert!(!location_always_shown(group));
        }
        assert!(!draws_type_glyph(&RowDetail::Web {
            url: "https://example.com".to_owned(),
        }));
        assert!(draws_type_glyph(&RowDetail::File {
            path: r"C:\a.txt".to_owned(),
        }));
    }

    #[test]
    fn labels_cover_every_group() {
        assert_eq!(
            [
                SearchGroup::Browser,
                SearchGroup::Application,
                SearchGroup::Todo,
                SearchGroup::Note,
                SearchGroup::Shelf,
                SearchGroup::File,
                SearchGroup::Folder,
            ]
            .map(group_label),
            ["浏览器", "应用", "待办", "便签", "收纳", "文件", "文件夹"]
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
                source_path: None,
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
    fn float_backdrop_stays_acrylic_only_while_expanded_and_wanted() {
        assert_eq!(
            float_backdrop(Backdrop::Acrylic, false),
            FloatBackdrop::PersistentAcrylic
        );
        assert_eq!(
            float_backdrop(Backdrop::Acrylic, true),
            FloatBackdrop::Solid,
            "收起的把手不用磨砂"
        );
        assert_eq!(float_backdrop(Backdrop::Solid, false), FloatBackdrop::Solid);
        assert_eq!(float_backdrop(Backdrop::Solid, true), FloatBackdrop::Solid);
        assert_eq!(persistent_acrylic_abgr(false), 0x48F3_F3F3);
        assert_eq!(persistent_acrylic_abgr(true), 0x4820_2020);
        assert_eq!(
            persistent_acrylic_abgr(false) & 0x00FF_FFFF,
            solid_rgb(false)
        );
        assert_eq!(persistent_acrylic_abgr(true) & 0x00FF_FFFF, solid_rgb(true));
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

    fn monitor(bounds: WorkArea, work: WorkArea) -> MonitorArea {
        MonitorArea { bounds, work }
    }

    #[test]
    fn missing_origin_uses_the_default_placement() {
        let cursor = WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let placed = place_bar(None, &[monitor(cursor, cursor)], cursor, 680);
        assert_eq!(placed.origin, (620, 260));
        assert_eq!(placed.work, cursor);
        assert_eq!(placed.monitor, None);
    }

    #[test]
    fn saved_origin_uses_the_monitor_that_contains_the_point() {
        let primary = WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let second_bounds = WorkArea {
            left: 1920,
            top: 0,
            right: 4480,
            bottom: 1440,
        };
        let second_work = WorkArea {
            left: 1920,
            top: 40,
            right: 4480,
            bottom: 1400,
        };
        let placed = place_bar(
            Some((2000, 100)),
            &[
                monitor(primary, primary),
                monitor(second_bounds, second_work),
            ],
            primary,
            680,
        );
        assert_eq!(placed.origin, (2000, 100));
        assert_eq!(placed.work, second_work);
        assert_eq!(placed.monitor, Some(1));
    }

    #[test]
    fn saved_origin_off_every_monitor_falls_back_and_edges_are_half_open() {
        let cursor = WorkArea {
            left: 100,
            top: 50,
            right: 1000,
            bottom: 800,
        };
        let monitors = [monitor(cursor, cursor)];
        let fallback = place_bar(Some((-5000, 10)), &monitors, cursor, 400);
        assert_eq!(fallback.origin, bar_origin(cursor, 400));
        assert_eq!(fallback.monitor, None);
        assert_eq!(fallback.work, cursor);

        assert_eq!(
            place_bar(Some((1000, 50)), &monitors, cursor, 400).monitor,
            None
        );
        assert_eq!(
            place_bar(Some((100, 800)), &monitors, cursor, 400).monitor,
            None
        );
        let on_corner = place_bar(Some((100, 50)), &monitors, cursor, 400);
        assert_eq!(on_corner.origin, (100, 50));
        assert_eq!(on_corner.monitor, Some(0));
    }

    #[test]
    fn negative_monitor_keeps_a_point_on_it_and_empty_list_falls_back() {
        let bounds = WorkArea {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        let placed = place_bar(Some((-100, 10)), &[monitor(bounds, bounds)], bounds, 680);
        assert_eq!(placed.origin, (-100, 10));
        assert_eq!(placed.monitor, Some(0));

        let cursor = WorkArea {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        };
        let fallback = place_bar(Some((10, 10)), &[], cursor, 200);
        assert_eq!(fallback.origin, bar_origin(cursor, 200));
        assert_eq!(fallback.monitor, None);
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
        let default_y = work.top + work.height() / 4;
        assert_eq!(max_results_height(work, default_y, 144, 56, 24), 540 - 80);
    }

    #[test]
    fn results_list_stays_inside_the_work_area_at_any_y() {
        const INPUT: u32 = 56;
        const GAP: u32 = 24;
        const CAP: u32 = 9 * ROW_HEIGHT + 2 * LIST_PAD;
        let screens = [
            WorkArea {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            WorkArea {
                left: 1920,
                top: 0,
                right: 3200,
                bottom: 800,
            },
            WorkArea {
                left: -1920,
                top: -200,
                right: 0,
                bottom: 880,
            },
        ];
        for work in screens {
            for dpi in [96, 120, 144] {
                let mut y = work.top - 300;
                while y < work.bottom + 80 {
                    let room = max_results_height(work, y, dpi, INPUT, GAP);
                    let shown = room.min(CAP);
                    let available = i64::from(work.bottom).saturating_sub(i64::from(y)).max(0);
                    if shown > 0 {
                        let used = i64::from(to_physical(
                            INPUT.saturating_add(shown).saturating_add(GAP),
                            dpi,
                        ));
                        assert!(
                            used <= available,
                            "y={y} dpi={dpi} room={room} shown={shown} used={used} available={available} work={work:?}"
                        );
                    }
                    let reserved = i64::from(to_physical(INPUT + GAP, dpi));
                    if available < reserved {
                        assert_eq!(room, 0, "y={y} dpi={dpi} work={work:?}");
                    }
                    y += 7;
                }
                assert_eq!(
                    max_results_height(work, work.bottom.saturating_sub(1), dpi, INPUT, GAP),
                    0
                );
                assert_eq!(
                    max_results_height(work, work.bottom.saturating_add(40), dpi, INPUT, GAP),
                    0
                );
            }
        }
    }

    #[test]
    fn dragging_down_on_a_150_percent_screen_shrinks_the_list() {
        let work = WorkArea {
            left: 1920,
            top: 0,
            right: 3200,
            bottom: 800,
        };
        let dpi = 144;
        let default_y = work.top + work.height() / 4;
        let dragged = default_y + 16;
        let sized_for_default = {
            let below = work.height() - work.height() / 4;
            to_logical(below, dpi).saturating_sub(56 + 24)
        };
        let available = i64::from(work.bottom - dragged);
        assert!(i64::from(to_physical(56 + sized_for_default + 24, dpi)) > available);
        let room = max_results_height(work, dragged, dpi, 56, 24);
        assert!(room < sized_for_default);
        assert!(i64::from(to_physical(56 + room + 24, dpi)) <= available);
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
            Some(RowAction::Elevated(exe()))
        );
        assert_eq!(
            row_action(&detail, EnterChord::CtrlShift),
            Some(RowAction::Elevated(exe()))
        );
        assert_eq!(row_action(&detail, EnterChord::Other), None);
    }

    #[test]
    fn app_menu_keeps_alt_numbers_and_disables_a_missing_target() {
        let mut detail = app(exe());
        let RowDetail::App(hit) = &mut detail else {
            panic!("app");
        };
        hit.entry.source_path = Some(PathBuf::from(
            "C:/ProgramData/Microsoft/Windows/Start Menu/Programs/Demo.lnk",
        ));
        let menu = app_menu(&hit.entry);
        assert_eq!(
            menu.map(|item| (item.label, item.hint, item.enabled)),
            [
                ("以其他用户身份运行", "Alt+1", true),
                ("以管理员身份运行", "Alt+2", true),
                ("打开文件所在文件夹", "Alt+3", true),
                ("打开目标文件夹", "Alt+4", true),
                ("隐藏", "Alt+5", true),
                ("清理开始菜单入口", "Alt+6", true),
                ("卸载…", "Alt+7", false),
            ]
        );
        assert!(matches!(
            app_menu_command(&hit.entry, AppMenuAction::CleanStartMenu),
            Some(RowAction::CleanStartMenu { .. })
        ));
        assert_eq!(app_menu_command(&hit.entry, AppMenuAction::Uninstall), None);
        assert_eq!(
            app_menu_command(&hit.entry, AppMenuAction::OpenFileFolder),
            Some(RowAction::OpenFolder(PathBuf::from(
                "C:/ProgramData/Microsoft/Windows/Start Menu/Programs"
            )))
        );
        assert_eq!(
            app_menu_command(&hit.entry, AppMenuAction::OpenTargetFolder),
            Some(RowAction::OpenFolder(PathBuf::from("C:/Tools")))
        );
        assert!(matches!(
            app_menu_command(&hit.entry, AppMenuAction::RunAsUser),
            Some(RowAction::RunAsUser(_))
        ));
        assert!(matches!(
            app_menu_command(&hit.entry, AppMenuAction::Hide),
            Some(RowAction::HideApp { .. })
        ));

        let direct = app(exe());
        let RowDetail::App(direct) = &direct else {
            panic!("app");
        };
        assert!(!app_menu(&direct.entry)[3].enabled);
        assert!(!app_menu(&direct.entry)[5].enabled);
        assert!(!app_menu(&direct.entry)[6].enabled);
        assert_eq!(
            app_menu_command(&direct.entry, AppMenuAction::OpenTargetFolder),
            None
        );
        assert_eq!(
            app_menu_command(&direct.entry, AppMenuAction::OpenFileFolder),
            Some(RowAction::OpenFolder(PathBuf::from("C:/Tools")))
        );

        for target in [
            LaunchTarget::Aumid {
                aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned(),
            },
            LaunchTarget::Url {
                url: "steam://rungameid/570".to_owned(),
            },
        ] {
            let detail = app(target);
            let RowDetail::App(hit) = &detail else {
                panic!("app");
            };
            let menu = app_menu(&hit.entry);
            assert!(!menu[0].enabled);
            assert!(!menu[1].enabled);
            assert!(!menu[3].enabled);
            assert!(menu[4].enabled);
            assert!(!menu[5].enabled);
            assert_eq!(
                app_menu_command(&hit.entry, AppMenuAction::RunAsAdmin),
                None
            );
            assert!(matches!(
                app_menu_command(&hit.entry, AppMenuAction::Hide),
                Some(RowAction::HideApp { .. })
            ));
        }
        let store = app(LaunchTarget::Aumid {
            aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned(),
        });
        let RowDetail::App(store) = &store else {
            panic!("store");
        };
        assert!(app_menu(&store.entry)[6].enabled);
        assert_eq!(
            app_menu_command(&store.entry, AppMenuAction::Uninstall),
            Some(RowAction::UninstallApp)
        );
    }

    #[test]
    fn alt_digits_go_to_the_menu_only_while_it_is_open() {
        assert_eq!(alt_route(false, 1), AltRoute::Result(1));
        assert_eq!(alt_route(false, 9), AltRoute::Result(9));
        assert_eq!(
            alt_route(true, 1),
            AltRoute::Menu(Some(AppMenuAction::RunAsUser))
        );
        assert_eq!(
            alt_route(true, 4),
            AltRoute::Menu(Some(AppMenuAction::OpenTargetFolder))
        );
        assert_eq!(
            alt_route(true, 5),
            AltRoute::Menu(Some(AppMenuAction::Hide))
        );
        assert_eq!(
            alt_route(true, 6),
            AltRoute::Menu(Some(AppMenuAction::CleanStartMenu))
        );
        assert_eq!(
            alt_route(true, 7),
            AltRoute::Menu(Some(AppMenuAction::Uninstall))
        );
        assert_eq!(alt_route(true, 8), AltRoute::Menu(None));
        assert_eq!(alt_route(true, 9), AltRoute::Menu(None));
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
        let open = row_action(&browser, EnterChord::Plain).unwrap();
        assert_eq!(
            open,
            RowAction::Shell(LaunchTarget::Url {
                url: "https://example.com".to_owned()
            })
        );
        assert!(open.counts_as_open());
        assert_eq!(row_action(&browser, EnterChord::Ctrl), None);
        assert_eq!(group_label(SearchGroup::Browser), "浏览器");
        assert_eq!(match_span("打开网址", "cursor.com"), None);

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

        let web = RowDetail::Web {
            url: "https://example.com/a".to_owned(),
        };
        let open_web = row_action(&web, EnterChord::Plain).unwrap();
        assert_eq!(
            open_web,
            RowAction::Shell(LaunchTarget::Url {
                url: "https://example.com/a".to_owned()
            })
        );
        assert!(open_web.counts_as_open());
        assert_eq!(row_action(&web, EnterChord::Ctrl), None);
        assert_eq!(row_action(&web, EnterChord::CtrlShift), None);
    }

    #[test]
    fn open_folder_is_not_an_open() {
        assert!(!RowAction::OpenFolder(PathBuf::from("C:\\")).counts_as_open());
        assert!(!RowAction::HideApp { key: "path".into() }.counts_as_open());
        assert!(
            !RowAction::CleanStartMenu {
                path: PathBuf::from(
                    "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Demo.lnk"
                ),
            }
            .counts_as_open()
        );
        assert!(!RowAction::UninstallApp.counts_as_open());
        assert!(RowAction::Shell(exe()).counts_as_open());
        assert!(RowAction::Elevated(exe()).counts_as_open());
        assert!(RowAction::RunAsUser(exe()).counts_as_open());
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

    #[test]
    fn file_merge_keeps_the_row_and_a_new_path_group_selects_the_first() {
        let web = SearchRow {
            group: SearchGroup::WebSearch,
            kind: None,
            usage: UsageKey::WebSearch,
            label: "搜索".to_owned(),
            location: String::new(),
            icon: None,
            detail: RowDetail::WebSearch {
                url: "https://www.google.com/search?q=x".to_owned(),
            },
        };
        let file = row("indexed");
        let with_file = vec![file, web.clone()];
        assert_eq!(
            selection_after_refresh(Some(&UsageKey::WebSearch), true, &with_file, false),
            1,
            "文件结果接在后面时，原来选中的网页搜索留在新位置"
        );
        let with_path = vec![row("a-dir"), web];
        assert_eq!(
            selection_after_refresh(Some(&UsageKey::WebSearch), true, &with_path, true),
            0,
            "路径这一组新接到最前面时选中第一条"
        );
    }

    #[test]
    fn logical_pointer_rounds_to_physical_pixels() {
        assert_eq!(
            PointerPos::from_logical(40.0, 80.0, 1.5),
            Some(PointerPos::from_physical(60, 120))
        );
        assert_eq!(
            PointerPos::from_logical(10.4, 20.5, 1.0),
            Some(PointerPos::from_physical(10, 21))
        );
        assert_eq!(PointerPos::from_logical(1.0, 1.0, 0.0), None);
        assert_eq!(PointerPos::from_logical(1.0, 1.0, f32::NAN), None);
        assert_eq!(PointerPos::from_logical(f32::NAN, 1.0, 1.0), None);
    }

    #[test]
    fn stationary_pointer_does_not_change_selection_until_it_moves() {
        let rows = vec![row("a"), row("b"), row("c"), row("d")];
        let cursor = PointerPos::from_physical(40, 160);
        let mut arm = HoverArm::new();
        arm.reset(Some(cursor));

        let mut selected = reselect(None, false, &rows);
        assert_eq!(selected, 0, "打开时选中第一条");
        // 采样点和界面坐标差几个物理像素，仍是静止指针。
        let reported = PointerPos::from_physical(44, 164);
        if let Some(index) = hover_row(&mut arm, reported, 2, rows.len()) {
            selected = index;
        }
        assert_eq!(selected, 0, "指针停在原处不改选中");
        assert_eq!(
            hover_row(&mut arm, PointerPos::from_physical(45, 165), 2, rows.len()),
            None,
            "对齐后再偏 1 像素不算移动"
        );

        let moved = PointerPos::from_physical(44, 166);
        selected = hover_row(&mut arm, moved, 2, rows.len()).unwrap_or(selected);
        assert_eq!(selected, 2, "指针移动后选中悬停的那一行");
        assert_eq!(
            hover_row(&mut arm, moved, 3, rows.len()),
            None,
            "停住之后不再改选中"
        );

        // 结果刷新：锚点换成现在的位置。同一段输入仍保留已选中的那一条，静止指针不把它改掉。
        let key = UsageKey::File("c".to_owned());
        arm.reset(Some(moved));
        selected = reselect(Some(&key), true, &rows);
        assert_eq!(selected, 2);
        if let Some(index) = hover_row(&mut arm, moved, 0, rows.len()) {
            selected = index;
        }
        assert_eq!(selected, 2, "结果在静止指针下刷新不改选中");

        // 输入变了，回到第一条。指针还停着，也不改。
        arm.reset(Some(moved));
        selected = reselect(Some(&key), false, &rows);
        assert_eq!(selected, 0);
        if let Some(index) = hover_row(&mut arm, moved, 2, rows.len()) {
            selected = index;
        }
        assert_eq!(selected, 0);
        selected = hover_row(&mut arm, PointerPos::from_physical(90, 162), 1, rows.len())
            .unwrap_or(selected);
        assert_eq!(selected, 1, "刷新后再移动，才选中新的那一行");
    }

    #[test]
    fn hover_without_a_sampled_cursor_anchors_on_the_first_event() {
        let mut arm = HoverArm::new();
        arm.reset(None);
        assert_eq!(
            hover_row(&mut arm, PointerPos::from_physical(10, 10), 1, 3),
            None
        );
        assert_eq!(
            hover_row(&mut arm, PointerPos::from_physical(10, 40), 1, 3),
            Some(1)
        );
        assert_eq!(
            hover_row(&mut arm, PointerPos::from_physical(80, 40), 9, 3),
            None,
            "越界行号不选"
        );
    }

    #[test]
    fn hover_far_from_the_sample_selects_immediately() {
        let mut arm = HoverArm::new();
        arm.reset(Some(PointerPos::from_physical(0, 0)));
        assert_eq!(
            hover_row(&mut arm, PointerPos::from_physical(0, 40), 2, 4),
            Some(2)
        );
    }

    #[test]
    fn result_menus_follow_the_row_kind() {
        let app_detail = app(exe());
        let RowDetail::App(hit) = &app_detail else {
            panic!("app");
        };
        let menu = result_menu(&app_detail).expect("app menu");
        assert_eq!(
            menu.iter()
                .map(|item| (item.label, item.hint, item.enabled))
                .collect::<Vec<_>>(),
            app_menu(&hit.entry)
                .iter()
                .map(|item| (item.label, item.hint, item.enabled))
                .collect::<Vec<_>>()
        );

        let file = result_menu(&RowDetail::File {
            path: "C:/Docs/a.txt".into(),
        })
        .expect("file menu");
        assert_eq!(
            file.iter()
                .map(|item| (item.action, item.label, item.hint, item.enabled))
                .collect::<Vec<_>>(),
            vec![
                (
                    ResultMenuAction::OpenContainingFolder,
                    "打开所在文件夹",
                    "Alt+1",
                    true
                ),
                (ResultMenuAction::CopyPath, "复制路径", "Alt+2", true),
                (ResultMenuAction::CopyFile, "复制文件", "Alt+3", true),
                (ResultMenuAction::AddToShelf, "加入收纳…", "Alt+4", true),
            ]
        );
        let root = result_menu(&RowDetail::Folder {
            path: "notes.txt".into(),
        })
        .expect("folder menu");
        assert!(!root[0].enabled, "没有上一级时打开所在文件夹不可用");
        assert!(root[1].enabled && root[2].enabled && root[3].enabled);

        let note = result_menu(&RowDetail::Note { id: "n1".into() }).expect("note");
        assert_eq!(note.len(), 1);
        assert_eq!(
            (note[0].action, note[0].label, note[0].hint, note[0].enabled),
            (ResultMenuAction::FloatNote, "悬浮", "Alt+1", true)
        );
        let web = result_menu(&RowDetail::Web {
            url: "https://example.com".into(),
        })
        .expect("web");
        assert_eq!(
            (web[0].action, web[0].label, web[0].hint),
            (ResultMenuAction::CopyUrl, "复制网址", "Alt+1")
        );
        assert!(
            result_menu(&RowDetail::Todo {
                list_id: "l".into(),
                item_id: "i".into(),
            })
            .is_none()
        );
        assert!(result_menu(&RowDetail::PathTimeout).is_none());
    }

    #[test]
    fn shelf_pick_lists_groups_and_offers_a_default_when_empty() {
        assert_eq!(
            shelf_pick_menu(&[]),
            vec![ShelfPickItem {
                group_id: None,
                label: "收纳".into(),
                hint: "Alt+1",
            }]
        );
        let groups = [("a", "工作"), ("b", "个人")];
        let picks = shelf_pick_menu(&groups);
        assert_eq!(picks[0].group_id.as_deref(), Some("a"));
        assert_eq!(picks[0].label, "工作");
        assert_eq!(picks[0].hint, "Alt+1");
        assert_eq!(picks[1].hint, "Alt+2");
        assert_eq!(menu_index(1, picks.len()), Some(0));
        assert_eq!(menu_index(2, picks.len()), Some(1));
        assert_eq!(menu_index(3, picks.len()), None);
        assert_eq!(menu_index(4, 4), Some(3));
        assert_eq!(menu_index(5, 4), None);
    }

    #[test]
    fn caret_at_end_uses_utf8_bytes() {
        assert!(caret_at_end("ab", 2));
        assert!(!caret_at_end("ab", 1));
        assert!(caret_at_end("中", 3));
        assert!(!caret_at_end("中", 1));
        assert!(caret_at_end("", 0));
        assert!(!caret_at_end("ab", -1));
    }
}
