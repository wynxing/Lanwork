use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::shelves::{IncomingRef, IncomingWeb};
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
        service.get(&first.id).unwrap().todo_ids,
        ["todo-a".to_owned()]
    );
    assert_eq!(
        service.get(&second.id).unwrap().todo_ids,
        ["todo-a".to_owned()]
    );
    assert_eq!(std::fs::read(&first_path).unwrap(), first_before);
    assert_eq!(std::fs::read(&second_path).unwrap(), second_before);
    assert!(rx.try_recv().is_err());
}

#[test]
fn failed_web_write_does_not_leave_a_group() {
    let (_temp, store, service) = service();
    let rx = store.subscribe();
    service.fail_next_write_at(0);
    let err = service
        .add_web_refs(
            None,
            &[IncomingWeb {
                url: "https://example.com/a".to_owned(),
                title: "页".to_owned(),
            }],
        )
        .unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert!(service.list().unwrap().is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn moving_refs_relocates_paths_and_web_pages_without_touching_the_file() {
    let (temp, store, service) = service();
    let file = temp.path.join("note.txt");
    std::fs::write(&file, b"keep").unwrap();
    let source = service.create_group("甲").unwrap();
    let target = service.create_group("乙").unwrap();
    let other = service.create_group("丙").unwrap();
    let path = file.to_str().unwrap().to_owned();
    service
        .add_refs(
            Some(&source.id),
            &[IncomingRef {
                path: path.clone(),
                folder: false,
            }],
        )
        .unwrap();
    service
        .add_refs(
            Some(&target.id),
            &[IncomingRef {
                path: path.clone(),
                folder: false,
            }],
        )
        .unwrap();
    service
        .add_web_refs(
            Some(&source.id),
            &[IncomingWeb {
                url: "https://Example.com/a/".to_owned(),
                title: "页".to_owned(),
            }],
        )
        .unwrap();
    let rx = store.subscribe();
    let same = service
        .move_refs(&source.id, &source.id, &[path.clone()], &[])
        .unwrap();
    assert_eq!(same.moved, 0);
    assert!(rx.try_recv().is_err(), "同一分组不写盘");

    let moved = service
        .move_refs(
            &source.id,
            &other.id,
            std::slice::from_ref(&path),
            &["https://example.com/a".to_owned()],
        )
        .unwrap();
    assert_eq!(moved.moved, 2);
    assert!(moved.source.refs.is_empty());
    assert!(moved.source.web_refs.is_empty());
    assert_eq!(moved.target.refs.len(), 1);
    assert_eq!(moved.target.web_refs.len(), 1);
    assert_eq!(moved.target.web_refs[0].title, "页");
    assert_eq!(service.get(&target.id).unwrap().refs.len(), 1);
    assert_eq!(std::fs::read(&file).unwrap(), b"keep");

    let collapsed = service
        .move_refs(&target.id, &other.id, &[path], &[])
        .unwrap();
    assert_eq!(collapsed.moved, 1);
    assert!(collapsed.source.refs.is_empty());
    assert_eq!(
        collapsed.target.refs.len(),
        1,
        "目标里已有同一路径时不留第二条"
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"keep");
}

#[test]
fn a_failed_move_leaves_both_groups_unchanged() {
    let (_temp, store, service) = service();
    let source = service.create_group("甲").unwrap();
    let target = service.create_group("乙").unwrap();
    service
        .add_web_refs(
            Some(&source.id),
            &[IncomingWeb {
                url: "https://example.com/a".to_owned(),
                title: "页".to_owned(),
            }],
        )
        .unwrap();
    let rx = store.subscribe();
    service.fail_next_write_at(1);
    let err = service
        .move_refs(
            &source.id,
            &target.id,
            &[],
            &["https://example.com/a".to_owned()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert_eq!(service.get(&source.id).unwrap().web_refs.len(), 1);
    assert!(service.get(&target.id).unwrap().web_refs.is_empty());
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
    while !service.get(&first.id).unwrap().todo_ids.is_empty()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        service.get(&first.id).unwrap().todo_ids,
        Vec::<String>::new()
    );
    assert_eq!(
        service.get(&second.id).unwrap().todo_ids,
        ["todo-b".to_owned()]
    );
}
