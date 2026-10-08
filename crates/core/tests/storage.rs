use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use lanwork_core::storage::{
    BootHooks, ChangeMeta, CollectionKind, DocumentId, EntityKind, ExampleDocument, FnRepair,
    SCHEMA_VERSION, Store, StorePaths,
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
        let path = std::env::temp_dir().join(format!("lanwork-storage-test-{nanos}-{seq}"));
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

fn open_store(root: &Path) -> Store {
    Store::open(StorePaths {
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        user_profile: root.join("profile"),
        local_app_data: root.join("local"),
    })
    .unwrap()
}

fn sample(title: &str) -> ExampleDocument {
    ExampleDocument {
        schema_version: SCHEMA_VERSION,
        id: "doc".into(),
        title: title.into(),
        pinned: false,
    }
}

#[test]
fn write_then_read_matches_and_message_comes_after_the_bytes() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let doc = DocumentId::Note("n1".into());
    let value = sample("第一次");
    let rx = store.subscribe();
    assert!(rx.try_recv().is_err());

    let receipt = store
        .write_with(&doc, &value, ChangeMeta { revision: Some(2) })
        .unwrap();
    assert_eq!(receipt.generation, 1);

    let event = rx.try_recv().unwrap();
    let roundtrip = store.read_json::<ExampleDocument>(&doc).unwrap().unwrap();
    assert_eq!(roundtrip, value);
    assert_eq!(event.kind, EntityKind::Note);
    assert_eq!(event.id, "n1");
    assert_eq!(event.revision, Some(2));
    let raw = std::fs::read(store.document_path(&doc).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<ExampleDocument>(&raw).unwrap(),
        value
    );
    assert!(
        !store
            .document_path(&doc)
            .unwrap()
            .with_file_name("n1.json.tmp")
            .exists()
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn paths_with_cjk_and_spaces_roundtrip() {
    let temp = TempDir::new();
    let root = temp.path().join("数据 目录");
    let store = Store::open(StorePaths {
        data_dir: root.join("Lanwork 数据"),
        cache_dir: root.join("缓存 目录"),
        user_profile: root.join("profile"),
        local_app_data: root.join("local"),
    })
    .unwrap();
    let doc = DocumentId::Note("便签 一".into());
    let value = ExampleDocument {
        schema_version: 1,
        id: "便签 一".into(),
        title: "你好 世界".into(),
        pinned: true,
    };
    store.write_json(&doc, &value).unwrap();
    assert_eq!(
        store.read_json::<ExampleDocument>(&doc).unwrap().unwrap(),
        value
    );
}

#[test]
fn long_data_dir_roundtrips() {
    let temp = TempDir::new();
    let mut data_dir = temp.path().join("long");
    let component = "d".repeat(40);
    while path_units(&data_dir) < 280 {
        data_dir.push(&component);
    }
    assert!(path_units(&data_dir) > 260, "{}", data_dir.display());
    let store = Store::open(StorePaths {
        data_dir: data_dir.clone(),
        cache_dir: temp.path().join("cache"),
        user_profile: temp.path().join("profile"),
        local_app_data: temp.path().join("local"),
    })
    .unwrap();
    let doc = DocumentId::Todo("t1".into());
    let value = sample("长路径");
    store.write_json(&doc, &value).unwrap();
    assert_eq!(
        store.read_json::<ExampleDocument>(&doc).unwrap().unwrap(),
        value
    );
    assert!(path_units(&store.document_path(&doc).unwrap()) > 260);
}

#[test]
fn concurrent_writes_keep_the_last_complete_value() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let doc = DocumentId::Note("shared".into());
    let barrier = Arc::new(Barrier::new(100));
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for index in 0..100 {
        let store = store.clone();
        let barrier = Arc::clone(&barrier);
        let results = Arc::clone(&results);
        let doc = doc.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            let value = ExampleDocument {
                schema_version: 1,
                id: "shared".into(),
                title: format!("value-{index}"),
                pinned: false,
            };
            let receipt = store.write_json(&doc, &value).unwrap();
            results
                .lock()
                .unwrap()
                .push((receipt.generation, value.title));
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let results = results.lock().unwrap();
    assert_eq!(results.len(), 100);
    let (_, last_title) = results
        .iter()
        .max_by_key(|(generation, _)| *generation)
        .unwrap();
    let got = store.read_json::<ExampleDocument>(&doc).unwrap().unwrap();
    assert_eq!(got.title, *last_title);
}

#[test]
fn corrupt_json_is_isolated_and_other_files_remain() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let good = DocumentId::Note("好".into());
    let bad = DocumentId::Note("坏".into());
    store
        .write_json(
            &good,
            &ExampleDocument {
                schema_version: 1,
                id: "好".into(),
                title: "可见标题".into(),
                pinned: false,
            },
        )
        .unwrap();
    store.write_json(&bad, &sample("将被破坏")).unwrap();
    let bad_path = store.document_path(&bad).unwrap();
    std::fs::write(
        &bad_path,
        "not-json 便签正文不应出现NOTE-BODY-SECRET ghp_thisisnotarealtoken123456",
    )
    .unwrap();

    let loaded = store
        .read_collection::<ExampleDocument>(CollectionKind::Notes)
        .unwrap();
    assert_eq!(loaded.files.len(), 1);
    assert_eq!(loaded.files[0].id, "好");
    assert_eq!(loaded.files[0].value.title, "可见标题");
    assert_eq!(loaded.quarantined.len(), 1);
    assert!(!bad_path.exists());
    let quarantine = loaded.quarantined[0].quarantine.clone().unwrap();
    let name = quarantine.file_name().unwrap().to_string_lossy();
    assert!(name.contains(".corrupt-"), "{name}");
    let log = std::fs::read_to_string(store.log_path()).unwrap();
    assert!(log.contains("json quarantined"));
    assert!(!log.contains("NOTE-BODY-SECRET"));
    assert!(!log.contains("ghp_thisisnotarealtoken123456"));

    let direct = DocumentId::Note("直接".into());
    let direct_path = store.document_path(&direct).unwrap();
    std::fs::write(&direct_path, "{").unwrap();
    let err = store.read_json::<ExampleDocument>(&direct).unwrap_err();
    assert!(err.to_string().contains("文件已损坏"), "{err}");
    assert!(err.to_string().contains("直接.json"), "{err}");
    assert!(!direct_path.exists());
}

#[test]
fn old_records_missing_fields_still_read_and_new_writes_require_schema() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let doc = DocumentId::Note("old".into());
    let path = store.document_path(&doc).unwrap();
    std::fs::write(&path, r#"{"id":"old","title":"旧记录","future":true}"#).unwrap();
    let value = store.read_json::<ExampleDocument>(&doc).unwrap().unwrap();
    assert_eq!(value.schema_version, SCHEMA_VERSION);
    assert!(!value.pinned);
    assert_eq!(value.title, "旧记录");
    assert!(path.exists());

    let newer = DocumentId::Note("newer".into());
    let newer_path = store.document_path(&newer).unwrap();
    std::fs::write(
        &newer_path,
        r#"{"schemaVersion":99,"id":"newer","title":"未来"}"#,
    )
    .unwrap();
    let parsed = store.read_json::<ExampleDocument>(&newer).unwrap().unwrap();
    assert_eq!(parsed.schema_version, 99);
    assert!(newer_path.exists());

    #[derive(serde::Serialize)]
    struct NoSchema {
        id: String,
    }
    let missing = DocumentId::Note("missing".into());
    let err = store
        .write_json(
            &missing,
            &NoSchema {
                id: "missing".into(),
            },
        )
        .unwrap_err();
    assert!(err.to_string().contains("schemaVersion"), "{err}");
    assert!(!store.document_path(&missing).unwrap().exists());
}

#[test]
fn batch_publishes_only_after_commit_and_failure_publishes_nothing() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let rx = store.subscribe();
    let first = DocumentId::Todo("a".into());
    let second = DocumentId::Todo("b".into());
    let mut batch = store.begin_batch().unwrap();
    batch.write_json(&first, &sample("甲")).unwrap();
    batch
        .write_with(&second, &sample("乙"), ChangeMeta { revision: Some(3) })
        .unwrap();
    assert!(rx.try_recv().is_err());
    batch.commit().unwrap();
    let first_event = rx.try_recv().unwrap();
    let second_event = rx.try_recv().unwrap();
    assert_eq!(first_event.id, "a");
    assert_eq!(first_event.revision, None);
    assert_eq!(second_event.id, "b");
    assert_eq!(second_event.revision, Some(3));

    let rx = store.subscribe();
    {
        let mut dropped = store.begin_batch().unwrap();
        dropped
            .write_json(&DocumentId::Todo("c".into()), &sample("丙"))
            .unwrap();
    }
    assert!(rx.try_recv().is_err());
    assert!(
        store
            .read_json::<ExampleDocument>(&DocumentId::Todo("c".into()))
            .unwrap()
            .is_some()
    );

    let mut failed = store.begin_batch().unwrap();
    failed
        .write_json(&DocumentId::Shelf("s".into()), &sample("组"))
        .unwrap();
    assert!(
        failed
            .write_json(&DocumentId::Shelf("a/b".into()), &sample("坏"))
            .is_err()
    );
    assert!(failed.commit().is_err());
    assert!(rx.try_recv().is_err());
}

