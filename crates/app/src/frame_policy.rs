//! 悬浮窗在系统把 DWM 样式清掉之后要不要重设，以及收起时要不要拦住隐藏。
//!
//! 不调用窗口 API。Win32 消息在 `float_frame` 里翻译成这里的 [`FrameNotice`]。

/// 壳层或系统刚对这个悬浮窗做的一件事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameNotice {
    /// `WM_SIZE`，窗口不是最小化。
    SizeRestored,
    /// `WM_SIZE`，`SIZE_MINIMIZED`。
    SizeMinimized,
    /// `WM_SHOWWINDOW`，正在显示。
    Shown,
    /// `WM_SHOWWINDOW`，正在隐藏。
    Hidden,
    /// 主题、组合或系统设置变了。
    ThemeOrComposition,
    /// `WM_DPICHANGED`。
    DpiChanged,
    /// `WM_ACTIVATE`。
    Activated,
    /// 拖动或改大小的模态循环结束。
    SizeMoveEnded,
    /// `WM_SYSCOMMAND` / `SC_MINIMIZE`。
    SysMinimize,
    /// `WM_WINDOWPOSCHANGING`。`hide` 是 `SWP_HIDEWINDOW`。
    /// `iconic` 是建议位置落在最小化用的屏幕外坐标，并且这次允许移动。
    PosChanging { hide: bool, iconic: bool },
    /// 前台钩子里刚把窗口重新设成 `HWND_TOPMOST`。
    TopmostReasserted,
}

/// 收到 [`FrameNotice`] 之后外壳要做的事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameAction {
    /// 立刻按当前是收起还是展开重设 DWM。
    pub reapply: bool,
    /// 再投递一条窗口消息补设一次。系统经常在当前消息返回之后才把圆角和背景改回去。
    pub reapply_again: bool,
    /// 不要让这次消息把窗口隐藏或最小化。
    pub block_hide: bool,
    /// 窗口已经缩成图标时，用不激活的方式还原。
    pub restore: bool,
    /// 收起时带 `WS_EX_TOOLWINDOW`，一键回到桌面的枚举会跳过它。展开后去掉。
    pub tool_window: bool,
}

/// 外壳最小化窗口时使用的坐标。正常的虚拟屏幕不会到这里。
pub(crate) const ICONIC_ORIGIN: i32 = -32_000;

#[must_use]
pub(crate) fn is_iconic_origin(x: i32, y: i32) -> bool {
    x <= ICONIC_ORIGIN || y <= ICONIC_ORIGIN
}

