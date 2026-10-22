//! 备份、导出和导入。设置页调用这些命令。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lanwork_core::CivilDate;
use lanwork_core::backup::{BackupCommands, BackupKind, LaunchBackup, overview_text};
use lanwork_core::notes::{FloatGeometry, FloatStackSlot, NoteCommands, NoteError, NoteInput};
use lanwork_core::panel::{self, EnabledBytes, encode_enabled, is_enabled, read_enabled};
use lanwork_core::shelves::ShelfCommands;
use lanwork_core::storage::{BootHooks, EntityKind, Store, StorePaths};
use lanwork_core::todos::{FloatWindowFile, TodoCommands};

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
        let path = std::env::temp_dir().join(format!("lanwork-backup-{nanos}-{seq}"));
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

fn write_rel(store: &Store, relative: &str, body: &str) {
    let mut path = store.data_dir().to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn read_rel(store: &Store, relative: &str) -> String {
    let mut path = store.data_dir().to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    std::fs::read_to_string(path).unwrap()
}

fn day(year: i32, month: u8, day: u8) -> CivilDate {
    CivilDate::try_from_ymd(year, month, day).unwrap()
}

#[test]
fn export_includes_user_apps_and_omits_logs_env_and_referenced_file() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "notes/n1.json",
        r#"{"schemaVersion":1,"body":"note-body-marker-77"}"#,
    );
    write_rel(
        &fx.store,
        "user-apps.json",
        r#"{"schemaVersion":1,"portable":[{"name":"我的工具"}]}"#,
    );
    write_rel(
        &fx.store,
        "shelves/s1.json",
        r#"{"schemaVersion":1,"path":"C:\\outside\\kept-ref"}"#,
    );
    write_rel(
        &fx.store,
        "github/cache/acme%2Fapp.json",
        r#"{"schemaVersion":1,"repo":"cache-marker-77"}"#,
    );
    write_rel(
        &fx.store,
        "todo-purge-pending.json",
        r#"{"schemaVersion":1,"ids":["purge-marker-77"]}"#,
    );
    write_rel(
        &fx.store,
        "token.json",
        r#"{"schemaVersion":1,"token":"token-marker-77"}"#,
    );
    write_rel(&fx.store, "cache/web-icons/icon.bin", "web-icon-marker-77");
    let outside = fx._temp.path().join("outside.bin");
    std::fs::write(&outside, b"shelf-file-bytes-not-in-zip").unwrap();
    std::fs::write(
        fx.store.data_dir().join("logs").join("app.log"),
        b"log-secret-marker-77",
    )
    .unwrap();
    let dest = fx._temp.path().join("out.zip");
    let overview = commands.export(&dest, 1_700_000_000_000).unwrap();
    assert!(overview.user_apps);
    assert!(overview.notes == 1);
    assert!(!overview.watchlist);
    assert_eq!(overview.github_caches, 0);
    let bytes = std::fs::read(&dest).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("note-body-marker-77"));
    assert!(text.contains("我的工具"));
    assert!(text.contains("kept-ref"));
    assert!(!text.contains("log-secret-marker-77"));
    assert!(!text.contains("shelf-file-bytes-not-in-zip"));
    assert!(!text.contains("cache-marker-77"));
    assert!(!text.contains("import.pending"));
    assert!(!text.contains("purge-marker-77"));
    assert!(!text.contains("todo-purge-pending"));
    assert!(!text.contains("token-marker-77"));
    assert!(!text.contains("web-icon-marker-77"));
    assert!(!text.contains("cache/web-icons"));
    assert_zip_omits_env(&text, &["note-body-marker-77", "我的工具", "kept-ref"]);

    let full = commands.backup_manual(50).unwrap();
    let backed = String::from_utf8_lossy(&std::fs::read(full).unwrap()).into_owned();
    assert!(!backed.contains("cache-marker-77"));
    assert!(!backed.contains("github/cache"));
    assert!(!backed.contains("github/watchlist"));
    assert!(backed.contains("我的工具"));
    assert!(!backed.contains("log-secret-marker-77"));
    assert!(!backed.contains("purge-marker-77"));
    assert!(!backed.contains("token-marker-77"));
    assert!(!backed.contains("web-icon-marker-77"));
    assert_zip_omits_env(&backed, &["note-body-marker-77", "我的工具", "kept-ref"]);
}

