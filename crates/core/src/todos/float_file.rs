//! 待办悬浮窗的位置文件 `todo-float.json`。
//!
//! 不写进清单，也不改待办的完成状态、`order` 或修订。和 `complete_item` 不是同一次写入。
//! 无法解析时按存储层的单文件规则改名隔离，清单不动。不认识的 `schemaVersion` 不隔离，
//! 调用方拒绝这一份窗口记录，文件留在原地，之后也不写回去。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::notes::{FloatDock, FloatGeometry, FloatStackSlot};
use crate::storage::{
    Error as StorageError, IoAction, Store, atomic_write, fs_exists, fs_read, is_supported_schema,
    lock_mutex,
};

use super::error::TodoError;

/// 数据目录中的文件名。不在 `todos/` 里，避免被读成清单。
pub const FLOAT_FILE: &str = "todo-float.json";

/// 读到的窗口记录。三个字段都可以缺，表示没记过。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FloatWindowRecord {
    pub geometry: Option<FloatGeometry>,
    pub dock: Option<FloatDock>,
    pub stack: Option<FloatStackSlot>,
}

/// 这份文件的读取结果。清单是否打得开与此无关。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloatWindowFile {
    Record(FloatWindowRecord),
    /// `schemaVersion` 高于当前版本。文件没有改名，调用方不用里面的位置。
    UnsupportedSchema,
}

#[derive(Debug)]
pub(crate) struct Cache {
    known: bool,
    refused: bool,
    record: FloatWindowRecord,
    writes: u64,
}

impl Cache {
    pub(crate) fn new() -> Self {
        Self {
            known: false,
            refused: false,
            record: FloatWindowRecord::default(),
            writes: 0,
        }
    }
}

/// 导入替换数据目录之后丢掉这份缓存。下一次读写再从磁盘读。
pub(crate) fn forget(cache: &Mutex<Cache>) {
    let mut cache = lock_mutex(cache);
    cache.known = false;
    cache.refused = false;
    cache.record = FloatWindowRecord::default();
}

#[derive(Serialize)]
struct OnDisk {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "floatWindow", skip_serializing_if = "Option::is_none")]
    geometry: Option<FloatGeometry>,
    #[serde(rename = "floatDock", skip_serializing_if = "Option::is_none")]
    dock: Option<FloatDock>,
    #[serde(rename = "floatStack", skip_serializing_if = "Option::is_none")]
    stack: Option<FloatStackSlot>,
}

enum Loaded {
    Missing,
    Record(FloatWindowRecord),
    Unsupported,
}

pub(crate) fn read(store: &Store, cache: &Mutex<Cache>) -> Result<FloatWindowFile, TodoError> {
    let mut cache = lock_mutex(cache);
    ensure(&mut cache, store)?;
    Ok(view(&cache))
}

/// 记下展开位置和贴边。叠页字段不动。值和上次一样时不写盘。
pub(crate) fn set_geometry(
    store: &Store,
    cache: &Mutex<Cache>,
    geometry: FloatGeometry,
    dock: Option<FloatDock>,
) -> Result<(), TodoError> {
    let mut cache = lock_mutex(cache);
    ensure(&mut cache, store)?;
    if cache.refused {
        return Err(TodoError::FloatSchema);
    }
    if cache.record.geometry == Some(geometry) && cache.record.dock == dock {
        return Ok(());
    }
    let mut next = cache.record.clone();
    next.geometry = Some(geometry);
    next.dock = dock;
    write(store, &mut cache, &next)
}

/// 记下叠页。位置和贴边不动。`None` 表示不在任何一叠里。值没变不写盘。
pub(crate) fn set_stack(
    store: &Store,
    cache: &Mutex<Cache>,
    slot: Option<FloatStackSlot>,
) -> Result<(), TodoError> {
    let mut cache = lock_mutex(cache);
    ensure(&mut cache, store)?;
    if cache.refused {
        return Err(TodoError::FloatSchema);
    }
    if cache.record.stack == slot {
        return Ok(());
    }
    let mut next = cache.record.clone();
    next.stack = slot;
    write(store, &mut cache, &next)
}

#[cfg(test)]
pub(crate) fn write_count(cache: &Mutex<Cache>) -> u64 {
    lock_mutex(cache).writes
}

fn view(cache: &Cache) -> FloatWindowFile {
    if cache.refused {
        FloatWindowFile::UnsupportedSchema
    } else {
        FloatWindowFile::Record(cache.record.clone())
    }
}

