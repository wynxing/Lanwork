//! 待办、便签和收纳引用的常驻索引。
//!
//! 查询只读这份内存。变更来自 `EntityChanged` 之后对服务内存的重读，不读盘。

use std::collections::HashMap;

use crate::notes::{NoteCommands, display_title};
use crate::panel::shelf_source;
use crate::search::{
    FieldInput, FieldRole, Hit, MatchIndex, SearchGroup, prepare, query_prepared, rank_hits,
};
use crate::shelves::{ShelfCommands, normalize_web_url, web_display_name};
use crate::todos::TodoCommands;

use super::model::{RowDetail, SearchRow, UsageKey};
use super::usage::{UsageBook, rank_weight};

pub(crate) struct MemoryIndex {
    todos: Vec<IndexedTodo>,
    todo_match: MatchIndex,
    notes: Vec<IndexedNote>,
    note_match: MatchIndex,
    shelves: Vec<IndexedShelf>,
    shelf_match: MatchIndex,
}

struct IndexedTodo {
    item_id: String,
    list_id: String,
    list_name: String,
    title: String,
}

struct IndexedNote {
    id: String,
    title: String,
    body: String,
}

struct IndexedShelf {
    group_id: String,
    group_name: String,
    label: String,
    item: ShelfHit,
}

enum ShelfHit {
    Path { path: String, folder: bool },
    Web { url: String, key: String },
}

impl MemoryIndex {
    pub(crate) fn empty() -> Self {
        Self {
            todos: Vec::new(),
            todo_match: MatchIndex::new(),
            notes: Vec::new(),
            note_match: MatchIndex::new(),
            shelves: Vec::new(),
            shelf_match: MatchIndex::new(),
        }
    }

    pub(crate) fn rebuild_todos(&mut self, todos: &TodoCommands) -> Result<(), String> {
        let names = list_names(todos)?;
        let snapshot = todos.index_snapshot().map_err(|err| err.to_string())?;
        let mut rows = Vec::with_capacity(snapshot.len());
        let mut match_index = MatchIndex::new();
        for (index, entry) in snapshot.into_iter().enumerate() {
            let Some(id) = u64::try_from(index).ok() else {
                break;
            };
            match_index.insert(
                id,
                &[FieldInput {
                    role: FieldRole::Name,
                    text: &entry.title,
                }],
            );
            rows.push(IndexedTodo {
                list_name: names.get(&entry.list_id).cloned().unwrap_or_default(),
                item_id: entry.item_id,
                list_id: entry.list_id,
                title: entry.title,
            });
        }
        self.todos = rows;
        self.todo_match = match_index;
        Ok(())
    }

    pub(crate) fn rebuild_notes(&mut self, notes: &NoteCommands) {
        let listed = notes.list();
        let mut rows = Vec::with_capacity(listed.len());
        let mut match_index = MatchIndex::new();
        for (index, note) in listed.into_iter().enumerate() {
            let Some(id) = u64::try_from(index).ok() else {
                break;
            };
            let mut fields = Vec::with_capacity(note.tags.len() + 2);
            fields.push(FieldInput {
                role: FieldRole::Name,
                text: &note.title,
            });
            for tag in &note.tags {
                fields.push(FieldInput {
                    role: FieldRole::Tag,
                    text: tag,
                });
            }
            fields.push(FieldInput {
                role: FieldRole::Body,
                text: &note.body,
            });
            match_index.insert(id, &fields);
            rows.push(IndexedNote {
                id: note.id,
                title: note.title,
                body: note.body,
            });
        }
        self.notes = rows;
        self.note_match = match_index;
    }

    /// 收纳服务打不开时清空。文件和文件夹按保存的名称索引，网页引用按标题和网址。
    /// 同一分组里的路径不重复；同一网址在全部分组里只应有一条。
    pub(crate) fn rebuild_shelves(&mut self, shelves: Option<&ShelfCommands>) {
        let Some(shelves) = shelves else {
            self.shelves.clear();
            self.shelf_match = MatchIndex::new();
            return;
        };
        let listed = shelves.list().unwrap_or_default();
        let mut rows = Vec::new();
        let mut match_index = MatchIndex::new();
        for shelf in listed {
            for item in &shelf.refs {
                let Some(id) = u64::try_from(rows.len()).ok() else {
                    break;
                };
                if item.name.trim().is_empty() {
                    continue;
                }
                match_index.insert(
                    id,
                    &[FieldInput {
                        role: FieldRole::Name,
                        text: &item.name,
                    }],
                );
                rows.push(IndexedShelf {
                    group_id: shelf.id.clone(),
                    group_name: shelf.name.clone(),
                    label: item.name.clone(),
                    item: ShelfHit::Path {
                        path: item.path.clone(),
                        folder: item.folder,
                    },
                });
            }
            for web in &shelf.web_refs {
                let Some(id) = u64::try_from(rows.len()).ok() else {
                    break;
                };
                let Some(normalized) = normalize_web_url(&web.url) else {
                    continue;
                };
                let mut fields = Vec::with_capacity(2);
                if !web.title.trim().is_empty() {
                    fields.push(FieldInput {
                        role: FieldRole::Name,
                        text: &web.title,
                    });
                }
                fields.push(FieldInput {
                    role: FieldRole::Body,
                    text: &web.url,
                });
                match_index.insert(id, &fields);
                rows.push(IndexedShelf {
                    group_id: shelf.id.clone(),
                    group_name: shelf.name.clone(),
                    label: web_display_name(&web.title, &web.url),
                    item: ShelfHit::Web {
                        key: normalized.key,
                        url: web.url.clone(),
                    },
                });
            }
        }
        self.shelves = rows;
        self.shelf_match = match_index;
    }

