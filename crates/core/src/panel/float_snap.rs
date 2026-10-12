//! 便签和收纳悬浮窗的贴边几何。不读光标，也不调用窗口 API。
//!
//! 产品规格写了：松手时贴住屏幕外侧的左、右或上边就收成细条；两块屏幕共用的边不算；
//! 点细条展开，再点或失焦收回；把展开的窗口拖离这条边就取消贴边。
//! 细条有多厚、离边多近算贴住、同时贴住两条边时选哪一条、滑出要多久，规格没有写。
//! 下面的常数是当前做法，不是产品规则，待定。

use crate::notes::{DockEdge, FloatDock};
use crate::shell::{BASE_DPI, to_physical};

/// 细条在贴边方向上的厚度，逻辑像素。随所贴屏幕的 DPI 换成物理像素。待定。
pub const STRIP_LOGICAL_PX: u32 = 8;

/// 窗口对应的边离屏幕外边不超过这么多逻辑像素，松手算贴住。待定。
pub const SNAP_SLOP_LOGICAL: u32 = 12;

/// 按下和松开的位移不超过这么多逻辑像素，当成点击，不当成拖动。待定。
pub const DRAG_MOVE_LOGICAL: u32 = 4;

/// 滑出或收回分成这么多步。每步多久由界面定。待定。
pub const ANIM_STEPS: u32 = 8;

/// 一块屏幕的外沿，物理像素，右和下是开区间。`dpi` 是这块屏幕的有效 DPI。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub dpi: u32,
}

/// 窗口矩形，物理像素。`x`、`y` 是虚拟屏幕坐标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxBox {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PxBox {
    #[must_use]
    pub fn right(self) -> i32 {
        self.x.saturating_add_unsigned(self.width)
    }

    #[must_use]
    pub fn bottom(self) -> i32 {
        self.y.saturating_add_unsigned(self.height)
    }
}

/// 一段真正的屏幕外边。共用边已经从这里去掉。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OuterEdge {
    pub edge: DockEdge,
    /// 左、右是这条竖线的 x；上是这条横线的 y。
    pub at: i32,
    /// 沿边的起点（含）和终点（不含）。左、右是 y；上是 x。
    pub start: i32,
    pub end: i32,
    pub dpi: u32,
}

/// 展开时的窗口，以及可选的贴边。`expanded` 始终是展开尺寸，不是细条。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockState {
    pub expanded: PxBox,
    pub dock: Option<FloatDock>,
}

/// 界面在拖动结束、点细条、失焦时交给几何模块的一件事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockInput {
    DragFinished {
        rect: PxBox,
        /// 松开时的左上角相对按下时移动超过了点击阈值。
        moved: bool,
        /// 这次拖动开始时窗口是收起的细条。
        from_collapsed: bool,
        /// 宽或高变了。位置没动时用来分辨改大小和拖动。
        size_changed: bool,
    },
    StripClick,
    Blur,
}

/// 一次状态变化。`animate` 为真时界面从 `from` 滑到 `to`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub state: DockState,
    pub from: PxBox,
    pub to: PxBox,
    pub animate: bool,
}

#[must_use]
pub fn strip_thickness(dpi: u32) -> u32 {
    let px = to_physical(STRIP_LOGICAL_PX, dpi);
    u32::try_from(px).unwrap_or(1).max(1)
}

#[must_use]
pub fn snap_slop(dpi: u32) -> i32 {
    to_physical(SNAP_SLOP_LOGICAL, dpi).max(1)
}

/// 点 `(x, y)` 落在哪块屏幕上，就用那块的 DPI。都不在时用 96。
#[must_use]
pub fn dpi_at(screens: &[ScreenBounds], x: i32, y: i32) -> u32 {
    screens
        .iter()
        .find(|screen| x >= screen.left && x < screen.right && y >= screen.top && y < screen.bottom)
        .map(|screen| screen.dpi.max(1))
        .unwrap_or(BASE_DPI)
}

