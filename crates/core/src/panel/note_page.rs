//! 便签页的数据整理：列表行、标签筛选、回收站视图、定位，以及悬浮窗的默认大小和位置。
//!
//! 输入是 [`NoteCommands`](crate::notes::NoteCommands) 的快照。这里不读写盘，不读时钟。
//! 规则仍在便签服务里。列表顺序（置顶在前，其后按更新时间从新到旧）由服务给出，这里不重排回收站以外的顺序。

use super::float_snap::{DockState, PxBox, ScreenBounds, dock_applies};
use crate::notes::{FloatGeometry, Note};
use crate::shell::{WorkArea, to_logical, to_physical};

use super::todo_page::trash_days_left;

/// 便签页当前显示的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteView {
    List,
    Trash,
}

/// 列表一行。文字字段已经是要显示的样子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    pub id: String,
    pub title: String,
    pub pinned: bool,
    /// 标签用顿号连成一行。没有标签时为空。
    pub tags: String,
    /// 回收站行：还剩几天被清除。
    pub trash_note: String,
}

/// 所有未删除便签上出现过的标签，按列表顺序里第一次出现的先后，不重复。
#[must_use]
pub fn tag_names(notes: &[Note]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for tag in notes.iter().flat_map(|note| &note.tags) {
        if !names.contains(tag) {
            names.push(tag.clone());
        }
    }
    names
}

fn row_of(note: &Note, now_ms: i64, in_trash: bool) -> NoteRow {
    NoteRow {
        id: note.id.clone(),
        title: note.display_title().to_owned(),
        pinned: note.pinned,
        tags: note.tags.join("、"),
        trash_note: match note.deleted_at {
            Some(deleted) if in_trash => {
                format!("剩 {} 天", trash_days_left(deleted.as_millis(), now_ms))
            }
            _ => String::new(),
        },
    }
}

/// 列表视图的行。`tag` 不为空时只留带这个标签的便签，整段相等，不是子串。
#[must_use]
pub fn list_rows(notes: &[Note], tag: Option<&str>) -> Vec<NoteRow> {
    notes
        .iter()
        .filter(|note| tag.is_none_or(|tag| note.tags.iter().any(|existing| existing == tag)))
        .map(|note| row_of(note, 0, false))
        .collect()
}

/// 回收站视图的行，最近删除的在前。
#[must_use]
pub fn trash_rows(deleted: &[Note], now_ms: i64) -> Vec<NoteRow> {
    let mut notes: Vec<&Note> = deleted.iter().collect();
    notes.sort_by(|left, right| {
        right
            .deleted_at
            .cmp(&left.deleted_at)
            .then(left.id.cmp(&right.id))
    });
    notes
        .into_iter()
        .map(|note| row_of(note, now_ms, true))
        .collect()
}

/// 搜索结果和托盘要定位到的便签在哪个视图里。找不到时返回 `None`。
#[must_use]
pub fn locate_note(notes: &[Note], deleted: &[Note], id: &str) -> Option<NoteView> {
    if notes.iter().any(|note| note.id == id) {
        Some(NoteView::List)
    } else if deleted.iter().any(|note| note.id == id) {
        Some(NoteView::Trash)
    } else {
        None
    }
}

/// 悬浮窗没有记住的大小时的逻辑尺寸。产品规格没有写，这是实现选择。
pub const FLOAT_WIDTH: u32 = 320;
pub const FLOAT_HEIGHT: u32 = 360;
/// 悬浮窗可以缩到的最小逻辑尺寸。
pub const FLOAT_MIN_WIDTH: u32 = 200;
pub const FLOAT_MIN_HEIGHT: u32 = 120;
/// 没有记住位置时，同时打开的悬浮窗依次错开的逻辑像素，最多错开 [`FLOAT_CASCADE_STEPS`] 次。
pub const FLOAT_CASCADE: u32 = 28;
pub const FLOAT_CASCADE_STEPS: u32 = 8;

/// 没有记住位置时的左上角，物理像素：工作区居中，已经打开 `open` 个悬浮窗时再向右下错开。
#[must_use]
pub fn float_default_origin(
    work: WorkArea,
    width_px: i32,
    height_px: i32,
    dpi: u32,
    open: usize,
) -> (i32, i32) {
    let steps = u32::try_from(open).unwrap_or(u32::MAX) % FLOAT_CASCADE_STEPS;
    let shift = crate::shell::to_physical(FLOAT_CASCADE * steps, dpi);
    (
        work.left + (work.width() - width_px) / 2 + shift,
        work.top + (work.height() - height_px) / 2 + shift,
    )
}

