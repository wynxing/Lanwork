//! 搜索使用次数。文件是数据目录里的 `search-usage.json`。
//!
//! 条目键是已有的 id 或路径。按输入分开的次数用规范化输入的 SHA-256，不写输入原文。
//! 规范化是去掉首尾空白并转成小写。写入沿用存储层的临时文件和原子替换。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::storage::{self, Store};

use super::model::UsageKey;

/// 次数变化后，界面隔这么久再写盘。退出时另行写一次。
pub const USAGE_SAVE_DELAY: Duration = Duration::from_secs(30);

/// 条目总次数的上限。超出时去掉次数较低的。
pub(crate) const MAX_ITEM_ENTRIES: usize = 4096;
/// 按输入分开的记录上限。超出时去掉次数较低的。
pub(crate) const MAX_QUERY_ENTRIES: usize = 8192;

const SCHEMA_VERSION: u32 = 1;

fn current_schema() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiskStamp {
    Missing,
    Present { len: u64, hash: u64 },
}

#[derive(Debug)]
struct UsageMemory {
    book: UsageBook,
    dirty: bool,
    stamp: DiskStamp,
    /// `schemaVersion` 不是本程序能写的版本。内存可以计数，写盘保持原文件。
    frozen: bool,
}

/// 还在数据里的待办、便签和收纳。某一类读失败时对应的 `*_known` 为假，这次不删那一类。
#[derive(Debug, Default)]
pub(crate) struct UsageKeep {
    pub todos_known: bool,
    pub todos: HashSet<String>,
    pub notes_known: bool,
    pub notes: HashSet<String>,
    pub shelves_known: bool,
    pub shelf_files: HashSet<String>,
    pub shelf_folders: HashSet<String>,
    pub webs: HashSet<String>,
}

impl UsageKeep {
    pub(crate) fn shelf_token(group_id: &str, path: &str) -> String {
        format!("{group_id}\n{path}")
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageBook {
    items: HashMap<UsageKey, u64>,
    queries: HashMap<String, HashMap<UsageKey, u64>>,
}

/// 同一种命中类型内的排序分量。这次输入的次数占高 32 位，条目总次数占低 32 位。
#[must_use]
pub(crate) fn usage_rank(query_count: u64, item_count: u64) -> u64 {
    let query = u64::from(u32::try_from(query_count).unwrap_or(u32::MAX));
    let item = u64::from(u32::try_from(item_count).unwrap_or(u32::MAX));
    (query << 32) | item
}

/// 去掉首尾空白并转成小写后的 SHA-256，十六进制小写。
#[must_use]
pub fn query_hash(text: &str) -> String {
    let normalized = text.trim().to_lowercase();
    hex_encode(Sha256::digest(normalized.as_bytes()).as_slice())
}

pub(crate) fn rank_query_hash(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(query_hash(trimmed))
    }
}

pub(crate) fn rank_weight(book: &UsageBook, query_hash: Option<&str>, key: &UsageKey) -> u64 {
    usage_rank(book.query_count(query_hash, key), book.item_count(key))
}

impl UsageBook {
    fn record(&mut self, key: &UsageKey, query: &str) -> bool {
        if matches!(key, UsageKey::WebSearch | UsageKey::PathTimeout) || key_blank(key) {
            return false;
        }
        let count = self.items.entry(key.clone()).or_insert(0);
        *count = count.saturating_add(1);
        if let Some(hash) = rank_query_hash(query) {
            let row = self.queries.entry(hash).or_default();
            let count = row.entry(key.clone()).or_insert(0);
            *count = count.saturating_add(1);
        }
        true
    }

    pub(crate) fn item_count(&self, key: &UsageKey) -> u64 {
        self.items.get(key).copied().unwrap_or(0)
    }

    pub(crate) fn query_count(&self, query_hash: Option<&str>, key: &UsageKey) -> u64 {
        let Some(hash) = query_hash else {
            return 0;
        };
        self.queries
            .get(hash)
            .and_then(|rows| rows.get(key))
            .copied()
            .unwrap_or(0)
    }

    /// 去掉已经不在待办、便签、收纳里的记录，并套上条数上限。
    fn prune(&mut self, keep: &UsageKeep) -> bool {
        let before = self.len();
        self.items.retain(|key, _| alive(key, keep));
        self.queries.retain(|_, rows| {
            rows.retain(|key, _| alive(key, keep));
            !rows.is_empty()
        });
        let capped = self.enforce_caps(MAX_ITEM_ENTRIES, MAX_QUERY_ENTRIES);
        capped || self.len() != before
    }

