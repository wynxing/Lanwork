use std::time::{Duration, Instant};

/// 产品规格里的热角。默认是右上，也可以关掉。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft = 1,
    TopRight = 2,
    BottomLeft = 3,
    BottomRight = 4,
    Off = 0,
}

impl Corner {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::TopLeft,
            2 => Self::TopRight,
            3 => Self::BottomLeft,
            4 => Self::BottomRight,
            _ => Self::Off,
        }
    }
}

/// 光标落在哪块屏幕的角上。产品规格没有写，两种都是待定参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorPick {
    /// 只认主显示器。
    Primary,
    /// 认包含该点的显示器。点落在两块屏幕之间的空隙时，退回主显示器。
    /// 空隙怎么算，规格没有写。
    Cursor,
}

/// Win32 显示器矩形：右和下是开区间。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub rect: PxRect,
    pub primary: bool,
}

/// 角内区域的边长是 `corner_px` 个物理像素。0 表示没有区域。
/// 边长规格没有写，调用方传入的值是待定参数。
pub fn in_corner(rect: PxRect, corner: Corner, corner_px: u32, x: i32, y: i32) -> bool {
    if corner == Corner::Off || corner_px == 0 || rect.is_empty() {
        return false;
    }
    let px = i32::try_from(corner_px).unwrap_or(i32::MAX);
    let horizontal = match corner {
        Corner::TopLeft | Corner::BottomLeft => x >= rect.left && x < rect.left.saturating_add(px),
        Corner::TopRight | Corner::BottomRight => {
            x >= rect.right.saturating_sub(px) && x < rect.right
        }
        Corner::Off => false,
    };
    let vertical = match corner {
        Corner::TopLeft | Corner::TopRight => y >= rect.top && y < rect.top.saturating_add(px),
        Corner::BottomLeft | Corner::BottomRight => {
            y >= rect.bottom.saturating_sub(px) && y < rect.bottom
        }
        Corner::Off => false,
    };
    horizontal && vertical
}

/// 角区域靠屏幕内侧的那一个像素。
pub fn corner_point(rect: PxRect, corner: Corner) -> Option<(i32, i32)> {
    if corner == Corner::Off || rect.is_empty() {
        return None;
    }
    Some(match corner {
        Corner::TopLeft => (rect.left, rect.top),
        Corner::TopRight => (rect.right.saturating_sub(1), rect.top),
        Corner::BottomLeft => (rect.left, rect.bottom.saturating_sub(1)),
        Corner::BottomRight => (rect.right.saturating_sub(1), rect.bottom.saturating_sub(1)),
        Corner::Off => return None,
    })
}

/// 角区域中心。实机移动光标时用它，避免绝对坐标取整落到区域外。
pub fn aim_point(rect: PxRect, corner: Corner, corner_px: u32) -> Option<(i32, i32)> {
    if corner == Corner::Off || corner_px == 0 || rect.is_empty() {
        return None;
    }
    let px = i32::try_from(corner_px).unwrap_or(1).max(1);
    let inset = (px - 1) / 2;
    Some(match corner {
        Corner::TopLeft => (
            rect.left.saturating_add(inset),
            rect.top.saturating_add(inset),
        ),
        Corner::TopRight => (
            rect.right.saturating_sub(1).saturating_sub(inset),
            rect.top.saturating_add(inset),
        ),
        Corner::BottomLeft => (
            rect.left.saturating_add(inset),
            rect.bottom.saturating_sub(1).saturating_sub(inset),
        ),
        Corner::BottomRight => (
            rect.right.saturating_sub(1).saturating_sub(inset),
            rect.bottom.saturating_sub(1).saturating_sub(inset),
        ),
        Corner::Off => return None,
    })
}

/// 显示器中心。中心若仍落在角区域内，则改到角区域的右下方外侧。
pub fn outside_point(rect: PxRect, corner: Corner, corner_px: u32) -> Option<(i32, i32)> {
    if rect.is_empty() {
        return None;
    }
    let center = (
        rect.left.saturating_add(rect.right) / 2,
        rect.top.saturating_add(rect.bottom) / 2,
    );
    if !in_corner(rect, corner, corner_px, center.0, center.1) {
        return Some(center);
    }
    let px = i32::try_from(corner_px).unwrap_or(i32::MAX);
    let x = rect.left.saturating_add(px).saturating_add(1);
    let y = rect.top.saturating_add(px).saturating_add(1);
    if rect.contains(x, y) && !in_corner(rect, corner, corner_px, x, y) {
        Some((x, y))
    } else {
        None
    }
}

