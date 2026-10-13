//! 收纳服务的验收测试。
//!
//! 分组数据写在临时目录里。用户文件另放一份，所有操作之后都还在原路径。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lanwork_core::shelves::{
    DEFAULT_GROUP_NAME, FloatGeometry, IncomingRef, ShelfCommands, ShelfError, normalize_path,
};
use lanwork_core::storage::{CollectionKind, DocumentId, EntityKind, Store, StorePaths};
use lanwork_core::todos::{
    NewTodo, PURGE_PENDING_FILE, TRASH_RETENTION_MS, TodoCommands, TodoError, TodoNotice,
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
        let path = std::env::temp_dir().join(format!("lanwork-shelves-{nanos}-{seq}"));
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
    _temp: TempDir,
    store: Store,
    user_dir: PathBuf,
}

fn fixture() -> Fixture {
    let temp = TempDir::new();
    let root = temp.path();
    let user_dir = root.join("user");
    std::fs::create_dir_all(&user_dir).unwrap();
    let store = Store::open(StorePaths {
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        user_profile: root.join("profile"),
        local_app_data: root.join("local"),
    })
    .unwrap();
    Fixture {
        _temp: temp,
        store,
        user_dir,
    }
}

fn commands(store: &Store) -> ShelfCommands {
    ShelfCommands::open_at(store.clone(), || 1_700).unwrap()
}

fn user_file(fix: &Fixture, name: &str, bytes: &[u8]) -> String {
    let path = fix.user_dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
    path.to_str().unwrap().to_owned()
}

fn incoming(path: &str, folder: bool) -> IncomingRef {
    IncomingRef {
        path: path.to_owned(),
        folder,
    }
}

fn assert_unchanged(path: &str, bytes: &[u8]) {
    assert_eq!(std::fs::read(path).unwrap(), bytes, "{path}");
}

fn shelf_json_count(store: &Store) -> usize {
    let dir = store.collection_dir(CollectionKind::Shelves);
    if !dir.is_dir() {
        return 0;
    }
    std::fs::read_dir(&dir)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .ok()
                .and_then(|entry| entry.path().extension().map(|ext| ext == "json"))
                .unwrap_or(false)
        })
        .count()
}

#[test]
fn drop_three_paths_then_delete_group_keeps_the_files() {
    let fix = fixture();
    let rx = fix.store.subscribe();
    let cmds = commands(&fix.store);
    let first = user_file(&fix, "a.txt", b"alpha");
    let second = user_file(&fix, "b.txt", b"beta");
    let folder = fix.user_dir.join("dir");
    std::fs::create_dir_all(&folder).unwrap();
    let nested = folder.join("c.txt");
    std::fs::write(&nested, b"gamma").unwrap();
    let folder_text = folder.to_str().unwrap().to_owned();
    let nested_text = nested.to_str().unwrap().to_owned();

    let added = cmds
        .add_refs(
            None,
            &[
                incoming(&first, false),
                incoming(&second, false),
                incoming(&folder_text, true),
            ],
        )
        .unwrap();
    let group = added.group.unwrap();
    assert_eq!(added.added, 3);
    assert_eq!(group.name, DEFAULT_GROUP_NAME);
    assert_eq!(group.refs.len(), 3);
    assert_eq!(group.refs[0].name, "a.txt");
    assert!(group.refs[2].folder);
    assert_eq!(group.refs[0].added_at, 1_700);
    assert_eq!(shelf_json_count(&fix.store), 1);
    let event = rx.try_recv().unwrap();
    assert_eq!(event.kind, EntityKind::Shelf);
    assert_eq!(event.id, group.id);
    assert_eq!(event.revision, None);
    assert!(rx.try_recv().is_err());

    let raw = std::fs::read_to_string(
        fix.store
            .document_path(&DocumentId::Shelf(group.id.clone()))
            .unwrap(),
    )
    .unwrap();
    assert!(raw.contains("\"schemaVersion\":1"), "{raw}");
    assert!(raw.contains("\"name\":\"收纳\""), "{raw}");
    assert!(!raw.contains("todoId"), "{raw}");

    cmds.remove_ref(&group.id, &first).unwrap();
    cmds.delete_group(&group.id).unwrap();
    assert!(cmds.list().unwrap().is_empty());
    assert_eq!(shelf_json_count(&fix.store), 0);
    assert_unchanged(&first, b"alpha");
    assert_unchanged(&second, b"beta");
    assert_unchanged(&nested_text, b"gamma");
    assert!(Path::new(&folder_text).is_dir());
}