fn ensure(cache: &mut Cache, store: &Store) -> Result<(), TodoError> {
    if cache.known {
        return Ok(());
    }
    match load(store)? {
        Loaded::Missing => {
            cache.known = true;
            cache.refused = false;
            cache.record = FloatWindowRecord::default();
        }
        Loaded::Record(record) => {
            cache.known = true;
            cache.refused = false;
            cache.record = record;
        }
        Loaded::Unsupported => {
            cache.known = true;
            cache.refused = true;
            cache.record = FloatWindowRecord::default();
        }
    }
    Ok(())
}

fn load(store: &Store) -> Result<Loaded, TodoError> {
    let path = store.data_dir().join(FLOAT_FILE);
    if !fs_exists(&path) {
        return Ok(Loaded::Missing);
    }
    let bytes = fs_read(&path)
        .map_err(|source| TodoError::Storage(StorageError::io(IoAction::Read, &path, source)))?;
    match parse(&bytes) {
        Ok(Parse::Record(record)) => Ok(Loaded::Record(record)),
        Ok(Parse::Unsupported) => Ok(Loaded::Unsupported),
        Err(err) => {
            quarantine(store, &path, &err)?;
            Ok(Loaded::Missing)
        }
    }
}

enum Parse {
    Record(FloatWindowRecord),
    Unsupported,
}

/// 只用来在 JSON 不是对象时得到一个 [`serde_json::Error`]。
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
    Ok(Parse::Record(FloatWindowRecord {
        geometry: obj.get("floatWindow").and_then(geometry_of),
        dock: obj.get("floatDock").and_then(FloatDock::from_json),
        stack: obj.get("floatStack").and_then(FloatStackSlot::from_json),
    }))
}

fn geometry_of(value: &Value) -> Option<FloatGeometry> {
    let obj = value.as_object()?;
    Some(FloatGeometry {
        x: i32::try_from(obj.get("x")?.as_i64()?).ok()?,
        y: i32::try_from(obj.get("y")?.as_i64()?).ok()?,
        width: u32::try_from(obj.get("width")?.as_u64()?).ok()?,
        height: u32::try_from(obj.get("height")?.as_u64()?).ok()?,
    })
}

