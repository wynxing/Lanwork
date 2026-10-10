//! 待办服务的验收测试。可见行为以 product.md「待办」为准。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lanwork_core::CivilDate as Date;
use lanwork_core::storage::{CollectionKind, DocumentId, EntityKind, Store, StorePaths};
use lanwork_core::todos::{
    ClockTime, DEFAULT_DEFER_DAYS, ListKind, NewTodo, PendingTopic, Recurrence, RecurrenceRule,
    SourceKind, TRASH_RETENTION_MS, TodoCommands, TodoError, TodoList, TodoSource,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lanwork-todos-{nanos}-{seq}"));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct Fixture {
    temp: TempDir,
    paths: StorePaths,
    store: Store,
    todos: TodoCommands,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new();
        let paths = StorePaths {
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            user_profile: temp.path().join("profile"),
            local_app_data: temp.path().join("local"),
        };
        let store = Store::open(paths.clone()).unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        Self {
            temp,
            paths,
            store,
            todos,
        }
    }

    fn reboot(self) -> Self {
        let store = Store::open(self.paths.clone()).unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        Self {
            temp: self.temp,
            paths: self.paths,
            store,
            todos,
        }
    }
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

fn date(year: i32, month: u8, day: u8) -> Date {
    Date::try_from_ymd(year, month, day).unwrap()
}

fn new_todo(title: &str) -> NewTodo {
    NewTodo {
        title: title.into(),
        due: None,
        remind_at: None,
        recurrence: None,
        source: None,
    }
}

fn recurrence(rule: RecurrenceRule, until: Option<Date>) -> Recurrence {
    Recurrence {
        rule,
        until,
        month_day: None,
    }
}

fn source_pr() -> TodoSource {
    TodoSource::try_new(
        SourceKind::GithubPr,
        "https://github.com/wynxing/Lanwork/pull/12",
        "wynxing/Lanwork",
        12,
    )
    .unwrap()
}

fn list<'a>(lists: &'a [TodoList], id: &str) -> &'a TodoList {
    lists.iter().find(|list| list.id == id).unwrap()
}

fn write_raw(store: &Store, id: &str, body: &str) {
    let path = store.document_path(&DocumentId::Todo(id.into())).unwrap();
    std::fs::write(path, body).unwrap()
}

#[test]
fn list_crud_inbox_and_item_order_follow_the_action_table() {
    let fixture = Fixture::new();
    let err = fixture.todos.create_list("   ").unwrap_err();
    assert!(matches!(err, TodoError::BlankName), "{err}");
    let work = fixture.todos.create_list("工作").unwrap();
    let life = fixture.todos.create_list("生活").unwrap();
    fixture.todos.rename_list(&work, "工作事项").unwrap();
    fixture
        .todos
        .reorder_lists(&[life.clone(), work.clone()])
        .unwrap();

    let inbox = fixture.todos.ensure_inbox().unwrap();
    assert_eq!(fixture.todos.ensure_inbox().unwrap(), inbox);
    let lists = fixture.todos.lists().unwrap();
    assert_eq!(list(&lists, &inbox).kind, ListKind::Inbox);
    assert_eq!(list(&lists, &inbox).name, "收件箱");
    assert_eq!(
        lists
            .iter()
            .filter(|item| item.kind == ListKind::Inbox)
            .count(),
        1
    );
    let named = fixture.todos.create_list("收件箱").unwrap();
    assert_ne!(named, inbox);
    assert_eq!(fixture.todos.ensure_inbox().unwrap(), inbox);

    let first = fixture.todos.create_item(&work, new_todo("甲")).unwrap();
    let second = fixture.todos.create_item(&work, new_todo("乙")).unwrap();
    fixture
        .todos
        .reorder_items(&work, &[second.clone(), first.clone()])
        .unwrap();
    let lists = fixture.todos.lists().unwrap();
    let items = &list(&lists, &work).items;
    assert_eq!(items[0].id, second);
    assert_eq!(items[1].id, first);

    let empty = fixture.todos.create_list("空").unwrap();
    fixture.todos.delete_list(&empty).unwrap();
    assert!(
        fixture
            .todos
            .lists()
            .unwrap()
            .iter()
            .all(|item| item.id != empty)
    );
    assert!(matches!(
        fixture.todos.delete_list(&inbox).unwrap_err(),
        TodoError::CannotDeleteInbox
    ));
    assert!(matches!(
        fixture.todos.delete_list(&work).unwrap_err(),
        TodoError::ListNotEmpty
    ));
    assert!(matches!(
        fixture
            .todos
            .reorder_lists(std::slice::from_ref(&work))
            .unwrap_err(),
        TodoError::InvalidOrder
    ));

    let again = fixture.reboot();
    let lists = again.todos.lists().unwrap();
    assert_eq!(list(&lists, &work).name, "工作事项");
    assert!(list(&lists, &work).order > list(&lists, &life).order);
    let items = &list(&lists, &work).items;
    assert_eq!(items[0].title, "乙");
    assert_eq!(items[1].title, "甲");
    assert_eq!(list(&lists, &inbox).kind, ListKind::Inbox);
}

#[test]
fn reload_keeps_due_recurrence_source_and_current() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let mut draft = new_todo("提交周报");
    draft.due = Some(date(2026, 10, 8));
    draft.remind_at = ClockTime::try_new(9, 30);
    draft.recurrence = Some(recurrence(RecurrenceRule::Weekly, Some(date(2026, 12, 31))));
    draft.source = Some(source_pr());
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    fixture.todos.set_current(&item_id).unwrap();

    let fixture = fixture.reboot();
    let stored = fixture.todos.item(&item_id).unwrap();
    assert_eq!(stored.list_id, list_id);
    assert_eq!(stored.item.title, "提交周报");
    assert_eq!(stored.item.due, Some(date(2026, 10, 8)));
    assert_eq!(stored.item.remind_at, ClockTime::try_new(9, 30));
    assert_eq!(
        stored.item.recurrence,
        Some(recurrence(RecurrenceRule::Weekly, Some(date(2026, 12, 31))))
    );
    assert_eq!(
        stored.item.source.as_ref().map(|source| source.number),
        Some(12)
    );
    assert_eq!(
        stored.item.source.as_ref().map(|source| source.kind),
        Some(SourceKind::GithubPr)
    );
    assert!(stored.item.current);
    assert!(stored.item.current_since.is_some());
}

#[test]
fn old_record_missing_fields_opens_and_can_be_edited() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths.clone()).unwrap();
    write_raw(
        &store,
        "l1",
        r#"{"id":"l1","name":"工作","items":[{"id":"a","title":"旧待办"}]}"#,
    );
    let todos = TodoCommands::open(store);
    todos.boot().unwrap();
    let stored = todos.item("a").unwrap();
    assert_eq!(stored.item.title, "旧待办");
    assert!(stored.item.due.is_none());
    assert!(stored.item.recurrence.is_none());
    assert!(stored.item.source.is_none());
    assert!(!stored.item.current);
    todos.rename_item("a", "仍可编辑").unwrap();
    drop(todos);
    let store = Store::open(paths).unwrap();
    let todos = TodoCommands::open(store);
    todos.boot().unwrap();
    assert_eq!(todos.item("a").unwrap().item.title, "仍可编辑");
}

