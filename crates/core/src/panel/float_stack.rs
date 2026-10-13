//! 便签和收纳悬浮窗的叠页。不读光标，也不调用窗口 API。
//!
//! 产品规格写了：松手时重叠超过一半就合成一叠；左右键翻页；拖标题栏只移动整叠；
//! 把某一个页点拖出去就拆出那一页；点一个页点换到那一页；整叠一起贴边；
//! 两条已经收起的把手在同一条屏幕边上靠近时合成一叠；成员、当前页和贴边要能记下。
//! 一半按哪一边的面积算、合成后用谁的位置、页序、当前页、到头是否循环、多近算靠近，规格没有写。
//! 页点的直径、间距和点击换页之外的细节，规格也没有写成唯一做法。
//! 下面的选择是当前做法，不是产品规则，待定。

use crate::notes::{DockEdge, FloatDock, FloatGeometry, FloatStackSlot};
use crate::shell::{WorkArea, to_physical};

use super::float_snap::{PxBox, ScreenBounds};

/// 两条收起的把手沿边相距不超过这么多逻辑像素，就合成一叠。待定。
///
/// 两条收起的把手沿边离这么多逻辑像素以内就合成一叠。不是产品规格。
pub const DOCK_MERGE_GAP_LOGICAL: u32 = 48;

/// 页点一行里，每个点占的槽宽。点画在槽的正中。待定。
pub const DOT_SLOT_LOGICAL: u32 = 12;
/// 页点那一行窗口的逻辑高度。挂在内容框外面。待定。
pub const DOT_ROW_LOGICAL_HEIGHT: u32 = 16;

/// 把手上最多画这么多个点。再多的页数仍只画这些点。待定。
pub const HANDLE_DOT_CAP: usize = 16;

/// 一叠里的一页。便签和收纳可以混在同一叠。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StackPage {
    Note(String),
    Shelf(String),
}

/// 一叠。`pages` 至少两页时才算叠；只剩一页时由调用方把它拆成普通悬浮窗。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageStack {
    pub id: String,
    pub pages: Vec<StackPage>,
    pub current: usize,
}

/// 拖动从哪开始。标题栏拖的是整叠；页点拖的是被按住的那一页。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragOrigin {
    TitleBar,
    PageDot,
}

/// 左右键。到头就停，不循环。循环与否待定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flip {
    Left,
    Right,
}

/// 松手之后叠页这一层要做的事。贴边几何仍由 `float_snap` 决定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropEffect {
    /// 整叠（或单独一个悬浮窗）保持在一起，接着走原来的移动或贴边。
    MoveTogether,
    /// 把被拖的那一页拆出去。其余页留在拖动开始时的位置。
    /// 页点拖动时是被按住的那一页，不一定是当时显示的页。
    DetachCurrent,
    /// 被拖的整叠并进 `target` 所在的那一叠。
    MergeUnit { target: StackPage },
    /// 只把被拖的那一页并进 `target`。页点拖到另一个悬浮窗上时用这个。
    MergeCurrent { target: StackPage },
}

/// 松手时交给叠页规则的材料。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropInput {
    pub origin: DragOrigin,
    /// 按下时窗口看得见的矩形。
    pub start: PxBox,
    /// 松开时窗口看得见的矩形。页点拖动时，这是整叠的矩形跟着页点挪过的那一份。
    pub released: PxBox,
    pub moved: bool,
    /// 这次拖动开始时窗口是收起的把手。把手拖动移动整叠，不拆页。
    pub from_collapsed: bool,
    pub page_count: usize,
    /// 松开的位置会贴上屏幕外侧。整叠一起贴，不拆页。
    pub snapped: bool,
    /// 其他悬浮窗看得见的矩形。不含自己。
    pub others: Vec<OtherFloat>,
}

/// 另一个开着的悬浮窗。`page` 是它当前显示的那一页。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtherFloat {
    pub page: StackPage,
    pub rect: PxBox,
}

/// 从一叠里拆出一页的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachOutcome {
    pub removed: StackPage,
    /// 还剩至少两页。
    pub remaining: Option<PageStack>,
    /// 拆完只剩一页时，那一页不再成叠。
    pub leftover: Option<StackPage>,
}

/// 带叠页字段的一页，用来从文件还原。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackMember {
    pub page: StackPage,
    pub slot: FloatStackSlot,
}

/// 启动时读到的一页。`geometry` 缺了仍留在叠里，位置用同一叠里另一页的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupPage {
    pub page: StackPage,
    pub slot: FloatStackSlot,
    pub geometry: Option<FloatGeometry>,
    pub dock: Option<FloatDock>,
}

/// 启动时要打开的一叠。只显示 `stack.current` 那一页，其余页藏着，不另开窗口。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackRestore {
    pub stack: PageStack,
    pub geometry: FloatGeometry,
    pub dock: Option<FloatDock>,
}

impl StackRestore {
    /// 看得见的那一页。
    #[must_use]
    pub fn visible(&self) -> &StackPage {
        &self.stack.pages[self.stack.current]
    }

    /// 仍在这一叠里、但不另开窗口的页。
    #[must_use]
    pub fn parked(&self) -> Vec<StackPage> {
        self.stack
            .pages
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.stack.current)
            .map(|(_, page)| page.clone())
            .collect()
    }

    /// 把手上或窗口下方要画的点数。一页是 0。
    #[must_use]
    pub fn dot_count(&self) -> usize {
        handle_dot_count(self.stack.pages.len())
    }

    /// 收起时点画在把手上。
    #[must_use]
    pub fn dots_on_handle(&self) -> bool {
        self.dock.is_some_and(|dock| dock.collapsed) && self.dot_count() > 0
    }

    /// 展开时点画在窗口下方，不画在把手上。
    #[must_use]
    pub fn dots_below_window(&self) -> bool {
        !self.dock.is_some_and(|dock| dock.collapsed) && self.dot_count() > 0
    }
}

