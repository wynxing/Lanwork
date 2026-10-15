//! 已经发出的提醒，记在数据目录的 `reminder-sent.json`。
//!
//! 不进导出和备份。导入不删除它，也不把它当成清单。
//! 无法解析时按单文件规则隔离。不认识的 `schemaVersion` 不隔离、不写回。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::CivilDate;
use crate::storage::{
    Error as StorageError, IoAction, Store, atomic_write, fs_exists, fs_read, is_supported_schema,
    lock_mutex,
};

use super::error::TodoError;
use super::model::{ClockTime, TodoList};
use super::remind::SentKey;

/// 数据目录中的文件名。不在 `todos/` 里。
pub const SENT_FILE: &str = "reminder-sent.json";

#[derive(Debug)]
pub(crate) struct Cache {
    known: bool,
    refused: bool,
    sent: BTreeSet<SentKey>,
}

impl Cache {
    pub(crate) fn new() -> Self {
        Self {
            known: false,
            refused: false,
            sent: BTreeSet::new(),
        }
    }
}

pub(crate) fn forget(cache: &std::sync::Mutex<Cache>) {
    let mut cache = lock_mutex(cache);
    *cache = Cache::new();
}

pub(crate) fn current(
    store: &Store,
    cache: &std::sync::Mutex<Cache>,
) -> Result<BTreeSet<SentKey>, TodoError> {
    let mut cache = lock_mutex(cache);
    ensure(&mut cache, store)?;
    Ok(cache.sent.clone())
}

/// 把这次发出的键记下来，并丢掉已经对不上当前到期日和提醒时刻的旧键。
pub(crate) fn mark(
    store: &Store,
    cache: &std::sync::Mutex<Cache>,
    keys: &[SentKey],
    lists: &[TodoList],
) -> Result<(), TodoError> {
    let mut cache = lock_mutex(cache);
    ensure(&mut cache, store)?;
    for key in keys {
        cache.sent.insert(key.clone());
    }
    cache.sent.retain(|key| still_current(lists, key));
    if cache.refused {
        store.log_warn("提醒记录的版本无法识别，本次不写回");
        return Ok(());
    }
    write(store, &cache.sent)
}

fn still_current(lists: &[TodoList], key: &SentKey) -> bool {
    lists.iter().flat_map(|list| list.items.iter()).any(|item| {
        item.id == key.id
            && item.due == Some(key.due)
            && item
                .remind_at
                .is_some_and(|time| time.minute_of_day() == key.minute)
    })
}

fn ensure(cache: &mut Cache, store: &Store) -> Result<(), TodoError> {
    if cache.known {
        return Ok(());
    }
    match load(store)? {
        Loaded::Missing => {
            cache.known = true;
            cache.refused = false;
            cache.sent.clear();
        }
        Loaded::Keys(keys) => {
            cache.known = true;
            cache.refused = false;
            cache.sent = keys;
        }
        Loaded::Unsupported => {
            cache.known = true;
            cache.refused = true;
            cache.sent.clear();
        }
    }
    Ok(())
}

enum Loaded {
    Missing,
    Keys(BTreeSet<SentKey>),
    Unsupported,
}

fn load(store: &Store) -> Result<Loaded, TodoError> {
    let path = store.data_dir().join(SENT_FILE);
    if !fs_exists(&path) {
        return Ok(Loaded::Missing);
    }
    let bytes = fs_read(&path)
        .map_err(|source| TodoError::Storage(StorageError::io(IoAction::Read, &path, source)))?;
    match parse(&bytes) {
        Ok(Parse::Keys(keys)) => Ok(Loaded::Keys(keys)),
        Ok(Parse::Unsupported) => Ok(Loaded::Unsupported),
        Err(err) => {
            quarantine(store, &path, &err)?;
            Ok(Loaded::Missing)
        }
    }
}

enum Parse {
    Keys(BTreeSet<SentKey>),
    Unsupported,
}

#[derive(Debug, Deserialize)]
struct JsonMap {}

fn parse(bytes: &[u8]) -> Result<Parse, serde_json::Error> {
    let value: Value = serde_json::from_slice(bytes)?;
    let Some(obj) = value.as_object() else {
        return Err(serde_json::from_value::<JsonMap>(value).unwrap_err());
    };
    match obj.get("schemaVersion") {
        None => {}
        Some(Value::Number(number)) => match number.as_u64() {
            Some(version) if is_supported_schema(u32::try_from(version).unwrap_or(u32::MAX)) => {}
            _ => return Ok(Parse::Unsupported),
        },
        Some(_) => return Ok(Parse::Unsupported),
    }
    let mut keys = BTreeSet::new();
    if let Some(sent) = obj.get("sent").and_then(Value::as_array) {
        for entry in sent {
            if let Some(key) = key_of(entry) {
                keys.insert(key);
            }
        }
    }
    Ok(Parse::Keys(keys))
}

fn key_of(value: &Value) -> Option<SentKey> {
    let obj = value.as_object()?;
    let id = obj.get("id")?.as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let due = parse_date(obj.get("due")?.as_str()?)?;
    let minute = parse_time(obj.get("remindAt")?.as_str()?)?;
    Some(SentKey { id, due, minute })
}

fn parse_date(text: &str) -> Option<CivilDate> {
    if text.len() != 10
        || text.as_bytes().get(4) != Some(&b'-')
        || text.as_bytes().get(7) != Some(&b'-')
    {
        return None;
    }
    let year: i32 = text.get(0..4)?.parse().ok()?;
    let month: u8 = text.get(5..7)?.parse().ok()?;
    let day: u8 = text.get(8..10)?.parse().ok()?;
    CivilDate::try_from_ymd(year, month, day)
}