#[test]
fn blank_title_is_rejected_and_writes_nothing() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    for title in ["", "   ", "\n\t", "　"] {
        let err = fixture
            .todos
            .create_item(&list_id, new_todo(title))
            .unwrap_err();
        assert!(matches!(err, TodoError::BlankTitle), "{title:?} {err}");
    }
    assert!(
        list(&fixture.todos.lists().unwrap(), &list_id)
            .items
            .is_empty()
    );
    let item_id = fixture
        .todos
        .create_item(&list_id, new_todo("有标题"))
        .unwrap();
    let err = fixture.todos.rename_item(&item_id, " ").unwrap_err();
    assert!(matches!(err, TodoError::BlankTitle), "{err}");
    assert_eq!(fixture.todos.item(&item_id).unwrap().item.title, "有标题");
}

#[test]
fn complete_recurrence_crosses_month_and_year_in_one_write() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let cases = [
        (RecurrenceRule::Daily, date(2026, 1, 31), date(2026, 2, 1)),
        (RecurrenceRule::Daily, date(2026, 12, 31), date(2027, 1, 1)),
        (RecurrenceRule::Weekly, date(2026, 1, 28), date(2026, 2, 4)),
        (RecurrenceRule::Weekly, date(2026, 12, 30), date(2027, 1, 6)),
        (
            RecurrenceRule::Biweekly,
            date(2026, 12, 20),
            date(2027, 1, 3),
        ),
        (
            RecurrenceRule::Monthly,
            date(2026, 1, 15),
            date(2026, 2, 15),
        ),
        (
            RecurrenceRule::Monthly,
            date(2026, 12, 31),
            date(2027, 1, 31),
        ),
        (
            RecurrenceRule::Monthly,
            date(2024, 1, 29),
            date(2024, 2, 29),
        ),
    ];
    for (rule, from, to) in cases {
        let mut draft = new_todo("周期");
        draft.due = Some(from);
        draft.recurrence = Some(recurrence(rule, None));
        draft.source = Some(source_pr());
        let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
        let rx = fixture.store.subscribe();
        fixture.todos.complete_item(&item_id).unwrap();
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(events.len(), 1, "{rule:?}");
        assert_eq!(events[0].kind, EntityKind::Todo);
        assert_eq!(events[0].id, list_id);
        assert!(events[0].revision.is_none());
        let lists = fixture.todos.lists().unwrap();
        let items = &list(&lists, &list_id).items;
        let done = items.iter().find(|item| item.id == item_id).unwrap();
        assert!(done.completed);
        assert_eq!(done.source, Some(source_pr()));
        let next = items.iter().find(|item| item.due == Some(to)).unwrap();
        assert!(!next.completed);
        assert_eq!(next.source, Some(source_pr()));
        assert!(!next.current);
        assert_ne!(next.id, item_id);
    }
    let disk: TodoList = fixture
        .store
        .read_json(&DocumentId::Todo(list_id))
        .unwrap()
        .unwrap();
    assert_eq!(disk.items.len(), cases.len() * 2);
}

#[test]
fn complete_generates_the_until_day_and_stops_after_it() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let mut draft = new_todo("直到截止日");
    draft.due = Some(date(2026, 3, 1));
    draft.recurrence = Some(recurrence(RecurrenceRule::Daily, Some(date(2026, 3, 2))));
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    fixture.todos.complete_item(&item_id).unwrap();
    let lists = fixture.todos.lists().unwrap();
    let next_due = list(&lists, &list_id)
        .items
        .iter()
        .find(|item| !item.completed)
        .unwrap()
        .due;
    assert_eq!(next_due, Some(date(2026, 3, 2)));

    let mut later = new_todo("已经晚于截止日");
    later.due = Some(date(2026, 3, 3));
    later.recurrence = Some(recurrence(RecurrenceRule::Daily, Some(date(2026, 3, 2))));
    let later_id = fixture.todos.create_item(&list_id, later).unwrap();
    fixture.todos.complete_item(&later_id).unwrap();
    assert!(fixture.todos.item(&later_id).unwrap().item.completed);
    assert_eq!(
        list(&fixture.todos.lists().unwrap(), &list_id)
            .items
            .iter()
            .filter(|item| !item.completed)
            .count(),
        1
    );
}

#[test]
fn monthly_31_in_a_short_month_lands_on_the_last_day() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let mut draft = new_todo("每月 31 日");
    draft.due = Some(date(2026, 1, 31));
    draft.recurrence = Some(recurrence(RecurrenceRule::Monthly, None));
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    fixture.todos.complete_item(&item_id).unwrap();
    let lists = fixture.todos.lists().unwrap();
    let next = list(&lists, &list_id)
        .items
        .iter()
        .find(|item| !item.completed)
        .unwrap();
    assert_eq!(next.due, Some(date(2026, 2, 28)));
    assert_eq!(
        next.recurrence.as_ref().and_then(|rule| rule.month_day),
        Some(31)
    );
    let next_id = next.id.clone();
    fixture.todos.complete_item(&next_id).unwrap();
    let lists = fixture.todos.lists().unwrap();
    let after = list(&lists, &list_id)
        .items
        .iter()
        .find(|item| !item.completed)
        .unwrap();
    assert_eq!(after.due, Some(date(2026, 3, 31)));
    assert_eq!(
        after.recurrence.as_ref().and_then(|rule| rule.month_day),
        Some(31)
    );
}

#[test]
fn monthly_without_saved_anchor_uses_the_current_due_day() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths).unwrap();
    write_raw(
        &store,
        "work",
        r#"{"schemaVersion":1,"id":"work","name":"工作","kind":"normal","items":[{"id":"clamped","title":"已在月末","due":"2026-02-28","recurrence":{"rule":"monthly"}},{"id":"jan","title":"仍是31","due":"2026-01-31","recurrence":{"rule":"monthly"}}]}"#,
    );
    let todos = TodoCommands::open(store);
    todos.boot().unwrap();
    assert!(
        todos
            .item("clamped")
            .unwrap()
            .item
            .recurrence
            .unwrap()
            .month_day
            .is_none()
    );
    todos.complete_item("clamped").unwrap();
    let lists = todos.lists().unwrap();
    let after_clamp = lists
        .iter()
        .flat_map(|list| &list.items)
        .find(|item| !item.completed && item.title == "已在月末")
        .unwrap();
    assert_eq!(after_clamp.due, Some(date(2026, 3, 28)));
    assert_eq!(
        after_clamp
            .recurrence
            .as_ref()
            .and_then(|rule| rule.month_day),
        Some(28)
    );

    todos.complete_item("jan").unwrap();
    let lists = todos.lists().unwrap();
    let february = lists
        .iter()
        .flat_map(|list| &list.items)
        .find(|item| !item.completed && item.title == "仍是31")
        .unwrap();
    assert_eq!(february.due, Some(date(2026, 2, 28)));
    assert_eq!(
        february.recurrence.as_ref().and_then(|rule| rule.month_day),
        Some(31)
    );
    let february_id = february.id.clone();
    todos.complete_item(&february_id).unwrap();
    let lists = todos.lists().unwrap();
    let march = lists
        .iter()
        .flat_map(|list| &list.items)
        .find(|item| !item.completed && item.title == "仍是31")
        .unwrap();
    assert_eq!(march.due, Some(date(2026, 3, 31)));
}