#[must_use]
pub fn dock_merge_gap(dpi: u32) -> i32 {
    to_physical(DOCK_MERGE_GAP_LOGICAL, dpi).max(0)
}

/// 相交面积严格大于较小那个矩形的一半。空矩形不算。相等的一半不算。
///
/// 用较小的那一个做分母是当前做法，待定。
#[must_use]
pub fn overlaps_more_than_half(a: PxBox, b: PxBox) -> bool {
    let area_a = box_area(a);
    let area_b = box_area(b);
    if area_a == 0 || area_b == 0 {
        return false;
    }
    let inter = intersection_area(a, b);
    inter.saturating_mul(2) > area_a.min(area_b)
}

/// 把手上要画的点数。一页不画。超过 [`HANDLE_DOT_CAP`] 时只画那么多。待定。
#[must_use]
pub fn handle_dot_count(pages: usize) -> usize {
    if pages < 2 {
        0
    } else {
        pages.min(HANDLE_DOT_CAP)
    }
}

/// 把 `dragged` 接在 `target` 后面。已经在 `target` 里的页不重复。
///
/// 位置和贴边由调用方沿用 `target` 的，不在这里。当前页改成 `dragged` 当时显示的那一页。
/// 叠的 id 沿用 `target`。这些都待定。
#[must_use]
pub fn merge_stacks(target: PageStack, dragged: PageStack) -> PageStack {
    let dragged_current = dragged
        .pages
        .get(dragged.current)
        .cloned()
        .or_else(|| dragged.pages.first().cloned());
    let mut pages = target.pages;
    for page in dragged.pages {
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    let current = dragged_current
        .as_ref()
        .and_then(|page| pages.iter().position(|item| item == page))
        .unwrap_or(0);
    PageStack {
        id: target.id,
        pages,
        current,
    }
}

/// 拆出 `index` 那一页。下标越界时原样返回，`removed` 用当前页顶上，避免调用方崩。
#[must_use]
pub fn detach_page(stack: &PageStack, index: usize) -> DetachOutcome {
    if stack.pages.is_empty() {
        return DetachOutcome {
            removed: StackPage::Note(String::new()),
            remaining: None,
            leftover: None,
        };
    }
    let index = index.min(stack.pages.len() - 1);
    let removed = stack.pages[index].clone();
    let mut pages = stack.pages.clone();
    pages.remove(index);
    let current = if index < stack.current {
        stack.current - 1
    } else if index == stack.current {
        index.min(pages.len().saturating_sub(1))
    } else {
        stack.current
    };
    if pages.len() >= 2 {
        DetachOutcome {
            removed,
            remaining: Some(PageStack {
                id: stack.id.clone(),
                pages,
                current,
            }),
            leftover: None,
        }
    } else {
        DetachOutcome {
            removed,
            remaining: None,
            leftover: pages.into_iter().next(),
        }
    }
}

/// 向左或向右翻一页。已经在一端时返回 `None`。
#[must_use]
pub fn flip_index(stack: &PageStack, dir: Flip) -> Option<usize> {
    match dir {
        Flip::Left => stack.current.checked_sub(1),
        Flip::Right => {
            let next = stack.current.saturating_add(1);
            (next < stack.pages.len()).then_some(next)
        }
    }
}

/// 打不开的页从一叠里拿掉之后还留下什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilteredPages {
    pub pages: Vec<StackPage>,
    /// 原来的当前页还留着时，它在 `pages` 里的新下标。被丢掉时是 `None`。
    pub current: Option<usize>,
    pub dropped: Vec<StackPage>,
}

/// 留下 `can_open` 为真的页。当前页被丢掉时 `current` 是 `None`，调用方不要因此藏起还看得见的页。
#[must_use]
pub fn filter_open_pages(
    stack: &PageStack,
    can_open: impl Fn(&StackPage) -> bool,
) -> FilteredPages {
    let mut pages = Vec::new();
    let mut dropped = Vec::new();
    let mut current = None;
    for (index, page) in stack.pages.iter().enumerate() {
        if can_open(page) {
            if index == stack.current {
                current = Some(pages.len());
            }
            pages.push(page.clone());
        } else {
            dropped.push(page.clone());
        }
    }
    FilteredPages {
        pages,
        current,
        dropped,
    }
}

/// 目标页确实显示出来之后，才藏其余页。显示失败时一张都不藏。
#[must_use]
pub fn pages_to_park(pages: &[StackPage], shown: &StackPage, shown_ok: bool) -> Vec<StackPage> {
    if !shown_ok {
        return Vec::new();
    }
    pages
        .iter()
        .filter(|page| *page != shown)
        .cloned()
        .collect()
}