#[test]
fn failed_repair_stops_before_index_and_does_not_invent_default_data() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let indexed = Arc::new(AtomicBool::new(false));
    let during = Arc::clone(&indexed);
    let mut hooks = BootHooks::new();
    hooks.repairs.push(Box::new(FnRepair::new(
        "currentSince",
        move |store: &Store| {
            let during = Arc::clone(&during);
            let result = store.build_index(|_| {
                during.store(true, Ordering::SeqCst);
                Ok(())
            });
            assert!(result.is_err());
            Err("修复失败".into())
        },
    )));
    let err = store.boot(hooks).unwrap_err();
    assert!(err.to_string().contains("加载修复失败"), "{err}");
    assert!(!store.is_ready());
    assert!(!indexed.load(Ordering::SeqCst));
    assert!(!store.document_path(&DocumentId::Config).unwrap().exists());
    let after = Arc::clone(&indexed);
    assert!(
        store
            .build_index(|_| {
                after.store(true, Ordering::SeqCst);
                Ok(())
            })
            .is_err()
    );
    assert!(store.handle_notification_click(|_| Ok(())).is_err());
    assert!(!indexed.load(Ordering::SeqCst));
    assert!(
        store
            .write_json(&DocumentId::Config, &sample("不再写"))
            .is_err()
    );
}