#[test]
fn normalized_paths_dedup_inside_one_group_and_repeat_across_groups() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let added = cmds
        .add_refs(
            None,
            &[
                incoming(r"C:\A\b.txt", false),
                incoming(r"c:/a/B.TXT\", false),
                incoming(r"\\?\C:\A\B\..\b.txt", false),
            ],
        )
        .unwrap();
    let group = added.group.unwrap();
    assert_eq!(added.added, 1);
    assert_eq!(group.refs.len(), 1);
    assert_eq!(group.refs[0].path, r"C:\A\b.txt");
    assert_eq!(group.refs[0].name, "b.txt");

    let other = cmds.create_group("另一组").unwrap();
    let again = cmds
        .add_refs(Some(&other.id), &[incoming(r"c:/a/B.TXT\", false)])
        .unwrap();
    assert_eq!(again.added, 1);
    assert_eq!(again.group.unwrap().refs[0].path, r"c:\a\B.TXT");
    assert_eq!(cmds.get(&group.id).unwrap().refs.len(), 1);

    let verbatim = cmds
        .add_refs(
            Some(&other.id),
            &[incoming(r"\\?\UNC\server\share\a\..\b.txt", false)],
        )
        .unwrap();
    assert_eq!(verbatim.added, 1);
    assert_eq!(
        verbatim.group.unwrap().refs[1].path,
        r"\\server\share\b.txt"
    );
    assert_ne!(
        normalize_path(r"C:\A\x").unwrap().key,
        normalize_path(r"C:\AB\x").unwrap().key
    );
    assert_eq!(
        normalize_path(r"C:\A\..\AB\x").unwrap().key,
        normalize_path(r"C:\AB\x").unwrap().key
    );
    assert_eq!(
        normalize_path(r"C:\foo..\bar").unwrap().stored,
        r"C:\foo..\bar"
    );
}

#[test]
fn empty_library_creates_the_default_group_with_the_refs() {
    let fix = fixture();
    let rx = fix.store.subscribe();
    let cmds = commands(&fix.store);
    assert!(cmds.list().unwrap().is_empty());
    let file = user_file(&fix, "note.txt", b"keep");
    let added = cmds.add_refs(None, &[incoming(&file, false)]).unwrap();
    assert_eq!(added.group.unwrap().name, "收纳");
    assert_eq!(rx.try_iter().count(), 1);
    assert_unchanged(&file, b"keep");

    let named = cmds.create_group("工作").unwrap();
    let err = cmds.add_refs(None, &[incoming(&file, false)]).unwrap_err();
    assert!(matches!(err, ShelfError::TargetGroupRequired), "{err}");
    assert_eq!(cmds.list().unwrap().len(), 2);
    assert!(cmds.get(&named.id).unwrap().refs.is_empty());
}

#[test]
fn invalid_path_does_not_create_an_empty_default_group() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let file = user_file(&fix, "ok.txt", b"ok");
    let err = cmds
        .add_refs(
            None,
            &[
                incoming(&file, false),
                incoming(r"relative\file.txt", false),
            ],
        )
        .unwrap_err();
    assert!(matches!(err, ShelfError::InvalidPath { .. }), "{err}");
    assert!(cmds.list().unwrap().is_empty());
    assert_eq!(shelf_json_count(&fix.store), 0);
    assert_unchanged(&file, b"ok");
}

#[test]
fn failed_auto_create_leaves_no_group_and_no_event() {
    let fix = fixture();
    let rx = fix.store.subscribe();
    let cmds = commands(&fix.store);
    let file = user_file(&fix, "pending.txt", b"pending");
    let dir = fix.store.collection_dir(CollectionKind::Shelves);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::write(&dir, b"blocked").unwrap();
    let err = cmds.add_refs(None, &[incoming(&file, false)]).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("写入失败") || text.contains("无法创建目录"),
        "{text}"
    );
    assert!(cmds.list().unwrap().is_empty());
    assert!(rx.try_recv().is_err());
    assert!(!dir.is_dir());
    assert_unchanged(&file, b"pending");
}

#[cfg(unix)]
#[test]
fn failed_add_keeps_the_previous_refs() {
    use std::os::unix::fs::PermissionsExt;

    let fix = fixture();
    let rx = fix.store.subscribe();
    let cmds = commands(&fix.store);
    let original = user_file(&fix, "old.txt", b"old");
    let extra = user_file(&fix, "new.txt", b"new");
    let group = cmds
        .add_refs(None, &[incoming(&original, false)])
        .unwrap()
        .group
        .unwrap();
    assert!(rx.try_recv().is_ok());
    let path = fix
        .store
        .document_path(&DocumentId::Shelf(group.id.clone()))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let dir = path.parent().unwrap();
    let mode = std::fs::metadata(dir).unwrap().permissions().mode();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let err = cmds
        .add_refs(Some(&group.id), &[incoming(&extra, false)])
        .unwrap_err();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert_eq!(cmds.get(&group.id).unwrap().refs.len(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(rx.try_recv().is_err());
    assert_unchanged(&original, b"old");
    assert_unchanged(&extra, b"new");
}

#[cfg(windows)]
#[test]
fn failed_add_keeps_the_previous_refs() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let fix = fixture();
    let rx = fix.store.subscribe();
    let cmds = commands(&fix.store);
    let original = user_file(&fix, "old.txt", b"old");
    let extra = user_file(&fix, "new.txt", b"new");
    let group = cmds
        .add_refs(None, &[incoming(&original, false)])
        .unwrap()
        .group
        .unwrap();
    assert!(rx.try_recv().is_ok());
    let path = fix
        .store
        .document_path(&DocumentId::Shelf(group.id.clone()))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let err = cmds
        .add_refs(Some(&group.id), &[incoming(&extra, false)])
        .unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert_eq!(cmds.get(&group.id).unwrap().refs.len(), 1);
    assert!(rx.try_recv().is_err());
    drop(lock);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_unchanged(&original, b"old");
    assert_unchanged(&extra, b"new");
}

#[test]
fn groups_can_be_renamed_reordered_and_reloaded() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let first = cmds.create_group("  甲  ").unwrap();
    let second = cmds.create_group("乙").unwrap();
    assert_eq!(first.name, "甲");
    assert!(cmds.create_group("   ").is_err());
    cmds.reorder_groups(&[second.id.clone(), first.id.clone()])
        .unwrap();
    let listed = cmds.list().unwrap();
    assert_eq!(listed[0].id, second.id);
    assert_eq!(listed[0].order, 0);
    assert_eq!(listed[1].order, 1);
    let renamed = cmds.rename_group(&first.id, "甲二").unwrap();
    assert_eq!(renamed.name, "甲二");
    let reloaded = ShelfCommands::open(fix.store.clone()).unwrap();
    assert_eq!(reloaded.get(&first.id).unwrap().name, "甲二");
    assert_eq!(reloaded.list().unwrap()[0].id, second.id);
}

fn geometry(x: i32) -> FloatGeometry {
    FloatGeometry {
        x,
        y: -20,
        width: 320,
        height: 360,
    }
}

#[test]
fn float_geometry_is_per_group_survives_other_edits_and_reloads() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let first = cmds.create_group("甲").unwrap();
    let second = cmds.create_group("乙").unwrap();
    assert_eq!(first.float, None);
    let file = user_file(&fix, "a.txt", b"a");

    cmds.set_float(&first.id, geometry(100), None).unwrap();
    cmds.set_float(&second.id, geometry(900), None).unwrap();
    cmds.add_refs(Some(&first.id), &[incoming(&file, false)])
        .unwrap();
    cmds.rename_group(&first.id, "甲二").unwrap();
    cmds.link_todo(&first.id, "todo-1").unwrap();
    cmds.reorder_groups(&[second.id.clone(), first.id.clone()])
        .unwrap();

    let reloaded = ShelfCommands::open(fix.store.clone()).unwrap();
    let got = reloaded.get(&first.id).unwrap();
    assert_eq!(got.float, Some(geometry(100)));
    assert_eq!(got.name, "甲二");
    assert_eq!(got.refs.len(), 1);
    assert_eq!(reloaded.get(&second.id).unwrap().float, Some(geometry(900)));
    assert_unchanged(&file, b"a");
}

