//! 第一批 MCP 工具。写入都走待办命令和便签命令。

use std::sync::atomic::Ordering;

use serde_json::{Map, Value, json};

use crate::notes::{Note, NoteCommands, NoteError, NoteInput};
use crate::todos::{
    ClockTime, ListKind, NewTodo, Recurrence, RecurrenceRule, TodoError, TodoItem, TodoPatch,
};

use super::{PendingOp, Shared};

const TODO_WRITE: &[&str] = &[
    "id",
    "revision",
    "title",
    "listId",
    "due",
    "remindAt",
    "recurrence",
];
const TODO_CREATE: &[&str] = &["title", "listId", "due", "remindAt", "recurrence"];
const NOTE_FIELDS: &[&str] = &["id", "revision", "title", "body", "tags", "pinned"];

pub(super) fn definitions() -> Value {
    json!([
        tool(
            "list_lists",
            "读取清单的 id、名称和种类",
            json!({ "type": "object", "properties": {}, "additionalProperties": false })
        ),
        tool(
            "list_todos",
            "读取待办。默认不含回收站",
            json!({
                "type": "object",
                "properties": {
                    "listId": { "type": "string" },
                    "includeTrash": { "type": "boolean" }
                },
                "additionalProperties": false
            })
        ),
        tool(
            "get_todo",
            "按 id 读取一条待办。回收站里的也能读到",
            obj_id(false)
        ),
        tool(
            "create_todo",
            "新建一条待办。未指定清单时放进收件箱",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "listId": { "type": "string" },
                    "due": { "type": ["string", "null"] },
                    "remindAt": { "type": ["string", "null"] },
                    "recurrence": { "type": ["object", "null"] }
                },
                "required": ["title"],
                "additionalProperties": false
            })
        ),
        tool(
            "update_todo",
            "修改已有待办里允许的字段",
            todo_write_schema()
        ),
        tool(
            "complete_todo",
            "完成待办。带周期的按产品规格生成下一次",
            obj_id(true)
        ),
        tool("uncomplete_todo", "取消完成", obj_id(true)),
        tool("delete_todo", "软删除，进入回收站", obj_id(true)),
        tool("restore_todo", "从回收站恢复待办", obj_id(true)),
        tool(
            "list_notes",
            "读取便签。默认不含回收站",
            json!({
                "type": "object",
                "properties": { "includeTrash": { "type": "boolean" } },
                "additionalProperties": false
            })
        ),
        tool(
            "get_note",
            "按 id 读取一篇便签，含正文和修订号。回收站里的也能读到",
            obj_id(false)
        ),
        tool(
            "create_note",
            "新建一篇便签",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "pinned": { "type": "boolean" }
                },
                "additionalProperties": false
            })
        ),
        tool(
            "update_note",
            "按修订号保存便签。冲突时不覆盖",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "revision": { "type": "integer" },
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "pinned": { "type": "boolean" }
                },
                "required": ["id", "revision"],
                "additionalProperties": false
            })
        ),
        tool("delete_note", "软删除便签，进入回收站", obj_id(true)),
        tool("restore_note", "从回收站恢复便签", obj_id(true)),
    ])
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
    })
}

fn obj_id(revision: bool) -> Value {
    if revision {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "revision": { "type": "integer" }
            },
            "required": ["id", "revision"],
            "additionalProperties": false
        })
    } else {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string" } },
            "required": ["id"],
            "additionalProperties": false
        })
    }
}

fn todo_write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "revision": { "type": "integer" },
            "title": { "type": "string" },
            "listId": { "type": "string" },
            "due": { "type": ["string", "null"] },
            "remindAt": { "type": ["string", "null"] },
            "recurrence": { "type": ["object", "null"] }
        },
        "required": ["id", "revision"],
        "additionalProperties": false
    })
}

