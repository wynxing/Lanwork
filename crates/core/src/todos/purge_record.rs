//! 永久删除还没让收纳解除关联时留下的 id。
//!
//! 文件在数据目录，不在 `todos/`。`todos/*.json` 会被读成清单。
//! 先写入再删条目：进程在清单写盘之后中断时，记录还在，下次可以补解除。
//! 记录无法读取时不覆盖，避免把尚未处理的 id 当成空。

use std::collections::BTreeSet;
use std::io;

use serde::{Deserialize, Serialize};

use crate::storage::{
    Error as StorageError, IoAction, SCHEMA_VERSION, Store, atomic_write, is_supported_schema,
};

use super::error::TodoError;

/// 数据目录中的文件名。
pub const PURGE_PENDING_FILE: &str = "todo-purge-pending.json";

#[derive(Debug, Serialize, Deserialize)]
struct PurgePendingFile {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    ids: BTreeSet<String>,
}

pub(crate) fn read(store: &Store) -> Result<BTreeSet<String>, TodoError> {
    let path = store.data_dir().join(PURGE_PENDING_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(_) => return Err(TodoError::PurgeRecord),
    };
    let file: PurgePendingFile =
        serde_json::from_slice(&bytes).map_err(|_| TodoError::PurgeRecord)?;
    if !is_supported_schema(file.schema_version) {
        return Err(TodoError::PurgeRecord);
    }
    Ok(file.ids)
}

pub(crate) fn write(store: &Store, ids: &BTreeSet<String>) -> Result<(), TodoError> {
    let path = store.data_dir().join(PURGE_PENDING_FILE);
    let file = PurgePendingFile {
        schema_version: SCHEMA_VERSION,
        ids: ids.clone(),
    };
    let bytes = serde_json::to_vec(&file).map_err(|_| TodoError::Storage(StorageError::Encode))?;
    atomic_write(&path, &bytes)
        .map_err(|source| TodoError::Storage(StorageError::io(IoAction::Replace, path, source)))
}

pub(crate) fn remove(store: &Store) -> Result<(), TodoError> {
    let path = store.data_dir().join(PURGE_PENDING_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(TodoError::Storage(StorageError::io(
            IoAction::Remove,
            path,
            source,
        ))),
    }
}
