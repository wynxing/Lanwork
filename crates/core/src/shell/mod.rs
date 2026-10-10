//! 外壳里不依赖窗口 API 的决定。
//!
//! 单实例的互斥量、托盘菜单、热键注册、主题和开机启动的系统调用在 `crates/app`。
//! 这里固定菜单文字、热键修改顺序、图标像素、主题解析，以及退出时是否还有没写盘的便签窗口。

mod bar;
mod bind;
mod icon;
mod startup;
mod theme;

pub use bar::{
    BAR_WIDTH, BASE_DPI, Backdrop, BackdropInput, DARK_SOLID_RGB, EnterChord, EscapeAction,
    GROUP_GAP, LIGHT_SOLID_RGB, LIST_PAD, MIN_BACKDROP_BUILD, NO_RESULTS, ROW_HEIGHT, RowAction,
    SHORTCUT_COUNT, WorkArea, bar_origin, choose_backdrop, escape_action, group_label,
    location_always_shown, match_span, max_results_height, move_selection, path_target,
    pointed_row, reselect, row_action, row_offsets, shortcut_rows, shortcut_target, solid_rgb,
    status_line, text_after_hide, to_logical, to_physical,
};
pub use bind::{
    BindError, HotkeyPort, PANEL_HOTKEY_ID, RegisteredHotkey, SEARCH_HOTKEY_ID, SaveHotkeyError,
    apply_hotkeys, desired_bindings, save_hotkeys,
};
pub use icon::{TRAY_ICON_PX, has_badge_pixels, tray_icon_rgba};
pub use startup::{RunValueAction, quoted_executable, run_value_action};
pub use theme::{
    ResolvedTheme, SystemLight, is_immersive_color_set, resolve_theme, system_light_from_dword,
};

/// 托盘菜单的一项。顺序与产品规格「面板」里列出的入口一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayItem {
    pub title: &'static str,
    pub command: ShellCommand,
}

/// 外壳收到的命令。搜索条和面板窗口的可见内容不在这里绘制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommand {
    OpenPanel,
    OpenSettings,
    NewNote,
    RefreshGithub,
    Quit,
    /// 搜索条热键。显示或收起搜索条由搜索条窗口实现。
    SearchHotkey,
    /// 面板热键。打开面板由面板窗口实现。
    PanelHotkey,
    /// 第二个进程已经通知本进程。本进程接着显示什么，产品规格没有写。
    SecondInstance,
}

/// 右键菜单的五项。退出在最后。
pub const TRAY_MENU: [TrayItem; 5] = [
    TrayItem {
        title: "打开面板",
        command: ShellCommand::OpenPanel,
    },
    TrayItem {
        title: "设置",
        command: ShellCommand::OpenSettings,
    },
    TrayItem {
        title: "新建便签",
        command: ShellCommand::NewNote,
    },
    TrayItem {
        title: "刷新 GitHub",
        command: ShellCommand::RefreshGithub,
    },
    TrayItem {
        title: "退出",
        command: ShellCommand::Quit,
    },
];

/// 渲染器尝试顺序。先 FemtoVG，初始化失败再用软件渲染。
pub const RENDERER_ORDER: [&str; 2] = ["femtovg", "software"];

/// 退出决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitDecision {
    Exit,
    /// 还有便签窗口里的修改没有写盘。不结束进程，也不丢弃正文。
    Stay,
}

/// 退出前先按「便签」保存未写入的修改。`unsaved` 是保存之后仍然没有写盘的便签窗口数：
/// 为 0 才结束进程，否则留下，由用户在那些窗口里选择重试或放弃修改。
#[must_use]
pub fn quit_decision(unsaved: usize) -> QuitDecision {
    if unsaved == 0 {
        QuitDecision::Exit
    } else {
        QuitDecision::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_menu_ends_with_quit_and_renderer_order_is_fixed() {
        assert_eq!(TRAY_MENU[4].title, "退出");
        assert_eq!(TRAY_MENU[4].command, ShellCommand::Quit);
        assert_eq!(
            TRAY_MENU.map(|item| item.title),
            ["打开面板", "设置", "新建便签", "刷新 GitHub", "退出"]
        );
        assert_eq!(RENDERER_ORDER, ["femtovg", "software"]);
        assert_eq!(quit_decision(0), QuitDecision::Exit);
        assert_eq!(quit_decision(1), QuitDecision::Stay);
    }
}