fn parse_time(text: &str) -> Option<u16> {
    if text.len() != 5 || text.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let hour: u8 = text.get(0..2)?.parse().ok()?;
    let minute: u8 = text.get(3..5)?.parse().ok()?;
    ClockTime::try_new(hour, minute).map(|time| time.minute_of_day())
}

fn write(store: &Store, sent: &BTreeSet<SentKey>) -> Result<(), TodoError> {
    let entries: Vec<_> = sent
        .iter()
        .map(|key| {
            json!({
                "id": key.id,
                "due": format!("{:04}-{:02}-{:02}", key.due.year(), key.due.month(), key.due.day()),
                "remindAt": format!("{:02}:{:02}", key.minute / 60, key.minute % 60),
            })
        })
        .collect();
    let mut bytes = serde_json::to_vec(&json!({
        "schemaVersion": crate::storage::SCHEMA_VERSION,
        "sent": entries,
    }))
    .map_err(|_| TodoError::Storage(StorageError::Encode))?;
    bytes.push(b'\n');
    store.with_write_lock(|| {
        let path = store.data_dir().join(SENT_FILE);
        atomic_write(&path, &bytes).map_err(|source| {
            TodoError::Storage(StorageError::io(IoAction::Replace, &path, source))
        })
    })
}

fn quarantine(
    store: &Store,
    path: &std::path::Path,
    err: &serde_json::Error,
) -> Result<(), TodoError> {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let file_name = path.file_name().unwrap_or_default();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = std::ffi::OsString::from(file_name);
    name.push(format!(".corrupt-{millis}-{seq}"));
    let target = path.with_file_name(name);
    store.with_write_lock(|| match std::fs::rename(path, &target) {
        Ok(()) => {
            store.log_warn(&format!(
                "json quarantined path={} quarantine={} line={} column={}",
                path.display(),
                target.display(),
                err.line(),
                err.column(),
            ));
            Ok(())
        }
        Err(source) => {
            store.log_warn(&format!(
                "json quarantine rename failed path={} line={} column={} os={:?}",
                path.display(),
                err.line(),
                err.column(),
                source.raw_os_error(),
            ));
            Err(TodoError::Storage(StorageError::io(
                IoAction::Rename,
                path,
                source,
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CivilDate;
    use crate::storage::test_temp::TempDir;
    use crate::storage::{Store, StorePaths};
    use crate::todos::{ClockTime, NewTodo, ReminderInstant, TodoCommands};

    fn open() -> (TempDir, StorePaths, Store, TodoCommands) {
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
        (temp, paths, store, todos)
    }

    fn when() -> ReminderInstant {
        ReminderInstant {
            date: CivilDate::try_from_ymd(2026, 10, 14).unwrap(),
            time: ClockTime::try_new(9, 0).unwrap(),
        }
    }

    #[test]
    fn sent_keys_round_trip_and_drop_when_the_reminder_changes() {
        let (_temp, paths, store, todos) = open();
        let list_id = todos.create_list("工作").unwrap();
        let item_id = todos
            .create_item(
                &list_id,
                NewTodo {
                    title: "买菜".into(),
                    due: Some(when().date),
                    remind_at: Some(when().time),
                    recurrence: None,
                    source: None,
                },
            )
            .unwrap();
        let key = SentKey {
            id: item_id.clone(),
            due: when().date,
            minute: when().time.minute_of_day(),
        };
        todos
            .mark_reminders_sent(std::slice::from_ref(&key))
            .unwrap();
        drop(todos);
        drop(store);
        let store = Store::open(paths).unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let now = when();
        let planned = todos.reminder_plan(now, None).unwrap();
        assert!(planned.due_now.is_empty());
        assert!(planned.missed.is_empty());

        todos.set_reminder(&item_id, 11, 0).unwrap();
        let later = todos
            .reminder_plan(
                ReminderInstant {
                    date: when().date,
                    time: ClockTime::try_new(11, 0).unwrap(),
                },
                None,
            )
            .unwrap();
        assert_eq!(later.due_now.len(), 1);
        let new_key = SentKey {
            id: item_id,
            due: when().date,
            minute: 11 * 60,
        };
        todos
            .mark_reminders_sent(std::slice::from_ref(&new_key))
            .unwrap();
        let raw = std::fs::read_to_string(store.data_dir().join(SENT_FILE)).unwrap();
        assert!(!raw.contains("09:00"));
        assert!(raw.contains("11:00"));
    }

    #[test]
    fn corrupt_sent_log_is_quarantined_and_lists_still_open() {
        let (_temp, _paths, store, todos) = open();
        let path = store.data_dir().join(SENT_FILE);
        std::fs::write(&path, b"{").unwrap();
        let _ = todos.reminder_plan(when(), None).unwrap();
        assert!(!path.exists());
        let quarantined = std::fs::read_dir(store.data_dir()).unwrap().any(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.file_name().into_string().ok())
                .is_some_and(|name| name.starts_with("reminder-sent.json.corrupt-"))
        });
        assert!(quarantined);
        assert!(todos.lists().is_ok());
    }

    #[test]
    fn export_whitelist_does_not_name_the_sent_log() {
        let source = include_str!("../backup/package.rs");
        assert!(!source.contains("reminder-sent.json"));
    }
}