    fn len(&self) -> usize {
        self.items.len() + self.query_len()
    }

    fn query_len(&self) -> usize {
        self.queries.values().map(HashMap::len).sum()
    }

    fn enforce_caps(&mut self, max_items: usize, max_queries: usize) -> bool {
        let mut changed = false;
        if self.items.len() > max_items {
            let drop_n = self.items.len() - max_items;
            let mut ranked: Vec<(u64, String, UsageKey)> = self
                .items
                .iter()
                .map(|(key, count)| (*count, sort_token(key), key.clone()))
                .collect();
            ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
            for (_, _, key) in ranked.into_iter().take(drop_n) {
                self.items.remove(&key);
                for rows in self.queries.values_mut() {
                    rows.remove(&key);
                }
            }
            self.queries.retain(|_, rows| !rows.is_empty());
            changed = true;
        }
        if self.query_len() > max_queries {
            let drop_n = self.query_len() - max_queries;
            let mut ranked = Vec::new();
            for (hash, rows) in &self.queries {
                for (key, count) in rows {
                    ranked.push((*count, hash.clone(), sort_token(key), key.clone()));
                }
            }
            ranked.sort_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
                    .then_with(|| left.2.cmp(&right.2))
            });
            for (_, hash, _, key) in ranked.into_iter().take(drop_n) {
                if let Some(rows) = self.queries.get_mut(&hash) {
                    rows.remove(&key);
                }
            }
            self.queries.retain(|_, rows| !rows.is_empty());
            changed = true;
        }
        changed
    }
}

fn alive(key: &UsageKey, keep: &UsageKeep) -> bool {
    match key {
        UsageKey::Todo(id) => !keep.todos_known || keep.todos.contains(id),
        UsageKey::Note(id) => !keep.notes_known || keep.notes.contains(id),
        UsageKey::ShelfFile { group_id, path } => {
            !keep.shelves_known
                || keep
                    .shelf_files
                    .contains(&UsageKeep::shelf_token(group_id, path))
        }
        UsageKey::ShelfFolder { group_id, path } => {
            !keep.shelves_known
                || keep
                    .shelf_folders
                    .contains(&UsageKeep::shelf_token(group_id, path))
        }
        UsageKey::Web(url) => !keep.shelves_known || keep.webs.contains(url),
        UsageKey::WebSearch | UsageKey::PathTimeout => false,
        UsageKey::App(_) | UsageKey::Browser(_) | UsageKey::File(_) | UsageKey::Folder(_) => true,
    }
}

fn key_blank(key: &UsageKey) -> bool {
    match key {
        UsageKey::App(id) | UsageKey::Todo(id) | UsageKey::Note(id) | UsageKey::Web(id) => {
            id.trim().is_empty()
        }
        UsageKey::Browser(url) | UsageKey::File(url) | UsageKey::Folder(url) => {
            url.trim().is_empty()
        }
        UsageKey::ShelfFile { group_id, path } | UsageKey::ShelfFolder { group_id, path } => {
            group_id.trim().is_empty() || path.trim().is_empty()
        }
        UsageKey::WebSearch | UsageKey::PathTimeout => true,
    }
}

fn sort_token(key: &UsageKey) -> String {
    match key {
        UsageKey::App(id) => format!("app\n{id}"),
        UsageKey::Todo(id) => format!("todo\n{id}"),
        UsageKey::Note(id) => format!("note\n{id}"),
        UsageKey::Web(url) => format!("web\n{url}"),
        UsageKey::Browser(url) => format!("browser\n{url}"),
        UsageKey::File(path) => format!("file\n{path}"),
        UsageKey::Folder(path) => format!("folder\n{path}"),
        UsageKey::ShelfFile { group_id, path } => format!("shelfFile\n{group_id}\n{path}"),
        UsageKey::ShelfFolder { group_id, path } => format!("shelfFolder\n{group_id}\n{path}"),
        UsageKey::WebSearch => "webSearch".to_owned(),
        UsageKey::PathTimeout => "pathTimeout".to_owned(),
    }
}

enum Decoded {
    Ready(UsageBook),
    Corrupt,
    Unsupported,
}