/// 一块显示器的工作区，物理像素，以及这块屏的有效 DPI。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorWork {
    pub area: WorkArea,
    pub dpi: u32,
}

/// 记住的悬浮窗放回屏幕上的结果，物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloatPlacement {
    pub origin: (i32, i32),
    pub size: (u32, u32),
    /// 记住的值放不回原处，已经被拉回工作区或改了大小。
    pub adjusted: bool,
}

/// 把记住的位置和大小放回工作区。`min` 已经是物理像素。
///
/// 悬浮窗实际走 [`contain_float`]：最小尺寸按那块屏的 DPI 从逻辑像素换算，工作区更小时可以小于最小尺寸。
/// 这里的 `min` 由调用方算好。工作区比 `min` 还小时也不小于 `min`，窗口可能仍伸出工作区。
#[must_use]
pub fn restore_float(
    saved: FloatGeometry,
    areas: &[WorkArea],
    fallback: WorkArea,
    min: (u32, u32),
) -> FloatPlacement {
    let width = saved.width.max(min.0);
    let height = saved.height.max(min.1);
    let area = chosen_area(saved.x, saved.y, width, height, areas, fallback);
    let width = width.min(u32::try_from(area.width()).unwrap_or(0).max(min.0));
    let height = height.min(u32::try_from(area.height()).unwrap_or(0).max(min.1));
    place_in(saved, area, width, height)
}

/// 不贴边的悬浮窗整扇留在工作区里。产品规格「便签」的「工作区」。
///
/// 取与窗口重叠面积最大的工作区；都不重叠时用 `fallback`（光标所在显示器）。
/// 最小尺寸是 `min_logical`，按选中这块屏的有效 DPI 换成物理像素。
/// 大小先收到不超过这块工作区；工作区比最小尺寸还小时缩到工作区那么大。再把位置拉回。
/// 已经记下的宽高仍是物理像素，这里不按 DPI 重算它们，只在小于最小尺寸时放大。
#[must_use]
pub fn contain_float(
    saved: FloatGeometry,
    monitors: &[MonitorWork],
    fallback: MonitorWork,
    min_logical: (u32, u32),
) -> FloatPlacement {
    let monitor = chosen_monitor(
        saved.x,
        saved.y,
        saved.width.max(1),
        saved.height.max(1),
        monitors,
        fallback,
    );
    let min_width = physical_len(min_logical.0, monitor.dpi);
    let min_height = physical_len(min_logical.1, monitor.dpi);
    let width = fit_span(saved.width.max(min_width), monitor.area.width());
    let height = fit_span(saved.height.max(min_height), monitor.area.height());
    place_in(saved, monitor.area, width, height)
}

