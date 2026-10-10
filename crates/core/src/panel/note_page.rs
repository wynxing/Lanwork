//! 便签页的数据整理：列表行、标签筛选、回收站视图、定位，以及悬浮窗的默认大小和位置。
//!
//! 输入是 [`NoteCommands`](crate::notes::NoteCommands) 的快照。这里不读写盘，不读时钟。
//! 规则仍在便签服务里。列表顺序（置顶在前，其后按更新时间从新到旧）由服务给出，这里不重排回收站以外的顺序。

use crate::notes::Note;
use crate::shell::WorkArea;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::TimestampMillis;

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
}