fn decode_book(bytes: &[u8]) -> Decoded {
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Decoded::Corrupt,
    };
    let Some(obj) = value.as_object() else {
        return Decoded::Corrupt;
    };
    let version = match obj.get("schemaVersion") {
        None => SCHEMA_VERSION,
        Some(serde_json::Value::Number(number)) => match number.as_u64() {
            Some(version) => u32::try_from(version).unwrap_or(u32::MAX),
            None => return Decoded::Corrupt,
        },
        Some(_) => return Decoded::Corrupt,
    };
    if version != SCHEMA_VERSION {
        return Decoded::Unsupported;
    }
    let file: FileDto = match serde_json::from_value(value) {
        Ok(file) => file,
        Err(_) => return Decoded::Corrupt,
    };
    let _ = file.schema_version;
    let mut book = UsageBook::default();
    for row in file.items {
        if let Some((key, count)) = row.into_count() {
            book.items.insert(key, count);
        }
    }
    for row in file.queries {
        if !valid_hash(&row.query_hash) {
            continue;
        }
        if let Some((key, count)) = row.item.into_count() {
            book.queries
                .entry(row.query_hash.to_lowercase())
                .or_default()
                .insert(key, count);
        }
    }
    Decoded::Ready(book)
}

fn encode_book(book: &UsageBook) -> Vec<u8> {
    let mut items: Vec<RowDto> = book
        .items
        .iter()
        .filter_map(|(key, count)| RowDto::from_key(key, *count))
        .collect();
    items.sort_by_key(RowDto::sort_key);
    let mut queries = Vec::new();
    for (hash, rows) in &book.queries {
        for (key, count) in rows {
            if let Some(item) = RowDto::from_key(key, *count) {
                queries.push(QueryDto {
                    query_hash: hash.clone(),
                    item,
                });
            }
        }
    }
    queries.sort_by(|left, right| {
        left.query_hash
            .cmp(&right.query_hash)
            .then_with(|| left.item.sort_key().cmp(&right.item.sort_key()))
    });
    let file = FileDto {
        schema_version: SCHEMA_VERSION,
        items,
        queries,
    };
    let mut bytes = serde_json::to_vec(&file).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    bytes
}