#[test]
fn monthly_non_31_missing_day_does_not_guess() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let mut draft = new_todo("每月 30 日");
    draft.due = Some(date(2026, 1, 30));
    draft.recurrence = Some(recurrence(RecurrenceRule::Monthly, None));
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    let path = fixture
        .store
        .document_path(&DocumentId::Todo(list_id))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let rx = fixture.store.subscribe();
    let err = fixture.todos.complete_item(&item_id).unwrap_err();
    assert_eq!(
        err.pending_topic(),
        Some(PendingTopic::MonthlyMissingDay),
        "{err}"
    );
    assert!(!fixture.todos.item(&item_id).unwrap().item.completed);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(rx.try_recv().is_err());
}

#[test]
fn complete_on_until_does_not_generate_the_next() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let mut draft = new_todo("截止日当天");
    draft.due = Some(date(2026, 3, 2));
    draft.recurrence = Some(recurrence(RecurrenceRule::Daily, Some(date(2026, 3, 2))));
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    fixture.todos.complete_item(&item_id).unwrap();
    let lists = fixture.todos.lists().unwrap();
    let items = &list(&lists, &list_id).items;
    assert_eq!(items.len(), 1);
    assert!(items[0].completed);
    assert_eq!(items[0].id, item_id);
    assert!(items[0].generated_next.is_none());
    fixture.todos.uncomplete_item(&item_id).unwrap();
    let stored = fixture.todos.item(&item_id).unwrap();
    assert!(!stored.item.completed);
    assert!(stored.item.ever_completed);
    assert_eq!(
        list(&fixture.todos.lists().unwrap(), &list_id).items.len(),
        1
    );
}

fn item_ids(lists: &[TodoList], list_id: &str) -> Vec<String> {
    list(lists, list_id)
        .items
        .iter()
        .map(|item| item.id.clone())
        .collect()
}

#[test]
fn uncomplete_puts_the_item_back_in_its_previous_place() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let before = fixture
        .todos
        .create_item(&list_id, new_todo("前面"))
        .unwrap();
    let item_id = fixture
        .todos
        .create_item(&list_id, new_todo("这条"))
        .unwrap();
    let after = fixture
        .todos
        .create_item(&list_id, new_todo("后面"))
        .unwrap();
    fixture.todos.complete_item(&item_id).unwrap();
    assert!(fixture.todos.item(&item_id).unwrap().item.completed);
    assert_eq!(
        item_ids(&fixture.todos.lists().unwrap(), &list_id),
        [before.clone(), item_id.clone(), after.clone()]
    );
    fixture.todos.uncomplete_item(&item_id).unwrap();
    let stored = fixture.todos.item(&item_id).unwrap();
    assert!(!stored.item.completed);
    assert!(stored.item.generated_next.is_none());
    let fixture = fixture.reboot();
    assert_eq!(
        item_ids(&fixture.todos.lists().unwrap(), &list_id),
        [before, item_id, after]
    );
}

#[test]
fn uncomplete_recurring_drops_only_the_untouched_generated_next() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let before = fixture
        .todos
        .create_item(&list_id, new_todo("前面"))
        .unwrap();
    let mut draft = new_todo("周期");
    draft.due = Some(date(2026, 3, 1));
    draft.remind_at = ClockTime::try_new(9, 0);
    draft.recurrence = Some(recurrence(RecurrenceRule::Daily, None));
    draft.source = Some(source_pr());
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    let after = fixture
        .todos
        .create_item(&list_id, new_todo("后面"))
        .unwrap();
    fixture.todos.complete_item(&item_id).unwrap();
    let recorded = fixture
        .todos
        .item(&item_id)
        .unwrap()
        .item
        .generated_next
        .unwrap();
    let next_id = recorded.id.clone();
    assert_eq!(recorded.title, "周期");
    assert_eq!(recorded.due, Some(date(2026, 3, 2)));
    assert_eq!(recorded.remind_at, ClockTime::try_new(9, 0));
    assert_eq!(recorded.source, Some(source_pr()));
    let next = fixture.todos.item(&next_id).unwrap();
    assert!(next.item.generated_untouched);
    assert!(!next.item.ever_completed);
    assert_eq!(next.item.due, Some(date(2026, 3, 2)));
    fixture.todos.rename_item(&item_id, "完成后改标题").unwrap();
    assert_eq!(
        item_ids(&fixture.todos.lists().unwrap(), &list_id),
        [
            before.clone(),
            item_id.clone(),
            next_id.clone(),
            after.clone()
        ]
    );
    let open = fixture
        .todos
        .index_snapshot()
        .unwrap()
        .into_iter()
        .map(|entry| entry.item_id)
        .collect::<Vec<_>>();
    assert!(open.contains(&next_id));
    assert!(!open.contains(&item_id));
    fixture.todos.uncomplete_item(&item_id).unwrap();
    assert!(matches!(
        fixture.todos.item(&next_id).unwrap_err(),
        TodoError::ItemNotFound { .. }
    ));
    let stored = fixture.todos.item(&item_id).unwrap();
    assert!(!stored.item.completed);
    assert!(stored.item.ever_completed);
    assert!(stored.item.generated_next.is_none());
    assert_eq!(stored.item.title, "完成后改标题");
    let fixture = fixture.reboot();
    assert_eq!(
        item_ids(&fixture.todos.lists().unwrap(), &list_id),
        [before, item_id.clone(), after]
    );
    let indexed = fixture
        .todos
        .index_snapshot()
        .unwrap()
        .into_iter()
        .map(|entry| entry.item_id)
        .collect::<Vec<_>>();
    assert!(indexed.contains(&item_id));
    fixture.todos.complete_item(&item_id).unwrap();
    let again = fixture
        .todos
        .item(&item_id)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    assert_ne!(again, next_id);
    assert_eq!(
        fixture.todos.item(&again).unwrap().item.due,
        Some(date(2026, 3, 2))
    );
}

#[test]
fn uncomplete_keeps_a_generated_next_that_was_edited_or_finished() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let mut draft = new_todo("周期");
    draft.due = Some(date(2026, 3, 1));
    draft.recurrence = Some(recurrence(RecurrenceRule::Daily, None));
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    fixture.todos.complete_item(&item_id).unwrap();
    let next_id = fixture
        .todos
        .item(&item_id)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    fixture.todos.rename_item(&next_id, "改了").unwrap();
    fixture.todos.rename_item(&next_id, "周期").unwrap();
    fixture.todos.uncomplete_item(&item_id).unwrap();
    assert!(!fixture.todos.item(&item_id).unwrap().item.completed);
    assert!(
        fixture
            .todos
            .item(&item_id)
            .unwrap()
            .item
            .generated_next
            .is_none()
    );
    assert_eq!(fixture.todos.item(&next_id).unwrap().item.title, "周期");
    assert!(
        !fixture
            .todos
            .item(&next_id)
            .unwrap()
            .item
            .generated_untouched
    );

    fixture.todos.complete_item(&item_id).unwrap();
    let next_id = fixture
        .todos
        .item(&item_id)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    fixture.todos.complete_item(&next_id).unwrap();
    let grandchild = fixture
        .todos
        .item(&next_id)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    fixture.todos.uncomplete_item(&item_id).unwrap();
    assert!(fixture.todos.item(&next_id).unwrap().item.completed);
    assert_eq!(
        fixture.todos.item(&grandchild).unwrap().item.due,
        Some(date(2026, 3, 3))
    );

    fixture.todos.uncomplete_item(&next_id).unwrap();
    assert!(matches!(
        fixture.todos.item(&grandchild).unwrap_err(),
        TodoError::ItemNotFound { .. }
    ));
    assert!(fixture.todos.item(&next_id).unwrap().item.ever_completed);
    assert!(!fixture.todos.item(&next_id).unwrap().item.completed);
    fixture.todos.complete_item(&item_id).unwrap();
    let third = fixture
        .todos
        .item(&item_id)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    fixture.todos.complete_item(&third).unwrap();
    fixture.todos.uncomplete_item(&third).unwrap();
    fixture.todos.uncomplete_item(&item_id).unwrap();
    assert_eq!(fixture.todos.item(&third).unwrap().item.title, "周期");
}

