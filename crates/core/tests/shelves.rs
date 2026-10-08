//! 收纳服务的验收测试。
//!
//! 分组数据写在临时目录里。用户文件另放一份，所有操作之后都还在原路径。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lanwork_core::shelves::{
    DEFAULT_GROUP_NAME, IncomingRef, ShelfCommands, ShelfError, normalize_path,
};
use lanwork_core::storage::{CollectionKind, DocumentId, EntityKind, Store, StorePaths};
use lanwork_core::todos::TodoNotice;

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

#[test]
fn linking_is_one_todo_per_group_and_many_groups_per_todo() {
    let fix = fixture();
    let file = user_file(&fix, "keep.txt", b"keep");
    let cmds = commands(&fix.store);
    let first = cmds.create_group("甲").unwrap();
    let second = cmds.create_group("乙").unwrap();
    cmds.add_refs(Some(&first.id), &[incoming(&file, false)])
        .unwrap();
    cmds.link_todo(&first.id, "todo-a").unwrap();
    cmds.link_todo(&second.id, "todo-a").unwrap();
    let same = cmds.link_todo(&first.id, "todo-a").unwrap();
    assert_eq!(same.todo_id.as_deref(), Some("todo-a"));
    let err = cmds.link_todo(&first.id, "todo-b").unwrap_err();
    assert!(matches!(err, ShelfError::AlreadyLinked { .. }), "{err}");
    assert!(cmds.link_todo(&first.id, "bad/id").is_err());
    cmds.link_todo(&second.id, "missing-todo").unwrap_err();
    let replaced = cmds.link_todo(&second.id, "missing-todo");
    assert!(replaced.is_err());
    cmds.unlink_todo(&second.id).unwrap();
    let linked = cmds.link_todo(&second.id, "missing-todo").unwrap();
    assert_eq!(linked.todo_id.as_deref(), Some("missing-todo"));

    let cleared = cmds.unlink_todo_everywhere("todo-a").unwrap();
    assert_eq!(cleared, vec![first.id.clone()]);
    assert!(cmds.get(&first.id).unwrap().todo_id.is_none());
    cmds.link_todo(&first.id, "todo-a").unwrap();
    cmds.link_todo(&second.id, "gone").unwrap_err();
    cmds.unlink_todo(&second.id).unwrap();
    cmds.link_todo(&second.id, "gone").unwrap();
    cmds.apply_todo_notice(&TodoNotice::Purged {
        item_id: "gone".into(),
    })
    .unwrap();
    assert!(cmds.get(&second.id).unwrap().todo_id.is_none());
    assert_eq!(
        cmds.get(&first.id).unwrap().todo_id.as_deref(),
        Some("todo-a")
    );
    assert_unchanged(&file, b"keep");
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
        let production = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .unwrap_or(&source);
        for token in forbidden {
            assert!(
                !production.contains(token),
                "{} contains {token}",
                path.display()
            );
        }
    }
}