pub(super) fn call(
    shared: &Shared,
    client: &str,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    if !shared.enabled.load(Ordering::Acquire) {
        return Err("未授权".to_owned());
    }
    let args = match args {
        Value::Object(_) => args,
        Value::Null => &Value::Null,
        _ => return Err("参数无效".to_owned()),
    };
    match name {
        "list_lists" => list_lists(shared, args),
        "list_todos" => list_todos(shared, args),
        "get_todo" => get_todo(shared, args),
        "create_todo" => create_todo(shared, args),
        "update_todo" => update_todo(shared, client, args),
        "complete_todo" => todo_gate(
            shared,
            client,
            args,
            PendingOp::Complete,
            |todos, id, revision| todos.complete_at(id, revision),
        ),
        "uncomplete_todo" => todo_gate(
            shared,
            client,
            args,
            PendingOp::Uncomplete,
            |todos, id, revision| todos.uncomplete_at(id, revision),
        ),
        "delete_todo" => todo_gate(
            shared,
            client,
            args,
            PendingOp::Delete,
            |todos, id, revision| todos.soft_delete_at(id, revision),
        ),
        "restore_todo" => todo_gate(
            shared,
            client,
            args,
            PendingOp::Restore,
            |todos, id, revision| todos.restore_at(id, revision),
        ),
        "list_notes" => list_notes(shared, args),
        "get_note" => get_note(shared, args),
        "create_note" => create_note(shared, args),
        "update_note" => update_note(shared, args),
        "delete_note" => note_gate(shared, args, true),
        "restore_note" => note_gate(shared, args, false),
        _ => Err("不在范围内".to_owned()),
    }
}

pub(super) fn todo_message(err: &TodoError) -> String {
    match err {
        TodoError::Storage(_) => "写盘失败".to_owned(),
        other => other.to_string(),
    }
}

fn note_message(err: &NoteError) -> String {
    match err {
        NoteError::Store(_) => "写盘失败".to_owned(),
        other => other.to_string(),
    }
}

fn ready(shared: &Shared) -> Result<(), String> {
    if shared.enabled.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err("未授权".to_owned())
    }
}

fn object<'a>(args: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let map = match args {
        Value::Null => return Ok(empty_map()),
        Value::Object(map) => map,
        _ => return Err("参数无效".to_owned()),
    };
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("参数无效".to_owned());
    }
    Ok(map)
}

fn empty_map() -> &'static Map<String, Value> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<Map<String, Value>> = OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn require_id(map: &Map<String, Value>) -> Result<String, String> {
    match map.get("id") {
        Some(Value::String(id)) if !id.is_empty() => Ok(id.clone()),
        _ => Err("参数无效".to_owned()),
    }
}

fn require_revision(map: &Map<String, Value>) -> Result<u64, String> {
    match map.get("revision") {
        Some(Value::Number(number)) => number.as_u64().ok_or_else(|| "参数无效".to_owned()),
        _ => Err("参数无效".to_owned()),
    }
}

fn list_lists(shared: &Shared, args: &Value) -> Result<Value, String> {
    object(args, &[])?;
    ready(shared)?;
    let lists = shared.todos.lists().map_err(|err| todo_message(&err))?;
    let lists: Vec<_> = lists
        .iter()
        .map(|list| {
            json!({
                "id": list.id,
                "name": list.name,
                "kind": match list.kind {
                    ListKind::Inbox => "inbox",
                    ListKind::Normal => "normal",
                },
            })
        })
        .collect();
    Ok(json!({ "lists": lists }))
}

fn list_todos(shared: &Shared, args: &Value) -> Result<Value, String> {
    let map = object(args, &["listId", "includeTrash"])?;
    let list_id = optional_string(map, "listId")?;
    let include_trash = optional_bool(map, "includeTrash")?.unwrap_or(false);
    ready(shared)?;
    let lists = shared.todos.lists().map_err(|err| todo_message(&err))?;
    if let Some(list_id) = &list_id
        && !lists.iter().any(|list| &list.id == list_id)
    {
        return Err(format!("找不到清单：{list_id}"));
    }
    let mut todos = Vec::new();
    for list in &lists {
        if list_id.as_ref().is_some_and(|id| id != &list.id) {
            continue;
        }
        for item in &list.items {
            if item.in_trash() && !include_trash {
                continue;
            }
            todos.push(todo_json(&list.id, item));
        }
    }
    Ok(json!({ "todos": todos }))
}

