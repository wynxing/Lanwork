//! 便签服务的验收测试。界面入口和冲突后的选择仍等产品规格，对应用例标了 ignore。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lanwork_core::notes::{
    EMPTY_TITLE_DISPLAY, NoteCommands, NoteError, NoteInput, TimestampMillis,
};
use lanwork_core::storage::{CollectionKind, DocumentId, EntityKind, Store, StorePaths};

const ONE_MIB: usize = 1024 * 1024;

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
        let path = std::env::temp_dir().join(format!("lanwork-notes-{nanos}-{seq}"));
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

struct ManualClock {
    millis: AtomicI64,
}

impl ManualClock {
    fn new(millis: i64) -> Arc<Self> {
        Arc::new(Self {
            millis: AtomicI64::new(millis),
        })
    }

    fn set(&self, millis: i64) {
        self.millis.store(millis, Ordering::SeqCst);
    }

    fn now(&self) -> TimestampMillis {
        TimestampMillis::from_millis(self.millis.load(Ordering::SeqCst))
    }
}

struct Fixture {
    _temp: TempDir,
    store: Store,
}

fn fixture() -> Fixture {
    let temp = TempDir::new();
    let root = temp.path();
    let store = Store::open(StorePaths {
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        user_profile: root.join("profile"),
        local_app_data: root.join("local"),
    })
    .unwrap();
    Fixture { _temp: temp, store }
}

fn input(title: &str, body: &str) -> NoteInput {
    NoteInput {
        title: title.to_owned(),
        body: body.to_owned(),
        tags: Vec::new(),
        pinned: false,
    }
}