#[test]
fn retained_list_is_newest_first_and_drops_the_oldest_without_import_copies() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    assert!(commands.list_retained().unwrap().is_empty());
    write_rel(
        &fx.store,
        "config.json",
        r#"{"schemaVersion":1,"launchAtStartup":false}"#,
    );
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"one"}"#,
    );
    write_rel(
        &fx.store,
        "user-apps.json",
        r#"{"schemaVersion":1,"portable":[]}"#,
    );
    let package = fx._temp.path().join("pkg.zip");
    let overview = commands.export(&package, 30).unwrap();
    assert_eq!(
        overview_text(&overview),
        "待办 0，便签 1，收纳 0，配置，应用"
    );
    assert!(!overview_text(&overview).contains(env!("CARGO_PKG_VERSION")));

    commands.backup_manual(1_700_000_000_000).unwrap();
    commands
        .backup_on_launch(day(2026, 10, 15), 1_700_000_000_100)
        .unwrap();
    let listed = commands.list_retained().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].kind, BackupKind::Auto);
    assert_eq!(listed[0].label(), "自动 2026-10-15");
    assert_eq!(listed[1].kind, BackupKind::Manual);
    assert_eq!(listed[1].label(), "手动 2023-11-14 22:13");

    commands.import(&package, 40).unwrap();
    assert_eq!(
        commands.list_retained().unwrap().len(),
        2,
        "导入前的备份不进入这 7 份"
    );
    assert!(fx.store.backups_dir().join("import-40.zip").is_file());

    for millis in [100, 200, 300, 400, 500, 600, 700] {
        commands.backup_manual(millis).unwrap();
    }
    let listed = commands.list_retained().unwrap();
    assert_eq!(listed.len(), 7);
    assert!(
        listed
            .iter()
            .all(|item| item.at_ms != 100 && item.at_ms != 200)
    );
    assert_eq!(listed[0].at_ms, 1_700_000_000_100);
    assert!(listed.iter().any(|item| item.at_ms == 700));
    assert!(fx.store.backups_dir().join("import-40.zip").is_file());
}

fn assert_zip_omits_env(text: &str, intentional: &[&str]) {
    for (key, value) in std::env::vars() {
        if value.len() < 8 || intentional.iter().any(|part| part.contains(&value)) {
            continue;
        }
        assert!(!text.contains(&value), "导出包含环境变量 {key} 的值");
    }
}

#[test]
fn second_launch_same_day_does_not_backup_again() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    let today = day(2026, 10, 9);
    let first = commands.backup_on_launch(today, 1000).unwrap();
    assert!(matches!(first, LaunchBackup::Created { .. }));
    let second = commands.backup_on_launch(today, 2000).unwrap();
    assert_eq!(second, LaunchBackup::AlreadyDone);
    let names = zip_names(&fx.store);
    assert_eq!(
        names
            .iter()
            .filter(|name| name.starts_with("auto-"))
            .count(),
        1
    );

    let manual = commands.backup_manual(1500).unwrap();
    assert!(manual.ends_with("manual-1500.zip"));
    let again = commands.backup_on_launch(today, 3000).unwrap();
    assert_eq!(again, LaunchBackup::AlreadyDone);

    let next = commands.backup_on_launch(day(2026, 10, 10), 4000).unwrap();
    assert!(matches!(next, LaunchBackup::Created { .. }));
}

#[test]
fn eighth_backup_deletes_the_oldest_and_keeps_import_copies() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(&fx.store, "config.json", r#"{"schemaVersion":1}"#);
    let package = fx._temp.path().join("pkg.zip");
    commands.export(&package, 10).unwrap();
    commands.import(&package, 20).unwrap();
    assert!(
        zip_names(&fx.store)
            .iter()
            .any(|name| name.starts_with("import-"))
    );

    for millis in [100, 200, 300, 400, 500, 600, 700, 800] {
        commands.backup_manual(millis).unwrap();
    }
    let names = zip_names(&fx.store);
    let manuals: Vec<_> = names
        .iter()
        .filter(|name| name.starts_with("manual-"))
        .cloned()
        .collect();
    assert_eq!(manuals.len(), 7);
    assert!(!manuals.iter().any(|name| name == "manual-100.zip"));
    assert!(manuals.iter().any(|name| name == "manual-800.zip"));
    assert!(names.iter().any(|name| name.starts_with("import-")));
    assert!(!fx.store.backups_dir().join("auto-day.txt").exists());
}

#[test]
fn import_replaces_data_and_drops_files_absent_from_the_package() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "notes/keep.json",
        r#"{"schemaVersion":1,"body":"old-body"}"#,
    );
    write_rel(
        &fx.store,
        "user-apps.json",
        r#"{"schemaVersion":1,"portable":[{"name":"旧名称"}]}"#,
    );
    let package = fx._temp.path().join("pkg.zip");
    commands.export(&package, 30).unwrap();

    write_rel(
        &fx.store,
        "notes/keep.json",
        r#"{"schemaVersion":1,"body":"new-body"}"#,
    );
    write_rel(
        &fx.store,
        "notes/extra.json",
        r#"{"schemaVersion":1,"body":"extra-body"}"#,
    );
    std::fs::remove_file(fx.store.user_apps_path()).unwrap();

    let overview = commands.import(&package, 40).unwrap();
    assert!(overview.user_apps);
    assert_eq!(overview.notes, 1);
    assert!(read_rel(&fx.store, "notes/keep.json").contains("old-body"));
    assert!(read_rel(&fx.store, "user-apps.json").contains("旧名称"));
    assert!(
        !fx.store
            .data_dir()
            .join("notes")
            .join("extra.json")
            .exists()
    );
    assert!(fx.store.read_import_pending().unwrap().is_none());
}