/// 没有贴边，或记下的贴边对不上这些屏幕时，把展开矩形收进工作区。贴边对得上时原样返回。
#[must_use]
pub fn settle_float(
    state: DockState,
    screens: &[ScreenBounds],
    monitors: &[MonitorWork],
    fallback: MonitorWork,
) -> DockState {
    if state.dock.is_some() && dock_applies(&state, screens) {
        return state;
    }
    let placed = contain_float(
        FloatGeometry {
            x: state.expanded.x,
            y: state.expanded.y,
            width: state.expanded.width,
            height: state.expanded.height,
        },
        monitors,
        fallback,
        (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
    );
    DockState {
        expanded: PxBox {
            x: placed.origin.0,
            y: placed.origin.1,
            width: placed.size.0,
            height: placed.size.1,
        },
        dock: None,
    }
}

/// 不贴边的窗口最多能用到的逻辑宽高，就是它所在工作区的逻辑尺寸。
#[must_use]
pub fn logical_work_limits(
    rect: PxBox,
    monitors: &[MonitorWork],
    fallback: MonitorWork,
) -> (u32, u32) {
    let monitor = chosen_monitor(
        rect.x,
        rect.y,
        rect.width.max(1),
        rect.height.max(1),
        monitors,
        fallback,
    );
    (
        to_logical(monitor.area.width(), monitor.dpi).max(1),
        to_logical(monitor.area.height(), monitor.dpi).max(1),
    )
}

fn place_in(saved: FloatGeometry, area: WorkArea, width: u32, height: u32) -> FloatPlacement {
    let x = clamp_start(saved.x, width, area.left, area.right);
    let y = clamp_start(saved.y, height, area.top, area.bottom);
    FloatPlacement {
        origin: (x, y),
        size: (width, height),
        adjusted: (x, y) != (saved.x, saved.y) || (width, height) != (saved.width, saved.height),
    }
}

fn chosen_area(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    areas: &[WorkArea],
    fallback: WorkArea,
) -> WorkArea {
    areas
        .iter()
        .copied()
        .map(|area| (overlap(x, y, width, height, area), area))
        .filter(|(shared, _)| *shared > 0)
        .max_by_key(|(shared, _)| *shared)
        .map_or(fallback, |(_, area)| area)
}

fn chosen_monitor(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    monitors: &[MonitorWork],
    fallback: MonitorWork,
) -> MonitorWork {
    monitors
        .iter()
        .copied()
        .map(|monitor| (overlap(x, y, width, height, monitor.area), monitor))
        .filter(|(shared, _)| *shared > 0)
        .max_by_key(|(shared, _)| *shared)
        .map_or(fallback, |(_, monitor)| monitor)
}

/// 工作区放得下时用 `want`。放不下时缩到工作区。工作区宽度或高度是 0 时保留 `want`。
fn fit_span(want: u32, span: i32) -> u32 {
    let span = u32::try_from(span.max(0)).unwrap_or(0);
    let want = want.max(1);
    if span == 0 { want } else { want.min(span) }
}

fn physical_len(logical: u32, dpi: u32) -> u32 {
    u32::try_from(to_physical(logical, dpi).max(1)).unwrap_or(1)
}

fn overlap(x: i32, y: i32, width: u32, height: u32, area: WorkArea) -> i64 {
    let right = i64::from(x) + i64::from(width);
    let bottom = i64::from(y) + i64::from(height);
    let shared_w = right.min(i64::from(area.right)) - i64::from(x).max(i64::from(area.left));
    let shared_h = bottom.min(i64::from(area.bottom)) - i64::from(y).max(i64::from(area.top));
    if shared_w <= 0 || shared_h <= 0 {
        0
    } else {
        shared_w * shared_h
    }
}

fn clamp_start(start: i32, length: u32, low: i32, high: i32) -> i32 {
    let last = i64::from(high) - i64::from(length);
    let clamped = i64::from(start).min(last).max(i64::from(low));
    i32::try_from(clamped).unwrap_or(low)
}

#[cfg(test)]
mod tests {
    use super::super::float_snap::{DockInput, ScreenBounds, transition};
    use super::*;
    use crate::notes::{DockEdge, TimestampMillis};

    fn note(id: &str, title: &str, tags: &[&str], pinned: bool, deleted: Option<i64>) -> Note {
        Note {
            id: id.to_owned(),
            title: title.to_owned(),
            body: String::new(),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            pinned,
            created_at: TimestampMillis::from_millis(0),
            updated_at: TimestampMillis::from_millis(0),
            deleted_at: deleted.map(TimestampMillis::from_millis),
            revision: 1,
            float: None,
            float_dock: None,
            float_stack: None,
        }
    }

    #[test]
    fn rows_show_the_display_title_pin_and_tags() {
        let notes = [
            note("a", "", &["工作", "会议"], true, None),
            note("b", "买菜", &[], false, None),
        ];
        let rows = list_rows(&notes, None);
        assert_eq!(rows[0].title, "无标题");
        assert!(rows[0].pinned);
        assert_eq!(rows[0].tags, "工作、会议");
        assert_eq!(rows[1].title, "买菜");
        assert_eq!(rows[1].tags, "");
        assert_eq!(rows[1].trash_note, "");
    }

    #[test]
    fn tag_filter_is_a_whole_tag_match_and_keeps_the_service_order() {
        let notes = [
            note("a", "甲", &["工作"], false, None),
            note("b", "乙", &["工作室"], false, None),
            note("c", "丙", &["工作", "会议"], false, None),
        ];
        let ids: Vec<_> = list_rows(&notes, Some("工作"))
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(ids, ["a", "c"]);
        assert!(list_rows(&notes, Some("工")).is_empty());
    }

    #[test]
    fn tag_names_are_unique_in_first_seen_order() {
        let notes = [
            note("a", "", &["工作", "会议"], false, None),
            note("b", "", &["会议", "家"], false, None),
        ];
        assert_eq!(tag_names(&notes), ["工作", "会议", "家"]);
        assert!(tag_names(&[]).is_empty());
    }

    #[test]
    fn trash_rows_list_the_latest_deleted_first_with_days_left() {
        let day = 86_400_000;
        let deleted = [
            note("old", "旧", &[], false, Some(1_000)),
            note("new", "新", &[], false, Some(5 * day)),
        ];
        let rows = trash_rows(&deleted, 6 * day);
        assert_eq!(rows[0].id, "new");
        assert_eq!(rows[0].trash_note, "剩 29 天");
        assert_eq!(rows[1].id, "old");
        assert_eq!(rows[1].trash_note, "剩 25 天");
    }

    #[test]
    fn locate_finds_the_list_or_the_trash() {
        let live = [note("a", "", &[], false, None)];
        let gone = [note("b", "", &[], false, Some(1))];
        assert_eq!(locate_note(&live, &gone, "a"), Some(NoteView::List));
        assert_eq!(locate_note(&live, &gone, "b"), Some(NoteView::Trash));
        assert_eq!(locate_note(&live, &gone, "c"), None);
    }

    #[test]
    fn float_windows_cascade_from_the_center_and_wrap() {
        let work = WorkArea {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        assert_eq!(float_default_origin(work, 400, 300, 96, 0), (300, 250));
        assert_eq!(float_default_origin(work, 400, 300, 96, 2), (356, 306));
        assert_eq!(
            float_default_origin(work, 400, 300, 96, 8),
            float_default_origin(work, 400, 300, 96, 0)
        );
        assert_eq!(float_default_origin(work, 400, 300, 192, 1), (356, 306));
    }

    const MIN: (u32, u32) = (200, 120);

    fn area(left: i32, top: i32, right: i32, bottom: i32) -> WorkArea {
        WorkArea {
            left,
            top,
            right,
            bottom,
        }
    }

    fn geometry(x: i32, y: i32, width: u32, height: u32) -> FloatGeometry {
        FloatGeometry {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_window_inside_a_work_area_stays_where_it_was() {
        let primary = area(0, 0, 1920, 1040);
        let placed = restore_float(geometry(300, 200, 400, 300), &[primary], primary, MIN);
        assert_eq!(placed.origin, (300, 200));
        assert_eq!(placed.size, (400, 300));
        assert!(!placed.adjusted);
    }

    #[test]
    fn a_window_on_a_second_monitor_is_kept_when_that_monitor_is_still_there() {
        let primary = area(0, 0, 1920, 1040);
        let left = area(-1280, 0, 0, 984);
        let placed = restore_float(
            geometry(-1000, 100, 400, 300),
            &[primary, left],
            primary,
            MIN,
        );
        assert_eq!(placed.origin, (-1000, 100));
        assert!(!placed.adjusted);
    }

    #[test]
    fn a_window_on_a_removed_monitor_is_pulled_into_the_fallback_area() {
        let primary = area(0, 0, 1920, 1040);
        let placed = restore_float(geometry(-1000, 100, 400, 300), &[primary], primary, MIN);
        assert_eq!(placed.origin, (0, 100));
        assert!(placed.adjusted);
        let far = restore_float(geometry(9000, 9000, 400, 300), &[primary], primary, MIN);
        assert_eq!(far.origin, (1520, 740));
    }

    #[test]
    fn a_window_hanging_over_an_edge_is_pulled_back_in_full() {
        let primary = area(0, 0, 1920, 1040);
        let placed = restore_float(geometry(1800, 1000, 400, 300), &[primary], primary, MIN);
        assert_eq!(placed.origin, (1520, 740));
        assert!(placed.adjusted);
    }

    #[test]
    fn the_biggest_overlap_picks_the_work_area() {
        let left = area(-1280, 0, 0, 984);
        let primary = area(0, 0, 1920, 1040);
        let placed = restore_float(geometry(-100, 50, 400, 300), &[left, primary], left, MIN);
        assert_eq!(placed.origin, (0, 50));
    }

    #[test]
    fn a_window_bigger_than_the_area_shrinks_but_not_below_the_minimum() {
        let small = area(0, 0, 800, 600);
        let placed = restore_float(geometry(0, 0, 3000, 2000), &[small], small, MIN);
        assert_eq!(placed.size, (800, 600));
        let tiny = restore_float(geometry(10, 10, 5, 5), &[small], small, MIN);
        assert_eq!(tiny.size, MIN);
        assert!(tiny.adjusted);
    }

    #[test]
    fn no_work_areas_at_all_uses_the_fallback() {
        let fallback = area(0, 0, 1024, 768);
        let placed = restore_float(geometry(5000, 5000, 400, 300), &[], fallback, MIN);
        assert_eq!(placed.origin, (624, 468));
    }

    fn monitor(left: i32, top: i32, right: i32, bottom: i32, dpi: u32) -> MonitorWork {
        MonitorWork {
            area: area(left, top, right, bottom),
            dpi,
        }
    }

    fn screen(left: i32, top: i32, right: i32, bottom: i32, dpi: u32) -> ScreenBounds {
        ScreenBounds {
            left,
            top,
            right,
            bottom,
            dpi,
        }
    }

    fn inside(placed: &FloatPlacement, work: WorkArea) {
        let right = placed.origin.0 + i32::try_from(placed.size.0).unwrap();
        let bottom = placed.origin.1 + i32::try_from(placed.size.1).unwrap();
        assert!(placed.origin.0 >= work.left, "left {}", placed.origin.0);
        assert!(placed.origin.1 >= work.top, "top {}", placed.origin.1);
        assert!(right <= work.right, "right {right}");
        assert!(bottom <= work.bottom, "bottom {bottom}");
    }

    #[test]
    fn a_float_past_the_right_edge_is_pulled_into_that_monitors_work_area() {
        let primary = monitor(0, 0, 1920, 1040, 96);
        let placed = contain_float(
            geometry(1700, 80, 320, 360),
            &[primary],
            primary,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_eq!(placed.origin, (1600, 80));
        assert_eq!(placed.size, (320, 360));
        inside(&placed, primary.area);
    }

    #[test]
    fn the_bottom_edge_does_not_snap_so_a_low_window_is_still_pulled_up() {
        let primary = monitor(0, 0, 1920, 1040, 96);
        let placed = contain_float(
            geometry(400, 900, 320, 360),
            &[primary],
            primary,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_eq!(placed.origin, (400, 680));
        assert_eq!(placed.size, (320, 360));
        inside(&placed, primary.area);
    }

    #[test]
    fn growing_past_the_work_area_shrinks_and_stays_inside() {
        let primary = monitor(0, 40, 1860, 1080, 96);
        let placed = contain_float(
            geometry(1500, 100, 600, 400),
            &[primary],
            primary,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_eq!(placed.size, (600, 400));
        assert_eq!(placed.origin, (1260, 100));
        inside(&placed, primary.area);
    }

    #[test]
    fn a_second_monitor_uses_its_own_dpi_for_the_minimum_size() {
        let primary = monitor(0, 0, 1920, 1040, 96);
        let right = monitor(1920, 0, 3840, 1000, 144);
        let placed = contain_float(
            geometry(3600, 40, 100, 100),
            &[primary, right],
            primary,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_eq!(placed.size, (300, 180), "144 DPI 下最小 200×120 逻辑像素");
        assert_eq!(placed.origin.0, 3840 - 300);
        assert_eq!(placed.origin.1, 40);
        inside(&placed, right.area);
    }

    #[test]
    fn a_window_mostly_on_the_higher_dpi_monitor_is_not_sized_with_the_other_dpi() {
        let left = monitor(-1920, 0, 0, 1080, 96);
        let right = monitor(0, 0, 1920, 1080, 144);
        let placed = contain_float(
            geometry(-50, 100, 400, 300),
            &[left, right],
            left,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_eq!(placed.size, (400, 300));
        assert_eq!(placed.origin, (0, 100));
        inside(&placed, right.area);
    }

    #[test]
    fn a_work_area_smaller_than_the_minimum_still_holds_the_whole_window() {
        let tiny = monitor(10, 20, 110, 90, 96);
        let placed = contain_float(
            geometry(0, 0, 400, 300),
            &[tiny],
            tiny,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_eq!(placed.size, (100, 70));
        assert_eq!(placed.origin, (10, 20));
        inside(&placed, tiny.area);
    }

    #[test]
    fn a_removed_monitor_uses_the_fallback_and_its_dpi() {
        let fallback = monitor(0, 0, 1280, 800, 144);
        let placed = contain_float(
            geometry(9000, 9000, 50, 50),
            &[],
            fallback,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_eq!(placed.size, (300, 180));
        inside(&placed, fallback.area);
    }

    #[test]
    fn a_cascaded_default_that_would_hang_off_is_pulled_back() {
        let work = area(0, 0, 500, 400);
        let origin = float_default_origin(work, 400, 300, 96, 7);
        let primary = MonitorWork {
            area: work,
            dpi: 96,
        };
        let placed = contain_float(
            geometry(origin.0, origin.1, 400, 300),
            &[primary],
            primary,
            (FLOAT_MIN_WIDTH, FLOAT_MIN_HEIGHT),
        );
        assert_ne!(origin, placed.origin);
        inside(&placed, work);
    }

    #[test]
    fn snapping_to_an_outer_edge_is_kept_and_a_near_miss_is_clamped() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let primary = monitor(0, 48, 1920, 1040, 96);
        let snapped_rect = PxBox {
            x: 400,
            y: 0,
            width: 320,
            height: 360,
        };
        let snapped = transition(
            &DockState {
                expanded: snapped_rect,
                dock: None,
            },
            &screens,
            DockInput::DragFinished {
                rect: snapped_rect,
                moved: true,
                from_collapsed: false,
                size_changed: false,
            },
        );
        let kept = settle_float(snapped.state, &screens, &[primary], primary);
        assert_eq!(kept.dock.map(|dock| dock.edge), Some(DockEdge::Top));
        assert_eq!(kept.expanded.y, 0, "贴上边时留在显示器外沿，不拉进工作区");

        let missed = PxBox {
            x: 1613,
            y: 100,
            width: 320,
            height: 360,
        };
        let dropped = transition(
            &DockState {
                expanded: missed,
                dock: None,
            },
            &screens,
            DockInput::DragFinished {
                rect: missed,
                moved: true,
                from_collapsed: false,
                size_changed: false,
            },
        );
        assert!(
            dropped.state.dock.is_none(),
            "离右边 13 物理像素，超过 12 的贴边阈值"
        );
        let kept = settle_float(dropped.state, &screens, &[primary], primary);
        assert!(kept.dock.is_none());
        assert_eq!(kept.expanded.x, 1920 - 320);
        assert_eq!(kept.expanded.y, 100);
        let placed = FloatPlacement {
            origin: (kept.expanded.x, kept.expanded.y),
            size: (kept.expanded.width, kept.expanded.height),
            adjusted: true,
        };
        inside(&placed, primary.area);
    }

    #[test]
    fn a_resize_that_grows_past_the_right_side_is_pulled_back_when_it_does_not_snap() {
        let screens = [screen(0, 0, 1920, 1080, 96)];
        let primary = monitor(0, 0, 1920, 1040, 96);
        let grown = PxBox {
            x: 1700,
            y: 80,
            width: 400,
            height: 360,
        };
        let resized = transition(
            &DockState {
                expanded: PxBox {
                    x: 1700,
                    y: 80,
                    width: 200,
                    height: 360,
                },
                dock: None,
            },
            &screens,
            DockInput::DragFinished {
                rect: grown,
                moved: false,
                from_collapsed: false,
                size_changed: true,
            },
        );
        assert!(resized.state.dock.is_none());
        let kept = settle_float(resized.state, &screens, &[primary], primary);
        assert_eq!(kept.expanded.width, 400);
        assert_eq!(kept.expanded.x, 1520);
        assert_eq!(kept.expanded.y, 80);
        let placed = FloatPlacement {
            origin: (kept.expanded.x, kept.expanded.y),
            size: (kept.expanded.width, kept.expanded.height),
            adjusted: true,
        };
        inside(&placed, primary.area);
    }
}