fn json_file(store: &Store, id: &str) -> serde_json::Value {
    let path = store
        .document_path(&DocumentId::Note(id.to_owned()))
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn note_count(store: &Store) -> usize {
    let dir = store.collection_dir(CollectionKind::Notes);
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
fn create_update_reload_roundtrip() {
    let fix = fixture();
    let clock = ManualClock::new(1_000);
    let clock_for_open = Arc::clone(&clock);
    let rx = fix.store.subscribe();
    let commands = NoteCommands::open_at(fix.store.clone(), move || clock_for_open.now()).unwrap();

    let mut created_input = input(" 会议纪要 ", "第一行\n第二行");
    created_input.tags = vec!["项目".into()];
    let created = commands.create(&created_input).unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(created.created_at.as_millis(), 1_000);
    assert_eq!(created.updated_at.as_millis(), 1_000);
    assert_eq!(created.title, " 会议纪要 ");
    assert!(created.deleted_at.is_none());
    let created_event = rx.try_recv().unwrap();
    assert_eq!(created_event.kind, EntityKind::Note);
    assert_eq!(created_event.id, created.id);
    assert_eq!(created_event.revision, Some(1));

    clock.set(2_500);
    let mut updated_input = input("纪要", "改过的正文");
    updated_input.tags = vec!["项目".into(), "项目".into()];
    updated_input.pinned = true;
    let updated = commands
        .save(&created.id, created.revision, &updated_input)
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.created_at.as_millis(), 1_000);
    assert_eq!(updated.updated_at.as_millis(), 2_500);
    assert_eq!(updated.body, "改过的正文");
    assert_eq!(updated.tags, vec!["项目".to_owned()]);
    assert!(updated.pinned);
    assert_eq!(updated_input.body, "改过的正文");
    let updated_event = rx.try_recv().unwrap();
    assert_eq!(updated_event.revision, Some(2));
    assert!(rx.try_recv().is_err());

    let reloaded = NoteCommands::open(fix.store.clone()).unwrap();
    assert_eq!(reloaded.get(&created.id).unwrap(), updated);
    let raw = json_file(&fix.store, &created.id);
    assert_eq!(raw["schemaVersion"], 1);
    assert_eq!(raw["id"], created.id);
    assert_eq!(raw["title"], "纪要");
    assert_eq!(raw["body"], "改过的正文");
    assert_eq!(raw["pinned"], true);
    assert_eq!(raw["revision"], 2);
    assert_eq!(raw["createdAt"], 1_000);
    assert_eq!(raw["updatedAt"], 2_500);
    assert!(raw.get("deletedAt").is_none());
    assert_eq!(note_count(&fix.store), 1);
}

#[test]
fn pinned_notes_sort_first() {
    let fix = fixture();
    let clock = ManualClock::new(1_000);
    let clock_for_open = Arc::clone(&clock);
    let commands = NoteCommands::open_at(fix.store.clone(), move || clock_for_open.now()).unwrap();

    let mut older = input("旧的置顶", "a");
    older.pinned = true;
    let older = commands.create(&older).unwrap();

    clock.set(3_000);
    let middle = commands.create(&input("中间", "b")).unwrap();

    clock.set(5_000);
    let newer = commands.create(&input("更新的普通", "c")).unwrap();

    let ids: Vec<_> = commands.list().into_iter().map(|note| note.id).collect();
    assert_eq!(
        ids,
        vec![older.id.clone(), newer.id.clone(), middle.id.clone()]
    );
    assert!(commands.list()[0].pinned);
    assert!(!commands.list()[1].pinned);

    clock.set(7_000);
    let pinned_newer = commands
        .set_pinned(&newer.id, newer.revision, true)
        .unwrap();
    assert_eq!(pinned_newer.revision, newer.revision + 1);
    assert_eq!(pinned_newer.updated_at.as_millis(), 7_000);
    let ids: Vec<_> = commands.list().into_iter().map(|note| note.id).collect();
    assert_eq!(ids[0], newer.id);
    assert_eq!(ids[1], older.id);

    let err = commands
        .save(&newer.id, newer.revision, &input("过期", "x"))
        .unwrap_err();
    assert!(matches!(err, NoteError::Conflict { .. }), "{err}");
    assert_eq!(commands.get(&newer.id).unwrap().body, "c");
}

#[test]
fn one_mib_body_roundtrip_records_write_time() {
    let fix = fixture();
    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    let body = "a".repeat(ONE_MIB);
    let original = input("大便签", &body);
    let created = commands.create(&original).unwrap();

    let started = Instant::now();
    let saved = commands
        .save(&created.id, created.revision, &original)
        .unwrap();
    let elapsed = started.elapsed();
    // 不是性能目标。数字在 `cargo test one_mib_body_roundtrip_records_write_time -- --nocapture` 里。
    println!("note_write_1mib_ms={:.3}", elapsed.as_secs_f64() * 1000.0);
    assert!(
        elapsed.as_secs() < 60,
        "1 MiB rewrite took {elapsed:?}; this bound only detects a hung write"
    );

    assert_eq!(saved.body.len(), ONE_MIB);
    assert_eq!(original.body.len(), ONE_MIB);
    let reloaded = NoteCommands::open(fix.store.clone()).unwrap();
    let loaded = reloaded.get(&created.id).unwrap();
    assert_eq!(loaded.body.len(), ONE_MIB);
    assert!(loaded.body == saved.body, "reloaded body differs");
    let path = fix
        .store
        .document_path(&DocumentId::Note(created.id))
        .unwrap();
    let file_len = std::fs::metadata(&path).unwrap().len();
    assert!(file_len > ONE_MIB as u64, "file len {file_len}");
}

#[test]
fn tags_with_cjk_and_spaces_dedup_and_filter() {
    let fix = fixture();
    let clock = ManualClock::new(1_000);
    let clock_for_open = Arc::clone(&clock);
    let commands = NoteCommands::open_at(fix.store.clone(), move || clock_for_open.now()).unwrap();

    let mut pinned = input("有标签", "正文");
    pinned.pinned = true;
    pinned.tags = vec![
        " 工作 ".into(),
        "会议 记录".into(),
        "工作".into(),
        " ".into(),
        "会议 记录".into(),
    ];
    let pinned = commands.create(&pinned).unwrap();
    assert_eq!(pinned.tags, vec!["工作".to_owned(), "会议 记录".to_owned()]);

    clock.set(4_000);
    let mut other = input("另一篇", "x");
    other.tags = vec!["工作".into()];
    let other = commands.create(&other).unwrap();
    let retagged = commands
        .set_tags(
            &other.id,
            other.revision,
            &[" 工作 ".into(), "工作".into(), " ".into()],
        )
        .unwrap();
    assert_eq!(retagged.tags, vec!["工作".to_owned()]);
    assert_eq!(retagged.revision, other.revision + 1);

    let matched = commands.filter_by_tag(" 工作 ");
    assert_eq!(
        matched
            .iter()
            .map(|note| note.id.as_str())
            .collect::<Vec<_>>(),
        vec![pinned.id.as_str(), other.id.as_str()]
    );
    assert!(commands.filter_by_tag("会议").is_empty());
    assert_eq!(commands.filter_by_tag("会议 记录").len(), 1);
    assert!(commands.filter_by_tag("   ").is_empty());

    let reloaded = NoteCommands::open(fix.store.clone()).unwrap();
    assert_eq!(reloaded.get(&pinned.id).unwrap().tags, pinned.tags);
    let raw = std::fs::read(
        fix.store
            .document_path(&DocumentId::Note(pinned.id.clone()))
            .unwrap(),
    )
    .unwrap();
    let raw = String::from_utf8(raw).unwrap();
    assert!(raw.contains("工作"), "{raw}");
    assert!(raw.contains("会议 记录"), "{raw}");

    let deleted = commands.soft_delete(&pinned.id, pinned.revision).unwrap();
    assert!(
        commands
            .filter_by_tag("工作")
            .iter()
            .all(|note| note.id != deleted.id)
    );
}

#[test]
fn empty_title_displays_untitled() {
    let fix = fixture();
    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    for title in ["", " ", "　", "\n\t"] {
        let note = commands.create(&input(title, "正文")).unwrap();
        assert_eq!(note.title, title, "stored title");
        assert_eq!(note.display_title(), EMPTY_TITLE_DISPLAY);
        let reloaded = NoteCommands::open(fix.store.clone())
            .unwrap()
            .get(&note.id)
            .unwrap();
        assert_eq!(reloaded.title, title);
        assert_eq!(reloaded.display_title(), EMPTY_TITLE_DISPLAY);
        assert_eq!(json_file(&fix.store, &note.id)["title"], title);
    }

    let named = commands.create(&input("会议", "正文")).unwrap();
    assert_eq!(named.display_title(), "会议");
    let literal = commands.create(&input("无标题", "正文")).unwrap();
    assert_eq!(literal.title, "无标题");
    assert_eq!(literal.display_title(), "无标题");
}

#[test]
fn second_save_from_same_revision_conflicts() {
    let fix = fixture();
    let rx = fix.store.subscribe();
    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    let created = commands.create(&input("标题", "原始")).unwrap();
    assert!(rx.try_recv().is_ok());

    let caller_a = commands.clone();
    let caller_b = commands.clone();
    let revision = created.revision;
    let first = input("标题", "第一次");
    let second = input("标题", "第二次");
    let saved = caller_a.save(&created.id, revision, &first).unwrap();
    let event = rx.try_recv().unwrap();
    assert_eq!(event.revision, Some(saved.revision));

    let err = caller_b.save(&created.id, revision, &second).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("便签保存冲突"), "{text}");
    assert!(!text.contains("第二次"), "{text}");
    match err {
        NoteError::Conflict {
            id,
            expected,
            actual,
        } => {
            assert_eq!(id, created.id);
            assert_eq!(expected, revision);
            assert_eq!(actual, saved.revision);
        }
        other => panic!("expected conflict, got {other}"),
    }
    assert_eq!(second.body, "第二次");
    assert!(rx.try_recv().is_err());
    assert_eq!(commands.get(&created.id).unwrap().body, "第一次");

    let reloaded = NoteCommands::open(fix.store.clone()).unwrap();
    let on_disk = reloaded.get(&created.id).unwrap();
    assert_eq!(on_disk.body, "第一次");
    assert_eq!(on_disk.revision, saved.revision);
    let raw = std::fs::read_to_string(
        fix.store
            .document_path(&DocumentId::Note(created.id))
            .unwrap(),
    )
    .unwrap();
    assert!(raw.contains("第一次"), "{raw}");
    assert!(!raw.contains("第二次"), "{raw}");
}