#[test]
fn export_and_import_round_trip_search_usage() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    let marker = "a1b2c3d4e5f60718293a4b5c6d7e8f90112233445566778899aabbccddeeff00";
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"note-for-usage"}"#,
    );
    write_rel(
        &fx.store,
        "search-usage.json",
        &format!(
            r#"{{"schemaVersion":1,"items":[{{"kind":"todo","id":"t1","count":4}}],"queries":[{{"queryHash":"{marker}","kind":"todo","id":"t1","count":2}}]}}"#
        ),
    );
    let package = fx._temp.path().join("pkg.zip");
    commands.export(&package, 90).unwrap();
    let exported = String::from_utf8_lossy(&std::fs::read(&package).unwrap()).into_owned();
    assert!(exported.contains("search-usage.json"));
    assert!(exported.contains(marker));

    write_rel(
        &fx.store,
        "search-usage.json",
        r#"{"schemaVersion":1,"items":[],"queries":[]}"#,
    );
    commands.import(&package, 100).unwrap();
    let restored = read_rel(&fx.store, "search-usage.json");
    assert!(restored.contains(marker));
    assert!(restored.contains("\"count\":4"));

    let manual = commands.backup_manual(110).unwrap();
    let backed = String::from_utf8_lossy(&std::fs::read(manual).unwrap()).into_owned();
    assert!(backed.contains("search-usage.json"));
    assert!(backed.contains(marker));
}

#[test]
fn corrupt_search_usage_does_not_fail_backup_export_or_import() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"note-beside-corrupt-usage"}"#,
    );
    let notes_only = fx._temp.path().join("notes-only.zip");
    commands.export(&notes_only, 10).unwrap();

    let query = "raw-query-photoshop-marker";
    let corrupt = format!("{{not json {query}");
    write_rel(&fx.store, "search-usage.json", &corrupt);

    let manual = commands.backup_manual(20).unwrap();
    let backed = String::from_utf8_lossy(&std::fs::read(&manual).unwrap()).into_owned();
    assert!(!backed.contains(query));
    assert!(!backed.contains("search-usage.json"));
    assert!(backed.contains("note-beside-corrupt-usage"));

    let exported_path = fx._temp.path().join("with-corrupt.zip");
    commands.export(&exported_path, 30).unwrap();
    let exported = String::from_utf8_lossy(&std::fs::read(&exported_path).unwrap()).into_owned();
    assert!(!exported.contains(query));
    assert!(!exported.contains("search-usage.json"));

    let log = std::fs::read_to_string(fx.store.log_path()).unwrap();
    assert!(log.contains("搜索使用次数文件无法解析，未写入备份"));
    assert!(!log.contains(query));

    match commands.backup_on_launch(day(2026, 10, 22), 40).unwrap() {
        LaunchBackup::Created { path } => {
            let auto = String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned();
            assert!(!auto.contains(query));
            assert!(auto.contains("note-beside-corrupt-usage"));
        }
        LaunchBackup::AlreadyDone => panic!("expected a launch backup"),
    }

    commands.import(&notes_only, 50).unwrap();
    assert_eq!(read_rel(&fx.store, "search-usage.json"), corrupt);
    assert!(read_rel(&fx.store, "notes/n.json").contains("note-beside-corrupt-usage"));
}

#[test]
fn foreign_schema_search_usage_is_copied_and_kept_when_package_omits_it() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    let marker = "schema-two-marker-9f3a";
    let raw = format!(r#"{{"schemaVersion":2,"query":"pow-raw-text","marker":"{marker}"}}"#);
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"note-beside-schema-two"}"#,
    );
    write_rel(&fx.store, "search-usage.json", &raw);

    let manual = commands.backup_manual(60).unwrap();
    let backed = std::fs::read(manual).unwrap();
    assert!(
        backed
            .windows(raw.len())
            .any(|window| window == raw.as_bytes())
    );

    let exported_path = fx._temp.path().join("schema2.zip");
    commands.export(&exported_path, 70).unwrap();
    let exported = std::fs::read(&exported_path).unwrap();
    assert!(
        exported
            .windows(raw.len())
            .any(|window| window == raw.as_bytes())
    );

    std::fs::remove_file(fx.store.data_dir().join("search-usage.json")).unwrap();
    let notes_only = fx._temp.path().join("notes-only.zip");
    commands.export(&notes_only, 80).unwrap();
    write_rel(&fx.store, "search-usage.json", &raw);

    commands.import(&notes_only, 90).unwrap();
    assert_eq!(read_rel(&fx.store, "search-usage.json"), raw);

    write_rel(
        &fx.store,
        "search-usage.json",
        r#"{"schemaVersion":1,"items":[],"queries":[]}"#,
    );
    commands.import(&exported_path, 100).unwrap();
    assert_eq!(read_rel(&fx.store, "search-usage.json"), raw);
}