#[test]
fn setting_the_same_float_geometry_writes_nothing_and_a_bad_id_is_refused() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let group = cmds.create_group("甲").unwrap();
    cmds.set_float(&group.id, geometry(5), None).unwrap();
    let events = fix.store.subscribe();
    cmds.set_float(&group.id, geometry(5), None).unwrap();
    assert!(
        events.try_recv().is_err(),
        "an unchanged value is not a write"
    );
    cmds.set_float(&group.id, geometry(6), None).unwrap();
    assert_eq!(events.try_recv().unwrap().kind, EntityKind::Shelf);
    assert!(cmds.set_float("../x", geometry(1), None).is_err());
    assert!(matches!(
        cmds.set_float("missing", geometry(1), None).unwrap_err(),
        ShelfError::NotFound { .. }
    ));
}

#[test]
fn a_file_written_before_float_windows_existed_still_opens_and_keeps_its_shape() {
    let fix = fixture();
    let path = fix
        .store
        .document_path(&DocumentId::Shelf("old".into()))
        .unwrap();
    std::fs::write(
        &path,
        r#"{"schemaVersion":1,"id":"old","name":"旧","order":0,"refs":[]}"#,
    )
    .unwrap();
    let cmds = ShelfCommands::open(fix.store.clone()).unwrap();
    let group = cmds.get("old").unwrap();
    assert_eq!(group.float, None);
    let unwritten = std::fs::read_to_string(&path).unwrap();
    assert!(!unwritten.contains("floatWindow"), "{unwritten}");
    cmds.rename_group("old", "旧二").unwrap();
    let rewritten = std::fs::read_to_string(&path).unwrap();
    assert!(!rewritten.contains("floatWindow"), "{rewritten}");
    cmds.set_float("old", geometry(7), None).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("floatWindow"), "{written}");
}