#[must_use]
pub fn moved_enough(from: (i32, i32), to: (i32, i32), dpi: u32) -> bool {
    let slop = i64::from(to_physical(DRAG_MOVE_LOGICAL, dpi).max(1));
    let dx = i64::from(from.0) - i64::from(to.0);
    let dy = i64::from(from.1) - i64::from(to.1);
    dx.abs() > slop || dy.abs() > slop
}

/// 每块屏幕的外侧左、右、上边。和另一块屏幕相接或被另一块盖住的那一段不算。
#[must_use]
pub fn outer_edges(screens: &[ScreenBounds]) -> Vec<OuterEdge> {
    let mut edges = Vec::new();
    for (index, screen) in screens.iter().copied().enumerate() {
        if screen.right <= screen.left || screen.bottom <= screen.top {
            continue;
        }
        push_vertical(
            &mut edges,
            screens,
            index,
            screen,
            DockEdge::Left,
            screen.left,
        );
        push_vertical(
            &mut edges,
            screens,
            index,
            screen,
            DockEdge::Right,
            screen.right,
        );
        push_top(&mut edges, screens, index, screen);
    }
    edges
}

/// 松手时窗口贴住的那一段外边。没贴住则 `None`。
///
/// 同时贴住多段时，先取间隙更小的；一样时先左、再右、再上。这个次序待定。
#[must_use]
pub fn snap_on_release(window: PxBox, screens: &[ScreenBounds]) -> Option<OuterEdge> {
    let mut best: Option<(i32, u8, i32, i32, OuterEdge)> = None;
    for edge in outer_edges(screens) {
        let Some(gap) = gap_to(window, &edge) else {
            continue;
        };
        let rank = edge_rank(edge.edge);
        let candidate = (gap, rank, edge.at, edge.start, edge);
        let replace = match best {
            None => true,
            Some(current) => {
                candidate.0 < current.0
                    || (candidate.0 == current.0 && candidate.1 < current.1)
                    || (candidate.0 == current.0
                        && candidate.1 == current.1
                        && (candidate.2, candidate.3) < (current.2, current.3))
            }
        };
        if replace {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, _, _, edge)| edge)
}

#[must_use]
pub fn edge_label(dock: Option<FloatDock>) -> &'static str {
    match dock.map(|dock| dock.edge) {
        Some(DockEdge::Left) => "left",
        Some(DockEdge::Right) => "right",
        Some(DockEdge::Top) => "top",
        None => "",
    }
}

/// 记下的贴边在这些屏幕上还能对上一条外边。
#[must_use]
pub fn dock_applies(state: &DockState, screens: &[ScreenBounds]) -> bool {
    resolve(state.dock, state.expanded, screens).is_some()
}

/// 现在该画出来的矩形。贴边对不上当前屏幕时，画出展开的窗口。
#[must_use]
pub fn visible_box(state: &DockState, screens: &[ScreenBounds]) -> PxBox {
    match resolve(state.dock, state.expanded, screens) {
        Some(resolved) if resolved.collapsed => strip_box(&resolved, state.expanded),
        Some(resolved) => flush_box(&resolved, state.expanded),
        None => state.expanded,
    }
}

#[must_use]
pub fn transition(state: &DockState, screens: &[ScreenBounds], input: DockInput) -> Transition {
    let from = visible_box(state, screens);
    let next = match input {
        DockInput::DragFinished {
            rect,
            moved,
            from_collapsed,
            size_changed,
        } => drag_finished(state, screens, rect, moved, from_collapsed, size_changed),
        DockInput::StripClick => toggle_strip(state, screens),
        DockInput::Blur => blur(state, screens),
    };
    let to = visible_box(&next, screens);
    Transition {
        state: next,
        from,
        to,
        animate: from != to,
    }
}

