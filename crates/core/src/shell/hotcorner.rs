//! 热角判定。不读光标，也不调用 Win32。
//!
//! 产品规格：所选角连续停留 350ms 后打开面板；离开该角后才允许再次触发；
//! 停留期间不重复打开；每一块显示器的所选角都可以触发；有全屏应用时不触发；
//! 可以关掉。角的像素边长、拖动时是否触发、跨屏全屏是否抑制，规格没有写，见架构文档。

use std::time::{Duration, Instant};

use crate::config::HotCorner;

/// 角区域的边长，物理像素。
///
/// 产品规格没有写这个数。32 与热角 spike 自测用的参数相同，不是产品规则，待定。
/// 不按 DPI 放大。
pub const CORNER_EDGE_PX: u32 = 32;

/// 进入角内后连续停留这么久才打开。产品规格写的是 350ms。
pub const DWELL: Duration = Duration::from_millis(350);

/// 桌面和外壳的窗口类名。这些窗口即使盖住显示器也不算全屏应用。
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "#32769",
];

/// Win32 矩形：右和下是开区间，物理像素。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    #[must_use]
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    fn intersects(self, other: Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    fn covers(self, monitor: Self) -> bool {
        self.left <= monitor.left
            && self.top <= monitor.top
            && self.right >= monitor.right
            && self.bottom >= monitor.bottom
    }
}

/// 前台窗口。类名是 `GetClassNameW` 的原文，矩形是 `GetWindowRect`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Foreground<'a> {
    pub class_name: &'a str,
    pub rect: Option<PxRect>,
}

/// 一轮轮询读到的光标、显示器和前台窗口。
///
/// `foreground` 只在光标已经落在所选角里时才需要。不在角里时可以是 `None`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation<'a> {
    pub cursor: (i32, i32),
    pub monitors: &'a [PxRect],
    pub foreground: Option<Foreground<'a>>,
    pub exclusive_d3d: bool,
}

/// 光标是否落在某一块显示器的所选角里。关闭时恒为假。
#[must_use]
pub fn hits_corner(monitors: &[PxRect], corner: HotCorner, corner_px: u32, x: i32, y: i32) -> bool {
    monitors
        .iter()
        .any(|rect| in_corner(*rect, corner, corner_px, x, y))
}

/// `(x, y)` 是否在这块显示器的所选角里。
///
/// 区域是靠角的 `corner_px`×`corner_px` 物理像素。右、下沿与 `rect` 一样是开区间。
#[must_use]
pub fn in_corner(rect: PxRect, corner: HotCorner, corner_px: u32, x: i32, y: i32) -> bool {
    if corner == HotCorner::Off || corner_px == 0 || rect.is_empty() {
        return false;
    }
    let px = i32::try_from(corner_px).unwrap_or(i32::MAX);
    let horizontal = match corner {
        HotCorner::TopLeft | HotCorner::BottomLeft => {
            x >= rect.left && x < rect.left.saturating_add(px)
        }
        HotCorner::TopRight | HotCorner::BottomRight => {
            x >= rect.right.saturating_sub(px) && x < rect.right
        }
        HotCorner::Off => false,
    };
    let vertical = match corner {
        HotCorner::TopLeft | HotCorner::TopRight => {
            y >= rect.top && y < rect.top.saturating_add(px)
        }
        HotCorner::BottomLeft | HotCorner::BottomRight => {
            y >= rect.bottom.saturating_sub(px) && y < rect.bottom
        }
        HotCorner::Off => false,
    };
    horizontal && vertical
}

/// 类名是不是桌面或外壳窗口。比较是精确的，不忽略大小写。
#[must_use]
pub fn is_shell_class(class_name: &str) -> bool {
    SHELL_CLASSES.contains(&class_name)
}