/// 两个都收起、贴在同一块屏幕的同一条边上，沿边和垂直于边的间隔都不超过 `max_gap`。
///
/// 碰上和重叠的间隔算 0。把手中心不在任何一块已知屏幕上时不合。上下叠放的两块屏幕、左右并排的两块屏幕，即使沿边数字很近，也不是同一条边。
#[must_use]
pub fn collapsed_handles_close(
    a: PxBox,
    a_edge: DockEdge,
    b: PxBox,
    b_edge: DockEdge,
    max_gap: i32,
    screens: &[ScreenBounds],
) -> bool {
    if a_edge != b_edge {
        return false;
    }
    let (along, across) = match a_edge {
        DockEdge::Left | DockEdge::Right => (
            span_gap(a.y, a.bottom(), b.y, b.bottom()),
            span_gap(a.x, a.right(), b.x, b.right()),
        ),
        DockEdge::Top => (
            span_gap(a.x, a.right(), b.x, b.right()),
            span_gap(a.y, a.bottom(), b.y, b.bottom()),
        ),
    };
    if along > max_gap || across > max_gap {
        return false;
    }
    match (screen_of(screens, a), screen_of(screens, b)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn span_gap(a0: i32, a1: i32, b0: i32, b1: i32) -> i32 {
    if a1 < b0 {
        b0.saturating_sub(a1)
    } else if b1 < a0 {
        a0.saturating_sub(b1)
    } else {
        0
    }
}

fn screen_of(screens: &[ScreenBounds], rect: PxBox) -> Option<usize> {
    let width = i32::try_from(rect.width).unwrap_or(0);
    let height = i32::try_from(rect.height).unwrap_or(0);
    let x = rect.x.saturating_add(width / 2);
    let y = rect.y.saturating_add(height / 2);
    screens.iter().position(|screen| {
        x >= screen.left && x < screen.right && y >= screen.top && y < screen.bottom
    })
}

/// 松手时先看有没有盖住另一个悬浮窗超过一半，再决定拆页还是整叠移动。
///
/// 页点拖动只带走被按住的那一页。标题栏始终带走整叠，离开原处也不拆页。
/// 收起的把手拖动不拆页。贴边优先于拆页，整叠一起贴。这些次序里，标题栏不拆页是规格；
/// 其余次序待定。
#[must_use]
pub fn drop_effect(input: &DropInput) -> DropEffect {
    if !input.moved {
        return DropEffect::MoveTogether;
    }
    if let Some(target) = best_overlap(&input.released, &input.others) {
        return match input.origin {
            DragOrigin::PageDot => DropEffect::MergeCurrent { target },
            DragOrigin::TitleBar => DropEffect::MergeUnit { target },
        };
    }
    if input.origin == DragOrigin::PageDot {
        if input.page_count >= 2 && !overlaps_more_than_half(input.released, input.start) {
            return DropEffect::DetachCurrent;
        }
        return DropEffect::MoveTogether;
    }
    DropEffect::MoveTogether
}

/// 按 `floatStack` 还原叠。同一 id 至少两页才算一叠。页按 `index` 排，相同则按页的种类和 id。
///
/// 多个 `current` 时留排序后的第一个。一个都没有时当前页是第一页。只出现一次的 id 不算一叠。
/// 这些都待定。
#[must_use]
pub fn assemble_stacks(members: &[StackMember]) -> Vec<PageStack> {
    let mut groups: Vec<Vec<&StackMember>> = Vec::new();
    for member in members {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group[0].slot.id == member.slot.id)
        {
            group.push(member);
        } else {
            groups.push(vec![member]);
        }
    }
    let mut stacks = Vec::new();
    for mut group in groups {
        group.sort_by(|left, right| {
            left.slot
                .index
                .cmp(&right.slot.index)
                .then_with(|| page_key(&left.page).cmp(&page_key(&right.page)))
        });
        let id = group[0].slot.id.clone();
        let mut pages = Vec::new();
        let mut current = None;
        for member in group {
            if pages.contains(&member.page) {
                continue;
            }
            if member.slot.current && current.is_none() {
                current = Some(pages.len());
            }
            pages.push(member.page.clone());
        }
        if pages.len() < 2 {
            continue;
        }
        stacks.push(PageStack {
            id,
            pages,
            current: current.unwrap_or(0),
        });
    }
    stacks.sort_by(|left, right| left.id.cmp(&right.id));
    stacks
}

/// 启动时把悬浮页按 `floatStack.id` 合成叠。
///
/// 页按 `index` 排，相同则按页的种类和 id。序号有空档也全部留下。同一页出现多次只留排序后的第一次。
/// 正好一个 `current` 时显示那一页。缺了或有多个时，显示记下的 `index` 为 0 的那一页；没有就显示排序后的第一页。
/// 凑不齐两页、或整叠都没有位置时，不打开。位置优先用当前页的，当前页没有就用排序后第一份有位置的。
#[must_use]
pub fn plan_startup(pages: &[StartupPage]) -> Vec<StackRestore> {
    let mut groups: Vec<Vec<&StartupPage>> = Vec::new();
    for page in pages {
        if page.slot.id.trim().is_empty() {
            continue;
        }
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group[0].slot.id == page.slot.id)
        {
            group.push(page);
        } else {
            groups.push(vec![page]);
        }
    }
    let mut restores = Vec::new();
    for mut group in groups {
        group.sort_by(|left, right| {
            left.slot
                .index
                .cmp(&right.slot.index)
                .then_with(|| page_key(&left.page).cmp(&page_key(&right.page)))
        });
        let mut kept = Vec::new();
        for page in group {
            if kept
                .iter()
                .any(|kept: &&StartupPage| kept.page == page.page)
            {
                continue;
            }
            kept.push(page);
        }
        if kept.len() < 2 {
            continue;
        }
        let current = startup_current(&kept);
        let Some((geometry, dock)) = startup_placement(&kept, current) else {
            continue;
        };
        let id = kept[0].slot.id.clone();
        restores.push(StackRestore {
            stack: PageStack {
                id,
                pages: kept.into_iter().map(|page| page.page.clone()).collect(),
                current,
            },
            geometry,
            dock,
        });
    }
    restores.sort_by(|left, right| left.stack.id.cmp(&right.stack.id));
    restores
}

/// 正好一个 `current` 用那一页。缺了或重复时回到 `index` 为 0 的页，没有就用排序后的第一页。
fn startup_current(pages: &[&StartupPage]) -> usize {
    let marked: Vec<usize> = pages
        .iter()
        .enumerate()
        .filter(|(_, page)| page.slot.current)
        .map(|(index, _)| index)
        .collect();
    if marked.len() == 1 {
        return marked[0];
    }
    pages
        .iter()
        .position(|page| page.slot.index == 0)
        .unwrap_or(0)
}