/// 整数线性插值。`den` 为 0 或 `num` 不小于 `den` 时得到 `to`。
#[must_use]
pub fn lerp_box(from: PxBox, to: PxBox, num: u32, den: u32) -> PxBox {
    if den == 0 || num >= den {
        return to;
    }
    PxBox {
        x: lerp_i32(from.x, to.x, num, den),
        y: lerp_i32(from.y, to.y, num, den),
        width: lerp_u32(from.width, to.width, num, den),
        height: lerp_u32(from.height, to.height, num, den),
    }
}

struct Resolved {
    edge: OuterEdge,
    offset: i32,
    collapsed: bool,
}

fn drag_finished(
    state: &DockState,
    screens: &[ScreenBounds],
    rect: PxBox,
    moved: bool,
    from_collapsed: bool,
    size_changed: bool,
) -> DockState {
    if !moved && from_collapsed {
        return toggle_strip(state, screens);
    }
    if !moved && size_changed && !from_collapsed {
        return resize_in_place(state, screens, rect);
    }
    if !moved {
        return *state;
    }
    match snap_on_release(rect, screens) {
        Some(edge) => snap_to(state, edge, rect, from_collapsed),
        None if state.dock.is_some() => undock(state, rect, from_collapsed),
        None => DockState {
            expanded: if from_collapsed {
                PxBox {
                    x: rect.x,
                    y: rect.y,
                    width: state.expanded.width,
                    height: state.expanded.height,
                }
            } else {
                rect
            },
            dock: None,
        },
    }
}

fn resize_in_place(state: &DockState, screens: &[ScreenBounds], rect: PxBox) -> DockState {
    let Some(dock) = state.dock else {
        return DockState {
            expanded: rect,
            dock: None,
        };
    };
    if dock.collapsed {
        return *state;
    }
    let sized = PxBox {
        width: rect.width,
        height: rect.height,
        ..state.expanded
    };
    let Some(resolved) = resolve(Some(dock), sized, screens) else {
        return DockState {
            expanded: sized,
            dock: Some(dock),
        };
    };
    let expanded = flush_box(
        &Resolved {
            collapsed: false,
            ..resolved
        },
        sized,
    );
    DockState {
        expanded,
        dock: Some(FloatDock {
            edge: dock.edge,
            offset: along_origin(dock.edge, expanded),
            collapsed: false,
        }),
    }
}

fn snap_to(state: &DockState, edge: OuterEdge, rect: PxBox, from_collapsed: bool) -> DockState {
    let size = if from_collapsed { state.expanded } else { rect };
    let offset = clamp_offset(
        along_origin(edge.edge, rect),
        along_length(edge.edge, size),
        edge.start,
        edge.end,
    );
    let dock = FloatDock {
        edge: edge.edge,
        offset,
        collapsed: true,
    };
    let expanded = flush_box(
        &Resolved {
            edge,
            offset,
            collapsed: false,
        },
        size,
    );
    DockState {
        expanded,
        dock: Some(dock),
    }
}

fn undock(state: &DockState, rect: PxBox, from_collapsed: bool) -> DockState {
    DockState {
        expanded: if from_collapsed {
            PxBox {
                x: rect.x,
                y: rect.y,
                width: state.expanded.width,
                height: state.expanded.height,
            }
        } else {
            rect
        },
        dock: None,
    }
}

fn toggle_strip(state: &DockState, screens: &[ScreenBounds]) -> DockState {
    let Some(dock) = state.dock else {
        return *state;
    };
    if resolve(Some(dock), state.expanded, screens).is_none() {
        return *state;
    }
    DockState {
        expanded: state.expanded,
        dock: Some(FloatDock {
            collapsed: !dock.collapsed,
            ..dock
        }),
    }
}

fn blur(state: &DockState, screens: &[ScreenBounds]) -> DockState {
    let Some(dock) = state.dock else {
        return *state;
    };
    if dock.collapsed || resolve(Some(dock), state.expanded, screens).is_none() {
        return *state;
    }
    DockState {
        expanded: state.expanded,
        dock: Some(FloatDock {
            collapsed: true,
            ..dock
        }),
    }
}