#[test]
fn a_group_links_many_todos_and_a_todo_links_many_groups() {
    let fix = fixture();
    let file = user_file(&fix, "keep.txt", b"keep");
    let cmds = commands(&fix.store);
    let first = cmds.create_group("甲").unwrap();
    let second = cmds.create_group("乙").unwrap();
    cmds.add_refs(Some(&first.id), &[incoming(&file, false)])
        .unwrap();
    cmds.link_todo(&first.id, "todo-a").unwrap();
    cmds.link_todo(&second.id, "todo-a").unwrap();
    let again = cmds.link_todo(&first.id, "todo-a").unwrap();
    assert_eq!(again.todo_ids, ["todo-a"]);
    let both = cmds.link_todo(&first.id, "todo-b").unwrap();
    assert_eq!(both.todo_ids, ["todo-a", "todo-b"]);
    assert_eq!(cmds.get(&second.id).unwrap().todo_ids, ["todo-a"]);
    assert!(cmds.link_todo(&first.id, "bad/id").is_err());
    assert!(cmds.unlink_todo(&first.id, "bad/id").is_err());

    let rx = fix.store.subscribe();
    let same = cmds.link_todo(&first.id, "todo-b").unwrap();
    assert_eq!(same.todo_ids, ["todo-a", "todo-b"]);
    assert!(rx.try_recv().is_err(), "linking the same todo wrote");

    let one_left = cmds.unlink_todo(&first.id, "todo-a").unwrap();
    assert_eq!(one_left.todo_ids, ["todo-b"]);
    assert_eq!(cmds.get(&second.id).unwrap().todo_ids, ["todo-a"]);
    while rx.try_recv().is_ok() {}
    cmds.unlink_todo(&first.id, "todo-a").unwrap();
    assert!(rx.try_recv().is_err(), "unlinking an absent link wrote");

    let back = cmds.link_todo(&first.id, "todo-a").unwrap();
    assert_eq!(back.todo_ids, ["todo-b", "todo-a"]);
    let reopened = commands(&fix.store);
    assert_eq!(
        reopened.get(&first.id).unwrap().todo_ids,
        ["todo-b", "todo-a"]
    );
    assert_unchanged(&file, b"keep");
}