#[test]
fn boot_runs_import_recovery_then_load_then_repair_before_index() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let backup = temp.path().join("backup.zip");
    store.write_import_pending(&backup).unwrap();
    let steps = Arc::new(Mutex::new(Vec::new()));
    let mut hooks = BootHooks::new();
    let recover_steps = Arc::clone(&steps);
    let backup_expected = backup.clone();
    hooks.recover_import = Some(Box::new(move |store, pending| {
        assert_eq!(pending.backup_path, backup_expected);
        recover_steps.lock().unwrap().push("recover");
        store.clear_import_pending().unwrap();
        Ok(())
    }));
    let load_steps = Arc::clone(&steps);
    hooks.load = Some(Box::new(move |_| {
        load_steps.lock().unwrap().push("load");
        Ok(())
    }));
    let repair_steps = Arc::clone(&steps);
    hooks
        .repairs
        .push(Box::new(FnRepair::new("movedAt", move |_: &Store| {
            repair_steps.lock().unwrap().push("repair");
            Ok(())
        })));
    let report = store.boot(hooks).unwrap();
    assert!(report.import_recovered);
    assert!(!store.import_pending_path().exists());
    let index_steps = Arc::clone(&steps);
    store
        .build_index(|_| {
            index_steps.lock().unwrap().push("index");
            Ok(())
        })
        .unwrap();
    store.handle_notification_click(|_| Ok(())).unwrap();
    assert_eq!(
        steps.lock().unwrap().as_slice(),
        ["recover", "load", "repair", "index"]
    );
}

#[test]
fn missing_import_recovery_does_not_load_or_index() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    store
        .write_import_pending(&temp.path().join("backup.zip"))
        .unwrap();
    let loaded = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&loaded);
    let mut hooks = BootHooks::new();
    hooks.load = Some(Box::new(move |_| {
        flag.store(true, Ordering::SeqCst);
        Ok(())
    }));
    let err = store.boot(hooks).unwrap_err();
    assert!(err.to_string().contains("导入未完成"), "{err}");
    assert!(!loaded.load(Ordering::SeqCst));
    assert!(store.import_pending_path().exists());
    assert!(store.build_index(|_| Ok(())).is_err());
}

#[test]
fn open_refuses_maydolist_and_does_not_create_it() {
    let temp = TempDir::new();
    let profile = temp.path().join("profile");
    let forbidden = profile.join("Documents").join("MayDolist");
    let err = Store::open(StorePaths {
        data_dir: forbidden.clone(),
        cache_dir: temp.path().join("cache"),
        user_profile: profile,
        local_app_data: temp.path().join("local"),
    })
    .unwrap_err();
    assert!(err.to_string().contains("不能使用该数据目录"), "{err}");
    assert!(!forbidden.exists());
}

#[cfg(unix)]
#[test]
fn write_error_keeps_original_and_skips_change_message() {
    let temp = TempDir::new();
    let store = open_store(temp.path());
    let doc = DocumentId::Note("kept".into());
    let path = store.document_path(&doc).unwrap();
    store.write_json(&doc, &sample("original")).unwrap();
    let rx = store.subscribe();
    let dir = path.parent().unwrap();
    let original_mode = std::fs::metadata(dir).unwrap().permissions();
    set_mode(dir, 0o555);
    let err = store.write_json(&doc, &sample("replacement"));
    set_permissions(dir, original_mode);
    let err = err.unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert!(
        err.to_string().contains(&path.display().to_string()),
        "{err}"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("original"), "{text}");
    assert!(!text.contains("replacement"));
    assert!(rx.try_recv().is_err());
    assert!(!dir.join("kept.json.tmp").exists());
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn set_permissions(path: &Path, permissions: std::fs::Permissions) {
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(windows)]
#[test]
fn locked_file_without_share_delete_keeps_original_and_removes_temp() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let temp = TempDir::new();
    let store = open_store(temp.path());
    let doc = DocumentId::Note("kept".into());
    let path = store.document_path(&doc).unwrap();
    store.write_json(&doc, &sample("original")).unwrap();
    let rx = store.subscribe();
    // FILE_SHARE_READ = 1。不带 FILE_SHARE_DELETE，替换必须失败。
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let err = store.write_json(&doc, &sample("replacement")).unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert!(
        err.to_string().contains(&path.display().to_string()),
        "{err}"
    );
    drop(lock);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("original"), "{text}");
    assert!(!text.contains("replacement"));
    assert!(rx.try_recv().is_err());
    assert!(!path.with_file_name("kept.json.tmp").exists());
}

fn path_units(path: &Path) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().count()
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().len()
    }
}