fn get_todo(shared: &Shared, args: &Value) -> Result<Value, String> {
    let map = object(args, &["id"])?;
    let id = require_id(map)?;
    ready(shared)?;
    let stored = shared.todos.item(&id).map_err(|err| todo_message(&err))?;
    Ok(todo_json(&stored.list_id, &stored.item))
}

fn create_todo(shared: &Shared, args: &Value) -> Result<Value, String> {
    let map = object(args, TODO_CREATE)?;
    let title = match map.get("title") {
        Some(Value::String(title)) => title.clone(),
        _ => return Err("参数无效".to_owned()),
    };
    if title.trim().is_empty() {
        return Err("标题为空".to_owned());
    }
    let list_id = optional_string(map, "listId")?;
    let due = optional_date(map, "due")?;
    let remind_at = optional_time(map, "remindAt")?;
    let recurrence = optional_recurrence(map)?;
    ready(shared)?;
    let list_id = match list_id {
        Some(list_id) => list_id,
        None => shared
            .todos
            .ensure_inbox()
            .map_err(|err| todo_message(&err))?,
    };
    let id = shared
        .todos
        .create_item(
            &list_id,
            NewTodo {
                title,
                due: due.flatten(),
                remind_at: remind_at.flatten(),
                recurrence: recurrence.flatten(),
                source: None,
            },
        )
        .map_err(|err| todo_message(&err))?;
    let stored = shared.todos.item(&id).map_err(|err| todo_message(&err))?;
    Ok(todo_json(&stored.list_id, &stored.item))
}

fn update_todo(shared: &Shared, client: &str, args: &Value) -> Result<Value, String> {
    let map = object(args, TODO_WRITE)?;
    let id = require_id(map)?;
    let revision = require_revision(map)?;
    let title = match map.get("title") {
        None => None,
        Some(Value::String(title)) => Some(title.clone()),
        Some(_) => return Err("参数无效".to_owned()),
    };
    if title.as_ref().is_some_and(|title| title.trim().is_empty()) {
        return Err("标题为空".to_owned());
    }
    let patch = TodoPatch {
        title,
        list_id: optional_string(map, "listId")?,
        due: optional_date(map, "due")?,
        remind_at: optional_time(map, "remindAt")?,
        recurrence: optional_recurrence(map)?,
    };
    ready(shared)?;
    match shared.guarded_todo(&id, || {
        shared.todos.apply_patch(&id, Some(revision), patch.clone())
    }) {
        Ok(item) => {
            let stored = shared
                .todos
                .item(&item.id)
                .map_err(|err| todo_message(&err))?;
            Ok(todo_json(&stored.list_id, &stored.item))
        }
        Err(TodoError::Conflict { id }) => {
            ready(shared)?;
            let title = shared.todo_title(&id);
            shared.store_conflict(&id, &title, client, PendingOp::Update(patch));
            Err(format!("待办保存冲突：{id}"))
        }
        Err(err) => Err(todo_message(&err)),
    }
}

fn todo_gate(
    shared: &Shared,
    client: &str,
    args: &Value,
    op: PendingOp,
    write: impl Fn(&crate::todos::TodoCommands, &str, u64) -> Result<(), TodoError>,
) -> Result<Value, String> {
    let map = object(args, &["id", "revision"])?;
    let id = require_id(map)?;
    let revision = require_revision(map)?;
    ready(shared)?;
    match shared.guarded_todo(&id, || write(&shared.todos, &id, revision)) {
        Ok(()) => {
            let stored = shared.todos.item(&id).map_err(|err| todo_message(&err))?;
            Ok(todo_json(&stored.list_id, &stored.item))
        }
        Err(TodoError::Conflict { id }) => {
            ready(shared)?;
            let title = shared.todo_title(&id);
            shared.store_conflict(&id, &title, client, op);
            Err(format!("待办保存冲突：{id}"))
        }
        Err(err) => Err(todo_message(&err)),
    }
}

fn list_notes(shared: &Shared, args: &Value) -> Result<Value, String> {
    let map = object(args, &["includeTrash"])?;
    let include_trash = optional_bool(map, "includeTrash")?.unwrap_or(false);
    ready(shared)?;
    let notes = notes(shared)?;
    let mut listed = notes.list();
    if include_trash {
        listed.extend(notes.deleted());
    }
    Ok(json!({ "notes": listed.iter().map(note_json).collect::<Vec<_>>() }))
}