fn startup_placement(
    pages: &[&StartupPage],
    current: usize,
) -> Option<(FloatGeometry, Option<FloatDock>)> {
    let chosen = pages.get(current).filter(|page| page.geometry.is_some());
    let page = chosen.or_else(|| pages.iter().find(|page| page.geometry.is_some()))?;
    Some((page.geometry?, page.dock))
}

/// 启动还原写进 `logs/app.log` 的那一行。
#[must_use]
pub fn restore_log_line(id: &str, pages: usize, current: usize) -> String {
    format!("叠页已还原 id={id} pages={pages} current={current}")
}

/// 把一叠写成每页的 `floatStack`。下标就是页序，当前页的 `current` 为真。
#[must_use]
pub fn slots_for(stack: &PageStack) -> Vec<(StackPage, FloatStackSlot)> {
    stack
        .pages
        .iter()
        .enumerate()
        .map(|(index, page)| {
            (
                page.clone(),
                FloatStackSlot {
                    id: stack.id.clone(),
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                    current: index == stack.current,
                },
            )
        })
        .collect()
}

/// 页点那一行的物理宽高。一页不占地方，调用方在少于两页时不要显示。
/// 超过 [`HANDLE_DOT_CAP`] 时宽度只按那么多个槽算。待定。
#[must_use]
pub fn dot_row_size(pages: usize, dpi: u32) -> (u32, u32) {
    let count = handle_dot_count(pages).max(1);
    let slot = u32::try_from(to_physical(DOT_SLOT_LOGICAL, dpi).max(1)).unwrap_or(1);
    let height = u32::try_from(to_physical(DOT_ROW_LOGICAL_HEIGHT, dpi).max(1)).unwrap_or(1);
    let slots = u32::try_from(count).unwrap_or(1);
    (slot.saturating_mul(slots).max(1), height)
}

/// 页点行挂在窗口外面，水平居中。优先贴着底边往下；底边放不下而顶上放得下时改挂在上边外面。
/// 改挂到上边是当前做法，待定。
#[must_use]
pub fn place_dots(
    frame: PxBox,
    row_width: u32,
    row_height: u32,
    screen_top: Option<i32>,
    screen_bottom: Option<i32>,
) -> PxBox {
    let width = row_width.max(1);
    let height = row_height.max(1);
    let frame_width = i32::try_from(frame.width).unwrap_or(0);
    let row = i32::try_from(width).unwrap_or(0);
    let x = frame.x.saturating_add(frame_width.saturating_sub(row) / 2);
    let below = frame.bottom();
    let y = match (screen_top, screen_bottom) {
        (Some(top), Some(bottom))
            if below.saturating_add_unsigned(height) > bottom
                && frame.y.saturating_sub_unsigned(height) >= top =>
        {
            frame.y.saturating_sub_unsigned(height)
        }
        _ => below,
    };
    PxBox {
        x,
        y,
        width,
        height,
    }
}

/// 页点行收进工作区。垂直方向先按 [`place_dots`] 决定挂在下面还是上面，再把整行挪进工作区。
/// `work` 为空时只按 [`place_dots`] 挂在下面。
#[must_use]
pub fn fit_dot_row(frame: PxBox, row_width: u32, row_height: u32, work: Option<WorkArea>) -> PxBox {
    let placed = place_dots(
        frame,
        row_width,
        row_height,
        work.map(|area| area.top),
        work.map(|area| area.bottom),
    );
    let Some(area) = work else {
        return placed;
    };
    let width = fit_len(placed.width, area.width());
    let height = fit_len(placed.height, area.height());
    PxBox {
        x: clamp_axis(placed.x, width, area.left, area.right),
        y: clamp_axis(placed.y, height, area.top, area.bottom),
        width,
        height,
    }
}

fn fit_len(want: u32, span: i32) -> u32 {
    let span = u32::try_from(span.max(0)).unwrap_or(0);
    let want = want.max(1);
    if span == 0 { want } else { want.min(span) }
}

fn clamp_axis(start: i32, length: u32, low: i32, high: i32) -> i32 {
    let last = i64::from(high) - i64::from(length);
    let clamped = i64::from(start).min(last).max(i64::from(low));
    i32::try_from(clamped).unwrap_or(low)
}

fn best_overlap(released: &PxBox, others: &[OtherFloat]) -> Option<StackPage> {
    let mut best: Option<(u128, usize, StackPage)> = None;
    for (index, other) in others.iter().enumerate() {
        if !overlaps_more_than_half(*released, other.rect) {
            continue;
        }
        let area = intersection_area(*released, other.rect);
        let replace = match &best {
            None => true,
            Some((best_area, best_index, _)) => {
                area > *best_area || (area == *best_area && index < *best_index)
            }
        };
        if replace {
            best = Some((area, index, other.page.clone()));
        }
    }
    best.map(|(_, _, page)| page)
}

fn page_key(page: &StackPage) -> (&str, &str) {
    match page {
        StackPage::Note(id) => ("note", id.as_str()),
        StackPage::Shelf(id) => ("shelf", id.as_str()),
    }
}

fn box_area(rect: PxBox) -> u128 {
    u128::from(rect.width) * u128::from(rect.height)
}

fn intersection_area(a: PxBox, b: PxBox) -> u128 {
    let width = overlap_span(a.x, a.right(), b.x, b.right());
    let height = overlap_span(a.y, a.bottom(), b.y, b.bottom());
    width * height
}

