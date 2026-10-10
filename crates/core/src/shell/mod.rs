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

/// 退出是否已经被请求。托盘「退出」设置它；保存问题解决或退出完成时清掉。
#[derive(Debug, Default)]
pub struct QuitGate {
    requested: std::sync::atomic::AtomicBool,
}

impl QuitGate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            requested: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn request(&self) {
        self.requested
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn clear(&self) {
        self.requested
            .store(false, std::sync::atomic::Ordering::Release);
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.requested.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// 便签窗口里的保存问题解决之后接着退出。
///
/// 这个调用来自窗口自己的回调，那时面板和悬浮窗的表还被借用着，当场再去保存会借用失败而不退出。
/// 所以只交给 `defer`，在下一轮事件循环里才调用 `attempt`；没有请求退出时什么也不做。
pub fn resume_quit_later(
    gate: &'static QuitGate,
    defer: impl FnOnce(Box<dyn FnOnce() + 'static>),
    attempt: impl FnOnce() + 'static,
) {
    if !gate.is_requested() {
        return;
    }
    defer(Box::new(move || {
        if gate.is_requested() {
            attempt();
        }
    }));
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;

    #[test]
    fn resuming_quit_waits_until_the_window_tables_are_released() {
        static GATE: QuitGate = QuitGate::new();
        GATE.request();
        let tables = Rc::new(RefCell::new(0_u8));
        let outcome = Rc::new(Cell::new(None));
        let queue: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::new(Vec::new());

        let held = tables.borrow_mut();
        let probe = Rc::clone(&tables);
        let result = Rc::clone(&outcome);
        resume_quit_later(
            &GATE,
            |task| queue.borrow_mut().push(task),
            move || result.set(Some(probe.try_borrow_mut().is_ok())),
        );
        assert_eq!(outcome.get(), None, "借用期间不能当场尝试退出");
        assert_eq!(queue.borrow().len(), 1);

        drop(held);
        let task = queue.borrow_mut().pop().unwrap();
        task();
        assert_eq!(outcome.get(), Some(true));
    }

    #[test]
    fn resuming_quit_does_nothing_when_quit_was_not_requested() {
        static GATE: QuitGate = QuitGate::new();
        let queued = Cell::new(0);
        resume_quit_later(&GATE, |_| queued.set(queued.get() + 1), || {});
        assert_eq!(queued.get(), 0);

        GATE.request();
        let ran = Rc::new(Cell::new(false));
        let flag = Rc::clone(&ran);
        let task: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
        resume_quit_later(
            &GATE,
            |next| *task.borrow_mut() = Some(next),
            move || flag.set(true),
        );
        GATE.clear();
        task.borrow_mut().take().unwrap()();
        assert!(!ran.get(), "排队之后退出被取消就不再尝试");
    }

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
