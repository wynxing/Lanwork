//! 收纳页拖入和拖出里不调用 Win32 的判定。OLE 的接口在 `crates/app`，数值与 `DROPEFFECT_*` 一致。
//!
//! 拖放用原生 OLE，原因见架构文档「运行时」和「收纳」的「拖放」。

/// `DROPEFFECT_NONE`
pub const EFFECT_NONE: u32 = 0;
/// `DROPEFFECT_COPY`
pub const EFFECT_COPY: u32 = 1;
/// `DROPEFFECT_MOVE`
pub const EFFECT_MOVE: u32 = 2;
/// `DROPEFFECT_LINK`
pub const EFFECT_LINK: u32 = 4;

/// 拖出允许的效果：复制和创建快捷方式，不含移动。
pub const OUTGOING_EFFECTS: u32 = EFFECT_COPY | EFFECT_LINK;

/// 放下区域，逻辑像素，窗口客户区坐标。`scale` 是物理像素与逻辑像素之比，0 按 1 处理。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DropZone {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl DropZone {
    /// 客户区物理像素坐标是否落在区域内。区域没有大小时一律在外。
    #[must_use]
    pub fn contains(&self, client_x_px: i32, client_y_px: i32) -> bool {
        if self.width <= 0.0 || self.height <= 0.0 {
            return false;
        }
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        #[allow(clippy::cast_precision_loss)]
        let (x, y) = (client_x_px as f32 / scale, client_y_px as f32 / scale);
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// 放下时给 OLE 的效果。只有收纳标签显示（`accepting`）、指针在区域内、数据里有文件时才接受。
/// 接受时优先复制，其次快捷方式；源只提供移动时拒绝。
#[must_use]
pub fn drop_effect(accepting: bool, inside: bool, has_files: bool, offered: u32) -> u32 {
    if !accepting || !inside || !has_files {
        return EFFECT_NONE;
    }
    let offered = offered & OUTGOING_EFFECTS;
    if offered & EFFECT_COPY != 0 {
        EFFECT_COPY
    } else if offered & EFFECT_LINK != 0 {
        EFFECT_LINK
    } else {
        EFFECT_NONE
    }
}

/// 按下之后的位移（物理像素）严格超过系统拖动阈值才开始拖出。
#[must_use]
pub fn passed_drag_threshold(dx_px: f32, dy_px: f32, cx: i32, cy: i32) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let (cx, cy) = (cx as f32, cy as f32);
    dx_px.abs() > cx || dy_px.abs() > cy
}

/// `IDropSource::QueryContinueDrag` 的决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragContinue {
    Continue,
    Drop,
    Cancel,
}

/// Esc 取消；左键松开就放下；否则继续。
#[must_use]
pub fn continue_drag(escape: bool, left_button_down: bool) -> DragContinue {
    if escape {
        DragContinue::Cancel
    } else if !left_button_down {
        DragContinue::Drop
    } else {
        DragContinue::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_effects_never_include_move() {
        assert_eq!(OUTGOING_EFFECTS & EFFECT_MOVE, 0);
        assert_eq!(OUTGOING_EFFECTS, EFFECT_COPY | EFFECT_LINK);
    }

    #[test]
    fn drop_prefers_copy_then_link_and_never_move() {
        let all = EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK;
        assert_eq!(drop_effect(true, true, true, all), EFFECT_COPY);
        assert_eq!(
            drop_effect(true, true, true, EFFECT_MOVE | EFFECT_LINK),
            EFFECT_LINK
        );
        assert_eq!(drop_effect(true, true, true, EFFECT_MOVE), EFFECT_NONE);
        assert_eq!(drop_effect(true, true, true, EFFECT_NONE), EFFECT_NONE);
    }

    #[test]
    fn drop_is_refused_outside_the_shelf_tab_zone_or_without_files() {
        let all = EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK;
        assert_eq!(drop_effect(false, true, true, all), EFFECT_NONE);
        assert_eq!(drop_effect(true, false, true, all), EFFECT_NONE);
        assert_eq!(drop_effect(true, true, false, all), EFFECT_NONE);
    }

    #[test]
    fn threshold_is_strictly_greater_than_the_system_metric_in_either_direction() {
        assert!(!passed_drag_threshold(4.0, 0.0, 4, 4));
        assert!(passed_drag_threshold(4.1, 0.0, 4, 4));
        assert!(passed_drag_threshold(0.0, -5.0, 4, 4));
        assert!(!passed_drag_threshold(-4.0, 4.0, 4, 4));
    }

    #[test]
    fn escape_cancels_even_while_the_button_is_down() {
        assert_eq!(continue_drag(true, true), DragContinue::Cancel);
        assert_eq!(continue_drag(false, false), DragContinue::Drop);
        assert_eq!(continue_drag(false, true), DragContinue::Continue);
    }

    #[test]
    fn zone_uses_physical_pixels_and_the_scale() {
        let zone = DropZone {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            scale: 2.0,
        };
        assert!(zone.contains(20, 40));
        assert!(zone.contains(219, 139));
        assert!(!zone.contains(220, 40));
        assert!(!zone.contains(20, 140));
        assert!(!zone.contains(19, 40));
        assert!(!DropZone::default().contains(0, 0));
        let unscaled = DropZone { scale: 0.0, ..zone };
        assert!(unscaled.contains(10, 20));
        assert!(!unscaled.contains(110, 20));
    }
}