#[test]
fn a_purged_todo_unlinks_only_itself_from_each_group() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let first = cmds.create_group("甲").unwrap();
    let second = cmds.create_group("乙").unwrap();
    let third = cmds.create_group("丙").unwrap();
    for todo in ["gone", "stay"] {
        cmds.link_todo(&first.id, todo).unwrap();
    }
    cmds.link_todo(&second.id, "gone").unwrap();
    cmds.link_todo(&third.id, "stay").unwrap();

    let cleared = cmds.unlink_todo_everywhere("gone").unwrap();
    assert_eq!(cleared, vec![first.id.clone(), second.id.clone()]);
    assert_eq!(cmds.get(&first.id).unwrap().todo_ids, ["stay"]);
    assert!(cmds.get(&second.id).unwrap().todo_ids.is_empty());
    assert_eq!(cmds.get(&third.id).unwrap().todo_ids, ["stay"]);

    cmds.link_todo(&second.id, "gone").unwrap();
    cmds.apply_todo_notice(&TodoNotice::Purged {
        item_id: "gone".into(),
    })
    .unwrap();
    assert!(cmds.get(&second.id).unwrap().todo_ids.is_empty());
    assert_eq!(cmds.get(&first.id).unwrap().todo_ids, ["stay"]);
}

#[test]
fn a_file_with_the_old_single_todo_field_reads_as_a_list_and_is_rewritten_as_one() {
    let fix = fixture();
    let path = fix
        .store
        .document_path(&DocumentId::Shelf("old".into()))
        .unwrap();
    std::fs::write(
        &path,
        r#"{"schemaVersion":1,"id":"old","name":"旧","order":0,"todoId":"t-old","refs":[]}"#,
    )
    .unwrap();
    let both = fix
        .store
        .document_path(&DocumentId::Shelf("both".into()))
        .unwrap();
    std::fs::write(
        &both,
        r#"{"schemaVersion":1,"id":"both","name":"两种","order":1,"todoId":"t-1","todoIds":["t-2","t-1"],"refs":[]}"#,
    )
    .unwrap();
    let cmds = ShelfCommands::open(fix.store.clone()).unwrap();
    assert_eq!(cmds.get("old").unwrap().todo_ids, ["t-old"]);
    assert_eq!(cmds.get("both").unwrap().todo_ids, ["t-1", "t-2"]);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"todoId\""),
        "reading must not rewrite the file"
    );

    cmds.link_todo("old", "t-new").unwrap();
    let rewritten = std::fs::read_to_string(&path).unwrap();
    assert!(!rewritten.contains("\"todoId\""), "{rewritten}");
    assert!(rewritten.contains("\"todoIds\""), "{rewritten}");
    let reopened = ShelfCommands::open(fix.store.clone()).unwrap();
    assert_eq!(reopened.get("old").unwrap().todo_ids, ["t-old", "t-new"]);

    cmds.unlink_todo("old", "t-old").unwrap();
    cmds.unlink_todo("old", "t-new").unwrap();
    let empty = std::fs::read_to_string(&path).unwrap();
    assert!(!empty.contains("todoId"), "{empty}");
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

fn draft(title: &str) -> NewTodo {
    NewTodo {
        title: title.to_owned(),
        due: None,
        remind_at: None,
        recurrence: None,
        source: None,
    }
}

#[test]
fn watch_after_boot_clears_the_purged_link_and_keeps_an_unrecorded_id() {
    let fix = fixture();
    let deleted_at = now_ms() - TRASH_RETENTION_MS;
    let list_path = fix
        .store
        .document_path(&DocumentId::Todo("work".into()))
        .unwrap();
    std::fs::create_dir_all(list_path.parent().unwrap()).unwrap();
    std::fs::write(
        &list_path,
        format!(
            r#"{{"schemaVersion":1,"id":"work","name":"工作","kind":"normal","items":[{{"id":"old","title":"过期","deletedAt":{deleted_at}}}]}}"#
        ),
    )
    .unwrap();
    let shelves = commands(&fix.store);
    let purged = shelves.create_group("已删").unwrap();
    let missing = shelves.create_group("尚无").unwrap();
    shelves.link_todo(&purged.id, "old").unwrap();
    shelves.link_todo(&missing.id, "missing-todo").unwrap();

    let todos = TodoCommands::open(fix.store.clone());
    todos.boot().unwrap();
    assert!(matches!(
        todos.item("old").unwrap_err(),
        TodoError::ItemNotFound { .. }
    ));
    let pending = fix.store.data_dir().join(PURGE_PENDING_FILE);
    let recorded = std::fs::read_to_string(&pending).unwrap();
    assert!(recorded.contains("\"old\""), "{recorded}");

    shelves.watch_todo_notices(&todos);
    assert!(shelves.get(&purged.id).unwrap().todo_ids.is_empty());
    assert_eq!(
        shelves.get(&missing.id).unwrap().todo_ids,
        ["missing-todo".to_owned()]
    );
    assert!(!pending.exists());
}