#[test]
fn uncomplete_keeps_a_generated_next_that_was_moved_reordered_or_trashed() {
    let fixture = Fixture::new();
    let inbox = fixture.todos.ensure_inbox().unwrap();
    let other = fixture.todos.create_list("其他").unwrap();
    let mut draft = new_todo("周期");
    draft.due = Some(date(2026, 3, 1));
    draft.recurrence = Some(recurrence(RecurrenceRule::Daily, None));
    let moved_parent = fixture.todos.create_item(&inbox, draft.clone()).unwrap();
    fixture.todos.complete_item(&moved_parent).unwrap();
    let moved_next = fixture
        .todos
        .item(&moved_parent)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    fixture.todos.process_move(&moved_next, &other).unwrap();
    fixture.todos.uncomplete_item(&moved_parent).unwrap();
    assert_eq!(fixture.todos.item(&moved_next).unwrap().list_id, other);

    let current_parent = fixture.todos.create_item(&inbox, draft.clone()).unwrap();
    fixture.todos.complete_item(&current_parent).unwrap();
    let current_next = fixture
        .todos
        .item(&current_parent)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    fixture.todos.set_current(&current_next).unwrap();
    fixture.todos.clear_current().unwrap();
    fixture.todos.uncomplete_item(&current_parent).unwrap();
    assert!(!fixture.todos.item(&current_next).unwrap().item.current);

    let trash_parent = fixture.todos.create_item(&inbox, draft.clone()).unwrap();
    fixture.todos.complete_item(&trash_parent).unwrap();
    let trash_next = fixture
        .todos
        .item(&trash_parent)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    fixture.todos.soft_delete(&trash_next).unwrap();
    fixture.todos.uncomplete_item(&trash_parent).unwrap();
    assert!(fixture.todos.item(&trash_next).unwrap().item.in_trash());

    let reorder_parent = fixture.todos.create_item(&inbox, draft).unwrap();
    let tail = fixture.todos.create_item(&inbox, new_todo("尾")).unwrap();
    fixture.todos.complete_item(&reorder_parent).unwrap();
    let reorder_next = fixture
        .todos
        .item(&reorder_parent)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    let mut order = item_ids(&fixture.todos.lists().unwrap(), &inbox);
    let next_at = order.iter().position(|id| id == &reorder_next).unwrap();
    order.remove(next_at);
    order.push(reorder_next.clone());
    let parent_at = order.iter().position(|id| id == &reorder_parent).unwrap();
    assert_ne!(order[parent_at + 1], reorder_next);
    fixture.todos.reorder_items(&inbox, &order).unwrap();
    fixture.todos.uncomplete_item(&reorder_parent).unwrap();
    assert!(!fixture.todos.item(&reorder_parent).unwrap().item.completed);
    assert_eq!(
        fixture.todos.item(&reorder_next).unwrap().item.title,
        "周期"
    );
    assert!(item_ids(&fixture.todos.lists().unwrap(), &inbox).contains(&tail));
}

#[test]
fn uncomplete_without_a_generation_record_does_not_guess() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths).unwrap();
    write_raw(
        &store,
        "work",
        r#"{"schemaVersion":1,"id":"work","name":"工作","kind":"normal","items":[{"id":"done","title":"旧","completed":true,"due":"2026-03-01","recurrence":{"rule":"daily"}},{"id":"next","title":"旧","due":"2026-03-02","recurrence":{"rule":"daily"}}]}"#,
    );
    let todos = TodoCommands::open(store);
    todos.boot().unwrap();
    todos.uncomplete_item("done").unwrap();
    assert!(!todos.item("done").unwrap().item.completed);
    assert!(todos.item("done").unwrap().item.generated_next.is_none());
    assert_eq!(todos.item("next").unwrap().item.due, Some(date(2026, 3, 2)));
}

#[test]
fn uncomplete_rejects_open_and_trashed_items_without_writing() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let open = fixture
        .todos
        .create_item(&list_id, new_todo("未完成"))
        .unwrap();
    let path = fixture
        .store
        .document_path(&DocumentId::Todo(list_id.clone()))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let rx = fixture.store.subscribe();
    let err = fixture.todos.uncomplete_item(&open).unwrap_err();
    assert!(matches!(err, TodoError::NotComplete), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(rx.try_recv().is_err());

    let done = fixture
        .todos
        .create_item(&list_id, new_todo("已完成"))
        .unwrap();
    fixture.todos.complete_item(&done).unwrap();
    fixture.todos.soft_delete(&done).unwrap();
    let before = std::fs::read(&path).unwrap();
    let rx = fixture.store.subscribe();
    let err = fixture.todos.uncomplete_item(&done).unwrap_err();
    assert!(matches!(err, TodoError::AlreadyInTrash), "{err}");
    assert!(fixture.todos.item(&done).unwrap().item.completed);
    assert!(fixture.todos.item(&done).unwrap().item.in_trash());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(rx.try_recv().is_err());
}

#[test]
fn uncomplete_write_failure_rolls_back() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let mut draft = new_todo("周期");
    draft.due = Some(date(2026, 3, 1));
    draft.recurrence = Some(recurrence(RecurrenceRule::Daily, None));
    let item_id = fixture.todos.create_item(&list_id, draft).unwrap();
    fixture.todos.complete_item(&item_id).unwrap();
    let next_id = fixture
        .todos
        .item(&item_id)
        .unwrap()
        .item
        .generated_next
        .unwrap()
        .id;
    let path = fixture
        .store
        .document_path(&DocumentId::Todo(list_id))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let rx = fixture.store.subscribe();
    let _block = BlockReplace::on(&path);
    let err = fixture.todos.uncomplete_item(&item_id).unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert!(fixture.todos.item(&item_id).unwrap().item.completed);
    assert_eq!(
        fixture
            .todos
            .item(&item_id)
            .unwrap()
            .item
            .generated_next
            .unwrap()
            .id,
        next_id
    );
    assert_eq!(
        fixture.todos.item(&next_id).unwrap().item.due,
        Some(date(2026, 3, 2))
    );
    assert!(rx.try_recv().is_err());
    drop(_block);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn soft_delete_restores_to_the_origin_list() {
    let fixture = Fixture::new();
    let origin = fixture.todos.create_list("原来").unwrap();
    let other = fixture.todos.create_list("其他").unwrap();
    let item_id = fixture
        .todos
        .create_item(&origin, new_todo("可恢复"))
        .unwrap();
    fixture.todos.soft_delete(&item_id).unwrap();
    let stored = fixture.todos.item(&item_id).unwrap();
    assert!(stored.item.deleted_at.is_some());
    assert_eq!(stored.item.origin_list_id.as_deref(), Some(origin.as_str()));
    assert!(fixture.todos.index_snapshot().unwrap().is_empty());
    fixture.todos.restore(&item_id).unwrap();
    let stored = fixture.todos.item(&item_id).unwrap();
    assert_eq!(stored.list_id, origin);
    assert!(stored.item.deleted_at.is_none());
    assert!(stored.item.origin_list_id.is_none());

    fixture.todos.soft_delete(&item_id).unwrap();
    let mut lists = fixture.todos.lists().unwrap();
    let from = lists.iter_mut().find(|item| item.id == origin).unwrap();
    let mut moved = from.items.pop().unwrap();
    moved.origin_list_id = Some(origin.clone());
    lists
        .iter_mut()
        .find(|item| item.id == other)
        .unwrap()
        .items
        .push(moved);
    for list in &lists {
        fixture
            .store
            .write_json(&DocumentId::Todo(list.id.clone()), list)
            .unwrap();
    }
    let fixture = fixture.reboot();
    fixture.todos.restore(&item_id).unwrap();
    let stored = fixture.todos.item(&item_id).unwrap();
    assert_eq!(stored.list_id, origin);
    assert!(stored.item.deleted_at.is_none());
    assert!(
        list(&fixture.todos.lists().unwrap(), &other)
            .items
            .iter()
            .all(|item| item.id != item_id)
    );
}