pub fn select_monitor(monitors: &[Monitor], pick: MonitorPick, x: i32, y: i32) -> Option<Monitor> {
    let primary = monitors
        .iter()
        .find(|monitor| monitor.primary)
        .or_else(|| monitors.first())
        .copied();
    match pick {
        MonitorPick::Primary => primary,
        MonitorPick::Cursor => monitors
            .iter()
            .find(|monitor| monitor.rect.contains(x, y))
            .copied()
            .or(primary),
    }
}

pub fn point_in_hot_corner(
    monitors: &[Monitor],
    pick: MonitorPick,
    corner: Corner,
    corner_px: u32,
    x: i32,
    y: i32,
) -> bool {
    let Some(monitor) = select_monitor(monitors, pick, x, y) else {
        return false;
    };
    in_corner(monitor.rect, corner, corner_px, x, y)
}

/// 停留计时。
///
/// 从「角外进入角内」开始算。达到 `dwell` 触发一次，之后停在角内不再触发。
/// 离开后重新武装。进程开始时视为在角外：第一拍如果已经在角内，会开始计时。
/// 启动时光标已经在角内要不要触发，规格没有写。
#[derive(Clone, Debug)]
pub struct Dwell {
    dwell: Duration,
    armed: bool,
    since: Option<Instant>,
}

impl Dwell {
    pub fn new(dwell: Duration) -> Self {
        Self {
            dwell,
            armed: true,
            since: None,
        }
    }

    /// 返回这一拍是否触发。
    pub fn sample(&mut self, inside: bool, now: Instant) -> bool {
        if !inside {
            self.since = None;
            self.armed = true;
            return false;
        }
        if !self.armed {
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since) >= self.dwell {
            self.armed = false;
            self.since = None;
            true
        } else {
            false
        }
    }

    pub fn is_waiting(&self) -> bool {
        self.armed && self.since.is_some()
    }