#[test]
fn watch_finishes_unlink_after_the_todo_write_and_leaves_other_ids() {
    let fix = fixture();
    let todos = TodoCommands::open(fix.store.clone());
    todos.boot().unwrap();
    let list_id = todos.create_list("工作").unwrap();
    let still = todos.create_item(&list_id, draft("还在")).unwrap();
    let pending = fix.store.data_dir().join(PURGE_PENDING_FILE);
    std::fs::write(
        &pending,
        format!(r#"{{"schemaVersion":1,"ids":["gone","{still}"]}}"#),
    )
    .unwrap();

    let shelves = commands(&fix.store);
    let gone_group = shelves.create_group("已写盘").unwrap();
    let still_group = shelves.create_group("还在").unwrap();
    let never_group = shelves.create_group("从未").unwrap();
    shelves.link_todo(&gone_group.id, "gone").unwrap();
    shelves.link_todo(&still_group.id, &still).unwrap();
    shelves.link_todo(&never_group.id, "missing-todo").unwrap();

    shelves.watch_todo_notices(&todos);
    assert!(shelves.get(&gone_group.id).unwrap().todo_ids.is_empty());
    assert_eq!(
        shelves.get(&still_group.id).unwrap().todo_ids,
        [still.as_str().to_owned()]
    );
    assert_eq!(
        shelves.get(&never_group.id).unwrap().todo_ids,
        ["missing-todo".to_owned()]
    );
    let left = std::fs::read_to_string(&pending).unwrap();
    assert!(left.contains(&format!("\"{still}\"")), "{left}");
    assert!(!left.contains("\"gone\""), "{left}");
}

#[test]
fn check_exists_reports_files_and_does_not_hang_on_an_unreachable_share() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let present = user_file(&fix, "here.txt", b"here");
    let missing = fix.user_dir.join("missing.txt");
    let missing_text = missing.to_str().unwrap().to_owned();
    let started = Instant::now();
    let found = cmds.check_exists(&[
        present.clone(),
        missing_text,
        r"\\203.0.113.1\lanwork-absent\missing.txt".to_owned(),
        r"not-absolute".to_owned(),
    ]);
    assert!(
        started.elapsed().as_secs() < 8,
        "existence check hung: {:?}",
        started.elapsed()
    );
    assert_eq!(found, vec![true, false, false, false]);
    assert_unchanged(&present, b"here");
}

#[test]
fn unsupported_schema_is_left_untouched() {
    let fix = fixture();
    let path = fix
        .store
        .document_path(&DocumentId::Shelf("future".into()))
        .unwrap();
    std::fs::write(&path, r#"{"schemaVersion":99,"id":"future","name":"保留"}"#).unwrap();
    let reloaded = ShelfCommands::open(fix.store.clone()).unwrap();
    assert!(reloaded.list().unwrap().is_empty());
    let err = reloaded.get("future").unwrap_err();
    assert!(matches!(
        err,
        ShelfError::UnsupportedSchema { found: 99, .. }
    ));
    let delete_err = reloaded.delete_group("future").unwrap_err();
    assert!(matches!(delete_err, ShelfError::UnsupportedSchema { .. }));
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("保留"), "{raw}");
}

/// 切掉 `#[cfg(test)] mod` 及其后的测试模块。先把换行归一成 `\n`，LF 和 CRLF 用同一处切分。
fn production_source(source: &str) -> String {
    let source = source.replace("\r\n", "\n").replace('\r', "\n");
    let markers = ["#[cfg(test)]\nmod ", "#[cfg(test)] mod "];
    let cut = markers
        .iter()
        .filter_map(|marker| source.find(marker))
        .min();
    match cut {
        Some(index) => source[..index].to_owned(),
        None => source,
    }
}

