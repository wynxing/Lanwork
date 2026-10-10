//! 备份、导出和导入。界面尚未调用这些命令。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lanwork_core::CivilDate;
use lanwork_core::backup::{BackupCommands, LaunchBackup};
use lanwork_core::notes::{FloatGeometry, NoteCommands, NoteError, NoteInput};
use lanwork_core::storage::{BootHooks, EntityKind, Store, StorePaths};

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
    let outside = fx._temp.path().join("outside.bin");
    std::fs::write(&outside, b"shelf-file-bytes-not-in-zip").unwrap();
    std::fs::write(
        fx.store.data_dir().join("logs").join("app.log"),
        b"log-secret-marker-77",
    )
    .unwrap();
    let dest = fx._temp.path().join("out.zip");
    let overview = commands.export(&dest, false, 1_700_000_000_000).unwrap();
    assert!(overview.user_apps);
    assert!(overview.notes == 1);
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
    assert_zip_omits_env(&text, &["note-body-marker-77", "我的工具", "kept-ref"]);

    let full = commands.backup_manual(50).unwrap();
    let backed = String::from_utf8_lossy(&std::fs::read(full).unwrap()).into_owned();
    assert!(backed.contains("cache-marker-77"));
    assert!(backed.contains("我的工具"));
    assert!(!backed.contains("log-secret-marker-77"));
    assert_zip_omits_env(
        &backed,
        &[
            "note-body-marker-77",
            "我的工具",
            "kept-ref",
            "cache-marker-77",
        ],
    );
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
    commands.export(&package, false, 10).unwrap();
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
    commands.export(&package, true, 30).unwrap();

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
    commands.export(&package, false, 70).unwrap();
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
fn import_without_github_cache_keeps_the_local_cache() {
    let fx = fixture();
    let commands = BackupCommands::open(fx.store.clone());
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"from-package"}"#,
    );
    write_rel(
        &fx.store,
        "github/cache/acme%2Fapp.json",
        r#"{"schemaVersion":1,"repo":"before-export"}"#,
    );
    let package = fx._temp.path().join("pkg.zip");
    commands.export(&package, false, 90).unwrap();
    write_rel(
        &fx.store,
        "notes/n.json",
        r#"{"schemaVersion":1,"body":"local-edit"}"#,
    );
    write_rel(
        &fx.store,
        "github/cache/acme%2Fapp.json",
        r#"{"schemaVersion":1,"repo":"still-local"}"#,
    );
    commands.import(&package, 91).unwrap();
    assert!(read_rel(&fx.store, "notes/n.json").contains("from-package"));
    assert!(read_rel(&fx.store, "github/cache/acme%2Fapp.json").contains("still-local"));
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
    notes.set_float(&note.id, geometry).unwrap();

    let commands = BackupCommands::open(fx.store.clone());
    let package = fx._temp.path().join("float.zip");
    commands.export(&package, true, 30).unwrap();
    notes
        .set_float(&note.id, FloatGeometry { x: 0, ..geometry })
        .unwrap();

    commands.import(&package, 40).unwrap();
    assert_eq!(notes.get(&note.id).unwrap().float, Some(geometry));
}