fn get_note(shared: &Shared, args: &Value) -> Result<Value, String> {
    let map = object(args, &["id"])?;
    let id = require_id(map)?;
    ready(shared)?;
    let note = notes(shared)?.get(&id).map_err(|err| note_message(&err))?;
    Ok(note_json(&note))
}

fn create_note(shared: &Shared, args: &Value) -> Result<Value, String> {
    let map = object(args, &["title", "body", "tags", "pinned"])?;
    let input = NoteInput {
        title: optional_string(map, "title")?.unwrap_or_default(),
        body: optional_string(map, "body")?.unwrap_or_default(),
        tags: optional_tags(map)?.unwrap_or_default(),
        pinned: optional_bool(map, "pinned")?.unwrap_or(false),
    };
    ready(shared)?;
    let note = notes(shared)?
        .create(&input)
        .map_err(|err| note_message(&err))?;
    Ok(note_json(&note))
}

fn update_note(shared: &Shared, args: &Value) -> Result<Value, String> {
    let map = object(args, NOTE_FIELDS)?;
    let id = require_id(map)?;
    let revision = require_revision(map)?;
    ready(shared)?;
    let notes = notes(shared)?;
    let current = notes.get(&id).map_err(|err| note_message(&err))?;
    if current.is_deleted() {
        return Err(format!("便签已在回收站：{id}"));
    }
    let input = NoteInput {
        title: optional_string(map, "title")?.unwrap_or(current.title),
        body: optional_string(map, "body")?.unwrap_or(current.body),
        tags: optional_tags(map)?.unwrap_or(current.tags),
        pinned: optional_bool(map, "pinned")?.unwrap_or(current.pinned),
    };
    ready(shared)?;
    let note = notes
        .save(&id, revision, &input)
        .map_err(|err| note_message(&err))?;
    Ok(note_json(&note))
}

fn note_gate(shared: &Shared, args: &Value, delete: bool) -> Result<Value, String> {
    let map = object(args, &["id", "revision"])?;
    let id = require_id(map)?;
    let revision = require_revision(map)?;
    ready(shared)?;
    let notes = notes(shared)?;
    let note = if delete {
        notes.soft_delete(&id, revision)
    } else {
        notes.restore(&id, revision)
    }
    .map_err(|err| note_message(&err))?;
    Ok(note_json(&note))
}

fn notes(shared: &Shared) -> Result<&NoteCommands, String> {
    shared.notes.as_ref().ok_or_else(|| "写盘失败".to_owned())
}

fn optional_string(map: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err("参数无效".to_owned()),
    }
}

fn optional_bool(map: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err("参数无效".to_owned()),
    }
}

fn optional_tags(map: &Map<String, Value>) -> Result<Option<Vec<String>>, String> {
    match map.get("tags") {
        None => Ok(None),
        Some(Value::Array(items)) => {
            let mut tags = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(tag) => tags.push(tag.clone()),
                    _ => return Err("参数无效".to_owned()),
                }
            }
            Ok(Some(tags))
        }
        Some(_) => Err("参数无效".to_owned()),
    }
}

/// `None` 保持原值。`Some(None)` 清空。只用于更新；新建时再拍平。
fn optional_date(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<Option<crate::CivilDate>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(text)) => {
            let date = parse_date(text).ok_or_else(|| "参数无效".to_owned())?;
            Ok(Some(Some(date)))
        }
        Some(_) => Err("参数无效".to_owned()),
    }
}

fn optional_time(map: &Map<String, Value>, key: &str) -> Result<Option<Option<ClockTime>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(text)) => {
            let time = parse_time(text).ok_or_else(|| "提醒时间无效".to_owned())?;
            Ok(Some(Some(time)))
        }
        Some(_) => Err("参数无效".to_owned()),
    }
}

fn optional_recurrence(map: &Map<String, Value>) -> Result<Option<Option<Recurrence>>, String> {
    match map.get("recurrence") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::Object(fields)) => Ok(Some(Some(recurrence(fields)?))),
        Some(_) => Err("参数无效".to_owned()),
    }
}

