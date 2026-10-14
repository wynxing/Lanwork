//! 处理模式的界面决定：这一轮的队列、按键、进度和清单选择。
//!
//! 不写盘，不读时钟，也不读配置。写盘仍走 `TodoCommands` 的 `process_*`。
//! 下一条 id 由 [`crate::todos::TodoCommands::next_inbox_incomplete_after`] 给出，这里只把它放进队列。
//! 顺延天数由调用方传入，缺省时用 [`crate::todos::DEFAULT_DEFER_DAYS`]。

use crate::todos::{ListKind, TodoItem, TodoList};

use super::todo_page::format_date;

/// 按键码。界面把 Slint 的 `Key` 收成这些整数再交给 [`key_hit`]。
pub const KEY_TODAY: i32 = 1;
pub const KEY_DEFER: i32 = 2;
pub const KEY_MOVE: i32 = 3;
pub const KEY_COMPLETE: i32 = 4;
pub const KEY_DELETE: i32 = 5;
pub const KEY_ENTER: i32 = 13;
pub const KEY_ESCAPE: i32 = 27;
pub const KEY_UP: i32 = 38;
pub const KEY_DOWN: i32 = 40;

/// 最后一条处理完之后，过这么久回到清单。
pub const CLEARED_HOLD_MS: u64 = 1_000;

pub const CLEARED_TEXT: &str = "收件箱已清空";
pub const EMPTY_TEXT: &str = "没有待办";

/// 产品规格没有写这两种失败时的句子。它们只报状态。
pub const NO_OTHER_LIST: &str = "没有其他清单";
pub const NO_TODAY: &str = "读不到今天的日期";

/// 一次按键要做的事。`None` 表示不改队列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Today,
    Defer,
    OpenMove,
    Complete,
    Delete,
    Exit,
    MoveUp,
    MoveDown,
    ConfirmMove,
    CancelMove,
    None,
}

/// [`key_hit`] 的输入。组合输入和带修饰键的数字不触发 `1` 至 `5`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub code: i32,
    /// 输入法还在组合。为真时不触发任何处理动作，按键交还输入法。
    pub composing: bool,
    /// Ctrl、Alt、Shift、Win 任一按下。
    pub modified: bool,
    pub picker_open: bool,
    pub showing_item: bool,
}

/// 按键结果。`consume` 为真时界面吃掉这个键，不交给输入框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHit {
    pub action: Action,
    pub consume: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Item,
    Cleared,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pass {
    ids: Vec<String>,
    index: usize,
    /// 下一次向前查询用的 id。正在显示一条时就是这一条。
    cursor: String,
    phase: Phase,
}

impl Pass {
    fn start(ids: Vec<String>) -> Self {
        if ids.is_empty() {
            return Self {
                ids,
                index: 0,
                cursor: String::new(),
                phase: Phase::Empty,
            };
        }
        let cursor = ids[0].clone();
        Self {
            ids,
            index: 0,
            cursor,
            phase: Phase::Item,
        }
    }

    fn showing_item(&self) -> bool {
        self.phase == Phase::Item && self.index < self.ids.len()
    }

    fn shown_id(&self) -> Option<&str> {
        if self.showing_item() {
            Some(self.ids[self.index].as_str())
        } else {
            None
        }
    }

    fn progress(&self) -> Option<String> {
        if !self.showing_item() || self.ids.is_empty() {
            return None;
        }
        Some(progress_label(self.index, self.ids.len()))
    }