fn valid_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FileDto {
    #[serde(rename = "schemaVersion", default = "current_schema")]
    schema_version: u32,
    #[serde(default)]
    items: Vec<RowDto>,
    #[serde(default)]
    queries: Vec<QueryDto>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct QueryDto {
    #[serde(rename = "queryHash")]
    query_hash: String,
    #[serde(flatten)]
    item: RowDto,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RowDto {
    kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    path: String,
    #[serde(default, rename = "groupId", skip_serializing_if = "String::is_empty")]
    group_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    url: String,
    count: u64,
}

impl RowDto {
    fn from_key(key: &UsageKey, count: u64) -> Option<Self> {
        if count == 0 {
            return None;
        }
        let mut row = Self {
            kind: String::new(),
            id: String::new(),
            path: String::new(),
            group_id: String::new(),
            url: String::new(),
            count,
        };
        match key {
            UsageKey::App(id) => {
                row.kind = "app".to_owned();
                row.id = id.clone();
            }
            UsageKey::Todo(id) => {
                row.kind = "todo".to_owned();
                row.id = id.clone();
            }
            UsageKey::Note(id) => {
                row.kind = "note".to_owned();
                row.id = id.clone();
            }
            UsageKey::Web(url) => {
                row.kind = "web".to_owned();
                row.url = url.clone();
            }
            UsageKey::Browser(url) => {
                row.kind = "browser".to_owned();
                row.url = url.clone();
            }
            UsageKey::File(path) => {
                row.kind = "file".to_owned();
                row.path = path.clone();
            }
            UsageKey::Folder(path) => {
                row.kind = "folder".to_owned();
                row.path = path.clone();
            }
            UsageKey::ShelfFile { group_id, path } => {
                row.kind = "shelfFile".to_owned();
                row.group_id = group_id.clone();
                row.path = path.clone();
            }
            UsageKey::ShelfFolder { group_id, path } => {
                row.kind = "shelfFolder".to_owned();
                row.group_id = group_id.clone();
                row.path = path.clone();
            }
            UsageKey::WebSearch | UsageKey::PathTimeout => return None,
        }
        if key_blank(key) { None } else { Some(row) }
    }

    fn into_count(self) -> Option<(UsageKey, u64)> {
        if self.count == 0 {
            return None;
        }
        let key = match self.kind.as_str() {
            "app" => UsageKey::App(self.id),
            "todo" => UsageKey::Todo(self.id),
            "note" => UsageKey::Note(self.id),
            "web" => UsageKey::Web(self.url),
            "browser" => UsageKey::Browser(self.url),
            "file" => UsageKey::File(self.path),
            "folder" => UsageKey::Folder(self.path),
            "shelfFile" => UsageKey::ShelfFile {
                group_id: self.group_id,
                path: self.path,
            },
            "shelfFolder" => UsageKey::ShelfFolder {
                group_id: self.group_id,
                path: self.path,
            },
            _ => return None,
        };
        if key_blank(&key) {
            None
        } else {
            Some((key, self.count))
        }
    }

    fn sort_key(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}",
            self.kind, self.id, self.group_id, self.path, self.url
        )
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn stamp_of(bytes: &[u8]) -> DiskStamp {
    let digest = Sha256::digest(bytes);
    let mut hash = 0u64;
    for (index, byte) in digest.iter().take(8).enumerate() {
        hash |= u64::from(*byte) << (8 * index);
    }
    DiskStamp::Present {
        len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        hash,
    }
}

fn read_stamp(path: &Path) -> std::io::Result<DiskStamp> {
    if !storage::fs_exists(path) {
        return Ok(DiskStamp::Missing);
    }
    let bytes = storage::fs_read(path)?;
    Ok(stamp_of(&bytes))
}

fn read_memory(store: &Store, path: &Path) -> UsageMemory {
    if !storage::fs_exists(path) {
        return UsageMemory {
            book: UsageBook::default(),
            dirty: false,
            stamp: DiskStamp::Missing,
            frozen: false,
        };
    }
    let bytes = match storage::fs_read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            store.log_warn(&format!("搜索使用次数文件读不到，这次从空记录开始：{err}"));
            return UsageMemory {
                book: UsageBook::default(),
                dirty: false,
                stamp: DiskStamp::Missing,
                frozen: true,
            };
        }
    };
    let stamp = stamp_of(&bytes);
    match decode_book(&bytes) {
        Decoded::Ready(book) => UsageMemory {
            book,
            dirty: false,
            stamp,
            frozen: false,
        },
        Decoded::Corrupt => {
            store.log_warn("搜索使用次数文件无法解析，这次从空记录开始");
            UsageMemory {
                book: UsageBook::default(),
                dirty: false,
                stamp,
                frozen: false,
            }
        }
        Decoded::Unsupported => {
            store.log_warn("搜索使用次数文件的 schemaVersion 不受支持，这次从空记录开始");
            UsageMemory {
                book: UsageBook::default(),
                dirty: false,
                stamp,
                frozen: true,
            }
        }
    }
}

pub(crate) struct UsageHandle {
    store: Store,
    memory: Mutex<UsageMemory>,
}

impl UsageHandle {
    pub(crate) fn open(store: Store) -> Arc<Self> {
        let path = store.search_usage_path();
        let memory = read_memory(&store, &path);
        let handle = Arc::new(Self {
            store,
            memory: Mutex::new(memory),
        });
        let reload: Arc<dyn storage::MemoryReload> = handle.clone();
        handle.store.watch_memory(&reload);
        handle
    }

    pub(crate) fn record(&self, key: &UsageKey, query: &str) -> bool {
        let mut memory = storage::lock_mutex(&self.memory);
        if !memory.book.record(key, query) {
            return false;
        }
        if !memory.frozen {
            memory.dirty = true;
        }
        true
    }

    pub(crate) fn item_count(&self, key: &UsageKey) -> u64 {
        storage::lock_mutex(&self.memory).book.item_count(key)
    }

    pub(crate) fn query_count_text(&self, key: &UsageKey, query: &str) -> u64 {
        let memory = storage::lock_mutex(&self.memory);
        memory
            .book
            .query_count(rank_query_hash(query).as_deref(), key)
    }

    pub(crate) fn with_book<T>(&self, body: impl FnOnce(&UsageBook) -> T) -> T {
        let memory = storage::lock_mutex(&self.memory);
        body(&memory.book)
    }