fn resolve(dock: Option<FloatDock>, expanded: PxBox, screens: &[ScreenBounds]) -> Option<Resolved> {
    let dock = dock?;
    let hint = match dock.edge {
        DockEdge::Left => expanded.x,
        DockEdge::Right => expanded.right(),
        DockEdge::Top => expanded.y,
    };
    let along = along_length(dock.edge, expanded);
    let edge = outer_edges(screens)
        .into_iter()
        .filter(|edge| edge.edge == dock.edge)
        .filter(|edge| (edge.at - hint).saturating_abs() <= snap_slop(edge.dpi))
        .filter(|edge| {
            let end = dock.offset.saturating_add_unsigned(along);
            overlap_len(dock.offset, end, edge.start, edge.end) > 0
        })
        .min_by_key(|edge| {
            let distance = i64::from(edge.at) - i64::from(hint);
            (distance.abs(), edge.at, edge.start)
        })?;
    let offset = clamp_offset(dock.offset, along, edge.start, edge.end);
    Some(Resolved {
        edge,
        offset,
        collapsed: dock.collapsed,
    })
}

fn flush_box(resolved: &Resolved, expanded: PxBox) -> PxBox {
    let offset = clamp_offset(
        resolved.offset,
        along_length(resolved.edge.edge, expanded),
        resolved.edge.start,
        resolved.edge.end,
    );
    match resolved.edge.edge {
        DockEdge::Left => PxBox {
            x: resolved.edge.at,
            y: offset,
            width: expanded.width,
            height: expanded.height,
        },
        DockEdge::Right => PxBox {
            x: resolved.edge.at.saturating_sub_unsigned(expanded.width),
            y: offset,
            width: expanded.width,
            height: expanded.height,
        },
        DockEdge::Top => PxBox {
            x: offset,
            y: resolved.edge.at,
            width: expanded.width,
            height: expanded.height,
        },
    }
}

fn strip_box(resolved: &Resolved, expanded: PxBox) -> PxBox {
    let thickness = strip_thickness(resolved.edge.dpi);
    let along = along_length(resolved.edge.edge, expanded).max(thickness);
    let span = u32::try_from(resolved.edge.end.saturating_sub(resolved.edge.start)).unwrap_or(0);
    let along = if span == 0 {
        along
    } else {
        along.min(span).max(1)
    };
    let offset = clamp_offset(
        resolved.offset,
        along,
        resolved.edge.start,
        resolved.edge.end,
    );
    match resolved.edge.edge {
        DockEdge::Left => PxBox {
            x: resolved.edge.at,
            y: offset,
            width: thickness,
            height: along,
        },
        DockEdge::Right => PxBox {
            x: resolved.edge.at.saturating_sub_unsigned(thickness),
            y: offset,
            width: thickness,
            height: along,
        },
        DockEdge::Top => PxBox {
            x: offset,
            y: resolved.edge.at,
            width: along,
            height: thickness,
        },
    }
}

fn along_origin(edge: DockEdge, rect: PxBox) -> i32 {
    match edge {
        DockEdge::Left | DockEdge::Right => rect.y,
        DockEdge::Top => rect.x,
    }
}

fn along_length(edge: DockEdge, rect: PxBox) -> u32 {
    match edge {
        DockEdge::Left | DockEdge::Right => rect.height,
        DockEdge::Top => rect.width,
    }
}

fn clamp_offset(offset: i32, length: u32, start: i32, end: i32) -> i32 {
    let length = i32::try_from(length).unwrap_or(i32::MAX);
    let last = end.saturating_sub(length);
    if last <= start {
        start
    } else {
        offset.clamp(start, last)
    }
}

fn gap_to(window: PxBox, edge: &OuterEdge) -> Option<i32> {
    let slop = snap_slop(edge.dpi);
    let (delta, overlap) = match edge.edge {
        DockEdge::Left => (
            (window.x - edge.at).saturating_abs(),
            overlap_len(window.y, window.bottom(), edge.start, edge.end),
        ),
        DockEdge::Right => (
            (window.right() - edge.at).saturating_abs(),
            overlap_len(window.y, window.bottom(), edge.start, edge.end),
        ),
        DockEdge::Top => (
            (window.y - edge.at).saturating_abs(),
            overlap_len(window.x, window.right(), edge.start, edge.end),
        ),
    };
    (delta <= slop && overlap > 0).then_some(delta)
}