fn write(store: &Store, cache: &mut Cache, record: &FloatWindowRecord) -> Result<(), TodoError> {
    let on_disk = OnDisk {
        schema_version: crate::storage::SCHEMA_VERSION,
        geometry: record.geometry,
        dock: record.dock,
        stack: record.stack.clone(),
    };
    let mut bytes =
        serde_json::to_vec(&on_disk).map_err(|_| TodoError::Storage(StorageError::Encode))?;
    bytes.push(b'\n');
    store.with_write_lock(|| {
        let path = store.data_dir().join(FLOAT_FILE);
        atomic_write(&path, &bytes).map_err(|source| {
            TodoError::Storage(StorageError::io(IoAction::Replace, &path, source))
        })
    })?;
    cache.record = record.clone();
    cache.known = true;
    cache.writes = cache.writes.saturating_add(1);
    Ok(())
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
    use crate::notes::{DockEdge, FloatDock, FloatStackSlot};
    use crate::storage::test_temp::TempDir;
    use crate::storage::{Store, StorePaths};
    use crate::todos::{NewTodo, TodoCommands};

    fn open() -> (TempDir, Store, TodoCommands) {
        let temp = TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            user_profile: temp.path().join("profile"),
            local_app_data: temp.path().join("local"),
        })
        .unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        (temp, store, todos)
    }

    fn geometry() -> FloatGeometry {
        FloatGeometry {
            x: 12,
            y: 34,
            width: 320,
            height: 360,
        }
    }

    fn dock() -> FloatDock {
        FloatDock {
            edge: DockEdge::Left,
            offset: 80,
            collapsed: true,
        }
    }

    #[test]
    fn missing_file_is_an_empty_record_and_lists_still_open() {
        let (_temp, _store, todos) = open();
        let file = todos.read_float_window().unwrap();
        assert_eq!(file, FloatWindowFile::Record(FloatWindowRecord::default()));
        let id = todos.create_list("工作").unwrap();
        assert!(todos.lists().unwrap().iter().any(|list| list.id == id));
    }

    #[test]
    fn geometry_dock_and_stack_round_trip_without_touching_todos() {
        let (_temp, store, todos) = open();
        let list_id = todos.create_list("工作").unwrap();
        let item_id = todos
            .create_item(
                &list_id,
                NewTodo {
                    title: "买菜".into(),
                    due: None,
                    remind_at: None,
                    recurrence: None,
                    source: None,
                },
            )
            .unwrap();
        let before = std::fs::read(store.data_dir().join(format!("todos/{list_id}.json"))).unwrap();
        todos.set_float_geometry(geometry(), Some(dock())).unwrap();
        let slot = FloatStackSlot {
            id: "stack-1".into(),
            index: 2,
            current: true,
        };
        todos.set_float_stack(Some(slot.clone())).unwrap();
        let FloatWindowFile::Record(record) = todos.read_float_window().unwrap() else {
            panic!("应读回记录");
        };
        assert_eq!(record.geometry, Some(geometry()));
        assert_eq!(record.dock, Some(dock()));
        assert_eq!(record.stack, Some(slot));
        let after = std::fs::read(store.data_dir().join(format!("todos/{list_id}.json"))).unwrap();
        assert_eq!(before, after, "写悬浮位置不改清单");
        todos.complete_item(&item_id).unwrap();
        let still = std::fs::read_to_string(store.data_dir().join(FLOAT_FILE)).unwrap();
        assert!(still.contains("\"x\":12"));
        assert!(still.contains("\"edge\":\"left\""));
        assert!(still.contains("stack-1"));
        assert_eq!(todos.float_file_writes(), 2);
        todos.set_float_geometry(geometry(), Some(dock())).unwrap();
        assert_eq!(todos.float_file_writes(), 2, "值没变不写盘");
    }

    #[test]
    fn unknown_edge_drops_only_the_dock_and_a_missing_schema_reads_as_one() {
        let (_temp, store, todos) = open();
        let path = store.data_dir().join(FLOAT_FILE);
        std::fs::write(
            &path,
            r#"{"floatWindow":{"x":1,"y":2,"width":3,"height":4},"floatDock":{"edge":"bottom","offset":0,"collapsed":false}}"#,
        )
        .unwrap();
        let FloatWindowFile::Record(record) = todos.read_float_window().unwrap() else {
            panic!("缺 schemaVersion 按 1 读");
        };
        assert_eq!(
            record.geometry,
            Some(FloatGeometry {
                x: 1,
                y: 2,
                width: 3,
                height: 4
            })
        );
        assert_eq!(record.dock, None);
    }

    #[test]
    fn legacy_pin_field_does_not_drop_geometry_dock_or_stack() {
        let (_temp, store, todos) = open();
        let path = store.data_dir().join(FLOAT_FILE);
        std::fs::write(
            &path,
            r#"{"schemaVersion":1,"floatWindow":{"x":8,"y":9,"width":320,"height":360},"floatDock":{"edge":"left","offset":12,"collapsed":false},"floatStack":{"id":"mix","index":2,"current":true},"floatPinned":true}"#,
        )
        .unwrap();
        let FloatWindowFile::Record(record) = todos.read_float_window().unwrap() else {
            panic!("应读回记录");
        };
        assert_eq!(record.geometry.unwrap().x, 8);
        assert_eq!(record.dock.unwrap().edge, DockEdge::Left);
        assert_eq!(record.stack.unwrap().id, "mix");
        todos
            .set_float_geometry(
                FloatGeometry {
                    x: 8,
                    y: 9,
                    width: 321,
                    height: 360,
                },
                record.dock,
            )
            .unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("\"x\":8"));
        assert!(saved.contains("\"width\":321"));
        assert!(saved.contains("\"edge\":\"left\""));
        assert!(saved.contains("mix"));
        assert!(
            !saved.contains("floatPinned"),
            "待办悬浮窗的记录里没有钉住字段"
        );
    }

    #[test]
    fn corrupt_json_is_quarantined_and_lists_stay() {
        let (_temp, store, todos) = open();
        let list_id = todos.create_list("工作").unwrap();
        let path = store.data_dir().join(FLOAT_FILE);
        std::fs::write(&path, b"{not json").unwrap();
        let file = todos.read_float_window().unwrap();
        assert_eq!(file, FloatWindowFile::Record(FloatWindowRecord::default()));
        assert!(!path.exists(), "坏文件被改名");
        let quarantined = std::fs::read_dir(store.data_dir())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("todo-float.json.corrupt-")
            });
        assert!(quarantined);
        assert!(
            store
                .data_dir()
                .join(format!("todos/{list_id}.json"))
                .exists()
        );
    }

    #[test]
    fn unsupported_schema_is_refused_and_not_overwritten() {
        let (_temp, store, todos) = open();
        let path = store.data_dir().join(FLOAT_FILE);
        let body = r#"{"schemaVersion":2,"floatWindow":{"x":9,"y":9,"width":9,"height":9}}"#;
        std::fs::write(&path, body).unwrap();
        assert_eq!(
            todos.read_float_window().unwrap(),
            FloatWindowFile::UnsupportedSchema
        );
        let err = todos.set_float_geometry(geometry(), None).unwrap_err();
        assert!(matches!(err, TodoError::FloatSchema));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        assert_eq!(todos.float_file_writes(), 0);
        todos.create_list("仍可写").unwrap();
    }
}