#[test]
fn restore_without_origin_list_goes_to_the_inbox() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths).unwrap();
    let recent = now_ms();
    write_raw(
        &store,
        "hold",
        &format!(
            r#"{{"schemaVersion":1,"id":"hold","name":"暂存","kind":"normal","items":[{{"id":"a","title":"无家可归","deletedAt":{recent},"originListId":"missing"}}]}}"#
        ),
    );
    let todos = TodoCommands::open(store);
    todos.boot().unwrap();
    todos.restore("a").unwrap();
    let stored = todos.item("a").unwrap();
    assert_eq!(stored.list_name, "收件箱");
    assert_eq!(stored.item.title, "无家可归");
    assert!(stored.item.deleted_at.is_none());
    assert!(stored.item.origin_list_id.is_none());
    let lists = todos.lists().unwrap();
    assert!(
        lists.iter().any(
            |list| list.kind == ListKind::Inbox && list.items.iter().any(|item| item.id == "a")
        )
    );
    assert!(
        lists
            .iter()
            .find(|list| list.id == "hold")
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn purge_only_removes_trash_and_emits_after_the_write() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let active = fixture
        .todos
        .create_item(&list_id, new_todo("还在清单"))
        .unwrap();
    let trashed = fixture
        .todos
        .create_item(&list_id, new_todo("在回收站"))
        .unwrap();
    fixture.todos.soft_delete(&trashed).unwrap();
    let rx = fixture.todos.subscribe_notices();
    let err = fixture.todos.purge(&active).unwrap_err();
    assert!(matches!(err, TodoError::NotInTrash), "{err}");
    assert!(rx.try_recv().is_err());
    assert_eq!(fixture.todos.item(&active).unwrap().item.title, "还在清单");

    let changes = fixture.store.subscribe();
    fixture.todos.purge(&trashed).unwrap();
    let notice = rx.try_recv().unwrap();
    assert_eq!(
        notice,
        lanwork_core::todos::TodoNotice::Purged {
            item_id: trashed.clone(),
        }
    );
    assert!(changes.try_recv().is_ok());
    assert!(matches!(
        fixture.todos.item(&trashed).unwrap_err(),
        TodoError::ItemNotFound { .. }
    ));
    assert!(
        list(&fixture.todos.lists().unwrap(), &list_id)
            .items
            .iter()
            .all(|item| item.id != trashed)
    );
}

#[test]
fn trash_older_than_thirty_days_is_purged_on_boot() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths).unwrap();
    let deleted_at = now_ms() - TRASH_RETENTION_MS;
    write_raw(
        &store,
        "work",
        &format!(
            r#"{{"schemaVersion":1,"id":"work","name":"工作","kind":"normal","items":[{{"id":"old","title":"过期","deletedAt":{deleted_at}}},{{"id":"fresh","title":"还在","deletedAt":{fresh}}}]}}"#,
            fresh = deleted_at + TRASH_RETENTION_MS
        ),
    );
    let todos = TodoCommands::open(store.clone());
    let rx = todos.subscribe_notices();
    todos.boot().unwrap();
    assert!(matches!(
        todos.item("old").unwrap_err(),
        TodoError::ItemNotFound { .. }
    ));
    assert!(todos.item("fresh").unwrap().item.deleted_at.is_some());
    let notice = rx.try_recv().unwrap();
    assert_eq!(
        notice,
        lanwork_core::todos::TodoNotice::Purged {
            item_id: "old".into()
        }
    );
    let disk: TodoList = store
        .read_json(&DocumentId::Todo("work".into()))
        .unwrap()
        .unwrap();
    assert!(disk.items.iter().all(|item| item.id != "old"));
    assert!(disk.items.iter().any(|item| item.id == "fresh"));
    let pending = store
        .data_dir()
        .join(lanwork_core::todos::PURGE_PENDING_FILE);
    let recorded = std::fs::read_to_string(&pending).unwrap();
    assert!(recorded.contains("\"old\""), "{recorded}");
    assert!(!recorded.contains("\"fresh\""), "{recorded}");
}

#[test]
fn unreadable_purge_record_does_not_fail_boot_or_drop_the_file() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths).unwrap();
    let deleted_at = now_ms() - TRASH_RETENTION_MS;
    write_raw(
        &store,
        "work",
        &format!(
            r#"{{"schemaVersion":1,"id":"work","name":"工作","kind":"normal","items":[{{"id":"old","title":"过期","deletedAt":{deleted_at}}}]}}"#
        ),
    );
    let pending = store
        .data_dir()
        .join(lanwork_core::todos::PURGE_PENDING_FILE);
    std::fs::write(&pending, b"{").unwrap();
    let todos = TodoCommands::open(store);
    todos.boot().unwrap();
    assert_eq!(todos.item("old").unwrap().item.title, "过期");
    assert_eq!(std::fs::read(&pending).unwrap(), b"{");
}