#[test]
fn soft_delete_and_restore_keep_file() {
    let fix = fixture();
    let clock = ManualClock::new(1_000);
    let clock_for_open = Arc::clone(&clock);
    let rx = fix.store.subscribe();
    let commands = NoteCommands::open_at(fix.store.clone(), move || clock_for_open.now()).unwrap();
    let created = commands.create(&input("留下", "正文")).unwrap();
    let _ = rx.try_recv().unwrap();
    let path = fix
        .store
        .document_path(&DocumentId::Note(created.id.clone()))
        .unwrap();

    clock.set(2_000);
    let deleted = commands.soft_delete(&created.id, created.revision).unwrap();
    assert_eq!(deleted.revision, 2);
    assert_eq!(
        deleted.deleted_at.map(TimestampMillis::as_millis),
        Some(2_000)
    );
    assert_eq!(deleted.created_at.as_millis(), 1_000);
    assert!(path.is_file());
    assert!(commands.list().is_empty());
    assert_eq!(commands.deleted().len(), 1);
    assert_eq!(rx.try_recv().unwrap().revision, Some(2));
    assert!(json_file(&fix.store, &created.id)["deletedAt"].is_number());

    let again = commands
        .soft_delete(&created.id, deleted.revision)
        .unwrap_err();
    assert!(matches!(again, NoteError::AlreadyDeleted { .. }), "{again}");
    assert_eq!(
        commands.get(&created.id).unwrap().revision,
        deleted.revision
    );

    clock.set(2_500);
    let edited = commands
        .save(&created.id, deleted.revision, &input("留下", "仍在回收站"))
        .unwrap();
    assert_eq!(edited.body, "仍在回收站");
    assert!(edited.is_deleted());
    assert!(commands.list().is_empty());

    clock.set(3_000);
    let restored = commands.restore(&created.id, edited.revision).unwrap();
    assert_eq!(restored.revision, edited.revision + 1);
    assert!(restored.deleted_at.is_none());
    assert_eq!(restored.body, "仍在回收站");
    assert!(path.is_file());
    assert_eq!(commands.list().len(), 1);
    assert!(commands.deleted().is_empty());
    assert!(
        json_file(&fix.store, &created.id)
            .get("deletedAt")
            .is_none()
    );

    let not_deleted = commands
        .restore(&created.id, restored.revision)
        .unwrap_err();
    assert!(
        matches!(not_deleted, NoteError::NotDeleted { .. }),
        "{not_deleted}"
    );

    let reloaded = NoteCommands::open(fix.store.clone()).unwrap();
    assert_eq!(reloaded.get(&created.id).unwrap(), restored);
}

