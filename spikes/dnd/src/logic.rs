//! 与 Win32 无关的拖放判定，供单元测试和 OLE 回调共用。

/// `DROPEFFECT_NONE`
pub const EFFECT_NONE: u32 = 0;
/// `DROPEFFECT_COPY`
pub const EFFECT_COPY: u32 = 1;
/// `DROPEFFECT_MOVE`
pub const EFFECT_MOVE: u32 = 2;
/// `DROPEFFECT_LINK`
pub const EFFECT_LINK: u32 = 4;

/// 拖出允许的效果。不含移动。
pub const OUTGOING_EFFECTS: u32 = EFFECT_COPY | EFFECT_LINK;

/// 区域内优先复制，其次快捷方式。源只提供移动时拒绝。
pub fn effect_for_drop(inside: bool, offered: u32) -> u32 {
    if !inside {
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

pub fn passed_drag_threshold(dx_px: f32, dy_px: f32, cx: i32, cy: i32) -> bool {
    dx_px > cx as f32 || dy_px > cy as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragContinue {
    Continue,
    Drop,
    Cancel,
}

pub fn query_continue(escape: bool, left_button_down: bool, force_cancel: bool) -> DragContinue {
    if force_cancel || escape {
        DragContinue::Cancel
    } else if !left_button_down {
        DragContinue::Drop
    } else {
        DragContinue::Continue
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Zone {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub scale: f32,
}

pub fn point_in_zone(zone: Zone, client_x_px: i32, client_y_px: i32) -> bool {
    if zone.w <= 0.0 || zone.h <= 0.0 {
        return false;
    }
    let scale = if zone.scale == 0.0 { 1.0 } else { zone.scale };
    let x = client_x_px as f32 / scale;
    let y = client_y_px as f32 / scale;
    x >= zone.x && y >= zone.y && x < zone.x + zone.w && y < zone.y + zone.h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_effects_exclude_move() {
        assert_eq!(OUTGOING_EFFECTS & EFFECT_MOVE, 0);
    }

    #[test]
    fn drop_effect_prefers_copy_and_never_move() {
        let all = EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK;
        assert_eq!(effect_for_drop(true, all), EFFECT_COPY);
        assert_eq!(
            effect_for_drop(true, EFFECT_MOVE | EFFECT_LINK),
            EFFECT_LINK
        );
        assert_eq!(effect_for_drop(true, EFFECT_MOVE), EFFECT_NONE);
        assert_eq!(effect_for_drop(false, all), EFFECT_NONE);
        assert_eq!(effect_for_drop(true, EFFECT_NONE), EFFECT_NONE);
    }

    #[test]
    fn threshold_is_strictly_greater_than_system_metric() {
        assert!(!passed_drag_threshold(4.0, 0.0, 4, 4));
        assert!(passed_drag_threshold(4.1, 0.0, 4, 4));
        assert!(passed_drag_threshold(0.0, 5.0, 4, 4));
    }

    #[test]
    fn escape_cancels_even_while_button_is_down() {
        assert_eq!(query_continue(true, true, false), DragContinue::Cancel);
        assert_eq!(query_continue(false, false, false), DragContinue::Drop);
        assert_eq!(query_continue(false, true, true), DragContinue::Cancel);
        assert_eq!(query_continue(false, true, false), DragContinue::Continue);
    }

    #[test]
    fn zone_uses_client_pixels_and_scale() {
        let zone = Zone {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
            scale: 2.0,
        };
        assert!(point_in_zone(zone, 20, 40));
        assert!(point_in_zone(zone, 219, 139));
        assert!(!point_in_zone(zone, 220, 40));
        assert!(!point_in_zone(zone, 20, 140));
        assert!(!point_in_zone(Zone::default(), 0, 0));
    }
}