#[test]
fn http_sources_open_and_other_schemes_are_rejected() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let item_id = fixture
        .todos
        .create_item(&list_id, new_todo("来源"))
        .unwrap();
    for url in [
        "javascript:alert(1)",
        "file:///C:/Windows",
        "ftp://example.com",
        "http://",
    ] {
        let err = fixture
            .todos
            .set_source(&item_id, SourceKind::GithubIssue, url, "wynxing/Lanwork", 1)
            .unwrap_err();
        assert!(
            matches!(err, TodoError::RejectedSource | TodoError::InvalidSource),
            "{url} {err}"
        );
        assert!(fixture.todos.item(&item_id).unwrap().item.source.is_none());
    }
    let https = "https://github.com/wynxing/Lanwork/issues/12";
    fixture
        .todos
        .set_source(
            &item_id,
            SourceKind::GithubIssue,
            https,
            "wynxing/Lanwork",
            12,
        )
        .unwrap();
    assert_eq!(fixture.todos.open_source(&item_id).unwrap(), https);
    fixture
        .todos
        .set_source(
            &item_id,
            SourceKind::GithubPr,
            "HTTP://example.com/pr",
            "wynxing/Lanwork",
            3,
        )
        .unwrap();
    assert_eq!(
        fixture.todos.open_source(&item_id).unwrap(),
        "HTTP://example.com/pr"
    );

    let mut stored = fixture.todos.lists().unwrap();
    let list = stored.iter_mut().find(|item| item.id == list_id).unwrap();
    list.items[0].source = Some(TodoSource {
        kind: SourceKind::GithubIssue,
        url: "javascript:alert(1)".into(),
        repo: "wynxing/Lanwork".into(),
        number: 1,
    });
    fixture
        .store
        .write_json(&DocumentId::Todo(list_id.clone()), list)
        .unwrap();
    let fixture = fixture.reboot();
    let err = fixture.todos.open_source(&item_id).unwrap_err();
    assert!(matches!(err, TodoError::RejectedSource), "{err}");
    assert_eq!(
        fixture
            .todos
            .item(&item_id)
            .unwrap()
            .item
            .source
            .unwrap()
            .url,
        "javascript:alert(1)"
    );
}

#[test]
fn current_marker_replaces_the_previous_one_and_survives_reload() {
    let fixture = Fixture::new();
    let first_list = fixture.todos.create_list("甲").unwrap();
    let second_list = fixture.todos.create_list("乙").unwrap();
    let first = fixture
        .todos
        .create_item(&first_list, new_todo("一"))
        .unwrap();
    let second = fixture
        .todos
        .create_item(&second_list, new_todo("二"))
        .unwrap();
    let rx = fixture.store.subscribe();
    fixture.todos.set_current(&first).unwrap();
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, first_list);

    let rx = fixture.store.subscribe();
    fixture.todos.set_current(&second).unwrap();
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec![second_list.as_str(), first_list.as_str()]
    );
    assert!(fixture.todos.item(&second).unwrap().item.current);
    assert!(
        fixture
            .todos
            .item(&second)
            .unwrap()
            .item
            .current_since
            .is_some()
    );
    assert!(!fixture.todos.item(&first).unwrap().item.current);
    let rx = fixture.store.subscribe();
    fixture.todos.set_current(&second).unwrap();
    assert!(rx.try_recv().is_err());
    fixture.todos.clear_current().unwrap();
    assert!(!fixture.todos.item(&second).unwrap().item.current);

    fixture.todos.set_current(&first).unwrap();
    let fixture = fixture.reboot();
    assert!(fixture.todos.item(&first).unwrap().item.current);
    assert!(!fixture.todos.item(&second).unwrap().item.current);
}

#[test]
fn same_list_current_switch_publishes_after_both_writes() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let first = fixture.todos.create_item(&list_id, new_todo("一")).unwrap();
    let second = fixture.todos.create_item(&list_id, new_todo("二")).unwrap();
    fixture.todos.set_current(&first).unwrap();
    let rx = fixture.store.subscribe();
    fixture.todos.set_current(&second).unwrap();
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.id == list_id));
    assert!(fixture.todos.item(&second).unwrap().item.current);
    assert!(!fixture.todos.item(&first).unwrap().item.current);
}

#[test]
fn move_preserves_id_and_github_source_and_publishes_target_first() {
    let fixture = Fixture::new();
    let source_list = fixture.todos.create_list("来源").unwrap();
    let target_list = fixture.todos.create_list("目标").unwrap();
    let mut draft = new_todo("带走来源");
    draft.source = Some(source_pr());
    let item_id = fixture.todos.create_item(&source_list, draft).unwrap();
    fixture.todos.set_current(&item_id).unwrap();
    let rx = fixture.store.subscribe();
    let missing = fixture.todos.process_move(&item_id, &target_list);
    assert!(matches!(missing.unwrap_err(), TodoError::NotInboxActive));
    assert!(rx.try_recv().is_err());
    assert_eq!(fixture.todos.item(&item_id).unwrap().list_id, source_list);

    let inbox = fixture.todos.ensure_inbox().unwrap();
    let mut draft = new_todo("收件箱里的来源");
    draft.source = Some(source_pr());
    let item_id = fixture.todos.create_item(&inbox, draft).unwrap();
    let rx = fixture.store.subscribe();
    fixture.todos.process_move(&item_id, &target_list).unwrap();
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec![target_list.as_str(), inbox.as_str()]
    );
    let stored = fixture.todos.item(&item_id).unwrap();
    assert_eq!(stored.list_id, target_list);
    assert_eq!(stored.item.id, item_id);
    assert_eq!(stored.item.source, Some(source_pr()));
    assert!(stored.item.moved_at.is_some());
    assert!(
        list(&fixture.todos.lists().unwrap(), &inbox)
            .items
            .iter()
            .all(|item| item.id != item_id)
    );
}

#[test]
fn process_mode_updates_inbox_items_and_cursor_query_skips_inactive_ones() {
    let fixture = Fixture::new();
    let inbox = fixture.todos.ensure_inbox().unwrap();
    let outside = fixture.todos.create_list("工作").unwrap();
    let outside_item = fixture
        .todos
        .create_item(&outside, new_todo("外面"))
        .unwrap();
    let today = date(2026, 10, 8);
    assert!(matches!(
        fixture
            .todos
            .process_today(&outside_item, today)
            .unwrap_err(),
        TodoError::NotInboxActive
    ));

    let first = fixture.todos.create_item(&inbox, new_todo("一")).unwrap();
    let second = fixture.todos.create_item(&inbox, new_todo("二")).unwrap();
    let third = fixture.todos.create_item(&inbox, new_todo("三")).unwrap();
    fixture.todos.process_today(&first, today).unwrap();
    assert_eq!(fixture.todos.item(&first).unwrap().item.due, Some(today));
    fixture.todos.process_defer(&first, 1, today).unwrap();
    assert_eq!(
        fixture.todos.item(&first).unwrap().item.due,
        Some(date(2026, 10, 9))
    );
    let mut with_due = new_todo("有到期日");
    with_due.due = Some(date(2026, 10, 1));
    let dated = fixture.todos.create_item(&inbox, with_due).unwrap();
    fixture.todos.process_defer(&dated, 30, today).unwrap();
    assert_eq!(
        fixture.todos.item(&dated).unwrap().item.due,
        Some(date(2026, 10, 31))
    );
    fixture
        .todos
        .process_defer(&second, DEFAULT_DEFER_DAYS, today)
        .unwrap();
    assert_eq!(
        fixture.todos.item(&second).unwrap().item.due,
        Some(date(2026, 10, 11))
    );
    for days in [0, 31] {
        let before = fixture.todos.item(&third).unwrap().item.due;
        let err = fixture
            .todos
            .process_defer(&third, days, today)
            .unwrap_err();
        assert!(matches!(err, TodoError::InvalidDefer), "{days} {err}");
        assert_eq!(fixture.todos.item(&third).unwrap().item.due, before);
    }

    let work = fixture.todos.create_list("处理").unwrap();
    fixture.todos.process_move(&first, &work).unwrap();
    assert_eq!(fixture.todos.item(&first).unwrap().list_id, work);
    fixture.todos.process_complete(&second).unwrap();
    assert!(fixture.todos.item(&second).unwrap().item.completed);
    fixture.todos.process_soft_delete(&dated).unwrap();
    assert!(fixture.todos.item(&dated).unwrap().item.in_trash());

    let mut active = [third.as_str()];
    active.sort_unstable();
    assert_eq!(
        fixture
            .todos
            .next_inbox_incomplete_after("")
            .unwrap()
            .as_deref(),
        Some(active[0])
    );
    assert!(
        fixture
            .todos
            .next_inbox_incomplete_after(&third)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        fixture.todos.process_today(&second, today).unwrap_err(),
        TodoError::NotInboxActive
    ));
    assert!(matches!(
        fixture.todos.process_defer(&dated, 1, today).unwrap_err(),
        TodoError::NotInboxActive
    ));
}