fn edge_rank(edge: DockEdge) -> u8 {
    match edge {
        DockEdge::Left => 0,
        DockEdge::Right => 1,
        DockEdge::Top => 2,
    }
}

fn overlap_len(a0: i32, a1: i32, b0: i32, b1: i32) -> i64 {
    let start = i64::from(a0).max(i64::from(b0));
    let end = i64::from(a1).min(i64::from(b1));
    (end - start).max(0)
}

fn push_vertical(
    edges: &mut Vec<OuterEdge>,
    screens: &[ScreenBounds],
    index: usize,
    screen: ScreenBounds,
    edge: DockEdge,
    at: i32,
) {
    let mut cuts = Vec::new();
    for (other_index, other) in screens.iter().copied().enumerate() {
        if other_index == index {
            continue;
        }
        let covers = match edge {
            DockEdge::Left => other.left < at && other.right >= at,
            DockEdge::Right => other.left <= at && other.right > at,
            DockEdge::Top => false,
        };
        if covers {
            cuts.push((other.top, other.bottom));
        }
    }
    for (start, end) in subtract_span(screen.top, screen.bottom, &cuts) {
        edges.push(OuterEdge {
            edge,
            at,
            start,
            end,
            dpi: screen.dpi,
        });
    }
}

fn push_top(
    edges: &mut Vec<OuterEdge>,
    screens: &[ScreenBounds],
    index: usize,
    screen: ScreenBounds,
) {
    let mut cuts = Vec::new();
    for (other_index, other) in screens.iter().copied().enumerate() {
        if other_index == index {
            continue;
        }
        if other.top < screen.top && other.bottom >= screen.top {
            cuts.push((other.left, other.right));
        }
    }
    for (start, end) in subtract_span(screen.left, screen.right, &cuts) {
        edges.push(OuterEdge {
            edge: DockEdge::Top,
            at: screen.top,
            start,
            end,
            dpi: screen.dpi,
        });
    }
}

fn subtract_span(start: i32, end: i32, cuts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    if end <= start {
        return Vec::new();
    }
    let mut blocks: Vec<(i32, i32)> = cuts
        .iter()
        .copied()
        .map(|(lo, hi)| (lo.max(start), hi.min(end)))
        .filter(|(lo, hi)| hi > lo)
        .collect();
    blocks.sort_by_key(|span| span.0);
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (lo, hi) in blocks {
        if let Some(last) = merged.last_mut()
            && lo <= last.1
        {
            last.1 = last.1.max(hi);
            continue;
        }
        merged.push((lo, hi));
    }
    let mut out = Vec::new();
    let mut cursor = start;
    for (lo, hi) in merged {
        if lo > cursor {
            out.push((cursor, lo));
        }
        cursor = cursor.max(hi);
    }
    if cursor < end {
        out.push((cursor, end));
    }
    out
}

fn lerp_i32(from: i32, to: i32, num: u32, den: u32) -> i32 {
    let delta = i64::from(to) - i64::from(from);
    let stepped = i64::from(from) + delta * i64::from(num) / i64::from(den);
    i32::try_from(stepped).unwrap_or(to)
}