#[test]
fn schema_one_search_usage_is_removed_when_package_omits_it() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"only-note"}"#,
    );
    let package = fx._temp.path().join("notes.zip");
    commands.export(&package, 110).unwrap();
    write_rel(
        &fx.store,
        "search-usage.json",
        r#"{"schemaVersion":1,"items":[{"kind":"todo","id":"t","count":1}],"queries":[]}"#,
    );
    commands.import(&package, 120).unwrap();
    assert!(!fx.store.data_dir().join("search-usage.json").exists());
}

#[test]
fn interrupted_import_restores_on_boot() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"original-body"}"#,
    );
    let backup = commands.backup_manual(60).unwrap();
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"half-replaced"}"#,
    );
    write_rel(
        &fx.store,
        "notes/added.json",
        r#"{"schemaVersion":1,"body":"added"}"#,
    );
    fx.store.write_import_pending(&backup).unwrap();

    let mut hooks = BootHooks::new();
    BackupCommands::register_boot_hooks(&mut hooks);
    let report = fx.store.boot(hooks).unwrap();
    assert!(report.import_recovered);
    assert!(fx.store.read_import_pending().unwrap().is_none());
    assert!(read_rel(&fx.store, "notes/n.json").contains("original-body"));
    assert!(
        !fx.store
            .data_dir()
            .join("notes")
            .join("added.json")
            .exists()
    );
}

#[test]
fn missing_backup_leaves_import_pending() {
    let fx = fixture();
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"stay"}"#,
    );
    let missing = fx.store.backups_dir().join("import-missing.zip");
    fx.store.write_import_pending(&missing).unwrap();
    let mut hooks = BootHooks::new();
    BackupCommands::register_boot_hooks(&mut hooks);
    let err = fx.store.boot(hooks).unwrap_err();
    assert!(err.to_string().contains("备份文件不存在"));
    assert!(fx.store.read_import_pending().unwrap().is_some());
    assert!(read_rel(&fx.store, "notes/n.json").contains("stay"));
}

#[test]
fn import_reloads_open_notes_so_a_later_save_does_not_restore_the_old_body() {
    let fx = fixture();
    let notes = NoteCommands::open(fx.store.clone()).unwrap();
    let created = notes
        .create(&NoteInput {
            title: "标题".to_owned(),
            body: "imported-body".to_owned(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();
    let commands = BackupCommands::open(fx.store.clone());
    let package = fx._temp.path().join("pkg.zip");
    commands.export(&package, 70).unwrap();
    let edited = notes
        .save(
            &created.id,
            created.revision,
            &NoteInput {
                title: "标题".to_owned(),
                body: "edited-body".to_owned(),
                tags: Vec::new(),
                pinned: false,
            },
        )
        .unwrap();
    let rx = fx.store.subscribe();
    commands.import(&package, 80).unwrap();
    let loaded = notes.get(&created.id).unwrap();
    assert_eq!(loaded.body, "imported-body");
    let err = notes
        .save(
            &created.id,
            edited.revision,
            &NoteInput {
                title: "标题".to_owned(),
                body: "edited-body".to_owned(),
                tags: Vec::new(),
                pinned: false,
            },
        )
        .unwrap_err();
    assert!(matches!(err, NoteError::Conflict { .. }), "{err}");
    let disk = read_rel(&fx.store, &format!("notes/{}.json", created.id));
    assert!(disk.contains("imported-body"), "{disk}");
    assert!(!disk.contains("edited-body"), "{disk}");
    let mut saw_note = false;
    while let Ok(event) = rx.try_recv() {
        if event.kind == EntityKind::Note && event.id == created.id {
            saw_note = true;
        }
    }
    assert!(saw_note);
}

#[test]
fn export_omits_github_files_that_are_already_on_disk() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "github/watchlist.json",
        r#"{"schemaVersion":1,"repos":["acme/app"]}"#,
    );
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"kept-note"}"#,
    );
    let package = fx._temp.path().join("pkg.zip");
    let overview = commands.export(&package, 90).unwrap();
    assert!(!overview.watchlist);
    assert_eq!(overview.github_caches, 0);
    let text = String::from_utf8_lossy(&std::fs::read(&package).unwrap()).into_owned();
    assert!(text.contains("kept-note"));
    assert!(!text.contains("acme/app"));
    assert!(!text.contains("github/watchlist"));
}

fn zip_names(store: &Store) -> Vec<String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(store.backups_dir()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".zip") {
            names.push(name);
        }
    }
    names.sort();
    names
}

