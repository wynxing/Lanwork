//! 收纳分组和引用的模型。
//!
//! 一个分组一个文件。字段名用架构文档里的 camelCase。
//! 新字段都有默认值。时间戳是 Unix 纪元起的 UTC 毫秒。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::notes::{FloatDock, FloatGeometry, FloatStackSlot, flexible_dock, flexible_stack};
use crate::storage::{SCHEMA_VERSION, is_supported_schema};

use super::error::ShelfError;
use super::path::normalize_path;

/// 没有任何分组时，`add_refs` 自动创建的分组名。
pub const DEFAULT_GROUP_NAME: &str = "收纳";

/// 存在性检查里每一条路径的上限。
///
/// 网络路径超过这个时间没有返回，就视为不存在。本地路径用同一上限，避免断开的盘符把调用挂住。
pub const EXISTS_TIMEOUT: Duration = Duration::from_secs(2);

/// 一个收纳分组。文件名是 [`Self::id`]，路径为 `shelves/<id>.json`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    pub id: String,
    pub name: String,
    pub order: i64,
    /// 关联的待办 id，按关联的先后。同一条只出现一次。旧文件的单条 `todoId` 读入时成为第一项。
    pub todo_ids: Vec<String>,
    pub refs: Vec<ShelfRef>,
    /// 网页引用。只保存网址和标题，不保存页面正文，也不保存网站图标字节。
    pub web_refs: Vec<WebRef>,
    /// 这个分组的悬浮窗上次的展开位置和大小。没有记录时为 `None`。旧文件没有这个字段。
    pub float: Option<FloatGeometry>,
    /// 贴边。没有记录时不贴边。旧文件里的 `floatPinned` 不进这个字段。
    pub float_dock: Option<FloatDock>,
    /// 叠页。没有记录时不在任何一叠里。
    pub float_stack: Option<FloatStackSlot>,
}

/// 一条路径引用。不包含文件内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfRef {
    pub path: String,
    pub name: String,
    pub folder: bool,
    pub added_at: i64,
}

/// 一条网页引用。`url` 是拖入时的原文（去掉首尾空白），比较用 [`super::weblink::normalize_web_url`]。
/// 网站图标不在这个结构里，也不写进分组文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRef {
    pub url: String,
    pub title: String,
    pub added_at: i64,
}

/// 拖入的一条网址。服务不访问网络，也不读取页面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingWeb {
    pub url: String,
    pub title: String,
}

/// 拖入时调用方给出的路径，以及它是不是文件夹。
///
/// 服务不探测用户路径的属性。文件夹标记以调用方这次传入的值为准。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingRef {
    pub path: String,
    pub folder: bool,
}

/// `add_refs` 的结果。没有写入时 `added` 为 0。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRefsOutcome {
    pub group: Option<Shelf>,
    pub added: usize,
}

/// 名称去掉首尾空白。只有空白时没有可保存的名称。
pub(crate) fn normalize_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(crate) fn sort_groups(groups: &mut [Shelf]) {
    groups.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn default_schema() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ShelfFile {
    #[serde(rename = "schemaVersion", default = "default_schema")]
    schema_version: u32,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    order: i64,
    /// 只读：写成 `todoIds` 之前的文件里的单条关联。写入时不再写这个字段。
    #[serde(rename = "todoId", default, skip_serializing)]
    legacy_todo_id: Option<String>,
    #[serde(rename = "todoIds", default, skip_serializing_if = "Vec::is_empty")]
    todo_ids: Vec<String>,
    #[serde(default)]
    refs: Vec<RefFile>,
    #[serde(rename = "webRefs", default, skip_serializing_if = "Vec::is_empty")]
    web_refs: Vec<WebRefFile>,
    #[serde(
        rename = "floatWindow",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    float: Option<FloatGeometry>,
    #[serde(
        rename = "floatDock",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_dock"
    )]
    float_dock: Option<FloatDock>,
    #[serde(
        rename = "floatStack",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_stack"
    )]
    float_stack: Option<FloatStackSlot>,
    /// 旧的逐窗钉住。读进来就丢掉，不再写回。
    #[serde(rename = "floatPinned", default, skip_serializing)]
    _legacy_float_pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebRefFile {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "addedAt", default)]
    added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefFile {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    folder: bool,
    #[serde(rename = "addedAt", default)]
    added_at: i64,
}

pub(crate) enum LoadedShelf {
    Group(Shelf),
    Unsupported { found: u32 },
}

impl ShelfFile {
    pub(crate) fn from_shelf(shelf: &Shelf) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: shelf.id.clone(),
            name: shelf.name.clone(),
            order: shelf.order,
            legacy_todo_id: None,
            todo_ids: shelf.todo_ids.clone(),
            float: shelf.float,
            float_dock: shelf.float_dock,
            float_stack: shelf.float_stack.clone(),
            _legacy_float_pinned: false,
            refs: shelf
                .refs
                .iter()
                .map(|item| RefFile {
                    path: item.path.clone(),
                    name: item.name.clone(),
                    folder: item.folder,
                    added_at: item.added_at,
                })
                .collect(),
            web_refs: shelf
                .web_refs
                .iter()
                .map(|item| WebRefFile {
                    url: item.url.clone(),
                    title: item.title.clone(),
                    added_at: item.added_at,
                })
                .collect(),
        }
    }

    /// 文件名是 id。JSON 里的 `id` 与文件名不同时，以文件名为准，不在加载时写回。
    ///
    /// 引用按去重键整理：同一键只留第一条，无法规范化的引用不进入内存。
    /// 下次写入这个分组时才会把整理后的列表写回去。
    pub(crate) fn into_loaded(self, file_id: String) -> LoadedShelf {
        if !is_supported_schema(self.schema_version) {
            return LoadedShelf::Unsupported {
                found: self.schema_version,
            };
        }
        let mut refs = Vec::new();
        let mut seen = Vec::new();
        for item in self.refs {
            let Ok(normalized) = normalize_path(&item.path) else {
                continue;
            };
            if seen.iter().any(|key| key == &normalized.key) {
                continue;
            }
            seen.push(normalized.key);
            let name = if item.name.trim().is_empty() {
                normalized.name
            } else {
                item.name
            };
            refs.push(ShelfRef {
                path: normalized.stored,
                name,
                folder: item.folder,
                added_at: item.added_at,
            });
        }
        let mut web_refs = Vec::new();
        let mut seen_web = Vec::new();
        for item in self.web_refs {
            let Some(normalized) = super::weblink::normalize_web_url(&item.url) else {
                continue;
            };
            if seen_web.iter().any(|key| key == &normalized.key) {
                continue;
            }
            seen_web.push(normalized.key);
            web_refs.push(WebRef {
                url: normalized.stored,
                title: item.title.trim().to_owned(),
                added_at: item.added_at,
            });
        }
        let mut todo_ids: Vec<String> = Vec::new();
        for id in self.legacy_todo_id.into_iter().chain(self.todo_ids) {
            if !todo_ids.contains(&id) {
                todo_ids.push(id);
            }
        }
        LoadedShelf::Group(Shelf {
            id: file_id,
            name: self.name,
            order: self.order,
            todo_ids,
            refs,
            web_refs,
            float: self.float,
            float_dock: self.float_dock,
            float_stack: self.float_stack,
        })
    }
}

pub(crate) fn unsupported(id: &str, found: u32) -> ShelfError {
    ShelfError::UnsupportedSchema {
        id: id.to_owned(),
        found,
    }
}