    /// 有未写盘的变化，或需要清掉已经不存在的条目时，原子写入。
    ///
    /// 磁盘上的文件和上次读入的不一致时，改以磁盘为准，避免盖掉导入刚写回的那一份。
    pub(crate) fn flush(&self, keep: &UsageKeep) {
        self.store.with_write_lock(|| {
            let path = self.store.search_usage_path();
            let mut memory = storage::lock_mutex(&self.memory);
            let stamp = match read_stamp(&path) {
                Ok(stamp) => stamp,
                Err(err) => {
                    self.store.log_warn(&format!("搜索使用次数没有写入：{err}"));
                    return;
                }
            };
            if stamp != memory.stamp {
                let loaded = read_memory(&self.store, &path);
                *memory = loaded;
            }
            if memory.frozen {
                return;
            }
            if memory.book.prune(keep) {
                memory.dirty = true;
            }
            if !memory.dirty {
                return;
            }
            let bytes = encode_book(&memory.book);
            if let Err(err) = storage::atomic_write(&path, &bytes) {
                self.store.log_warn(&format!("搜索使用次数没有写入：{err}"));
                return;
            }
            memory.stamp = stamp_of(&bytes);
            memory.dirty = false;
        });
    }
}

impl storage::MemoryReload for UsageHandle {
    fn reload_memory(&self) -> Result<(), String> {
        let path = self.store.search_usage_path();
        let loaded = read_memory(&self.store, &path);
        *storage::lock_mutex(&self.memory) = loaded;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{FieldRole, Hit, HitKind, rank_hits};
    use crate::storage::{Store, StorePaths};

    fn todo(id: &str) -> UsageKey {
        UsageKey::Todo(id.to_owned())
    }

    fn note(id: &str) -> UsageKey {
        UsageKey::Note(id.to_owned())
    }

    fn shelf_file(group: &str, path: &str) -> UsageKey {
        UsageKey::ShelfFile {
            group_id: group.to_owned(),
            path: path.to_owned(),
        }
    }

    #[test]
    fn round_trip_keeps_item_and_query_counts() {
        let mut book = UsageBook::default();
        assert!(book.record(&todo("t1"), "  Photoshop "));
        assert!(book.record(&todo("t1"), "photoshop"));
        assert!(book.record(&UsageKey::File(r"C:\a.txt".to_owned()), "a"));
        let decoded = match decode_book(&encode_book(&book)) {
            Decoded::Ready(book) => book,
            Decoded::Corrupt | Decoded::Unsupported => panic!("round trip"),
        };
        assert_eq!(decoded.item_count(&todo("t1")), 2);
        assert_eq!(
            decoded.query_count(Some(&query_hash("photoshop")), &todo("t1")),
            2
        );
        assert_eq!(
            decoded.item_count(&UsageKey::File(r"C:\a.txt".to_owned())),
            1
        );
    }

    #[test]
    fn missing_and_corrupt_files_load_as_empty() {
        let (_temp, store) = open_store();
        let missing = UsageHandle::open(store.clone());
        assert_eq!(missing.item_count(&todo("t1")), 0);

        let path = store.search_usage_path();
        std::fs::write(&path, b"{not json").unwrap();
        let corrupt = UsageHandle::open(store);
        assert_eq!(corrupt.item_count(&todo("t1")), 0);
        assert!(std::fs::read(&path).unwrap().starts_with(b"{not json"));
    }

    #[test]
    fn encoded_file_stores_the_hash_and_not_the_query_text() {
        let mut book = UsageBook::default();
        book.record(&todo("t1"), "  Photoshop ");
        let text = String::from_utf8(encode_book(&book)).unwrap();
        assert!(text.contains(&query_hash("photoshop")));
        assert!(!text.contains("Photoshop"));
        assert!(!text.contains("photoshop"));
        assert!(text.contains("\"queryHash\""));
        assert!(text.contains("\"id\":\"t1\""));
    }

    #[test]
    fn query_count_outranks_item_count_within_the_same_match_kind() {
        let hits = vec![
            hit(1, HitKind::Prefix),
            hit(2, HitKind::Prefix),
            hit(3, HitKind::Exact),
        ];
        let ranked = rank_hits(&hits, |id| match id {
            1 => usage_rank(0, 1_000),
            2 => usage_rank(1, 1),
            3 => usage_rank(0, 0),
            _ => 0,
        });
        assert_eq!(
            ranked.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
        assert!(usage_rank(1, 0) > usage_rank(0, u64::from(u32::MAX)));
    }

    #[test]
    fn prune_drops_missing_todos_notes_and_shelves_and_caps_entries() {
        let mut book = UsageBook::default();
        book.record(&todo("keep"), "ps");
        book.record(&todo("gone"), "ps");
        book.record(&note("keep-note"), "ps");
        book.record(&note("gone-note"), "ps");
        book.record(&shelf_file("g", r"C:\keep"), "ps");
        book.record(&shelf_file("g", r"C:\gone"), "ps");
        book.record(&UsageKey::Web("https://keep.example".to_owned()), "ps");
        book.record(&UsageKey::Web("https://gone.example".to_owned()), "ps");
        book.record(&UsageKey::App("app".to_owned()), "ps");
        let mut keep = UsageKeep {
            todos_known: true,
            notes_known: true,
            shelves_known: true,
            ..UsageKeep::default()
        };
        keep.todos.insert("keep".to_owned());
        keep.notes.insert("keep-note".to_owned());
        keep.shelf_files
            .insert(UsageKeep::shelf_token("g", r"C:\keep"));
        keep.webs.insert("https://keep.example".to_owned());
        assert!(book.prune(&keep));
        assert_eq!(book.item_count(&todo("keep")), 1);
        assert_eq!(book.item_count(&todo("gone")), 0);
        assert_eq!(book.item_count(&note("keep-note")), 1);
        assert_eq!(book.item_count(&note("gone-note")), 0);
        assert_eq!(book.item_count(&shelf_file("g", r"C:\keep")), 1);
        assert_eq!(book.item_count(&shelf_file("g", r"C:\gone")), 0);
        assert_eq!(
            book.item_count(&UsageKey::Web("https://keep.example".to_owned())),
            1
        );
        assert_eq!(
            book.item_count(&UsageKey::Web("https://gone.example".to_owned())),
            0
        );
        assert_eq!(book.item_count(&UsageKey::App("app".to_owned())), 1);
        assert_eq!(book.query_count(Some(&query_hash("ps")), &todo("gone")), 0);

        let mut crowded = UsageBook::default();
        for index in 0..5 {
            crowded.record(&todo(&format!("t{index}")), "ps");
            if index == 4 {
                crowded.record(&todo("t4"), "ps");
            }
        }
        assert!(crowded.enforce_caps(2, 1));
        assert!(crowded.items.len() <= 2);
        assert!(crowded.query_len() <= 1);
        assert_eq!(crowded.item_count(&todo("t4")), 2);
    }

    #[test]
    fn unsupported_schema_stays_on_disk() {
        let (_temp, store) = open_store();
        let path = store.search_usage_path();
        std::fs::write(
            &path,
            b"{\"schemaVersion\":2,\"marker\":\"future-marker\"}\n",
        )
        .unwrap();
        let usage = UsageHandle::open(store.clone());
        assert!(usage.record(&todo("t1"), "Photoshop"));
        usage.flush(&UsageKeep::default());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("future-marker"));
        assert!(text.contains("\"schemaVersion\":2"));
        assert!(!text.contains("Photoshop"));
    }

    #[test]
    fn flush_round_trip_and_blank_query_skips_the_query_row() {
        let (_temp, store) = open_store();
        let usage = UsageHandle::open(store.clone());
        let path = store.search_usage_path();
        assert!(!path.exists());
        assert!(usage.record(&todo("t1"), " pow "));
        assert!(!path.exists());
        usage.flush(&UsageKeep {
            todos_known: true,
            todos: HashSet::from(["t1".to_owned()]),
            ..UsageKeep::default()
        });
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&query_hash("pow")));
        assert!(!text.contains("pow"));
        let again = UsageHandle::open(store);
        assert_eq!(again.item_count(&todo("t1")), 1);
        assert_eq!(again.query_count_text(&todo("t1"), "POW"), 1);
        assert!(again.record(&todo("t1"), "   "));
        assert_eq!(again.query_count_text(&todo("t1"), "pow"), 1);
        assert!(!again.record(&UsageKey::WebSearch, "pow"));
        assert!(!again.record(&UsageKey::PathTimeout, r"C:\hang"));
    }

    fn hit(id: u64, kind: HitKind) -> Hit {
        Hit {
            id,
            role: FieldRole::Name,
            field_index: 0,
            kind,
            score: 1,
        }
    }

    fn open_store() -> (crate::storage::test_temp::TempDir, Store) {
        let temp = crate::storage::test_temp::TempDir::new();
        let root = temp.path();
        let store = Store::open(StorePaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
        })
        .unwrap();
        (temp, store)
    }
}
