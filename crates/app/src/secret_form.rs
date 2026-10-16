//! 填写密钥时写进凭据库的那些行。
//!
//! 空着的行不写。写到一半失败时，已经写上的留下，并指出失败的那一项。
//! 给用户看的句子只有名称，不含密钥原文。

use lanwork_toolhost::SecretStore;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SecretDraft {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) value: String,
}

#[derive(Debug)]
pub(crate) struct FailedSecret {
    pub(crate) label: String,
    /// 失败之前已经有一行写上了。调用方要重载开着的窗口，让这些值生效。
    pub(crate) wrote: bool,
    detail: String,
}

impl FailedSecret {
    /// 界面上的句子。只有这一项的名称，没有密钥原文，也没有凭据库的错误原文。
    #[must_use]
    pub(crate) fn message(&self) -> String {
        format!("「{}」没有写下", self.label)
    }

    #[must_use]
    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

/// 按顺序写入非空的行。第一次失败就停，后面的行不动。
pub(crate) fn save_secret_rows(
    store: &mut impl SecretStore,
    tool_id: &str,
    rows: &[SecretDraft],
) -> Result<(), FailedSecret> {
    let mut wrote = false;
    for row in rows {
        if row.value.is_empty() {
            continue;
        }
        if let Err(detail) = store.put(tool_id, &row.id, &row.value) {
            return Err(FailedSecret {
                label: row.label.clone(),
                wrote,
                detail,
            });
        }
        wrote = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SecretDraft, save_secret_rows};
    use lanwork_toolhost::SecretStore;

    struct Store {
        fail_on: usize,
        calls: usize,
        saved: Vec<(String, String, String)>,
    }

    impl SecretStore for Store {
        fn put(&mut self, tool_id: &str, secret_id: &str, value: &str) -> Result<(), String> {
            self.calls += 1;
            if self.calls == self.fail_on {
                return Err(format!("凭据写入失败：{value}"));
            }
            self.saved.push((
                tool_id.to_string(),
                secret_id.to_string(),
                value.to_string(),
            ));
            Ok(())
        }

        fn read(&self, _tool_id: &str, _secret_id: &str) -> Result<Option<String>, String> {
            Ok(None)
        }

        fn remove(&mut self, _tool_id: &str, _secret_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn row(id: &str, label: &str, value: &str) -> SecretDraft {
        SecretDraft {
            id: id.to_string(),
            label: label.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn a_failed_row_keeps_earlier_writes_and_names_only_the_label() {
        let rows = vec![
            row("first", "甲密钥", "one-secret"),
            row("second", "乙密钥", "two-secret"),
            row("blank", "空着", ""),
            row("third", "丙密钥", "three-secret"),
        ];
        let mut store = Store {
            fail_on: 2,
            calls: 0,
            saved: Vec::new(),
        };
        let failed = save_secret_rows(&mut store, "example.tool", &rows).unwrap_err();
        assert_eq!(failed.label, "乙密钥");
        assert!(failed.wrote);
        assert_eq!(failed.message(), "「乙密钥」没有写下");
        assert!(!failed.message().contains("one-secret"));
        assert!(!failed.message().contains("two-secret"));
        assert!(!failed.message().contains("three-secret"));
        assert!(failed.detail().contains("凭据写入失败"));
        assert_eq!(store.saved.len(), 1);
        assert_eq!(store.saved[0].1, "first");
        assert_eq!(store.calls, 2, "失败之后不再写后面的行");
    }

    #[test]
    fn the_first_failure_has_nothing_to_reload() {
        let mut store = Store {
            fail_on: 1,
            calls: 0,
            saved: Vec::new(),
        };
        let failed = save_secret_rows(
            &mut store,
            "example.tool",
            &[row("only", "API 密钥", "hidden")],
        )
        .unwrap_err();
        assert!(!failed.wrote);
        assert_eq!(failed.message(), "「API 密钥」没有写下");
        assert!(!failed.message().contains("hidden"));
        assert!(store.saved.is_empty());
    }

    #[test]
    fn empty_rows_are_skipped_and_a_full_save_reloads() {
        let mut store = Store {
            fail_on: 0,
            calls: 0,
            saved: Vec::new(),
        };
        save_secret_rows(
            &mut store,
            "example.tool",
            &[row("a", "甲", ""), row("b", "乙", "kept")],
        )
        .unwrap();
        assert_eq!(store.saved.len(), 1);
        assert_eq!(store.saved[0].1, "b");
    }
}
