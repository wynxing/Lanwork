//! JSON 文档的 `schemaVersion` 约定。
//!
//! 当前版本是 [`SCHEMA_VERSION`]。写入的每个 JSON 对象都带数字字段 `schemaVersion`。
//! 新字段用 `#[serde(default)]`，旧文件缺字段仍能反序列化。缺 `schemaVersion` 时按当前版本理解。
//! 高于当前版本的值可以读出，但 [`is_supported_schema`] 为假；存储层不因此隔离文件。
//! 字段名使用架构文档里的 camelCase。

use serde::{Deserialize, Serialize};

/// 存储层当前写出的 schema 版本。
pub const SCHEMA_VERSION: u32 = 1;

pub fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

/// `schemaVersion` 是否是本程序能负责的版本。
///
/// 缺字段在反序列化时已经变成 [`SCHEMA_VERSION`]，不会以 0 出现。显式的 0 不受支持。
pub fn is_supported_schema(version: u32) -> bool {
    (1..=SCHEMA_VERSION).contains(&version)
}

pub(crate) fn require_written_schema(bytes: &[u8]) -> Result<(), super::Error> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| super::Error::Encode)?;
    match value.get("schemaVersion") {
        Some(serde_json::Value::Number(number)) if number.as_u64().is_some() => Ok(()),
        Some(_) => Err(super::Error::InvalidSchema),
        None => Err(super::Error::MissingSchema),
    }
}

/// 示例文档，演示 `schemaVersion` 和后加字段的默认值。
///
/// 这不是便签或待办模型。后续模型按同样的 serde 约定写。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleDocument {
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    /// 后加字段。旧文件没有它时为 false。
    #[serde(default)]
    pub pinned: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_json_without_new_fields_reads() {
        let parsed: ExampleDocument =
            serde_json::from_str(r#"{"id":"old","title":"旧记录"}"#).unwrap();
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert!(!parsed.pinned);
        assert_eq!(parsed.title, "旧记录");
        assert!(is_supported_schema(parsed.schema_version));
    }

    #[test]
    fn unknown_field_is_ignored() {
        let parsed: ExampleDocument = serde_json::from_str(
            r#"{"schemaVersion":1,"id":"n","title":"t","future":{"keep":true}}"#,
        )
        .unwrap();
        assert_eq!(parsed.id, "n");
        assert!(!parsed.pinned);
    }

    #[test]
    fn newer_schema_is_not_supported_and_still_parses() {
        let parsed: ExampleDocument =
            serde_json::from_str(r#"{"schemaVersion":99,"id":"n","title":"t"}"#).unwrap();
        assert_eq!(parsed.schema_version, 99);
        assert!(!is_supported_schema(99));
        assert!(!is_supported_schema(0));
    }

    #[test]
    fn written_json_must_include_numeric_schema_version() {
        let bytes = serde_json::to_vec(&ExampleDocument {
            schema_version: 1,
            id: "a".into(),
            title: "b".into(),
            pinned: false,
        })
        .unwrap();
        assert!(require_written_schema(&bytes).is_ok());
        assert!(require_written_schema(br#"{"id":"a"}"#).is_err());
        assert!(require_written_schema(br#"{"schemaVersion":"1","id":"a"}"#).is_err());
    }
}