#[test]
fn overdue_count_and_next_reminder_follow_the_pure_functions() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let today = date(2026, 10, 8);
    let mut yesterday = new_todo("昨天");
    yesterday.due = Some(date(2026, 10, 7));
    yesterday.remind_at = ClockTime::try_new(8, 0);
    let overdue = fixture.todos.create_item(&list_id, yesterday).unwrap();
    let mut due_today = new_todo("今天");
    due_today.due = Some(today);
    due_today.remind_at = ClockTime::try_new(9, 0);
    let current = fixture.todos.create_item(&list_id, due_today).unwrap();
    fixture.todos.complete_item(&overdue).unwrap();
    assert_eq!(fixture.todos.overdue_count(today).unwrap(), 0);
    fixture.todos.soft_delete(&current).unwrap();
    assert_eq!(fixture.todos.overdue_count(today).unwrap(), 0);
    fixture.todos.restore(&current).unwrap();
    let mut late = new_todo("更早");
    late.due = Some(date(2026, 10, 1));
    let late_id = fixture.todos.create_item(&list_id, late).unwrap();
    assert_eq!(fixture.todos.overdue_count(today).unwrap(), 1);
    assert_eq!(
        fixture.todos.item(&late_id).unwrap().item.due,
        Some(date(2026, 10, 1))
    );
    let reminder = fixture
        .todos
        .next_reminder(&current, today, 9 * 60)
        .unwrap()
        .unwrap();
    assert_eq!(reminder.date, today);
    assert_eq!(reminder.time, ClockTime::try_new(9, 0).unwrap());
    assert!(
        fixture
            .todos
            .next_reminder(&current, today, 9 * 60 + 1)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        fixture.todos.set_reminder(&current, 24, 0).unwrap_err(),
        TodoError::InvalidReminder
    ));
}

#[test]
fn index_snapshot_skips_completed_and_trashed_items() {
    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let open = fixture
        .todos
        .create_item(&list_id, new_todo("未完成"))
        .unwrap();
    let done = fixture
        .todos
        .create_item(&list_id, new_todo("完成"))
        .unwrap();
    let trashed = fixture
        .todos
        .create_item(&list_id, new_todo("回收站"))
        .unwrap();
    fixture.todos.complete_item(&done).unwrap();
    fixture.todos.soft_delete(&trashed).unwrap();
    fixture.todos.set_current(&open).unwrap();
    let store = fixture.store.clone();
    let todos = fixture.todos.clone();
    let snap = store
        .build_index(|_| Ok(todos.index_snapshot().unwrap()))
        .unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].item_id, open);
    assert!(snap[0].current);
}

#[test]
fn interrupted_move_is_repaired_before_the_index_sees_data() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths).unwrap();
    write_raw(
        &store,
        "a",
        r#"{"schemaVersion":1,"id":"a","name":"源","kind":"normal","items":[{"id":"item","title":"旧","movedAt":1,"source":{"type":"github-issue","url":"https://github.com/wynxing/Lanwork/issues/12","repo":"wynxing/Lanwork","number":12}}]}"#,
    );
    write_raw(
        &store,
        "b",
        r#"{"schemaVersion":1,"id":"b","name":"目标","kind":"normal","items":[{"id":"item","title":"新","movedAt":2,"source":{"type":"github-issue","url":"https://github.com/wynxing/Lanwork/issues/12","repo":"wynxing/Lanwork","number":12}}]}"#,
    );
    assert!(store.build_index(|_| Ok(())).is_err());
    let todos = TodoCommands::open(store.clone());
    todos.boot().unwrap();
    let snap = store
        .build_index(|_| Ok(todos.index_snapshot().unwrap()))
        .unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].list_id, "b");
    assert_eq!(snap[0].item_id, "item");
    assert_eq!(snap[0].title, "新");
    assert_eq!(snap[0].source.as_ref().unwrap().number, 12);
    assert!(
        todos
            .lists()
            .unwrap()
            .iter()
            .find(|list| list.id == "a")
            .unwrap()
            .items
            .is_empty()
    );
    let source =
        std::fs::read_to_string(store.document_path(&DocumentId::Todo("a".into())).unwrap())
            .unwrap();
    assert!(!source.contains("\"id\":\"item\""));
    assert_eq!(todos.item("item").unwrap().list_id, "b");
}

#[test]
fn interrupted_current_switch_keeps_only_the_newer_marker() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths.clone()).unwrap();
    write_raw(
        &store,
        "a",
        r#"{"schemaVersion":1,"id":"a","name":"甲","kind":"normal","items":[{"id":"old","title":"旧","current":true,"currentSince":1}]}"#,
    );
    write_raw(
        &store,
        "b",
        r#"{"schemaVersion":1,"id":"b","name":"乙","kind":"normal","items":[{"id":"new","title":"新","current":true,"currentSince":9}]}"#,
    );
    assert!(store.build_index(|_| Ok(())).is_err());
    let todos = TodoCommands::open(store.clone());
    todos.boot().unwrap();
    let snap = store
        .build_index(|_| Ok(todos.index_snapshot().unwrap()))
        .unwrap();
    assert_eq!(snap.iter().filter(|entry| entry.current).count(), 1);
    assert_eq!(
        snap.iter().find(|entry| entry.current).unwrap().item_id,
        "new"
    );
    assert!(!todos.item("old").unwrap().item.current);
    assert!(todos.item("new").unwrap().item.current);
    assert_eq!(todos.item("new").unwrap().item.current_since, Some(9));

    let temp = TempDir::new();
    let store = Store::open(StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    })
    .unwrap();
    write_raw(
        &store,
        "one",
        r#"{"schemaVersion":1,"id":"one","name":"同一清单","kind":"normal","items":[{"id":"old","title":"旧","current":true,"currentSince":1},{"id":"new","title":"新","current":true,"currentSince":4}]}"#,
    );
    let todos = TodoCommands::open(store.clone());
    todos.boot().unwrap();
    let snap = store
        .build_index(|_| Ok(todos.index_snapshot().unwrap()))
        .unwrap();
    assert_eq!(snap.iter().filter(|entry| entry.current).count(), 1);
    assert!(!todos.item("old").unwrap().item.current);
    assert!(todos.item("new").unwrap().item.current);
}

#[test]
fn commands_before_boot_do_not_write() {
    let temp = TempDir::new();
    let store = Store::open(StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    })
    .unwrap();
    let todos = TodoCommands::open(store.clone());
    let err = todos.create_list("过早").unwrap_err();
    assert!(matches!(err, TodoError::NotLoaded), "{err}");
    assert!(store.build_index(|_| Ok(())).is_err());
    let dir = store.collection_dir(CollectionKind::Todos);
    let json = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"));
    assert!(!json);
}