    fn status(&self) -> Option<&'static str> {
        match self.phase {
            Phase::Cleared => Some(CLEARED_TEXT),
            Phase::Empty => Some(EMPTY_TEXT),
            Phase::Item => None,
        }
    }

    /// 把 id 大于当前条、这一轮还没有的未完成条目补进队列。不往回走。
    fn absorb_ahead(&mut self, fresh: &[String]) {
        let Some(current) = self.shown_id().map(str::to_owned) else {
            return;
        };
        for id in fresh {
            if id.as_str() <= current.as_str() || self.ids.iter().any(|have| have == id) {
                continue;
            }
            let pos = self
                .ids
                .iter()
                .position(|have| have.as_str() > id.as_str())
                .unwrap_or(self.ids.len());
            self.ids.insert(pos, id.clone());
        }
    }

    fn shown_in(&self, fresh: &[String]) -> bool {
        self.shown_id()
            .is_some_and(|id| fresh.iter().any(|fresh_id| fresh_id == id))
    }

    /// `next` 是 `next_inbox_incomplete_after(cursor)` 的结果。
    ///
    /// `None` 表示这一轮向前已经没有下一条，进入「收件箱已清空」。
    /// 比光标更小的 id 不回去（不重复）。队列里没有的更大 id 插进来（不跳过）。
    fn advance(&mut self, next: Option<String>) {
        if self.phase == Phase::Empty {
            return;
        }
        let Some(next) = next else {
            self.phase = Phase::Cleared;
            return;
        };
        if !self.cursor.is_empty() && next.as_str() <= self.cursor.as_str() {
            return;
        }
        if let Some(pos) = self.ids.iter().position(|id| id == &next) {
            self.index = pos;
        } else {
            let pos = self
                .ids
                .iter()
                .position(|id| id.as_str() > next.as_str())
                .unwrap_or(self.ids.len());
            self.ids.insert(pos, next.clone());
            self.index = pos;
        }
        self.cursor.clone_from(&next);
        self.phase = Phase::Item;
    }
}

/// 移动目标。不含收件箱。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveTarget {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Picker {
    targets: Vec<MoveTarget>,
    index: usize,
}

/// 打开「3 移动」的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMove {
    Opened,
    NoList,
    Unavailable,
}

/// 一轮处理。光标是条目 id，不另存一份待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pass: Pass,
    picker: Option<Picker>,
}

impl Session {
    #[must_use]
    pub fn start(ids: Vec<String>) -> Self {
        Self {
            pass: Pass::start(ids),
            picker: None,
        }
    }

    #[must_use]
    pub fn showing_item(&self) -> bool {
        self.pass.showing_item()
    }

    #[must_use]
    pub fn shown_id(&self) -> Option<&str> {
        self.pass.shown_id()
    }

    #[must_use]
    pub fn cursor(&self) -> &str {
        self.pass.cursor.as_str()
    }

    #[must_use]
    pub fn is_cleared(&self) -> bool {
        self.pass.phase == Phase::Cleared
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pass.phase == Phase::Empty
    }

    #[must_use]
    pub fn progress(&self) -> Option<String> {
        self.pass.progress()
    }

