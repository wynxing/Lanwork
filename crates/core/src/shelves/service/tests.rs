use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::storage::{DocumentId, Store, StorePaths};
use crate::todos::TodoNotice;

use super::ShelfService;

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
        let path = std::env::temp_dir().join(format!("lanwork-shelf-unit-{nanos}-{seq}"));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn service() -> (TempDir, Store, ShelfService) {
    let temp = TempDir::new();
    let store = Store::open(StorePaths {
        data_dir: temp.path.join("data"),
        cache_dir: temp.path.join("cache"),
        user_profile: temp.path.join("profile"),
        local_app_data: temp.path.join("local"),
    })
    .unwrap();
    let service = ShelfService::open_at(store.clone(), || 10).unwrap();
    (temp, store, service)
}

#[test]
fn second_write_failure_restores_the_first_group() {
    let (_temp, store, service) = service();
    let first = service.create_group("甲").unwrap();
    let second = service.create_group("乙").unwrap();
    service.link_todo(&first.id, "todo-a").unwrap();
    service.link_todo(&second.id, "todo-a").unwrap();
    let first_path = store
        .document_path(&DocumentId::Shelf(first.id.clone()))
        .unwrap();
    let second_path = store
        .document_path(&DocumentId::Shelf(second.id.clone()))
        .unwrap();
    let first_before = std::fs::read(&first_path).unwrap();
    let second_before = std::fs::read(&second_path).unwrap();
    let rx = store.subscribe();
    service.fail_next_write_at(1);
    let err = service.unlink_todo_everywhere("todo-a").unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert_eq!(
        service.get(&first.id).unwrap().todo_id.as_deref(),
        Some("todo-a")
    );
    assert_eq!(
        service.get(&second.id).unwrap().todo_id.as_deref(),
        Some("todo-a")
    );
    assert_eq!(std::fs::read(&first_path).unwrap(), first_before);
    assert_eq!(std::fs::read(&second_path).unwrap(), second_before);
    assert!(rx.try_recv().is_err());
}

#[test]
fn timed_out_probe_counts_as_missing() {
    let (_temp, _store, service) = service();
    service.set_exists_check(Duration::from_millis(40), |_path| {
        std::thread::sleep(Duration::from_secs(30));
        true
    });
    let started = std::time::Instant::now();
    let found = service.check_exists(&[r"\\server\share\offline.txt".to_owned()]);
    assert_eq!(found, vec![false]);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn purge_notice_clears_every_link() {
    let (_temp, _store, service) = service();
    let (sender, receiver) = std::sync::mpsc::channel();
    let first = service.create_group("甲").unwrap();
    let second = service.create_group("乙").unwrap();
    service.link_todo(&first.id, "todo-a").unwrap();
    service.link_todo(&second.id, "todo-b").unwrap();
    service.watch_notices(receiver);
    sender
        .send(TodoNotice::Purged {
            item_id: "todo-a".into(),
        })
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while service.get(&first.id).unwrap().todo_id.is_some() && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(service.get(&first.id).unwrap().todo_id, None);
    assert_eq!(
        service.get(&second.id).unwrap().todo_id.as_deref(),
        Some("todo-b")
    );
}