fn overlap_span(a0: i32, a1: i32, b0: i32, b1: i32) -> u128 {
    let start = i64::from(a0).max(i64::from(b0));
    let end = i64::from(a1).min(i64::from(b1));
    u128::try_from((end - start).max(0)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PxBox {
        PxBox {
            x,
            y,
            width,
            height,
        }
    }

    fn note(id: &str) -> StackPage {
        StackPage::Note(id.to_owned())
    }

    fn shelf(id: &str) -> StackPage {
        StackPage::Shelf(id.to_owned())
    }

    fn stack(id: &str, pages: Vec<StackPage>, current: usize) -> PageStack {
        PageStack {
            id: id.to_owned(),
            pages,
            current,
        }
    }

    fn slot(id: &str, index: u32, current: bool) -> FloatStackSlot {
        FloatStackSlot {
            id: id.to_owned(),
            index,
            current,
        }
    }

    fn other(page: StackPage, rect: PxBox) -> OtherFloat {
        OtherFloat { page, rect }
    }

    #[allow(clippy::too_many_arguments)]
    fn drop_at(
        origin: DragOrigin,
        start: PxBox,
        released: PxBox,
        moved: bool,
        from_collapsed: bool,
        page_count: usize,
        snapped: bool,
        others: Vec<OtherFloat>,
    ) -> DropEffect {
        drop_effect(&DropInput {
            origin,
            start,
            released,
            moved,
            from_collapsed,
            page_count,
            snapped,
            others,
        })
    }

    #[test]
    fn more_than_half_uses_the_smaller_rect_and_a_tie_does_not_count() {
        let base = rect(0, 0, 100, 100);
        assert!(overlaps_more_than_half(base, rect(0, 0, 100, 60)));
        assert!(!overlaps_more_than_half(base, rect(0, 50, 100, 100)));
        assert!(!overlaps_more_than_half(base, rect(50, 0, 100, 100)));
        assert!(!overlaps_more_than_half(base, rect(200, 0, 100, 100)));
        assert!(!overlaps_more_than_half(base, rect(0, 0, 0, 100)));
        assert!(overlaps_more_than_half(
            rect(0, 0, 200, 200),
            rect(10, 10, 40, 40)
        ));
        assert!(!overlaps_more_than_half(
            rect(0, 0, 200, 200),
            rect(180, 180, 40, 40)
        ));
    }

    #[test]
    fn merge_appends_notes_and_shelves_and_shows_the_dragged_page() {
        let target = stack("s", vec![note("a"), shelf("g")], 0);
        let dragged = stack("t", vec![note("b"), note("a")], 0);
        let merged = merge_stacks(target, dragged);
        assert_eq!(merged.id, "s");
        assert_eq!(
            merged.pages,
            vec![note("a"), shelf("g"), note("b")],
            "已经在叠里的页不重复"
        );
        assert_eq!(merged.current, 2, "当前页是拖过来的那一页");
    }

    #[test]
    fn detach_removes_the_page_and_a_single_leftover_is_not_a_stack() {
        let pages = stack("s", vec![note("a"), shelf("g"), note("b")], 1);
        let mid = detach_page(&pages, 1);
        assert_eq!(mid.removed, shelf("g"));
        let rest = mid.remaining.expect("还剩两页");
        assert_eq!(rest.pages, vec![note("a"), note("b")]);
        assert_eq!(rest.current, 1, "拆掉当前页后停在原来的下一页");

        let last = detach_page(&rest, 1);
        assert!(last.remaining.is_none());
        assert_eq!(last.leftover, Some(note("a")));
        assert_eq!(last.removed, note("b"));

        let head = detach_page(&pages, 0);
        assert_eq!(head.remaining.unwrap().current, 0);
    }

    #[test]
    fn arrows_stop_at_the_ends() {
        let pages = stack("s", vec![note("a"), note("b"), shelf("g")], 1);
        assert_eq!(flip_index(&pages, Flip::Left), Some(0));
        assert_eq!(flip_index(&pages, Flip::Right), Some(2));
        let at_start = stack("s", pages.pages.clone(), 0);
        assert_eq!(flip_index(&at_start, Flip::Left), None);
        let at_end = stack("s", pages.pages, 2);
        assert_eq!(flip_index(&at_end, Flip::Right), None);
    }

    #[test]
    fn title_drag_merges_the_whole_stack_and_never_detaches_a_page() {
        let start = rect(0, 0, 200, 200);
        let target = other(shelf("g"), rect(20, 20, 200, 200));
        let merged = drop_at(
            DragOrigin::TitleBar,
            start,
            rect(30, 30, 200, 200),
            true,
            false,
            1,
            false,
            vec![target],
        );
        assert_eq!(merged, DropEffect::MergeUnit { target: shelf("g") });

        let away = drop_at(
            DragOrigin::TitleBar,
            start,
            rect(400, 0, 200, 200),
            true,
            false,
            3,
            false,
            Vec::new(),
        );
        assert_eq!(away, DropEffect::MoveTogether, "标题栏离开原处也不拆页");

        let nudged = drop_at(
            DragOrigin::TitleBar,
            start,
            rect(20, 0, 200, 200),
            true,
            false,
            3,
            false,
            Vec::new(),
        );
        assert_eq!(nudged, DropEffect::MoveTogether);

        let snapped = drop_at(
            DragOrigin::TitleBar,
            start,
            rect(400, 0, 200, 200),
            true,
            false,
            3,
            true,
            Vec::new(),
        );
        assert_eq!(snapped, DropEffect::MoveTogether, "贴边时整叠一起走");

        let handle = drop_at(
            DragOrigin::TitleBar,
            rect(0, 100, 9, 48),
            rect(80, 100, 9, 48),
            true,
            true,
            3,
            false,
            Vec::new(),
        );
        assert_eq!(handle, DropEffect::MoveTogether, "收起的把手不拆页");

        let still = drop_at(
            DragOrigin::TitleBar,
            start,
            start,
            false,
            false,
            3,
            false,
            Vec::new(),
        );
        assert_eq!(still, DropEffect::MoveTogether);
    }

    #[test]
    fn a_page_dot_detaches_that_page_unless_it_is_dropped_onto_another_float() {
        let start = rect(0, 0, 200, 200);
        let away = drop_at(
            DragOrigin::PageDot,
            start,
            rect(300, 0, 200, 200),
            true,
            false,
            2,
            false,
            Vec::new(),
        );
        assert_eq!(away, DropEffect::DetachCurrent);

        let back = drop_at(
            DragOrigin::PageDot,
            start,
            rect(10, 0, 200, 200),
            true,
            false,
            2,
            false,
            Vec::new(),
        );
        assert_eq!(back, DropEffect::MoveTogether, "页点落回原处不拆");

        let onto = drop_at(
            DragOrigin::PageDot,
            start,
            rect(500, 0, 180, 180),
            true,
            false,
            2,
            false,
            vec![other(note("z"), rect(500, 0, 180, 180))],
        );
        assert_eq!(onto, DropEffect::MergeCurrent { target: note("z") });
    }

    #[test]
    fn the_larger_overlap_wins_when_two_floats_are_both_past_half() {
        let released = rect(0, 0, 100, 100);
        let effect = drop_at(
            DragOrigin::TitleBar,
            rect(0, 0, 100, 100),
            released,
            true,
            false,
            1,
            false,
            vec![
                other(note("small"), rect(0, 0, 100, 60)),
                other(shelf("full"), rect(0, 0, 100, 100)),
            ],
        );
        assert_eq!(
            effect,
            DropEffect::MergeUnit {
                target: shelf("full")
            }
        );
    }

    fn screen(left: i32, top: i32, right: i32, bottom: i32) -> ScreenBounds {
        ScreenBounds {
            left,
            top,
            right,
            bottom,
            dpi: 96,
        }
    }

    #[test]
    fn collapsed_handles_on_the_same_edge_merge_only_when_they_are_close() {
        let area = [screen(-100, -100, 4000, 4000)];
        let left = rect(0, 100, 9, 48);
        assert!(collapsed_handles_close(
            left,
            DockEdge::Left,
            rect(0, 160, 9, 48),
            DockEdge::Left,
            48,
            &area
        ));
        assert!(collapsed_handles_close(
            left,
            DockEdge::Left,
            rect(0, 100, 9, 48),
            DockEdge::Left,
            48,
            &area
        ));
        assert!(!collapsed_handles_close(
            left,
            DockEdge::Left,
            rect(0, 220, 9, 48),
            DockEdge::Left,
            48,
            &area
        ));
        assert!(!collapsed_handles_close(
            left,
            DockEdge::Left,
            rect(1900, 100, 9, 48),
            DockEdge::Right,
            48,
            &area
        ));
        assert!(!collapsed_handles_close(
            left,
            DockEdge::Left,
            rect(200, 100, 9, 48),
            DockEdge::Left,
            48,
            &area
        ));
        assert!(collapsed_handles_close(
            rect(40, 0, 48, 9),
            DockEdge::Top,
            rect(80, 0, 48, 9),
            DockEdge::Top,
            12,
            &area
        ));
        let stacked = [screen(0, 0, 1920, 1080), screen(0, 1080, 1920, 2160)];
        assert!(
            !collapsed_handles_close(
                rect(0, 1032, 9, 48),
                DockEdge::Left,
                rect(0, 1080, 9, 48),
                DockEdge::Left,
                48,
                &stacked
            ),
            "上下两块屏幕的左边不算同一条边"
        );
        let side_by_side = [screen(0, 0, 1920, 1080), screen(1920, 0, 3840, 1080)];
        assert!(
            !collapsed_handles_close(
                rect(0, 100, 9, 48),
                DockEdge::Left,
                rect(1920, 100, 9, 48),
                DockEdge::Left,
                48,
                &side_by_side
            ),
            "并排两块屏幕的左边不合"
        );
        assert!(!collapsed_handles_close(
            left,
            DockEdge::Left,
            rect(0, 160, 9, 48),
            DockEdge::Left,
            48,
            &[]
        ));
        assert_eq!(dock_merge_gap(96), 48);
        assert_eq!(dock_merge_gap(144), 72);
    }

    #[test]
    fn pages_that_cannot_open_drop_out_and_a_failed_show_parks_nothing() {
        let trashed = stack("s", vec![note("a"), note("trash"), note("c")], 1);
        let filtered = filter_open_pages(&trashed, |page| page != &note("trash"));
        assert_eq!(filtered.pages, vec![note("a"), note("c")]);
        assert!(filtered.current.is_none(), "当前页打不开时不另指一页");
        assert_eq!(filtered.dropped, vec![note("trash")]);

        let later = stack("s", vec![note("a"), note("trash"), note("c")], 2);
        let kept = filter_open_pages(&later, |page| page != &note("trash"));
        assert_eq!(kept.pages, vec![note("a"), note("c")]);
        assert_eq!(kept.current, Some(1));

        let show = note("c");
        assert!(pages_to_park(&kept.pages, &show, false).is_empty());
        assert_eq!(pages_to_park(&kept.pages, &show, true), vec![note("a")]);
    }

    #[test]
    fn old_slots_assemble_and_a_lone_or_blank_id_does_not_make_a_stack() {
        assert!(FloatStackSlot::from_json(&serde_json::json!(null)).is_none());
        assert!(FloatStackSlot::from_json(&serde_json::json!({"id":"  "})).is_none());
        assert!(FloatStackSlot::from_json(&serde_json::json!("s")).is_none());
        let parsed = FloatStackSlot::from_json(&serde_json::json!({
            "id": "s1",
            "index": 2,
            "current": true,
            "extra": 1
        }))
        .expect("多出来的字段不影响");
        assert_eq!(parsed, slot("s1", 2, true));
        let missing_flags = FloatStackSlot::from_json(&serde_json::json!({"id": "s1"})).unwrap();
        assert_eq!(missing_flags, slot("s1", 0, false));

        let members = vec![
            StackMember {
                page: note("b"),
                slot: slot("s", 2, false),
            },
            StackMember {
                page: shelf("g"),
                slot: slot("s", 0, false),
            },
            StackMember {
                page: note("a"),
                slot: slot("s", 1, true),
            },
            StackMember {
                page: note("solo"),
                slot: slot("only", 0, true),
            },
            StackMember {
                page: note("a"),
                slot: slot("s", 9, true),
            },
        ];
        let stacks = assemble_stacks(&members);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].pages, vec![shelf("g"), note("a"), note("b")]);
        assert_eq!(stacks[0].current, 1, "重复的 current 留排序后的第一个");

        let none_current = assemble_stacks(&[
            StackMember {
                page: note("a"),
                slot: slot("t", 1, false),
            },
            StackMember {
                page: note("b"),
                slot: slot("t", 0, false),
            },
        ]);
        assert_eq!(none_current[0].pages, vec![note("b"), note("a")]);
        assert_eq!(none_current[0].current, 0);

        let written = slots_for(&stacks[0]);
        assert!(!written[0].1.current);
        assert_eq!(written[1].1.index, 1);
        assert!(written[1].1.current);
        let again = assemble_stacks(
            &written
                .into_iter()
                .map(|(page, slot)| StackMember { page, slot })
                .collect::<Vec<_>>(),
        );
        assert_eq!(again, stacks);
    }

    #[test]
    fn the_dot_row_is_centered_under_the_frame_and_the_handle_dots_follow_the_page_count() {
        let frame = rect(10, 20, 200, 100);
        let (width, height) = dot_row_size(3, 96);
        assert_eq!(width, 3 * DOT_SLOT_LOGICAL);
        assert_eq!(height, DOT_ROW_LOGICAL_HEIGHT);
        let below = place_dots(frame, width, height, Some(0), Some(1080));
        assert_eq!(below.y, frame.bottom());
        assert_eq!(
            below.x,
            frame.x + (i32::try_from(frame.width).unwrap() - i32::try_from(width).unwrap()) / 2
        );
        assert!(below.y >= frame.bottom());
        let screen_bottom = frame.bottom() + 4;
        let above = place_dots(frame, width, height, Some(0), Some(screen_bottom));
        assert_eq!(above.y + i32::try_from(above.height).unwrap(), frame.y);
        assert_eq!(above.x, below.x);
        assert_eq!(dot_row_size(1, 96).0, DOT_SLOT_LOGICAL);
        assert_eq!(dot_row_size(40, 144).0, {
            let slot = u32::try_from(crate::shell::to_physical(DOT_SLOT_LOGICAL, 144)).unwrap();
            slot * u32::try_from(HANDLE_DOT_CAP).unwrap()
        });
        assert_eq!(handle_dot_count(1), 0);
        assert_eq!(handle_dot_count(3), 3);
        assert_eq!(handle_dot_count(40), HANDLE_DOT_CAP);
    }

    #[test]
    fn a_dot_row_wider_than_the_frame_stays_inside_the_work_area() {
        let frame = rect(1800, 900, 100, 160);
        let work = crate::shell::WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let placed = fit_dot_row(frame, 192, 16, Some(work));
        assert!(placed.x >= work.left);
        assert!(placed.x + i32::try_from(placed.width).unwrap() <= work.right);
        assert!(placed.y >= work.top);
        assert!(placed.y + i32::try_from(placed.height).unwrap() <= work.bottom);
        assert_eq!(placed.width, 192);
        assert_eq!(placed.x, 1920 - 192);
        assert_eq!(
            placed.y + i32::try_from(placed.height).unwrap(),
            frame.y,
            "底边放不下时改挂到框的上方"
        );
    }

    fn startup_page(page: StackPage, index: u32, current: bool, collapsed: bool) -> StartupPage {
        StartupPage {
            page,
            slot: slot("s1a1324ccd0d1", index, current),
            geometry: Some(FloatGeometry {
                x: 1100,
                y: 40,
                width: 320,
                height: 360,
            }),
            dock: Some(FloatDock {
                edge: DockEdge::Top,
                offset: 1227,
                collapsed,
            }),
        }
    }

    fn write_doc(store: &crate::storage::Store, note: bool, id: &str, body: &str) {
        use crate::storage::DocumentId;
        let doc = if note {
            DocumentId::Note(id.to_owned())
        } else {
            DocumentId::Shelf(id.to_owned())
        };
        let path = store.document_path(&doc).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// #97 之前写在磁盘上的那一叠：两格收纳、一篇便签，贴在上边并且收起。
    fn pre97_collapsed_stack(store: &crate::storage::Store) -> Vec<StartupPage> {
        use crate::notes::NoteCommands;
        use crate::shelves::ShelfCommands;

        let window = r#""floatWindow":{"x":1100,"y":40,"width":320,"height":360}"#;
        let dock = r#""floatDock":{"edge":"top","offset":1227,"collapsed":true}"#;
        let stack = |index: u32, current: bool| {
            format!(r#""floatStack":{{"id":"s1a1324ccd0d1","index":{index},"current":{current}}}"#)
        };
        write_doc(
            store,
            false,
            "s1a1324c226e1",
            &format!(
                r#"{{"schemaVersion":1,"id":"s1a1324c226e1","name":"甲","order":0,"refs":[],{window},{dock},{}}}"#,
                stack(0, false)
            ),
        );
        write_doc(
            store,
            false,
            "s1a12ce651ca1",
            &format!(
                r#"{{"schemaVersion":1,"id":"s1a12ce651ca1","name":"乙","order":1,"refs":[],{window},{dock},{}}}"#,
                stack(1, true)
            ),
        );
        write_doc(
            store,
            true,
            "n1a12cdd97661",
            &format!(
                r#"{{"schemaVersion":1,"id":"n1a12cdd97661","title":"记","body":"正文","revision":1,{window},{dock},{}}}"#,
                stack(2, false)
            ),
        );
        write_doc(
            store,
            true,
            "solo",
            &format!(
                r#"{{"schemaVersion":1,"id":"solo","title":"单独","body":"不在叠里","revision":1,{window},{dock}}}"#
            ),
        );
        let notes = NoteCommands::open(store.clone()).unwrap();
        let shelves = ShelfCommands::open(store.clone()).unwrap();
        let mut pages = Vec::new();
        for note in notes.list() {
            if let Some(slot) = note.float_stack {
                pages.push(StartupPage {
                    page: StackPage::Note(note.id),
                    slot,
                    geometry: note.float,
                    dock: note.float_dock,
                });
            }
        }
        for group in shelves.list().unwrap() {
            if let Some(slot) = group.float_stack {
                pages.push(StartupPage {
                    page: StackPage::Shelf(group.id),
                    slot,
                    geometry: group.float,
                    dock: group.float_dock,
                });
            }
        }
        pages
    }

    #[test]
    fn a_collapsed_top_docked_mixed_stack_restores_as_one_window() {
        use crate::storage::test_temp::TempDir;
        use crate::storage::{Store, StorePaths};

        let temp = TempDir::new();
        let root = temp.path();
        let store = Store::open(StorePaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
        })
        .unwrap();
        let pages = pre97_collapsed_stack(&store);
        let planned = plan_startup(&pages);
        assert_eq!(planned.len(), 1, "一叠只还原成一个窗口");
        let restored = &planned[0];
        assert_eq!(
            restored.stack.pages,
            vec![
                shelf("s1a1324c226e1"),
                shelf("s1a12ce651ca1"),
                note("n1a12cdd97661"),
            ]
        );
        assert_eq!(restored.visible(), &shelf("s1a12ce651ca1"));
        assert_eq!(
            restored.parked(),
            vec![shelf("s1a1324c226e1"), note("n1a12cdd97661")],
            "其余页藏着，不另开窗口，也不丢掉"
        );
        assert_eq!(restored.dock.map(|dock| dock.edge), Some(DockEdge::Top));
        assert_eq!(restored.dock.map(|dock| dock.offset), Some(1227));
        assert!(restored.dots_on_handle(), "收起的把手上画页点");
        assert!(!restored.dots_below_window());
        assert_eq!(restored.dot_count(), 3);

        let expanded = pages
            .iter()
            .map(|page| {
                let mut page = page.clone();
                if let Some(dock) = page.dock.as_mut() {
                    dock.collapsed = false;
                }
                page
            })
            .collect::<Vec<_>>();
        let open = plan_startup(&expanded);
        assert!(open[0].dots_below_window(), "展开时页点在窗口下方");
        assert!(!open[0].dots_on_handle());
        let frame = rect(
            open[0].geometry.x,
            open[0].geometry.y,
            open[0].geometry.width,
            open[0].geometry.height,
        );
        let (width, height) = dot_row_size(open[0].stack.pages.len(), 96);
        let below = place_dots(frame, width, height, Some(0), Some(10_000));
        assert_eq!(below.y, frame.bottom(), "页点行贴在内容框下面");

        let line = restore_log_line(
            &restored.stack.id,
            restored.stack.pages.len(),
            restored.stack.current,
        );
        store.log_info(&line);
        let path = store.log_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("app.log")
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains(&line), "app.log 仍写着还原记录：{text}");
    }

    #[test]
    fn startup_falls_back_when_current_is_missing_or_repeated_and_keeps_index_gaps() {
        let missing = plan_startup(&[
            startup_page(shelf("a"), 5, false, true),
            startup_page(note("b"), 2, false, true),
            startup_page(shelf("c"), 0, false, true),
        ]);
        assert_eq!(missing.len(), 1);
        assert_eq!(
            missing[0].stack.pages,
            vec![shelf("c"), note("b"), shelf("a")],
            "index 有空隙也按序留下"
        );
        assert_eq!(missing[0].stack.current, 0, "没有 current 时回到 index 0");

        let repeated = plan_startup(&[
            startup_page(note("n"), 4, true, true),
            startup_page(shelf("s0"), 0, false, true),
            startup_page(shelf("s1"), 1, true, true),
        ]);
        assert_eq!(
            repeated[0].stack.pages,
            vec![shelf("s0"), shelf("s1"), note("n")]
        );
        assert_eq!(
            repeated[0].visible(),
            &shelf("s0"),
            "重复的 current 回到 index 0"
        );
        assert_eq!(repeated[0].parked().len(), 2);

        let gapped = plan_startup(&[
            StartupPage {
                page: note("late"),
                slot: slot("gap", 8, false),
                geometry: None,
                dock: None,
            },
            StartupPage {
                page: shelf("early"),
                slot: slot("gap", 3, false),
                geometry: Some(FloatGeometry {
                    x: 1,
                    y: 2,
                    width: 200,
                    height: 100,
                }),
                dock: Some(FloatDock {
                    edge: DockEdge::Top,
                    offset: 10,
                    collapsed: true,
                }),
            },
        ]);
        assert_eq!(gapped[0].stack.pages, vec![shelf("early"), note("late")]);
        assert_eq!(
            gapped[0].stack.current, 0,
            "没有 index 0 时用排序后的第一页"
        );
        assert_eq!(gapped[0].visible(), &shelf("early"));
        assert_eq!(
            gapped[0].parked(),
            vec![note("late")],
            "没有位置的页也不丢掉"
        );
        assert_eq!(gapped[0].geometry.x, 1);
    }
}