#[test]
fn command_rejects_invalid_id() {
    let fix = fixture();
    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    let sample = input("t", "不应落盘的正文");
    let before = note_count(&fix.store);
    let ids = [
        String::new(),
        "a/b".into(),
        "a\\b".into(),
        "..".into(),
        "CON".into(),
        " trailing".into(),
        "a".repeat(201),
        "笔记/一".into(),
    ];
    for id in &ids {
        let err = commands.save(id, 1, &sample).unwrap_err();
        let text = err.to_string();
        assert!(matches!(err, NoteError::InvalidId { .. }), "{id}: {text}");
        assert!(text.contains("无效"), "{text}");
        assert!(!text.contains("不应落盘的正文"), "{text}");
        let get_err = commands.get(id).unwrap_err();
        assert!(
            matches!(get_err, NoteError::InvalidId { .. }),
            "{id}: {get_err}"
        );
    }
    assert_eq!(note_count(&fix.store), before);

    let missing = commands.get("missing-note").unwrap_err();
    assert!(matches!(missing, NoteError::NotFound { .. }), "{missing}");
    assert_eq!(note_count(&fix.store), before);
}

#[test]
fn old_file_missing_fields_loads_and_saves() {
    let fix = fixture();
    let path = fix
        .store
        .document_path(&DocumentId::Note("old".into()))
        .unwrap();
    std::fs::write(&path, r#"{"title":"旧","body":"正文","id":"other"}"#).unwrap();

    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    let loaded = commands.get("old").unwrap();
    assert_eq!(loaded.id, "old");
    assert_eq!(loaded.title, "旧");
    assert_eq!(loaded.body, "正文");
    assert!(loaded.tags.is_empty());
    assert!(!loaded.pinned);
    assert!(loaded.deleted_at.is_none());
    assert_eq!(loaded.revision, 0);
    assert_eq!(loaded.created_at.as_millis(), 0);
    let missing = commands.get("other").unwrap_err();
    assert!(matches!(missing, NoteError::NotFound { .. }), "{missing}");

    let saved = commands.save("old", 0, &input("旧", "新正文")).unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(json_file(&fix.store, "old")["id"], "old");
    assert_eq!(json_file(&fix.store, "old")["schemaVersion"], 1);
    assert_eq!(
        NoteCommands::open(fix.store.clone())
            .unwrap()
            .get("old")
            .unwrap()
            .body,
        "新正文"
    );
}

#[test]
fn unsupported_schema_does_not_block_other_notes() {
    let fix = fixture();
    let future = fix
        .store
        .document_path(&DocumentId::Note("future".into()))
        .unwrap();
    std::fs::write(
        &future,
        r#"{"schemaVersion":99,"id":"future","title":"以后","body":"保留"}"#,
    )
    .unwrap();
    std::fs::write(
        fix.store
            .document_path(&DocumentId::Note("ok".into()))
            .unwrap(),
        r#"{"schemaVersion":1,"id":"ok","title":"现在","body":"在"}"#,
    )
    .unwrap();

    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    assert_eq!(commands.get("ok").unwrap().body, "在");
    assert!(commands.list().iter().all(|note| note.id != "future"));
    let err = commands.get("future").unwrap_err();
    assert!(
        matches!(err, NoteError::UnsupportedSchema { found: 99, .. }),
        "{err}"
    );
    let save_err = commands
        .save("future", 1, &input("覆盖", "不行"))
        .unwrap_err();
    assert!(
        matches!(save_err, NoteError::UnsupportedSchema { .. }),
        "{save_err}"
    );
    let raw = std::fs::read_to_string(&future).unwrap();
    assert!(raw.contains("保留"), "{raw}");
    assert!(!raw.contains("不行"), "{raw}");
}

#[test]
fn corrupt_note_does_not_block_other_notes() {
    let fix = fixture();
    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    let kept = commands.create(&input("好的", "还在")).unwrap();
    let broken = fix
        .store
        .document_path(&DocumentId::Note("broken".into()))
        .unwrap();
    std::fs::write(&broken, b"{").unwrap();

    let reloaded = NoteCommands::open(fix.store.clone()).unwrap();
    assert_eq!(reloaded.get(&kept.id).unwrap().body, "还在");
    assert_eq!(reloaded.list().len(), 1);
    assert!(!broken.exists());
    let dir = fix.store.collection_dir(CollectionKind::Notes);
    let quarantined = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("broken.json.corrupt-")
        })
        .count();
    assert_eq!(quarantined, 1);
}