/// 这块显示器上有没有应抑制热角的全屏应用。
///
/// 前台窗口盖住整个 `rcMonitor`，且类名不是桌面或外壳时抑制。
/// `exclusive_d3d` 对应 `QUNS_RUNNING_D3D_FULL_SCREEN`：独占全屏的窗口矩形经常不可靠。
/// 这时若前台窗口与这块显示器相交、矩形为空或读不到，也抑制这块。
/// 前台窗口完全落在另一块显示器上时不抑制这块。跨屏是否应抑制，规格没有分开写，待定。
/// 没有前台窗口时，只有独占全屏才抑制调用方正在问的这一块。
#[must_use]
pub fn monitor_suppressed(
    monitor: PxRect,
    foreground: Option<&Foreground<'_>>,
    exclusive_d3d: bool,
) -> bool {
    let Some(foreground) = foreground else {
        return exclusive_d3d;
    };
    if is_shell_class(foreground.class_name) {
        return false;
    }
    match foreground.rect {
        Some(rect) if rect.covers(monitor) => true,
        Some(rect) if exclusive_d3d && (rect.is_empty() || rect.intersects(monitor)) => true,
        Some(_) => false,
        None => exclusive_d3d,
    }
}

/// 停留状态。轮询线程只在它变化时记日志，不把每一拍都记下来。
///
/// 这是实现里的观察，不是产品规格里的状态名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchPhase {
    /// 武装着，没有在计时。关闭时也是这个。
    Idle,
    /// 在角内，停留还没到 350ms。
    Dwelling,
    /// 到点之后解除武装，等光标离开。
    Disarmed,
}

/// 停留和当前选中的角。换角或关掉时清掉未完成的停留。
#[derive(Clone, Debug)]
pub struct CornerWatch {
    corner: HotCorner,
    armed: bool,
    since: Option<Instant>,
}

impl CornerWatch {
    #[must_use]
    pub fn new(corner: HotCorner) -> Self {
        Self {
            corner,
            armed: true,
            since: None,
        }
    }

    #[must_use]
    pub fn corner(&self) -> HotCorner {
        self.corner
    }

    /// 这一拍的停留状态。
    #[must_use]
    pub fn phase(&self) -> WatchPhase {
        if !self.armed {
            WatchPhase::Disarmed
        } else if self.since.is_some() {
            WatchPhase::Dwelling
        } else {
            WatchPhase::Idle
        }
    }

    /// 这一拍要不要打开面板。
    ///
    /// `observation` 为 `None` 表示这一拍没有读数（光标或显示器读失败）。
    /// 不把失败当成离开，也不打开。规格没有写失败算不算离开，待定。
    /// 关闭时不打开，并重新武装。
    pub fn poll(
        &mut self,
        corner: HotCorner,
        observation: Option<&Observation<'_>>,
        now: Instant,
    ) -> bool {
        if corner != self.corner {
            self.corner = corner;
            self.rearm();
        }
        if self.corner == HotCorner::Off {
            self.rearm();
            return false;
        }
        let Some(observation) = observation else {
            return false;
        };
        let (x, y) = observation.cursor;
        let mut inside = false;
        let mut suppressed = false;
        for monitor in observation.monitors {
            if !in_corner(*monitor, self.corner, CORNER_EDGE_PX, x, y) {
                continue;
            }
            inside = true;
            if monitor_suppressed(
                *monitor,
                observation.foreground.as_ref(),
                observation.exclusive_d3d,
            ) {
                suppressed = true;
            }
        }
        self.sample(inside, suppressed, now)
    }

    fn rearm(&mut self) {
        self.armed = true;
        self.since = None;
    }

    /// 在角内满 350ms 触发一次，然后解除武装。被抑制时到点也不打开，同样解除武装。
    /// 离开角内后重新武装。角内中途离开再进入，停留从头算。
    fn sample(&mut self, inside: bool, suppressed: bool, now: Instant) -> bool {
        if !inside {
            self.rearm();
            return false;
        }
        if !self.armed {
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since) >= DWELL {
            self.armed = false;
            self.since = None;
            !suppressed
        } else {
            false
        }
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

    fn primary() -> PxRect {
        PxRect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        }
    }