#[test]
fn float_geometry_goes_through_export_and_import() {
    let fx = fixture();
    let notes = NoteCommands::open(fx.store.clone()).unwrap();
    let note = notes
        .create(&NoteInput {
            title: "悬浮".to_owned(),
            body: "正文".to_owned(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();
    let geometry = FloatGeometry {
        x: 120,
        y: 80,
        width: 420,
        height: 300,
    };
    notes.set_float(&note.id, geometry, None).unwrap();

    let commands = BackupCommands::open(fx.store.clone());
    let package = fx._temp.path().join("float.zip");
    commands.export(&package, 30).unwrap();
    notes
        .set_float(&note.id, FloatGeometry { x: 0, ..geometry }, None)
        .unwrap();

    commands.import(&package, 40).unwrap();
    assert_eq!(notes.get(&note.id).unwrap().float, Some(geometry));
}

#[test]
fn float_dock_goes_through_export_import_and_backup_restore() {
    let fx = fixture();
    let notes = NoteCommands::open(fx.store.clone()).unwrap();
    let note = notes
        .create(&NoteInput {
            title: "贴边".to_owned(),
            body: "正文".to_owned(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();
    let other = notes
        .create(&NoteInput {
            title: "不贴".to_owned(),
            body: "正文".to_owned(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();
    let geometry = FloatGeometry {
        x: 0,
        y: 20,
        width: 300,
        height: 200,
    };
    let dock = lanwork_core::notes::FloatDock {
        edge: lanwork_core::notes::DockEdge::Top,
        offset: 20,
        collapsed: true,
    };
    let shelves = ShelfCommands::open(fx.store.clone()).unwrap();
    let docked_group = shelves.create_group("贴边").unwrap();
    let plain_group = shelves.create_group("不贴").unwrap();
    notes.set_float(&note.id, geometry, Some(dock)).unwrap();
    shelves
        .set_float(&docked_group.id, geometry, Some(dock))
        .unwrap();

    let commands = BackupCommands::open(fx.store.clone());
    let package = fx._temp.path().join("dock.zip");
    commands.export(&package, 30).unwrap();
    notes.set_float(&note.id, geometry, None).unwrap();
    shelves.set_float(&docked_group.id, geometry, None).unwrap();
    shelves
        .set_float(&plain_group.id, geometry, Some(dock))
        .unwrap();

    commands.import(&package, 40).unwrap();
    assert_eq!(notes.get(&note.id).unwrap().float_dock, Some(dock));
    assert!(notes.get(&other.id).unwrap().float_dock.is_none());
    assert_eq!(
        shelves.get(&docked_group.id).unwrap().float_dock,
        Some(dock)
    );
    assert!(shelves.get(&plain_group.id).unwrap().float_dock.is_none());

    notes.set_float(&note.id, geometry, None).unwrap();
    shelves.set_float(&docked_group.id, geometry, None).unwrap();
    let backup = commands.backup_manual(50).unwrap();
    notes.set_float(&note.id, geometry, Some(dock)).unwrap();
    shelves
        .set_float(&docked_group.id, geometry, Some(dock))
        .unwrap();
    commands.import(&backup, 60).unwrap();
    assert!(notes.get(&note.id).unwrap().float_dock.is_none());
    assert!(shelves.get(&docked_group.id).unwrap().float_dock.is_none());
}

#[test]
fn shelf_links_go_through_export_and_import_including_the_old_single_field() {
    let fx = fixture();
    write_rel(
        &fx.store,
        "shelves/old.json",
        r#"{"schemaVersion":1,"id":"old","name":"旧","order":0,"todoId":"t-old","refs":[]}"#,
    );
    let shelves = ShelfCommands::open(fx.store.clone()).unwrap();
    let many = shelves.create_group("新").unwrap();
    shelves.link_todo(&many.id, "t-1").unwrap();
    shelves.link_todo(&many.id, "t-2").unwrap();
    assert_eq!(shelves.get("old").unwrap().todo_ids, ["t-old"]);

    let commands = BackupCommands::open(fx.store.clone());
    let package = fx._temp.path().join("shelves.zip");
    commands.export(&package, 30).unwrap();
    shelves.unlink_todo(&many.id, "t-1").unwrap();
    shelves.link_todo("old", "t-extra").unwrap();

    commands.import(&package, 40).unwrap();
    assert_eq!(shelves.get("old").unwrap().todo_ids, ["t-old"]);
    assert_eq!(shelves.get(&many.id).unwrap().todo_ids, ["t-1", "t-2"]);
}

#[test]
fn shelf_web_refs_and_todo_float_travel_in_the_same_export() {
    let fx = fixture();
    write_rel(
        &fx.store,
        "shelves/web.json",
        r#"{"schemaVersion":1,"id":"web","name":"收纳","order":0,"refs":[],"webRefs":[{"url":"https://example.com/a","title":"示例","addedAt":1}]}"#,
    );
    write_rel(
        &fx.store,
        "todo-float.json",
        r#"{"schemaVersion":1,"floatWindow":{"x":4,"y":5,"width":6,"height":7},"marker":"keep-float"}"#,
    );
    let shelves = ShelfCommands::open(fx.store.clone()).unwrap();
    let commands = BackupCommands::open(fx.store.clone());
    let package = fx._temp.path().join("both.zip");
    commands.export(&package, 130).unwrap();
    let packed = String::from_utf8_lossy(&std::fs::read(&package).unwrap()).into_owned();
    assert!(packed.contains("webRefs"), "{packed}");
    assert!(packed.contains("https://example.com/a"));
    assert!(packed.contains("todo-float.json"));
    assert!(packed.contains("keep-float"));
    assert!(!packed.contains("<html"), "导出不含页面正文");

    write_rel(
        &fx.store,
        "shelves/web.json",
        r#"{"schemaVersion":1,"id":"web","name":"收纳","order":0,"refs":[]}"#,
    );
    write_rel(
        &fx.store,
        "todo-float.json",
        r#"{"schemaVersion":1,"floatWindow":{"x":1,"y":1,"width":1,"height":1}}"#,
    );
    commands.import(&package, 140).unwrap();
    let group = shelves.get("web").unwrap();
    assert_eq!(group.web_refs.len(), 1);
    assert_eq!(group.web_refs[0].url, "https://example.com/a");
    assert_eq!(group.web_refs[0].title, "示例");
    assert!(read_rel(&fx.store, "todo-float.json").contains("keep-float"));
}

#[test]
fn todo_float_file_is_copied_by_export_and_absent_from_old_packages() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "todo-float.json",
        r#"{"schemaVersion":1,"floatWindow":{"x":4,"y":5,"width":6,"height":7},"marker":"todo-float-marker"}"#,
    );
    write_rel(
        &fx.store,
        "todos/list.json",
        r#"{"schemaVersion":1,"id":"list"}"#,
    );
    let package = fx._temp.path().join("with-float.zip");
    let overview = commands.export(&package, 70).unwrap();
    assert_eq!(overview.todos, 1);
    let packed = String::from_utf8_lossy(&std::fs::read(&package).unwrap()).into_owned();
    assert!(packed.contains("todo-float.json"));
    assert!(packed.contains("todo-float-marker"));

    write_rel(
        &fx.store,
        "todo-float.json",
        r#"{"schemaVersion":1,"floatWindow":{"x":1,"y":1,"width":1,"height":1}}"#,
    );
    commands.import(&package, 80).unwrap();
    assert!(read_rel(&fx.store, "todo-float.json").contains("todo-float-marker"));
    assert!(read_rel(&fx.store, "todos/list.json").contains("list"));

    std::fs::remove_file(fx.store.data_dir().join("todo-float.json")).unwrap();
    let without = fx._temp.path().join("without-float.zip");
    commands.export(&without, 90).unwrap();
    commands.inspect(&without).unwrap();
    write_rel(
        &fx.store,
        "todo-float.json",
        r#"{"schemaVersion":1,"floatWindow":{"x":8,"y":8,"width":8,"height":8}}"#,
    );
    commands.import(&without, 100).unwrap();
    assert!(
        !fx.store.data_dir().join("todo-float.json").exists(),
        "旧包没有这份文件时导入不失败，本地这份按白名单删掉"
    );
    assert!(fx.store.data_dir().join("todos/list.json").exists());
}

#[test]
fn import_drops_the_open_todo_float_cache_so_a_later_write_does_not_restore_the_stack() {
    let fx = fixture();
    let todos = TodoCommands::open(fx.store.clone());
    todos.boot().unwrap();
    let commands = BackupCommands::open(fx.store.clone());
    let package = fx._temp.path().join("old-without-float.zip");
    commands.export(&package, 110).unwrap();
    assert!(
        !String::from_utf8_lossy(&std::fs::read(&package).unwrap()).contains("todo-float.json")
    );

    let before = FloatGeometry {
        x: 11,
        y: 22,
        width: 320,
        height: 360,
    };
    todos.set_float_geometry(before, None).unwrap();
    todos
        .set_float_stack(Some(FloatStackSlot {
            id: "pre-import-stack".into(),
            index: 1,
            current: true,
        }))
        .unwrap();
    assert!(read_rel(&fx.store, "todo-float.json").contains("pre-import-stack"));

    commands.import(&package, 120).unwrap();
    let path = fx.store.data_dir().join("todo-float.json");
    assert!(!path.exists(), "旧包没有这份文件时，本地文件被删掉");

    let FloatWindowFile::Record(record) = todos.read_float_window().unwrap() else {
        panic!("导入后应读到空记录");
    };
    assert_eq!(record, Default::default());
    assert!(!path.exists(), "读取不把已删除的文件写回去");

    let after = FloatGeometry {
        x: 40,
        y: 50,
        width: 200,
        height: 180,
    };
    todos.set_float_geometry(after, None).unwrap();
    let written = read_rel(&fx.store, "todo-float.json");
    assert!(written.contains("\"x\":40"), "{written}");
    assert!(
        !written.contains("pre-import-stack"),
        "记下新位置时不能把导入前的叠页写回去：{written}"
    );
    let FloatWindowFile::Record(record) = todos.read_float_window().unwrap() else {
        panic!("写入后应读到记录");
    };
    assert_eq!(record.geometry, Some(after));
    assert_eq!(record.stack, None);
    assert_eq!(record.dock, None);
}

const TOOL_MANIFEST: &str = r#"
id = "example.quota"
name = "API 额度"
description = "剩余额度"
entry = "main.lua"
services = ["secret", "storage"]

[[secret]]
id = "api_key"
label = "API 密钥"
header = "Authorization"
prefix = "Bearer "

[[component]]
id = "summary"
component = "text"
text = "没有数据"
"#;

#[test]
fn export_and_import_keep_tool_data_and_leave_secrets_behind() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "notes/n1.json",
        r#"{"schemaVersion":1,"body":"note-body-marker-77"}"#,
    );
    lanwork_core::panel::install_copy(
        &fx.store.data_dir(),
        "example.quota",
        "API 额度",
        "剩余额度",
        &[
            ("manifest.toml".to_string(), TOOL_MANIFEST.as_bytes().to_vec()),
            ("main.lua".to_string(), b"function on_load() end\n".to_vec()),
            (
                "icon.png".to_string(),
                b"\x89PNG\r\n\x1a\nicon-marker".to_vec(),
            ),
            ("helper.lua".to_string(), b"helper-secret-marker".to_vec()),
        ],
        br#"{"schemaVersion":1,"services":["notify"],"domains":[],"secrets":[],"value":"secret-marker-77"}"#,
    )
    .unwrap();
    write_rel(
        &fx.store,
        "tool-storage/example.quota.json",
        r#"{"schemaVersion":1,"entries":{"note":"storage-marker-77"}}"#,
    );
    write_rel(
        &fx.store,
        "tool-windows/example.quota.json",
        r#"{"schemaVersion":1,"floatWindow":{"x":12,"y":34,"width":320,"height":360},"floatPinned":false,"open":true}"#,
    );
    write_rel(
        &fx.store,
        "tool-enabled.json",
        r#"{"schemaVersion":1,"disabled":["example.quota","lanwork.float-shell-demo"],"token":"enabled-extra-marker"}"#,
    );
    write_rel(
        &fx.store,
        "tool-windows/lanwork.float-shell-demo.json",
        r#"{"schemaVersion":1,"open":true,"floatWindow":{"x":1,"y":1,"width":200,"height":120}}"#,
    );
    write_rel(&fx.store, "credentials.json", "credential-marker-77");
    for scratch in [
        "tools/.incoming-example.quota/leak.txt",
        "tools/.previous-example.quota/leak.txt",
        "tools/.hold-example.quota.0/leak.txt",
    ] {
        write_rel(&fx.store, scratch, "scratch-marker-77");
    }
    write_rel(
        &fx.store,
        "tools/example.bad/manifest.toml",
        "bad-body-marker",
    );
    write_rel(&fx.store, "tools/example.bad/main.lua", "nope");

    let dest = fx._temp.path().join("tools.zip");
    commands.export(&dest, 9).unwrap();
    let packed = String::from_utf8_lossy(&std::fs::read(&dest).unwrap()).into_owned();
    assert!(packed.contains("storage-marker-77"));
    assert!(packed.contains("note-body-marker-77"));
    assert!(packed.contains("icon-marker"));
    assert!(packed.contains("example.quota"));
    assert!(packed.contains("API 密钥"));
    assert!(!packed.contains("\"open\":true"));
    assert!(!packed.contains("secret-marker-77"));
    assert!(!packed.contains("helper-secret-marker"));
    assert!(!packed.contains("scratch-marker-77"));
    assert!(!packed.contains("credential-marker-77"));
    assert!(!packed.contains("bad-body-marker"));
    assert!(!packed.contains("enabled-extra-marker"));
    assert!(!packed.contains(".incoming-"));
    assert!(!packed.contains(".previous-"));
    assert!(!packed.contains(".hold-"));
    assert!(!packed.contains("lanwork.float-shell-demo"));
    let log = std::fs::read_to_string(fx.store.log_path()).unwrap_or_default();
    assert!(log.contains("工具未导出"));
    assert!(log.contains("example.bad"));
    assert!(!log.contains("secret-marker-77"));
    assert!(!log.contains("bad-body-marker"));

    write_rel(
        &fx.store,
        "notes/n1.json",
        r#"{"schemaVersion":1,"body":"replaced"}"#,
    );
    let _ = std::fs::remove_dir_all(fx.store.data_dir().join("tools"));
    commands.import(&dest, 10).unwrap();
    assert!(read_rel(&fx.store, "notes/n1.json").contains("note-body-marker-77"));
    assert!(read_rel(&fx.store, "tools/example.quota/main.lua").contains("on_load"));
    assert!(read_rel(&fx.store, "tools/example.quota/card.json").contains("API 额度"));
    let grant = read_rel(&fx.store, "tools/example.quota/grant.json");
    assert!(grant.contains("api_key"));
    assert!(grant.contains("API 密钥"));
    assert!(!grant.contains("secret-marker-77"));
    assert!(!grant.contains("notify"));
    assert!(
        !fx.store
            .data_dir()
            .join("tools/example.quota/helper.lua")
            .exists()
    );
    assert!(!fx.store.data_dir().join("tools/example.bad").exists());
    assert!(
        !fx.store
            .data_dir()
            .join("tools/.incoming-example.quota")
            .exists()
    );
    assert!(read_rel(&fx.store, "tool-storage/example.quota.json").contains("storage-marker-77"));
    let window = read_rel(&fx.store, "tool-windows/example.quota.json");
    assert!(window.contains("\"x\":12"));
    assert!(window.contains("\"floatPinned\":false"));
    assert!(!window.contains("\"open\":true"));
    assert!(read_rel(&fx.store, "tool-enabled.json").contains("example.quota"));
    assert!(!read_rel(&fx.store, "tool-enabled.json").contains("enabled-extra-marker"));
    assert_eq!(
        std::fs::read(fx.store.data_dir().join("credentials.json")).unwrap(),
        b"credential-marker-77"
    );
}

fn install_quota(store: &Store) {
    panel::install_copy(
        &store.data_dir(),
        "example.quota",
        "API 额度",
        "剩余额度",
        &[
            (
                "manifest.toml".to_string(),
                TOOL_MANIFEST.as_bytes().to_vec(),
            ),
            ("main.lua".to_string(), b"function on_load() end\n".to_vec()),
        ],
        br#"{"schemaVersion":1,"services":["storage"],"domains":[],"secrets":[]}"#,
    )
    .unwrap();
}

fn zip_contains(path: &Path, needle: &[u8]) -> bool {
    let bytes = std::fs::read(path).unwrap();
    !needle.is_empty() && bytes.windows(needle.len()).any(|window| window == needle)
}

fn enabled_bytes(ids: &[&str]) -> Vec<u8> {
    let disabled: BTreeSet<String> = ids.iter().map(|id| (*id).to_owned()).collect();
    encode_enabled(&disabled)
}

/// 包里的 `tool-enabled.json` 在 `disabled` 为空时也要写回。
/// 不写的话白名单清理会删掉它，下一次导入前的备份就不再带这份文件。
#[test]
fn import_writes_tool_enabled_for_empty_and_nonempty_disabled() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    install_quota(&fx.store);
    let path = fx.store.data_dir().join("tool-enabled.json");
    let empty = enabled_bytes(&[]);
    let disabled = enabled_bytes(&["example.quota"]);

    std::fs::write(&path, &empty).unwrap();
    let exported = fx._temp.path().join("enabled-empty.zip");
    commands.export(&exported, 1).unwrap();
    assert!(
        zip_contains(&exported, b"tool-enabled.json"),
        "导出应带上空的启用文件"
    );
    assert!(
        zip_contains(&exported, &empty),
        "导出里的启用文件应是空名单"
    );
    let manual = commands.backup_manual(2).unwrap();
    assert!(zip_contains(&manual, &empty), "备份应带上空的启用文件");

    std::fs::write(&path, &disabled).unwrap();
    commands.import(&exported, 3).unwrap();
    assert!(path.is_file(), "导入空名单后 tool-enabled.json 应仍在");
    assert_eq!(std::fs::read(&path).unwrap(), empty);
    let EnabledBytes::Known(set) = read_enabled(&fx.store.data_dir()).unwrap() else {
        panic!("应读回空的关闭名单");
    };
    assert!(set.is_empty());
    assert!(is_enabled(&set, "example.quota"));

    commands.import(&exported, 4).unwrap();
    let holding = fx.store.backups_dir().join("import-4.zip");
    assert!(
        zip_contains(&holding, &empty),
        "下一次导入前的备份仍应带上 tool-enabled.json"
    );
    assert_eq!(std::fs::read(&path).unwrap(), empty);

    std::fs::write(&path, &disabled).unwrap();
    let exported_off = fx._temp.path().join("enabled-off.zip");
    commands.export(&exported_off, 5).unwrap();
    assert!(zip_contains(&exported_off, &disabled));
    std::fs::remove_file(&path).unwrap();
    commands.import(&exported_off, 6).unwrap();
    assert!(path.is_file(), "导入非空名单后 tool-enabled.json 应写回");
    assert_eq!(std::fs::read(&path).unwrap(), disabled);
    let EnabledBytes::Known(set) = read_enabled(&fx.store.data_dir()).unwrap() else {
        panic!("应读回关掉的工具");
    };
    assert!(!is_enabled(&set, "example.quota"));

    let restore_from = commands.backup_manual(7).unwrap();
    std::fs::write(&path, &empty).unwrap();
    commands.import(&restore_from, 8).unwrap();
    assert!(path.is_file(), "从备份恢复后 tool-enabled.json 应仍在");
    assert_eq!(std::fs::read(&path).unwrap(), disabled);
    let EnabledBytes::Known(set) = read_enabled(&fx.store.data_dir()).unwrap() else {
        panic!("恢复后关掉的工具应仍在名单里");
    };
    assert!(!is_enabled(&set, "example.quota"));
}