#[test]
fn bad_list_files_do_not_drop_the_readable_ones_or_invent_an_inbox() {
    let temp = TempDir::new();
    let store = Store::open(StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    })
    .unwrap();
    write_raw(
        &store,
        "good",
        r#"{"schemaVersion":1,"id":"good","name":"好","kind":"normal","items":[{"id":"a","title":"在"}]}"#,
    );
    write_raw(&store, "bad", "{");
    let todos = TodoCommands::open(store.clone());
    todos.boot().unwrap();
    assert_eq!(todos.item("a").unwrap().item.title, "在");
    assert!(
        todos
            .lists()
            .unwrap()
            .iter()
            .all(|list| list.kind != ListKind::Inbox)
    );
    assert!(
        store
            .build_index(|_| Ok(todos.index_snapshot().unwrap()))
            .unwrap()
            .iter()
            .any(|entry| entry.item_id == "a")
    );
}

#[test]
fn mismatched_id_and_unsupported_schema_fail_boot() {
    let temp = TempDir::new();
    let paths = StorePaths {
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    };
    let store = Store::open(paths.clone()).unwrap();
    write_raw(
        &store,
        "l1",
        r#"{"schemaVersion":1,"id":"other","name":"错","kind":"normal"}"#,
    );
    let todos = TodoCommands::open(store);
    let err = todos.boot().unwrap_err();
    assert!(err.to_string().contains("不一致"), "{err}");

    let store = Store::open(paths).unwrap();
    std::fs::remove_file(store.document_path(&DocumentId::Todo("l1".into())).unwrap()).unwrap();
    write_raw(
        &store,
        "l2",
        r#"{"schemaVersion":99,"id":"l2","name":"新","kind":"normal"}"#,
    );
    let todos = TodoCommands::open(store.clone());
    let err = todos.boot().unwrap_err();
    assert!(err.to_string().contains("schemaVersion"), "{err}");
    assert!(!store.is_ready());
    assert!(store.build_index(|_| Ok(())).is_err());
}

/// 挡住第二次替换。Windows 上锁住目标文件；其他系统把旁边的 `.json.tmp` 做成目录，
/// 让这次替换在打开临时文件时失败，原文件保持不动。
struct BlockReplace {
    #[cfg(not(windows))]
    tmp: PathBuf,
    #[cfg(windows)]
    _file: std::fs::File,
}

impl BlockReplace {
    fn on(path: &Path) -> Self {
        #[cfg(windows)]
        {
            use std::fs::OpenOptions;
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ = 1。不带 FILE_SHARE_DELETE，替换必须失败。
            let _file = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(path)
                .unwrap();
            Self { _file }
        }
        #[cfg(not(windows))]
        {
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(".tmp");
            let tmp = PathBuf::from(tmp);
            std::fs::create_dir(&tmp).unwrap();
            Self { tmp }
        }
    }
}

impl Drop for BlockReplace {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        {
            let _ = std::fs::remove_dir(&self.tmp);
        }
    }
}

#[test]
fn later_write_failure_rolls_back_a_cross_list_move() {
    let fixture = Fixture::new();
    let inbox = fixture.todos.ensure_inbox().unwrap();
    let target = fixture.todos.create_list("工作").unwrap();
    let item_id = fixture.todos.create_item(&inbox, new_todo("搬走")).unwrap();
    let inbox_path = fixture
        .store
        .document_path(&DocumentId::Todo(inbox.clone()))
        .unwrap();
    let target_path = fixture
        .store
        .document_path(&DocumentId::Todo(target.clone()))
        .unwrap();
    let inbox_before = std::fs::read(&inbox_path).unwrap();
    let target_before = std::fs::read(&target_path).unwrap();
    let rx = fixture.store.subscribe();
    let _block = BlockReplace::on(&inbox_path);
    let err = fixture.todos.process_move(&item_id, &target).unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert_eq!(fixture.todos.item(&item_id).unwrap().list_id, inbox);
    assert!(
        list(&fixture.todos.lists().unwrap(), &target)
            .items
            .is_empty()
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(std::fs::read(&inbox_path).unwrap(), inbox_before);
    assert_eq!(std::fs::read(&target_path).unwrap(), target_before);
}

#[test]
fn later_write_failure_rolls_back_a_current_switch_across_lists() {
    let fixture = Fixture::new();
    let first_list = fixture.todos.create_list("甲").unwrap();
    let second_list = fixture.todos.create_list("乙").unwrap();
    let first = fixture
        .todos
        .create_item(&first_list, new_todo("旧当前"))
        .unwrap();
    let second = fixture
        .todos
        .create_item(&second_list, new_todo("新当前"))
        .unwrap();
    fixture.todos.set_current(&first).unwrap();
    let first_path = fixture
        .store
        .document_path(&DocumentId::Todo(first_list.clone()))
        .unwrap();
    let second_path = fixture
        .store
        .document_path(&DocumentId::Todo(second_list.clone()))
        .unwrap();
    let first_before = std::fs::read(&first_path).unwrap();
    let second_before = std::fs::read(&second_path).unwrap();
    let rx = fixture.store.subscribe();
    let _block = BlockReplace::on(&first_path);
    let err = fixture.todos.set_current(&second).unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    let stored_first = fixture.todos.item(&first).unwrap();
    let stored_second = fixture.todos.item(&second).unwrap();
    assert!(stored_first.item.current);
    assert!(!stored_second.item.current);
    assert!(rx.try_recv().is_err());
    assert_eq!(std::fs::read(&first_path).unwrap(), first_before);
    assert_eq!(std::fs::read(&second_path).unwrap(), second_before);
}

#[cfg(unix)]
#[test]
fn write_failure_keeps_memory_and_original_file() {
    use std::os::unix::fs::PermissionsExt;

    struct ResetMode {
        path: PathBuf,
        mode: u32,
    }

    impl Drop for ResetMode {
        fn drop(&mut self) {
            let _ =
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
        }
    }

    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let item_id = fixture
        .todos
        .create_item(&list_id, new_todo("原来"))
        .unwrap();
    let path = fixture
        .store
        .document_path(&DocumentId::Todo(list_id))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let rx = fixture.store.subscribe();
    let dir = path.parent().unwrap().to_path_buf();
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    let _reset = ResetMode {
        path: dir.clone(),
        mode,
    };
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let err = fixture.todos.rename_item(&item_id, "新标题").unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert_eq!(fixture.todos.item(&item_id).unwrap().item.title, "原来");
    assert!(rx.try_recv().is_err());
    drop(_reset);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[cfg(windows)]
#[test]
fn write_failure_keeps_memory_and_original_file() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = Fixture::new();
    let list_id = fixture.todos.create_list("工作").unwrap();
    let item_id = fixture
        .todos
        .create_item(&list_id, new_todo("原来"))
        .unwrap();
    let path = fixture
        .store
        .document_path(&DocumentId::Todo(list_id))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let rx = fixture.store.subscribe();
    // FILE_SHARE_READ = 1。不带 FILE_SHARE_DELETE，替换必须失败。
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let err = fixture.todos.rename_item(&item_id, "新标题").unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert_eq!(fixture.todos.item(&item_id).unwrap().item.title, "原来");
    assert!(rx.try_recv().is_err());
    drop(lock);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