#[test]
fn production_scan_ignores_test_modules_on_lf_and_crlf() {
    let body =
        "fn keep() {}\n#[cfg(test)]\nmod tests {\n    let _ = std::fs::remove_dir_all(\"x\");\n}\n";
    for source in [body.to_owned(), body.replace('\n', "\r\n")] {
        let production = production_source(&source);
        assert!(!production.contains("remove_dir_all"), "{production:?}");
        assert!(production.contains("fn keep()"));
    }
    let leaked = "fn bad() { std::fs::remove_dir_all(\"x\"); }\n#[cfg(test)]\nmod tests {}\n";
    for source in [leaked.to_owned(), leaked.replace('\n', "\r\n")] {
        assert!(production_source(&source).contains("remove_dir_all"));
    }
}

#[test]
fn service_source_does_not_mutate_user_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/shelves");
    let forbidden = [
        "remove_file",
        "remove_dir",
        "fs::copy",
        "fs::rename",
        "CopyFile",
        "MoveFile",
        "DeleteFile",
        "SHFileOperation",
    ];
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let production = production_source(&source);
        for token in forbidden {
            assert!(
                !production.contains(token),
                "{} contains {token}",
                path.display()
            );
        }
    }
}

#[test]
fn float_dock_is_per_group_and_an_old_pin_field_still_opens() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let first = cmds.create_group("甲").unwrap();
    let second = cmds.create_group("乙").unwrap();
    assert!(first.float_dock.is_none());
    let file = user_file(&fix, "a.txt", b"a");
    let dock = lanwork_core::notes::FloatDock {
        edge: lanwork_core::notes::DockEdge::Right,
        offset: 30,
        collapsed: true,
    };
    cmds.set_float(&first.id, geometry(100), Some(dock))
        .unwrap();
    cmds.add_refs(Some(&first.id), &[incoming(&file, false)])
        .unwrap();
    cmds.rename_group(&first.id, "甲二").unwrap();
    let reloaded = ShelfCommands::open(fix.store.clone()).unwrap();
    let got = reloaded.get(&first.id).unwrap();
    assert_eq!(got.float_dock, Some(dock));
    assert_eq!(got.float, Some(geometry(100)));
    assert_eq!(got.refs.len(), 1);
    assert!(reloaded.get(&second.id).unwrap().float_dock.is_none());
    assert_unchanged(&file, b"a");

    let path = fix
        .store
        .document_path(&DocumentId::Shelf("old".into()))
        .unwrap();
    std::fs::write(
        &path,
        r#"{"schemaVersion":1,"id":"old","name":"旧","order":0,"refs":[],"floatPinned":true}"#,
    )
    .unwrap();
    let cmds = ShelfCommands::open(fix.store.clone()).unwrap();
    assert!(cmds.get("old").unwrap().float_dock.is_none());
    cmds.rename_group("old", "旧二").unwrap();
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("floatPinned")
    );
}

#[test]
fn float_stack_round_trips_on_a_group_and_a_missing_field_stays_unset() {
    let fix = fixture();
    let cmds = commands(&fix.store);
    let group = cmds.create_group("甲").unwrap();
    assert!(group.float_stack.is_none());
    let slot = lanwork_core::notes::FloatStackSlot {
        id: "mix".to_owned(),
        index: 0,
        current: false,
    };
    let saved = cmds.set_stack(&group.id, Some(slot.clone())).unwrap();
    assert_eq!(saved.float_stack, Some(slot.clone()));
    assert_eq!(saved.name, "甲");
    assert!(saved.refs.is_empty());
    let reloaded = ShelfCommands::open(fix.store.clone()).unwrap();
    assert_eq!(reloaded.get(&group.id).unwrap().float_stack, Some(slot));

    let path = fix
        .store
        .document_path(&DocumentId::Shelf("plain".into()))
        .unwrap();
    std::fs::write(
        &path,
        r#"{"schemaVersion":1,"id":"plain","name":"旧","order":1,"refs":[]}"#,
    )
    .unwrap();
    let cmds = ShelfCommands::open(fix.store.clone()).unwrap();
    assert!(cmds.get("plain").unwrap().float_stack.is_none());
}