fn lerp_u32(from: u32, to: u32, num: u32, den: u32) -> u32 {
    let delta = i64::from(to) - i64::from(from);
    let stepped = i64::from(from) + delta * i64::from(num) / i64::from(den);
    u32::try_from(stepped).unwrap_or(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(left: i32, top: i32, right: i32, bottom: i32, dpi: u32) -> ScreenBounds {
        ScreenBounds {
            left,
            top,
            right,
            bottom,
            dpi,
        }
    }

    fn window(x: i32, y: i32, width: u32, height: u32) -> PxBox {
        PxBox {
            x,
            y,
            width,
            height,
        }
    }

    fn dock(edge: DockEdge, offset: i32, collapsed: bool) -> FloatDock {
        FloatDock {
            edge,
            offset,
            collapsed,
        }
    }

    fn state(expanded: PxBox, dock: Option<FloatDock>) -> DockState {
        DockState { expanded, dock }
    }

    #[test]
    fn a_single_screen_snaps_on_left_right_and_top_but_not_the_bottom_or_the_middle() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let edges = outer_edges(&screens);
        assert!(
            edges
                .iter()
                .any(|edge| edge.edge == DockEdge::Left && edge.at == 0)
        );
        assert!(
            edges
                .iter()
                .any(|edge| edge.edge == DockEdge::Right && edge.at == 1920)
        );
        assert!(
            edges
                .iter()
                .any(|edge| edge.edge == DockEdge::Top && edge.at == 0)
        );
        assert!(
            edges
                .iter()
                .all(|edge| edge.edge != DockEdge::Top || edge.at == 0)
        );

        let left = snap_on_release(window(2, 100, 320, 360), &screens).expect("left");
        assert_eq!(left.edge, DockEdge::Left);
        let right = snap_on_release(window(1920 - 320, 80, 320, 360), &screens).expect("right");
        assert_eq!(right.edge, DockEdge::Right);
        let top = snap_on_release(window(400, 0, 320, 360), &screens).expect("top");
        assert_eq!(top.edge, DockEdge::Top);

        assert!(snap_on_release(window(400, 1080 - 360, 320, 360), &screens).is_none());
        assert!(snap_on_release(window(400, 200, 320, 360), &screens).is_none());
    }

    #[test]
    fn the_shared_edge_between_two_monitors_side_by_side_does_not_snap() {
        let screens = [
            screen(0, 0, 1920, 1080, 96),
            screen(1920, 0, 3840, 1080, 144),
        ];
        assert!(
            snap_on_release(window(1920 - 300, 100, 300, 200), &screens).is_none(),
            "左屏的右边贴着右屏"
        );
        assert!(
            snap_on_release(window(1920, 100, 300, 200), &screens).is_none(),
            "右屏的左边贴着左屏"
        );
        let outer_left = snap_on_release(window(0, 40, 300, 200), &screens).expect("outer left");
        assert_eq!((outer_left.edge, outer_left.at), (DockEdge::Left, 0));
        let outer_right =
            snap_on_release(window(3840 - 300, 40, 300, 200), &screens).expect("outer right");
        assert_eq!(
            (outer_right.edge, outer_right.at, outer_right.dpi),
            (DockEdge::Right, 3840, 144)
        );
    }

    #[test]
    fn a_monitor_stacked_below_another_does_not_treat_the_shared_top_as_outer() {
        let screens = [
            screen(0, 0, 1920, 1080, 96),
            screen(0, 1080, 1920, 2160, 96),
        ];
        assert!(
            snap_on_release(window(100, 1080, 300, 200), &screens).is_none(),
            "下屏的上边贴着上屏的下边"
        );
        let top = snap_on_release(window(100, 0, 300, 200), &screens).expect("upper top");
        assert_eq!(top.at, 0);
        let side = snap_on_release(window(0, 1200, 300, 200), &screens).expect("lower left");
        assert_eq!(side.edge, DockEdge::Left);
    }

    #[test]
    fn only_the_part_of_an_edge_that_sticks_out_past_the_other_monitor_is_outer() {
        let screens = [
            screen(0, 0, 1920, 1080, 96),
            screen(1920, 200, 3840, 900, 96),
        ];
        let above = snap_on_release(window(1920 - 280, 0, 280, 80), &screens).expect("protruding");
        assert_eq!(above.edge, DockEdge::Right);
        assert!(above.start <= 0 && above.end <= 200);
        assert!(snap_on_release(window(1920 - 280, 400, 280, 100), &screens).is_none());
        let lower_top = snap_on_release(window(2000, 200, 200, 100), &screens).expect("b top");
        assert_eq!(lower_top.edge, DockEdge::Top);
        assert_eq!(lower_top.at, 200);
    }

    #[test]
    fn a_monitor_left_of_the_primary_keeps_its_outer_left_edge() {
        let screens = [screen(-1920, 0, 0, 1080, 96), screen(0, 0, 1920, 1080, 96)];
        assert!(snap_on_release(window(-200, 40, 200, 100), &screens).is_none());
        let left = snap_on_release(window(-1920, 40, 200, 100), &screens).expect("far left");
        assert_eq!(left.at, -1920);
    }

    #[test]
    fn slop_and_strip_thickness_follow_dpi() {
        assert_eq!(snap_slop(96), 12);
        assert_eq!(snap_slop(144), 18);
        assert_eq!(strip_thickness(96), 8);
        assert_eq!(strip_thickness(144), 12);

        let low = [screen(0, 0, 1920, 1080, 96)];
        assert!(snap_on_release(window(12, 200, 100, 100), &low).is_some());
        assert!(snap_on_release(window(13, 200, 100, 100), &low).is_none());

        let high = [screen(0, 0, 2560, 1440, 144)];
        assert!(snap_on_release(window(18, 200, 100, 100), &high).is_some());
        assert!(snap_on_release(window(19, 200, 100, 100), &high).is_none());
    }

    #[test]
    fn a_corner_prefers_the_closer_edge_and_then_left_over_top() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let both = snap_on_release(window(0, 0, 200, 200), &screens).expect("corner");
        assert_eq!(
            both.edge,
            DockEdge::Left,
            "间隙一样时先左后上，这个次序待定"
        );
        let nearer_top = snap_on_release(window(8, 0, 200, 200), &screens).expect("top");
        assert_eq!(nearer_top.edge, DockEdge::Top);
    }

    #[test]
    fn the_window_must_overlap_the_outer_segment() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        assert!(snap_on_release(window(0, 2000, 100, 100), &screens).is_none());
        assert!(snap_on_release(window(0, -100, 100, 50), &screens).is_none());
    }

    #[test]
    fn release_on_an_outer_edge_collapses_and_a_strip_click_or_blur_round_trips() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let open = state(window(400, 200, 320, 360), None);
        let snapped = transition(
            &open,
            &screens,
            DockInput::DragFinished {
                rect: window(0, 180, 320, 360),
                moved: true,
                from_collapsed: false,
                size_changed: true,
            },
        );
        assert!(snapped.animate);
        assert_eq!(snapped.state.dock.unwrap().edge, DockEdge::Left);
        assert!(snapped.state.dock.unwrap().collapsed);
        assert_eq!(snapped.state.expanded.width, 320);
        assert_eq!(snapped.to.width, strip_thickness(96));
        assert_eq!(snapped.to.x, 0);
        assert_eq!(snapped.to.y, 180);

        let expanded = transition(&snapped.state, &screens, DockInput::StripClick);
        assert!(!expanded.state.dock.unwrap().collapsed);
        assert_eq!(expanded.to, window(0, 180, 320, 360));

        let collapsed = transition(&expanded.state, &screens, DockInput::Blur);
        assert!(collapsed.state.dock.unwrap().collapsed);
        assert_eq!(collapsed.to.width, strip_thickness(96));

        let again = transition(&expanded.state, &screens, DockInput::StripClick);
        assert!(again.state.dock.unwrap().collapsed);
    }

    #[test]
    fn dragging_the_expanded_window_off_the_edge_clears_the_dock_and_blur_stops_collapsing() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let dock = dock(DockEdge::Left, 180, false);
        let snapped = state(window(0, 180, 320, 360), Some(dock));
        let away = transition(
            &snapped,
            &screens,
            DockInput::DragFinished {
                rect: window(80, 180, 320, 360),
                moved: true,
                from_collapsed: false,
                size_changed: false,
            },
        );
        assert!(away.state.dock.is_none());
        assert_eq!(away.to, window(80, 180, 320, 360));
        let blur = transition(&away.state, &screens, DockInput::Blur);
        assert_eq!(blur.state, away.state);
        assert!(!blur.animate);
    }

    #[test]
    fn dragging_a_collapsed_strip_off_the_edge_restores_the_expanded_size() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let collapsed = state(
            window(0, 180, 320, 360),
            Some(dock(DockEdge::Left, 180, true)),
        );
        let away = transition(
            &collapsed,
            &screens,
            DockInput::DragFinished {
                rect: window(100, 220, strip_thickness(96), 360),
                moved: true,
                from_collapsed: true,
                size_changed: true,
            },
        );
        assert!(away.state.dock.is_none());
        assert_eq!(away.state.expanded, window(100, 220, 320, 360));
    }

    #[test]
    fn a_click_without_movement_on_the_strip_expands_and_a_resize_does_not_collapse() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let collapsed = state(
            window(0, 180, 320, 360),
            Some(dock(DockEdge::Left, 180, true)),
        );
        let click = transition(
            &collapsed,
            &screens,
            DockInput::DragFinished {
                rect: window(0, 180, 8, 360),
                moved: false,
                from_collapsed: true,
                size_changed: false,
            },
        );
        assert!(!click.state.dock.unwrap().collapsed);

        let expanded = state(
            window(0, 180, 320, 360),
            Some(dock(DockEdge::Left, 180, false)),
        );
        let resized = transition(
            &expanded,
            &screens,
            DockInput::DragFinished {
                rect: window(0, 180, 400, 360),
                moved: false,
                from_collapsed: false,
                size_changed: true,
            },
        );
        assert!(!resized.state.dock.unwrap().collapsed);
        assert_eq!(resized.state.expanded.width, 400);
        assert_eq!(resized.state.expanded.x, 0);
    }

    #[test]
    fn blur_does_nothing_when_the_saved_edge_is_no_longer_outer() {
        let saved = state(
            window(0, 40, 320, 200),
            Some(dock(DockEdge::Left, 40, false)),
        );
        let gone = [screen(1920, 0, 3840, 1080, 96)];
        let blur = transition(&saved, &gone, DockInput::Blur);
        assert!(!blur.state.dock.unwrap().collapsed);
        assert_eq!(visible_box(&saved, &gone), saved.expanded);
    }

    #[test]
    fn right_and_top_strips_sit_on_the_outer_edge_at_both_dpis() {
        let right_screen = [screen(0, 0, 1920, 1080, 144)];
        let right = state(
            window(1920 - 300, 50, 300, 400),
            Some(dock(DockEdge::Right, 50, true)),
        );
        let strip = visible_box(&right, &right_screen);
        assert_eq!(strip.width, 12);
        assert_eq!(strip.right(), 1920);
        assert_eq!(strip.y, 50);
        assert_eq!(strip.height, 400);

        let top = state(window(30, 0, 500, 280), Some(dock(DockEdge::Top, 30, true)));
        let strip = visible_box(&top, &[screen(0, 0, 1920, 1080, 96)]);
        assert_eq!(strip.height, 8);
        assert_eq!(strip.y, 0);
        assert_eq!(strip.x, 30);
        assert_eq!(strip.width, 500);
    }

    #[test]
    fn lerp_hits_the_ends_and_the_middle() {
        let from = window(0, 0, 8, 100);
        let to = window(0, 0, 200, 100);
        assert_eq!(lerp_box(from, to, 0, 8), from);
        assert_eq!(lerp_box(from, to, 8, 8), to);
        assert_eq!(lerp_box(from, to, 4, 8).width, 104);
        assert!(moved_enough((0, 0), (5, 0), 96));
        assert!(!moved_enough((0, 0), (4, 0), 96));
        assert!(moved_enough((0, 0), (7, 0), 144));
        assert_eq!(dpi_at(&[screen(100, 0, 200, 100, 144)], 150, 10), 144);
        assert_eq!(dpi_at(&[screen(100, 0, 200, 100, 144)], 0, 0), 96);
    }
}