fn assert_failed_save_keeps_body(
    commands: &NoteCommands,
    path: &Path,
    id: &str,
    revision: u64,
    previous: &str,
) {
    let replacement = input("标题", "新正文不应落盘");
    let err = commands.save(id, revision, &replacement).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("写入失败"), "{text}");
    assert!(text.contains(&path.display().to_string()), "{text}");
    assert!(!text.contains("新正文不应落盘"), "{text}");
    assert_eq!(replacement.body, "新正文不应落盘");
    assert_eq!(commands.get(id).unwrap().body, previous);
}

fn assert_disk_unchanged(path: &Path, previous: &str) {
    let raw = std::fs::read_to_string(path).unwrap();
    assert!(raw.contains(previous), "{raw}");
    assert!(!raw.contains("新正文不应落盘"), "{raw}");
}

#[cfg(unix)]
struct ResetMode {
    path: PathBuf,
    permissions: std::fs::Permissions,
}

#[cfg(unix)]
impl Drop for ResetMode {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.permissions.clone());
    }
}

#[cfg(unix)]
#[test]
fn failed_save_keeps_body_in_memory() {
    use std::os::unix::fs::PermissionsExt;

    let fix = fixture();
    let rx = fix.store.subscribe();
    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    let created = commands.create(&input("标题", "原始正文")).unwrap();
    assert!(rx.try_recv().is_ok());
    let path = fix
        .store
        .document_path(&DocumentId::Note(created.id.clone()))
        .unwrap();
    let dir = path.parent().unwrap();
    let original_mode = std::fs::metadata(dir).unwrap().permissions();
    let _restore = ResetMode {
        path: dir.to_path_buf(),
        permissions: original_mode.clone(),
    };
    let mut readonly = original_mode;
    readonly.set_mode(0o555);
    std::fs::set_permissions(dir, readonly).unwrap();
    assert_failed_save_keeps_body(&commands, &path, &created.id, created.revision, "原始正文");
    assert_disk_unchanged(&path, "原始正文");
    assert!(rx.try_recv().is_err());
    drop(_restore);

    let kept = input("标题", "新正文不应落盘");
    let saved = commands.save(&created.id, created.revision, &kept).unwrap();
    assert_eq!(saved.body, "新正文不应落盘");
    assert_eq!(saved.revision, created.revision + 1);
}

