//! 悬浮窗在系统把 DWM 样式清掉之后要不要重设，以及收起时要不要拦住隐藏。
//!
//! 不调用窗口 API。Win32 消息在 `float_frame` 里翻译成这里的 [`FrameNotice`]。
//! 收起后看得见的范围也在这里算：窗口区域只留把手，点击区多出来的一圈另做命中。

use lanwork_core::panel::{HANDLE_LENGTH_LOGICAL, HANDLE_THICKNESS_LOGICAL};
use lanwork_core::shell::to_physical;

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

/// 客户区里把手所占的矩形。原点是客户区左上角，单位是物理像素。
///
/// 宽高按窗口 DPI 把 3×40 逻辑像素换算后再居中，和界面里 `EdgeHandle` 的画法同一套数。
/// 客户区比把手还小时收到客户区以内，避免区域比窗口大。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GripRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[must_use]
pub(crate) fn collapsed_grip(client_width: u32, client_height: u32, dpi: u32) -> GripRect {
    let client_width = client_width.max(1);
    let client_height = client_height.max(1);
    let thick = px_len(HANDLE_THICKNESS_LOGICAL, dpi).min(client_width.min(client_height));
    let long = px_len(HANDLE_LENGTH_LOGICAL, dpi);
    let horizontal = client_width >= client_height;
    let (mut width, mut height) = if horizontal {
        (long.min(client_width), thick.min(client_height))
    } else {
        (thick.min(client_width), long.min(client_height))
    };
    width = width.max(1);
    height = height.max(1);
    GripRect {
        x: i32::try_from((client_width - width) / 2).unwrap_or(0),
        y: i32::try_from((client_height - height) / 2).unwrap_or(0),
        width,
        height,
    }
}

/// 点在点击区里、但不在把手矩形里。这些像素被窗口区域裁掉，不能靠窗口自己收到鼠标。
#[must_use]
pub(crate) fn point_in_padding(
    client_width: u32,
    client_height: u32,
    dpi: u32,
    x: i32,
    y: i32,
) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let Ok(x) = u32::try_from(x) else {
        return false;
    };
    let Ok(y) = u32::try_from(y) else {
        return false;
    };
    if x >= client_width || y >= client_height {
        return false;
    }
    let grip = collapsed_grip(client_width, client_height, dpi);
    let left = u32::try_from(grip.x).unwrap_or(0);
    let top = u32::try_from(grip.y).unwrap_or(0);
    let in_grip = x >= left && y >= top && x - left < grip.width && y - top < grip.height;
    !in_grip
}

fn px_len(logical: u32, dpi: u32) -> u32 {
    u32::try_from(to_physical(logical, dpi).max(1)).unwrap_or(1)
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

    #[test]
    fn collapsed_grip_is_the_centered_handle_and_padding_is_the_rest_of_the_hit_area() {
        let grip = collapsed_grip(48, 9, 96);
        assert_eq!(
            grip,
            GripRect {
                x: 4,
                y: 3,
                width: 40,
                height: 3
            }
        );
        assert!(point_in_padding(48, 9, 96, 0, 4), "长度方向两端是点击区");
        assert!(point_in_padding(48, 9, 96, 24, 0), "厚度方向两侧是点击区");
        assert!(
            !point_in_padding(48, 9, 96, 24, 4),
            "把手正中不是点击区余量"
        );
        assert!(!point_in_padding(48, 9, 96, -1, 4));
        assert!(!point_in_padding(48, 9, 96, 48, 4));

        let side = collapsed_grip(9, 48, 96);
        assert_eq!(
            side,
            GripRect {
                x: 3,
                y: 4,
                width: 3,
                height: 40
            }
        );
        assert!(point_in_padding(9, 48, 96, 0, 24));
        assert!(!point_in_padding(9, 48, 96, 4, 24));

        // 144 DPI：9→14，3→5，48→72，40→60。居中后两侧各余 (14-5)/2 = 4。
        let scaled = collapsed_grip(72, 14, 144);
        assert_eq!(
            scaled,
            GripRect {
                x: 6,
                y: 4,
                width: 60,
                height: 5
            }
        );
        assert!(point_in_padding(72, 14, 144, 6, 0));
        assert!(!point_in_padding(72, 14, 144, 36, 6));
    }
}