fn recurrence(fields: &Map<String, Value>) -> Result<Recurrence, String> {
    if fields
        .keys()
        .any(|key| key != "rule" && key != "until" && key != "monthDay")
    {
        return Err("参数无效".to_owned());
    }
    let rule = match fields.get("rule").and_then(Value::as_str) {
        Some("daily") => RecurrenceRule::Daily,
        Some("weekly") => RecurrenceRule::Weekly,
        Some("biweekly") => RecurrenceRule::Biweekly,
        Some("monthly") => RecurrenceRule::Monthly,
        _ => return Err("参数无效".to_owned()),
    };
    let until = match fields.get("until") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(parse_date(text).ok_or_else(|| "参数无效".to_owned())?),
        Some(_) => return Err("参数无效".to_owned()),
    };
    let month_day = match fields.get("monthDay") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => {
            let day = number.as_u64().ok_or_else(|| "参数无效".to_owned())?;
            let day = u8::try_from(day).map_err(|_| "参数无效".to_owned())?;
            if !(1..=31).contains(&day) {
                return Err("参数无效".to_owned());
            }
            Some(day)
        }
        Some(_) => return Err("参数无效".to_owned()),
    };
    Ok(Recurrence {
        rule,
        until,
        month_day,
    })
}

fn parse_date(text: &str) -> Option<crate::CivilDate> {
    if text.len() != 10 {
        return None;
    }
    let year: i32 = text.get(0..4)?.parse().ok()?;
    let month: u8 = text.get(5..7)?.parse().ok()?;
    let day: u8 = text.get(8..10)?.parse().ok()?;
    if text.as_bytes().get(4) != Some(&b'-') || text.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    crate::CivilDate::try_from_ymd(year, month, day)
}

fn parse_time(text: &str) -> Option<ClockTime> {
    if text.len() != 5 || text.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let hour: u8 = text.get(0..2)?.parse().ok()?;
    let minute: u8 = text.get(3..5)?.parse().ok()?;
    ClockTime::try_new(hour, minute)
}

fn format_date(date: crate::CivilDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn format_time(time: ClockTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

fn todo_json(list_id: &str, item: &TodoItem) -> Value {
    let mut map = Map::new();
    map.insert("id".to_owned(), json!(item.id));
    map.insert("listId".to_owned(), json!(list_id));
    map.insert("title".to_owned(), json!(item.title));
    map.insert("completed".to_owned(), json!(item.completed));
    map.insert("revision".to_owned(), json!(item.revision));
    map.insert("order".to_owned(), json!(item.order));
    if let Some(due) = item.due {
        map.insert("due".to_owned(), json!(format_date(due)));
    }
    if let Some(remind) = item.remind_at {
        map.insert("remindAt".to_owned(), json!(format_time(remind)));
    }
    if let Some(recurrence) = &item.recurrence {
        map.insert(
            "recurrence".to_owned(),
            serde_json::to_value(recurrence).unwrap_or(Value::Null),
        );
    }
    if let Some(source) = &item.source {
        map.insert(
            "source".to_owned(),
            serde_json::to_value(source).unwrap_or(Value::Null),
        );
    }
    if let Some(deleted_at) = item.deleted_at {
        map.insert("deletedAt".to_owned(), json!(deleted_at));
    }
    Value::Object(map)
}

fn note_json(note: &Note) -> Value {
    let mut map = Map::new();
    map.insert("id".to_owned(), json!(note.id));
    map.insert("title".to_owned(), json!(note.title));
    map.insert("body".to_owned(), json!(note.body));
    map.insert("tags".to_owned(), json!(note.tags));
    map.insert("pinned".to_owned(), json!(note.pinned));
    map.insert("revision".to_owned(), json!(note.revision));
    map.insert("createdAt".to_owned(), json!(note.created_at.as_millis()));
    map.insert("updatedAt".to_owned(), json!(note.updated_at.as_millis()));
    if let Some(deleted_at) = note.deleted_at {
        map.insert("deletedAt".to_owned(), json!(deleted_at.as_millis()));
    }
    Value::Object(map)
}