#[must_use]
pub(crate) fn frame_action(collapsed: bool, notice: FrameNotice) -> FrameAction {
    let tool_window = collapsed;
    if !collapsed {
        let reapply = matches!(
            notice,
            FrameNotice::SizeRestored
                | FrameNotice::Shown
                | FrameNotice::ThemeOrComposition
                | FrameNotice::DpiChanged
                | FrameNotice::Activated
                | FrameNotice::SizeMoveEnded
                | FrameNotice::TopmostReasserted
        );
        let reapply_again = matches!(
            notice,
            FrameNotice::SizeRestored
                | FrameNotice::Shown
                | FrameNotice::ThemeOrComposition
                | FrameNotice::DpiChanged
                | FrameNotice::SizeMoveEnded
                | FrameNotice::TopmostReasserted
        );
        return FrameAction {
            reapply,
            reapply_again,
            block_hide: false,
            restore: false,
            tool_window,
        };
    }
    match notice {
        FrameNotice::SysMinimize => FrameAction {
            reapply: false,
            reapply_again: false,
            block_hide: true,
            restore: false,
            tool_window,
        },
        FrameNotice::PosChanging { hide, iconic } => FrameAction {
            reapply: false,
            reapply_again: hide || iconic,
            block_hide: hide || iconic,
            restore: false,
            tool_window,
        },
        FrameNotice::SizeMinimized => FrameAction {
            reapply: true,
            reapply_again: true,
            block_hide: false,
            restore: true,
            tool_window,
        },
        FrameNotice::Hidden => FrameAction {
            reapply: false,
            reapply_again: true,
            block_hide: false,
            restore: true,
            tool_window,
        },
        FrameNotice::SizeRestored
        | FrameNotice::Shown
        | FrameNotice::ThemeOrComposition
        | FrameNotice::DpiChanged
        | FrameNotice::Activated
        | FrameNotice::SizeMoveEnded
        | FrameNotice::TopmostReasserted => FrameAction {
            reapply: true,
            reapply_again: !matches!(notice, FrameNotice::Activated),
            block_hide: false,
            restore: false,
            tool_window,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collapsed(notice: FrameNotice) -> FrameAction {
        frame_action(true, notice)
    }

    fn expanded(notice: FrameNotice) -> FrameAction {
        frame_action(false, notice)
    }

    #[test]
    fn iconic_origin_is_only_the_minimized_coordinate() {
        assert!(is_iconic_origin(ICONIC_ORIGIN, 0));
        assert!(is_iconic_origin(0, ICONIC_ORIGIN - 1));
        assert!(!is_iconic_origin(-1920, 0));
        assert!(!is_iconic_origin(0, 0));
    }

    #[test]
    fn collapsed_window_reapplies_after_restore_show_theme_dpi_and_topmost() {
        for notice in [
            FrameNotice::SizeRestored,
            FrameNotice::Shown,
            FrameNotice::ThemeOrComposition,
            FrameNotice::DpiChanged,
            FrameNotice::SizeMoveEnded,
            FrameNotice::TopmostReasserted,
        ] {
            let action = collapsed(notice);
            assert!(action.reapply, "{notice:?}");
            assert!(action.reapply_again, "{notice:?}");
            assert!(action.tool_window, "{notice:?}");
            assert!(!action.block_hide, "{notice:?}");
            assert!(!action.restore, "{notice:?}");
        }
        let activated = collapsed(FrameNotice::Activated);
        assert!(activated.reapply);
        assert!(!activated.reapply_again);
        assert!(activated.tool_window);
    }

    #[test]
    fn collapsed_window_blocks_show_desktop_hide_and_restores_if_already_minimized() {
        let minimize = collapsed(FrameNotice::SysMinimize);
        assert!(minimize.block_hide);
        assert!(!minimize.reapply);
        assert!(minimize.tool_window);

        let hide = collapsed(FrameNotice::PosChanging {
            hide: true,
            iconic: false,
        });
        assert!(hide.block_hide);
        assert!(hide.reapply_again);

        let iconic = collapsed(FrameNotice::PosChanging {
            hide: false,
            iconic: true,
        });
        assert!(iconic.block_hide);

        let drag = collapsed(FrameNotice::PosChanging {
            hide: false,
            iconic: false,
        });
        assert!(!drag.block_hide);
        assert!(!drag.reapply_again);

        let minimized = collapsed(FrameNotice::SizeMinimized);
        assert!(minimized.restore);
        assert!(minimized.reapply);
        assert!(minimized.reapply_again);

        let hidden = collapsed(FrameNotice::Hidden);
        assert!(hidden.restore);
        assert!(hidden.reapply_again);
        assert!(!hidden.reapply);
    }

    #[test]
    fn expanded_window_can_be_minimized_and_still_reapplies_its_own_frame() {
        let minimize = expanded(FrameNotice::SysMinimize);
        assert!(!minimize.block_hide);
        assert!(!minimize.tool_window);
        assert!(!minimize.restore);

        let hidden = expanded(FrameNotice::PosChanging {
            hide: true,
            iconic: true,
        });
        assert!(!hidden.block_hide);
        assert!(!hidden.tool_window);

        assert!(!expanded(FrameNotice::SizeMinimized).restore);
        assert!(!expanded(FrameNotice::Hidden).reapply);

        let theme = expanded(FrameNotice::ThemeOrComposition);
        assert!(theme.reapply);
        assert!(theme.reapply_again);
        assert!(!theme.tool_window);

        let topmost = expanded(FrameNotice::TopmostReasserted);
        assert!(topmost.reapply);
        assert!(topmost.reapply_again);
    }
}