    /// 距离触发还要多久。没在等待时返回 None。
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        let since = self.since?;
        if !self.armed {
            return None;
        }
        let elapsed = now.saturating_duration_since(since);
        Some(self.dwell.saturating_sub(elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> PxRect {
        PxRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    }

    #[test]
    fn stay_350_triggers_once_then_stays_quiet() {
        let mut dwell = Dwell::new(Duration::from_millis(350));
        let t0 = Instant::now();
        assert!(!dwell.sample(true, t0));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(349)));
        assert!(dwell.sample(true, t0 + Duration::from_millis(350)));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(700)));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(5_000)));
    }

    #[test]
    fn leave_at_340_does_not_trigger() {
        let mut dwell = Dwell::new(Duration::from_millis(350));
        let t0 = Instant::now();
        assert!(!dwell.sample(true, t0));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(340)));
        assert!(!dwell.sample(false, t0 + Duration::from_millis(341)));
        assert!(!dwell.sample(false, t0 + Duration::from_millis(1_000)));
    }

    #[test]
    fn leave_then_reenter_triggers_a_second_time() {
        let mut dwell = Dwell::new(Duration::from_millis(350));
        let t0 = Instant::now();
        assert!(!dwell.sample(true, t0));
        assert!(dwell.sample(true, t0 + Duration::from_millis(350)));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(400)));
        assert!(!dwell.sample(false, t0 + Duration::from_millis(500)));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(600)));
        assert!(dwell.sample(true, t0 + Duration::from_millis(600 + 350)));
    }

    #[test]
    fn a_gap_inside_the_corner_restarts_the_dwell() {
        let mut dwell = Dwell::new(Duration::from_millis(350));
        let t0 = Instant::now();
        assert!(!dwell.sample(true, t0));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(300)));
        assert!(!dwell.sample(false, t0 + Duration::from_millis(301)));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(302)));
        assert!(!dwell.sample(true, t0 + Duration::from_millis(302 + 349)));
        assert!(dwell.sample(true, t0 + Duration::from_millis(302 + 350)));
    }

    #[test]
    fn off_corner_has_no_inside_pixels() {
        let rect = screen();
        assert!(!in_corner(rect, Corner::Off, 32, 1919, 0));
        assert!(!in_corner(rect, Corner::TopRight, 0, 1919, 0));
    }

    #[test]
    fn one_pixel_corners_match_the_exclusive_rect() {
        let rect = screen();
        assert!(in_corner(rect, Corner::TopLeft, 1, 0, 0));
        assert!(!in_corner(rect, Corner::TopLeft, 1, 1, 0));
        assert!(in_corner(rect, Corner::TopRight, 1, 1919, 0));
        assert!(!in_corner(rect, Corner::TopRight, 1, 1918, 0));
        assert!(in_corner(rect, Corner::BottomLeft, 1, 0, 1079));
        assert!(!in_corner(rect, Corner::BottomLeft, 1, 0, 1078));
        assert!(in_corner(rect, Corner::BottomRight, 1, 1919, 1079));
        assert!(!in_corner(rect, Corner::BottomRight, 1, 1919, 1080));
        assert_eq!(corner_point(rect, Corner::TopRight), Some((1919, 0)));
    }

    #[test]
    fn corner_span_uses_the_given_pixel_size() {
        let rect = screen();
        assert!(in_corner(rect, Corner::TopRight, 2, 1918, 0));
        assert!(in_corner(rect, Corner::TopRight, 2, 1919, 1));
        assert!(!in_corner(rect, Corner::TopRight, 2, 1917, 0));
        assert!(!in_corner(rect, Corner::TopRight, 2, 1919, 2));
    }

    #[test]
    fn negative_monitor_origin_still_has_a_top_right_pixel() {
        let rect = PxRect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(in_corner(rect, Corner::TopRight, 1, -1, 0));
        assert!(!in_corner(rect, Corner::TopRight, 1, -2, 0));
        assert_eq!(corner_point(rect, Corner::TopRight), Some((-1, 0)));
    }

    #[test]
    fn primary_pick_ignores_the_monitor_under_the_point() {
        let monitors = two_monitors();
        assert!(point_in_hot_corner(
            &monitors,
            MonitorPick::Primary,
            Corner::TopRight,
            1,
            99,
            0
        ));
        assert!(!point_in_hot_corner(
            &monitors,
            MonitorPick::Primary,
            Corner::TopRight,
            1,
            -1,
            0
        ));
    }

    #[test]
    fn cursor_pick_uses_the_monitor_that_contains_the_point() {
        let monitors = two_monitors();
        assert!(point_in_hot_corner(
            &monitors,
            MonitorPick::Cursor,
            Corner::TopRight,
            1,
            -1,
            0
        ));
        assert!(point_in_hot_corner(
            &monitors,
            MonitorPick::Cursor,
            Corner::TopRight,
            1,
            99,
            0
        ));
        assert!(!point_in_hot_corner(
            &monitors,
            MonitorPick::Cursor,
            Corner::TopRight,
            1,
            -1,
            1
        ));
    }

    #[test]
    fn cursor_pick_falls_back_to_primary_when_the_point_is_in_a_gap() {
        let monitors = [
            Monitor {
                rect: PxRect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                primary: true,
            },
            Monitor {
                rect: PxRect {
                    left: 200,
                    top: 0,
                    right: 300,
                    bottom: 100,
                },
                primary: false,
            },
        ];
        let chosen = select_monitor(&monitors, MonitorPick::Cursor, 150, 10).unwrap();
        assert!(chosen.primary);
        assert!(!point_in_hot_corner(
            &monitors,
            MonitorPick::Cursor,
            Corner::TopRight,
            1,
            150,
            0
        ));
    }

    #[test]
    fn aim_point_is_inside_the_corner_region() {
        let rect = screen();
        for corner in [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ] {
            let (x, y) = aim_point(rect, corner, 32).unwrap();
            assert!(in_corner(rect, corner, 32, x, y), "{corner:?} {x},{y}");
        }
    }

    #[test]
    fn outside_point_is_not_inside_the_corner() {
        let rect = screen();
        let (x, y) = outside_point(rect, Corner::TopRight, 32).unwrap();
        assert!(!in_corner(rect, Corner::TopRight, 32, x, y));
        assert!(rect.contains(x, y));
    }

    #[test]
    fn remaining_shrinks_until_the_dwell() {
        let mut dwell = Dwell::new(Duration::from_millis(350));
        let t0 = Instant::now();
        assert!(dwell.remaining(t0).is_none());
        assert!(!dwell.sample(true, t0));
        assert_eq!(dwell.remaining(t0), Some(Duration::from_millis(350)));
        assert_eq!(
            dwell.remaining(t0 + Duration::from_millis(100)),
            Some(Duration::from_millis(250))
        );
    }

    fn two_monitors() -> [Monitor; 2] {
        [
            Monitor {
                rect: PxRect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                primary: true,
            },
            Monitor {
                rect: PxRect {
                    left: -100,
                    top: 0,
                    right: 0,
                    bottom: 100,
                },
                primary: false,
            },
        ]
    }
}