    pub(crate) fn todo_ids(&self) -> Vec<String> {
        self.todos.iter().map(|row| row.item_id.clone()).collect()
    }

    pub(crate) fn note_ids(&self) -> Vec<String> {
        self.notes.iter().map(|row| row.id.clone()).collect()
    }

    pub(crate) fn rank_todos(
        &self,
        text: &str,
        usage: &UsageBook,
        query_hash: Option<&str>,
    ) -> Vec<SearchRow> {
        let hits = self.todo_match.query(text);
        let counts = self
            .todos
            .iter()
            .map(|row| rank_weight(usage, query_hash, &UsageKey::Todo(row.item_id.clone())))
            .collect::<Vec<_>>();
        rank_hits(&hits, |id| counts.get(id as usize).copied().unwrap_or(0))
            .into_iter()
            .filter_map(|hit| self.todo_row(hit))
            .collect()
    }

    pub(crate) fn rank_notes(
        &self,
        text: &str,
        usage: &UsageBook,
        query_hash: Option<&str>,
    ) -> Vec<SearchRow> {
        let hits = self.note_match.query(text);
        let counts = self
            .notes
            .iter()
            .map(|row| rank_weight(usage, query_hash, &UsageKey::Note(row.id.clone())))
            .collect::<Vec<_>>();
        rank_hits(&hits, |id| counts.get(id as usize).copied().unwrap_or(0))
            .into_iter()
            .filter_map(|hit| self.note_row(hit, text))
            .collect()
    }

    pub(crate) fn rank_shelves(
        &self,
        text: &str,
        usage: &UsageBook,
        query_hash: Option<&str>,
    ) -> Vec<SearchRow> {
        let hits = self.shelf_match.query(text);
        let counts = self
            .shelves
            .iter()
            .map(|row| rank_weight(usage, query_hash, &row.usage()))
            .collect::<Vec<_>>();
        rank_hits(&hits, |id| counts.get(id as usize).copied().unwrap_or(0))
            .into_iter()
            .filter_map(|hit| self.shelf_row(hit))
            .collect()
    }

    fn todo_row(&self, hit: Hit) -> Option<SearchRow> {
        let index = usize::try_from(hit.id).ok()?;
        let row = self.todos.get(index)?;
        Some(SearchRow {
            group: SearchGroup::Todo,
            kind: Some(hit.kind),
            usage: UsageKey::Todo(row.item_id.clone()),
            label: row.title.clone(),
            location: row.list_name.clone(),
            icon: None,
            detail: RowDetail::Todo {
                list_id: row.list_id.clone(),
                item_id: row.item_id.clone(),
            },
        })
    }

    fn note_row(&self, hit: Hit, query: &str) -> Option<SearchRow> {
        let index = usize::try_from(hit.id).ok()?;
        let row = self.notes.get(index)?;
        let location = if hit.role == FieldRole::Body {
            matching_line(&row.body, query).unwrap_or_default()
        } else {
            String::new()
        };
        Some(SearchRow {
            group: SearchGroup::Note,
            kind: Some(hit.kind),
            usage: UsageKey::Note(row.id.clone()),
            label: display_title(&row.title).to_owned(),
            location,
            icon: None,
            detail: RowDetail::Note { id: row.id.clone() },
        })
    }

    fn shelf_row(&self, hit: Hit) -> Option<SearchRow> {
        let index = usize::try_from(hit.id).ok()?;
        let row = self.shelves.get(index)?;
        let (icon, detail) = match &row.item {
            ShelfHit::Path {
                path,
                folder: false,
            } => (icon_for_path(path), RowDetail::File { path: path.clone() }),
            ShelfHit::Path { path, folder: true } => (
                icon_for_path(path),
                RowDetail::Folder { path: path.clone() },
            ),
            ShelfHit::Web { url, .. } => (None, RowDetail::Web { url: url.clone() }),
        };
        Some(SearchRow {
            group: SearchGroup::Shelf,
            kind: Some(hit.kind),
            usage: row.usage(),
            label: row.label.clone(),
            location: shelf_source(Some(&row.group_name)),
            icon,
            detail,
        })
    }
}

impl IndexedShelf {
    fn usage(&self) -> UsageKey {
        match &self.item {
            ShelfHit::Path {
                path,
                folder: false,
            } => UsageKey::ShelfFile {
                group_id: self.group_id.clone(),
                path: path.clone(),
            },
            ShelfHit::Path { path, folder: true } => UsageKey::ShelfFolder {
                group_id: self.group_id.clone(),
                path: path.clone(),
            },
            ShelfHit::Web { key, .. } => UsageKey::Web(key.clone()),
        }
    }
}

fn icon_for_path(path: &str) -> Option<super::model::IconKey> {
    if path.is_empty() {
        None
    } else {
        Some(super::model::IconKey {
            path: path.to_owned(),
            index: 0,
        })
    }
}

fn list_names(todos: &TodoCommands) -> Result<HashMap<String, String>, String> {
    let lists = todos.lists().map_err(|err| err.to_string())?;
    Ok(lists.into_iter().map(|list| (list.id, list.name)).collect())
}

/// 正文命中时，返回第一次单独命中的那一行。
///
/// 用和匹配引擎相同的正文规则逐行比较。跨行才命中时没有单独的一行。
pub(crate) fn matching_line(body: &str, query: &str) -> Option<String> {
    body.lines()
        .find(|line| line_matches(line, query))
        .map(str::to_owned)
}

fn line_matches(line: &str, query: &str) -> bool {
    let prepared = prepare(
        0,
        &[FieldInput {
            role: FieldRole::Body,
            text: line,
        }],
    );
    !query_prepared(std::slice::from_ref(&prepared), query).is_empty()
}