    #[must_use]
    pub fn status(&self) -> Option<&'static str> {
        self.pass.status()
    }

    pub fn absorb_ahead(&mut self, fresh: &[String]) {
        self.pass.absorb_ahead(fresh);
    }

    #[must_use]
    pub fn shown_in(&self, fresh: &[String]) -> bool {
        self.pass.shown_in(fresh)
    }

    pub fn advance(&mut self, next: Option<String>) {
        let before = self.pass.shown_id().map(str::to_owned);
        self.pass.advance(next);
        if !self.pass.showing_item() || self.pass.shown_id() != before.as_deref() {
            self.picker = None;
        }
    }

    #[must_use]
    pub fn picker_open(&self) -> bool {
        self.picker.is_some()
    }

    #[must_use]
    pub fn picker_names(&self) -> Vec<String> {
        self.picker
            .as_ref()
            .map(|picker| {
                picker
                    .targets
                    .iter()
                    .map(|target| target.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[must_use]
    pub fn picker_index(&self) -> usize {
        self.picker.as_ref().map_or(0, |picker| picker.index)
    }

    pub fn open_picker(&mut self, lists: &[TodoList]) -> OpenMove {
        if !self.pass.showing_item() {
            return OpenMove::Unavailable;
        }
        let targets = move_targets(lists);
        if targets.is_empty() {
            return OpenMove::NoList;
        }
        self.picker = Some(Picker { targets, index: 0 });
        OpenMove::Opened
    }

    pub fn close_picker(&mut self) {
        self.picker = None;
    }

    pub fn move_picker(&mut self, down: bool) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        picker.index = step_index(picker.index, picker.targets.len(), down);
    }

    /// 点某一行或按 Enter。返回目标清单 id，并关掉选择。
    pub fn take_pick(&mut self, index: usize) -> Option<String> {
        let id = self
            .picker
            .as_ref()
            .and_then(|picker| picker.targets.get(index))
            .map(|target| target.id.clone())?;
        self.picker = None;
        Some(id)
    }

    pub fn take_current_pick(&mut self) -> Option<String> {
        let index = self.picker.as_ref()?.index;
        self.take_pick(index)
    }
}

/// 卡片上要显示的文字。没有到期日或来源时对应字段是空字符串，界面不画那一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardText {
    pub title: String,
    pub due: String,
    pub source: String,
}

/// 收件箱里未完成、未进回收站的条目，按 id 排序。顺序与 `next_inbox_incomplete_after` 一致。
#[must_use]
pub fn inbox_queue(lists: &[TodoList]) -> Vec<String> {
    let mut ids: Vec<String> = lists
        .iter()
        .filter(|list| list.kind == ListKind::Inbox)
        .flat_map(|list| list.items.iter())
        .filter(|item| !item.completed && !item.in_trash())
        .map(|item| item.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// 当前停在收件箱、并且收件箱还有未完成条目时，清单顶部才有入口。
#[must_use]
pub fn entry_visible(viewing_inbox: bool, incomplete: usize) -> bool {
    viewing_inbox && incomplete > 0
}

#[must_use]
pub fn entry_label(count: usize) -> String {
    format!("处理 {count} 条")
}

#[must_use]
pub fn progress_label(index: usize, total: usize) -> String {
    format!("第 {} / {} 条", index + 1, total)
}

/// 五个按钮的文字。`days` 是设置里的顺延天数。
#[must_use]
pub fn action_labels(days: u32) -> [String; 5] {
    [
        "1 今天".to_owned(),
        format!("2 稍后 {days} 天"),
        "3 移动".to_owned(),
        "4 完成".to_owned(),
        "5 删除".to_owned(),
    ]
}

/// 除收件箱以外的清单，保持服务给出的顺序。
#[must_use]
pub fn move_targets(lists: &[TodoList]) -> Vec<MoveTarget> {
    lists
        .iter()
        .filter(|list| list.kind != ListKind::Inbox)
        .map(|list| MoveTarget {
            id: list.id.clone(),
            name: list.name.clone(),
        })
        .collect()
}

#[must_use]
pub fn card_text(lists: &[TodoList], id: &str) -> Option<CardText> {
    let item = find_item(lists, id)?;
    Some(CardText {
        title: item.title.clone(),
        due: item.due.map(format_date).unwrap_or_default(),
        source: item
            .source
            .as_ref()
            .map(|source| format!("{}#{}", source.repo, source.number))
            .unwrap_or_default(),
    })
}

#[must_use]
pub fn key_hit(input: KeyInput) -> KeyHit {
    if input.composing {
        return KeyHit {
            action: Action::None,
            consume: false,
        };
    }
    let known = matches!(
        input.code,
        KEY_TODAY
            | KEY_DEFER
            | KEY_MOVE
            | KEY_COMPLETE
            | KEY_DELETE
            | KEY_ENTER
            | KEY_ESCAPE
            | KEY_UP
            | KEY_DOWN
    );
    if !known {
        return KeyHit {
            action: Action::None,
            consume: false,
        };
    }
    if input.modified {
        return KeyHit {
            action: Action::None,
            consume: true,
        };
    }
    if input.picker_open && input.showing_item {
        let action = match input.code {
            KEY_UP => Action::MoveUp,
            KEY_DOWN => Action::MoveDown,
            KEY_ENTER => Action::ConfirmMove,
            KEY_ESCAPE => Action::CancelMove,
            _ => Action::None,
        };
        return KeyHit {
            action,
            consume: true,
        };
    }
    if input.code == KEY_ESCAPE {
        return KeyHit {
            action: Action::Exit,
            consume: true,
        };
    }
    if !input.showing_item {
        return KeyHit {
            action: Action::None,
            consume: true,
        };
    }
    let action = match input.code {
        KEY_TODAY => Action::Today,
        KEY_DEFER => Action::Defer,
        KEY_MOVE => Action::OpenMove,
        KEY_COMPLETE => Action::Complete,
        KEY_DELETE => Action::Delete,
        _ => Action::None,
    };
    KeyHit {
        action,
        consume: true,
    }
}

fn find_item<'a>(lists: &'a [TodoList], id: &str) -> Option<&'a TodoItem> {
    lists
        .iter()
        .find_map(|list| list.items.iter().find(|item| item.id == id))
}

fn step_index(index: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if down {
        index.saturating_add(1).min(len - 1)
    } else {
        index.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CivilDate;
    use crate::todos::{SourceKind, TodoSource};

    fn item(id: &str) -> TodoItem {
        TodoItem::new(id.to_owned(), format!("标题 {id}"), 0)
    }

    fn list(id: &str, kind: ListKind, items: Vec<TodoItem>) -> TodoList {
        TodoList {
            schema_version: 1,
            id: id.to_owned(),
            name: if kind == ListKind::Inbox {
                "收件箱".to_owned()
            } else {
                format!("清单 {id}")
            },
            kind,
            order: 0,
            items,
        }
    }

    fn key(code: i32, extra: impl FnOnce(&mut KeyInput)) -> KeyHit {
        let mut input = KeyInput {
            code,
            composing: false,
            modified: false,
            picker_open: false,
            showing_item: true,
        };
        extra(&mut input);
        key_hit(input)
    }

    #[test]
    fn queue_is_inbox_incomplete_sorted_by_id() {
        let mut done = item("b");
        done.completed = true;
        let mut trashed = item("a");
        trashed.deleted_at = Some(1);
        let lists = vec![
            list(
                "inbox",
                ListKind::Inbox,
                vec![item("c"), done, trashed, item("a2")],
            ),
            list("work", ListKind::Normal, vec![item("a0")]),
        ];
        assert_eq!(inbox_queue(&lists), ["a2", "c"]);
        assert!(entry_visible(true, 2));
        assert!(!entry_visible(false, 2));
        assert!(!entry_visible(true, 0));
        assert_eq!(entry_label(12), "处理 12 条");
    }

    #[test]
    fn pass_moves_forward_by_id_and_inserts_new_ones() {
        let mut session = Session::start(vec!["a".into(), "c".into(), "e".into()]);
        assert_eq!(session.progress().as_deref(), Some("第 1 / 3 条"));
        assert_eq!(session.shown_id(), Some("a"));

        session.absorb_ahead(&["a".into(), "b".into(), "c".into(), "d".into()]);
        assert_eq!(session.progress().as_deref(), Some("第 1 / 5 条"));
        assert_eq!(session.shown_id(), Some("a"), "新条目不把光标拉回去");

        session.advance(Some("b".into()));
        assert_eq!(session.shown_id(), Some("b"));
        assert_eq!(session.progress().as_deref(), Some("第 2 / 5 条"));

        session.advance(Some("a".into()));
        assert_eq!(session.shown_id(), Some("b"), "更小的 id 不重复");

        session.advance(Some("e".into()));
        assert_eq!(session.shown_id(), Some("e"));
        session.advance(None);
        assert!(session.is_cleared());
        assert_eq!(session.status(), Some(CLEARED_TEXT));
        assert!(session.progress().is_none());

        session.advance(Some("d".into()));
        assert!(session.is_cleared(), "清空之后不回到已经过过的 id");
        session.advance(Some("f".into()));
        assert_eq!(session.shown_id(), Some("f"));
        assert!(!session.is_cleared());
    }

    #[test]
    fn empty_inbox_shows_the_empty_line_and_does_not_clear() {
        let session = Session::start(Vec::new());
        assert!(session.is_empty());
        assert_eq!(session.status(), Some(EMPTY_TEXT));
        assert!(!session.is_cleared());
    }

    #[test]
    fn vanished_current_item_is_not_shown() {
        let session = Session::start(vec!["a".into(), "b".into()]);
        assert!(session.shown_in(&["a".into(), "b".into()]));
        assert!(!session.shown_in(&["b".into()]));
    }

    #[test]
    fn card_shows_title_due_and_source_mark() {
        let mut dated = item("a");
        dated.title = "写规格".into();
        dated.due = Some(CivilDate::try_from_ymd(2026, 10, 14).unwrap());
        dated.source = Some(
            TodoSource::try_new(
                SourceKind::GithubIssue,
                "https://github.com/wynxing/Lanwork/issues/24",
                "wynxing/Lanwork",
                24,
            )
            .unwrap(),
        );
        let lists = vec![list("inbox", ListKind::Inbox, vec![dated])];
        let card = card_text(&lists, "a").unwrap();
        assert_eq!(card.title, "写规格");
        assert_eq!(card.due, "2026-10-14");
        assert_eq!(card.source, "wynxing/Lanwork#24");

        let bare = card_text(&lists, "missing");
        assert!(bare.is_none());
        let plain = item("b");
        let lists = vec![list("inbox", ListKind::Inbox, vec![plain])];
        let card = card_text(&lists, "b").unwrap();
        assert_eq!(card.due, "");
        assert_eq!(card.source, "");
    }

    #[test]
    fn labels_use_the_snooze_days() {
        assert_eq!(
            action_labels(3),
            [
                "1 今天".to_owned(),
                "2 稍后 3 天".to_owned(),
                "3 移动".to_owned(),
                "4 完成".to_owned(),
                "5 删除".to_owned(),
            ]
        );
        assert_eq!(action_labels(12)[1], "2 稍后 12 天");
    }

    #[test]
    fn composing_and_modifiers_do_not_trigger_digits() {
        let hit = key(KEY_TODAY, |input| input.composing = true);
        assert_eq!(hit.action, Action::None);
        assert!(!hit.consume);

        let hit = key(KEY_DELETE, |input| input.modified = true);
        assert_eq!(hit.action, Action::None);
        assert!(hit.consume);

        assert_eq!(key(KEY_TODAY, |_| {}).action, Action::Today);
        assert_eq!(key(KEY_DEFER, |_| {}).action, Action::Defer);
        assert_eq!(key(KEY_MOVE, |_| {}).action, Action::OpenMove);
        assert_eq!(key(KEY_COMPLETE, |_| {}).action, Action::Complete);
        assert_eq!(key(KEY_DELETE, |_| {}).action, Action::Delete);
        assert_eq!(key(KEY_ESCAPE, |_| {}).action, Action::Exit);
    }

    #[test]
    fn picker_keys_choose_and_escape_only_closes_the_picker() {
        let mut session = Session::start(vec!["a".into()]);
        let lists = vec![
            list("inbox", ListKind::Inbox, vec![item("a")]),
            list("work", ListKind::Normal, vec![]),
            list("home", ListKind::Normal, vec![]),
        ];
        assert_eq!(session.open_picker(&lists), OpenMove::Opened);
        assert_eq!(session.picker_names(), ["清单 work", "清单 home"]);
        assert_eq!(session.picker_index(), 0);

        session.move_picker(false);
        assert_eq!(session.picker_index(), 0, "到顶不再往上");
        session.move_picker(true);
        session.move_picker(true);
        assert_eq!(session.picker_index(), 1, "到底不再往下");

        let hit = key(KEY_TODAY, |input| input.picker_open = true);
        assert_eq!(hit.action, Action::None);
        let hit = key(KEY_ESCAPE, |input| input.picker_open = true);
        assert_eq!(hit.action, Action::CancelMove);
        let hit = key(KEY_DOWN, |input| input.picker_open = true);
        assert_eq!(hit.action, Action::MoveDown);
        let hit = key(KEY_ENTER, |input| input.picker_open = true);
        assert_eq!(hit.action, Action::ConfirmMove);

        assert_eq!(session.take_current_pick().as_deref(), Some("home"));
        assert!(!session.picker_open());

        let only_inbox = vec![list("inbox", ListKind::Inbox, vec![item("a")])];
        assert_eq!(session.open_picker(&only_inbox), OpenMove::NoList);
    }

    #[test]
    fn cleared_digits_do_nothing_and_escape_exits() {
        let hit = key(KEY_TODAY, |input| input.showing_item = false);
        assert_eq!(hit.action, Action::None);
        assert!(hit.consume);
        let hit = key(KEY_ESCAPE, |input| input.showing_item = false);
        assert_eq!(hit.action, Action::Exit);
    }
}