    fn secondary() -> PxRect {
        PxRect {
            left: 2560,
            top: 0,
            right: 3520,
            bottom: 1707,
        }
    }

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn watch_at(
        watch: &mut CornerWatch,
        corner: HotCorner,
        cursor: (i32, i32),
        monitors: &[PxRect],
        foreground: Option<Foreground<'_>>,
        exclusive_d3d: bool,
        now: Instant,
    ) -> bool {
        let observation = Observation {
            cursor,
            monitors,
            foreground,
            exclusive_d3d,
        };
        watch.poll(corner, Some(&observation), now)
    }

    #[test]
    fn stay_350_opens_once_then_stays_quiet() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        let corner = (1919, 0);
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            corner,
            &monitors,
            None,
            false,
            t0
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            corner,
            &monitors,
            None,
            false,
            t0 + at(349)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            corner,
            &monitors,
            None,
            false,
            t0 + at(350)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            corner,
            &monitors,
            None,
            false,
            t0 + at(700)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            corner,
            &monitors,
            None,
            false,
            t0 + at(5_000)
        ));
    }

    #[test]
    fn phase_tracks_dwell_open_leave_and_suppress() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert_eq!(watch.phase(), WatchPhase::Idle);
        assert_eq!(watch.corner(), HotCorner::TopRight);
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0
        ));
        assert_eq!(watch.phase(), WatchPhase::Dwelling);
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(350)
        ));
        assert_eq!(watch.phase(), WatchPhase::Disarmed);
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(700)
        ));
        assert_eq!(watch.phase(), WatchPhase::Disarmed);
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (100, 100),
            &monitors,
            None,
            false,
            t0 + at(800)
        ));
        assert_eq!(watch.phase(), WatchPhase::Idle);

        let mut suppressed = CornerWatch::new(HotCorner::TopRight);
        let video = Foreground {
            class_name: "Chrome_WidgetWin_1",
            rect: Some(screen()),
        };
        assert!(!watch_at(
            &mut suppressed,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            Some(video),
            false,
            t0
        ));
        assert_eq!(suppressed.phase(), WatchPhase::Dwelling);
        assert!(!watch_at(
            &mut suppressed,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            Some(video),
            false,
            t0 + at(350)
        ));
        assert_eq!(suppressed.phase(), WatchPhase::Disarmed);
        assert!(!suppressed.poll(HotCorner::Off, None, t0 + at(400)));
        assert_eq!(suppressed.corner(), HotCorner::Off);
        assert_eq!(suppressed.phase(), WatchPhase::Idle);
    }

    #[test]
    fn leaving_before_350_does_not_open() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(340)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (960, 540),
            &monitors,
            None,
            false,
            t0 + at(341)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (960, 540),
            &monitors,
            None,
            false,
            t0 + at(1_000)
        ));
    }

    #[test]
    fn leave_and_reenter_opens_again() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(350)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(400)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (960, 540),
            &monitors,
            None,
            false,
            t0 + at(500)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(600)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(600 + 350)
        ));
    }

    #[test]
    fn a_gap_inside_the_corner_restarts_the_dwell() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(300)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (960, 540),
            &monitors,
            None,
            false,
            t0 + at(301)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(302)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(302 + 349)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(302 + 350)
        ));
    }

    #[test]
    fn a_missed_reading_does_not_count_as_leaving() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0
        ));
        assert!(!watch.poll(HotCorner::TopRight, None, t0 + at(200)));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(349)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(350)
        ));
    }

    #[test]
    fn off_does_not_open_and_turning_it_on_starts_the_dwell() {
        let mut watch = CornerWatch::new(HotCorner::Off);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert!(!hits_corner(
            &monitors,
            HotCorner::Off,
            CORNER_EDGE_PX,
            1919,
            0
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::Off,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(1_000)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(1_000)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(1_000 + 349)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(1_000 + 350)
        ));
    }

    #[test]
    fn switching_corner_restarts_the_dwell() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(300)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopLeft,
            (0, 0),
            &monitors,
            None,
            false,
            t0 + at(300)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopLeft,
            (0, 0),
            &monitors,
            None,
            false,
            t0 + at(300 + 349)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopLeft,
            (0, 0),
            &monitors,
            None,
            false,
            t0 + at(300 + 350)
        ));
    }

    #[test]
    fn turning_off_and_back_on_starts_over() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [screen()];
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(300)
        ));
        assert!(!watch.poll(HotCorner::Off, None, t0 + at(340)));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(340)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(340 + 349)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1919, 0),
            &monitors,
            None,
            false,
            t0 + at(340 + 350)
        ));
    }

    #[test]
    fn four_corners_use_the_exclusive_rect() {
        let rect = screen();
        assert!(in_corner(rect, HotCorner::TopLeft, 1, 0, 0));
        assert!(!in_corner(rect, HotCorner::TopLeft, 1, 1, 0));
        assert!(in_corner(rect, HotCorner::TopRight, 1, 1919, 0));
        assert!(!in_corner(rect, HotCorner::TopRight, 1, 1918, 0));
        assert!(in_corner(rect, HotCorner::BottomLeft, 1, 0, 1079));
        assert!(!in_corner(rect, HotCorner::BottomLeft, 1, 0, 1078));
        assert!(in_corner(rect, HotCorner::BottomRight, 1, 1919, 1079));
        assert!(!in_corner(rect, HotCorner::BottomRight, 1, 1919, 1080));
        assert!(!in_corner(rect, HotCorner::Off, 32, 1919, 0));
        assert!(!in_corner(rect, HotCorner::TopRight, 0, 1919, 0));
    }

    #[test]
    fn configured_edge_is_32_physical_pixels() {
        let rect = screen();
        assert_eq!(CORNER_EDGE_PX, 32);
        assert!(in_corner(
            rect,
            HotCorner::TopRight,
            CORNER_EDGE_PX,
            1888,
            0
        ));
        assert!(in_corner(
            rect,
            HotCorner::TopRight,
            CORNER_EDGE_PX,
            1919,
            31
        ));
        assert!(!in_corner(
            rect,
            HotCorner::TopRight,
            CORNER_EDGE_PX,
            1887,
            0
        ));
        assert!(!in_corner(
            rect,
            HotCorner::TopRight,
            CORNER_EDGE_PX,
            1919,
            32
        ));
        assert!(in_corner(
            rect,
            HotCorner::BottomLeft,
            CORNER_EDGE_PX,
            31,
            1048
        ));
        assert!(!in_corner(
            rect,
            HotCorner::BottomLeft,
            CORNER_EDGE_PX,
            32,
            1079
        ));
    }

    #[test]
    fn negative_monitor_origin_still_has_a_corner() {
        let rect = PxRect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(in_corner(rect, HotCorner::TopRight, 1, -1, 0));
        assert!(!in_corner(rect, HotCorner::TopRight, 1, -2, 0));
        assert!(hits_corner(
            &[rect, screen()],
            HotCorner::TopLeft,
            1,
            -1920,
            0
        ));
        assert!(!hits_corner(&[rect], HotCorner::TopRight, 1, 1919, 0));
    }

    #[test]
    fn every_monitor_corner_can_hit() {
        let monitors = [primary(), secondary()];
        assert!(hits_corner(
            &monitors,
            HotCorner::TopRight,
            CORNER_EDGE_PX,
            2559,
            0
        ));
        assert!(hits_corner(
            &monitors,
            HotCorner::TopRight,
            CORNER_EDGE_PX,
            3519,
            0
        ));
        assert!(hits_corner(&monitors, HotCorner::BottomLeft, 1, 2560, 1706));
        assert!(!hits_corner(&monitors, HotCorner::TopRight, 1, 2559, 1));
        assert!(!hits_corner(&monitors, HotCorner::TopRight, 1, 2000, 10));
    }

    #[test]
    fn a_point_in_the_gap_is_not_a_corner() {
        let monitors = [
            PxRect {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
            PxRect {
                left: 200,
                top: 0,
                right: 300,
                bottom: 100,
            },
        ];
        assert!(!hits_corner(&monitors, HotCorner::TopRight, 1, 150, 0));
    }

    #[test]
    fn fullscreen_video_on_the_cursor_monitor_does_not_open() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [primary(), secondary()];
        let video = Foreground {
            class_name: "Chrome_WidgetWin_1",
            rect: Some(primary()),
        };
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            Some(video),
            false,
            t0
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            Some(video),
            false,
            t0 + at(350)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            Some(video),
            false,
            t0 + at(800)
        ));
    }

    #[test]
    fn ending_fullscreen_while_still_in_the_corner_does_not_open_until_reentry() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [primary()];
        let video = Foreground {
            class_name: "Chrome_WidgetWin_1",
            rect: Some(primary()),
        };
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            Some(video),
            false,
            t0
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            Some(video),
            false,
            t0 + at(350)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            None,
            false,
            t0 + at(900)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (1280, 720),
            &monitors,
            None,
            false,
            t0 + at(1_000)
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            None,
            false,
            t0 + at(1_000)
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            None,
            false,
            t0 + at(1_000 + 350)
        ));
    }

    #[test]
    fn fullscreen_on_another_monitor_does_not_suppress_this_corner() {
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        let monitors = [primary(), secondary()];
        let video = Foreground {
            class_name: "Chrome_WidgetWin_1",
            rect: Some(primary()),
        };
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (3519, 0),
            &monitors,
            Some(video),
            false,
            t0
        ));
        assert!(watch_at(
            &mut watch,
            HotCorner::TopRight,
            (3519, 0),
            &monitors,
            Some(video),
            false,
            t0 + at(350)
        ));
    }

    #[test]
    fn desktop_covering_the_monitor_is_not_fullscreen() {
        let t0 = Instant::now();
        let monitors = [primary()];
        for class_name in ["Progman", "WorkerW", "Shell_TrayWnd", "#32769"] {
            let mut one = CornerWatch::new(HotCorner::TopRight);
            let desktop = Foreground {
                class_name,
                rect: Some(primary()),
            };
            assert!(
                !watch_at(
                    &mut one,
                    HotCorner::TopRight,
                    (2559, 0),
                    &monitors,
                    Some(desktop),
                    false,
                    t0
                ),
                "{class_name}"
            );
            assert!(
                watch_at(
                    &mut one,
                    HotCorner::TopRight,
                    (2559, 0),
                    &monitors,
                    Some(desktop),
                    false,
                    t0 + at(350)
                ),
                "{class_name}"
            );
            assert!(!monitor_suppressed(primary(), Some(&desktop), true));
        }
    }

    #[test]
    fn maximized_window_does_not_cover_the_monitor() {
        let work = PxRect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1400,
        };
        let window = Foreground {
            class_name: "Notepad",
            rect: Some(work),
        };
        assert!(!monitor_suppressed(primary(), Some(&window), false));
    }

    #[test]
    fn exclusive_fullscreen_without_a_covering_rect_suppresses_the_hit_monitor() {
        let monitors = [primary(), secondary()];
        let mut watch = CornerWatch::new(HotCorner::TopRight);
        let t0 = Instant::now();
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            None,
            true,
            t0
        ));
        assert!(!watch_at(
            &mut watch,
            HotCorner::TopRight,
            (2559, 0),
            &monitors,
            None,
            true,
            t0 + at(350)
        ));

        let elsewhere = Foreground {
            class_name: "Game",
            rect: Some(secondary()),
        };
        assert!(!monitor_suppressed(primary(), Some(&elsewhere), true));
        assert!(monitor_suppressed(secondary(), Some(&elsewhere), true));

        let empty = Foreground {
            class_name: "Game",
            rect: Some(PxRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            }),
        };
        assert!(monitor_suppressed(primary(), Some(&empty), true));
        assert!(!monitor_suppressed(primary(), Some(&empty), false));
        assert!(monitor_suppressed(
            primary(),
            Some(&Foreground {
                class_name: "Game",
                rect: None,
            }),
            true
        ));
    }

    #[test]
    fn shell_class_list_does_not_include_ordinary_windows() {
        assert!(is_shell_class("Progman"));
        assert!(is_shell_class("Shell_SecondaryTrayWnd"));
        assert!(!is_shell_class("progman"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
        assert!(!is_shell_class(""));
    }
}