#[cfg(windows)]
#[test]
fn failed_save_keeps_body_in_memory() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let fix = fixture();
    let rx = fix.store.subscribe();
    let commands = NoteCommands::open(fix.store.clone()).unwrap();
    let created = commands.create(&input("标题", "原始正文")).unwrap();
    assert!(rx.try_recv().is_ok());
    let path = fix
        .store
        .document_path(&DocumentId::Note(created.id.clone()))
        .unwrap();
    // FILE_SHARE_READ = 1。不带 FILE_SHARE_DELETE，替换必须失败。
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    assert_failed_save_keeps_body(&commands, &path, &created.id, created.revision, "原始正文");
    assert!(rx.try_recv().is_err());
    drop(lock);
    assert_disk_unchanged(&path, "原始正文");

    let kept = input("标题", "新正文不应落盘");
    let saved = commands.save(&created.id, created.revision, &kept).unwrap();
    assert_eq!(saved.body, "新正文不应落盘");
    assert_eq!(saved.revision, created.revision + 1);
}

/// #9 第 10 项还没写进 product.md。
/// 服务层已经能软删除和恢复。界面入口、用户能否删除、删除后是否进入回收站，仍待规格。
/// 在此之前不提供永久删除。
#[test]
#[ignore = "待 product.md 写入 #9 第 10 项：便签删除的界面入口，以及删除后是否进入回收站"]
fn note_delete_ui_and_recycle_policy_pending_spec() {
    panic!("待 product.md 写入 #9 第 10 项之后再验收删除的界面入口和回收站策略");
}

/// #9 第 16 项还没写进 product.md。
/// 服务层在 revision 不一致时返回冲突并且不覆盖磁盘。
/// 冲突后选择哪一版、关闭窗口和退出时如何处理未保存正文，仍待规格。
#[test]
#[ignore = "待 product.md 写入 #9 第 16 项：冲突后的版本选择，以及关闭和退出时的未保存正文"]
fn note_conflict_resolution_and_unsaved_close_pending_spec() {
    panic!("待 product.md 写入 #9 第 16 项之后再验收冲突版本选择和未保存正文的关闭、退出");
}
