//! Windows Search 只按文件名匹配的 SQL。
//!
//! 语句与 `spikes/fileidx` 里测通的那条相同。匹配方式（子串、前缀或整名，
//! 以及 `*`、`?` 和 `LIKE` 通配符是否同义）产品规格还没定。这里不生成正文检索。

use super::model::{MAX_FILE_RESULTS, clamp_results};

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn filename_like_sql(query: &str) -> String {
    let cap = clamp_results(MAX_FILE_RESULTS).max(1);
    format!(
        "SELECT TOP {cap} System.ItemPathDisplay, System.FileName, System.ItemNameDisplay, System.ItemType \
         FROM SystemIndex WHERE SCOPE='file:' AND System.FileName LIKE '{pattern}' \
         ORDER BY System.FileName",
        pattern = like_pattern(query),
    )
}

/// 没有 `*` 或 `?` 时按文件名子串。有通配符时 `*` 是任意长度，`?` 是一个字符。
///
/// 这是技术验证里实际发出的写法，不是已经定下的产品规则。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn like_pattern(query: &str) -> String {
    let wildcard = query.contains('*') || query.contains('?');
    let mut out = String::new();
    if !wildcard {
        out.push('%');
    }
    for ch in query.chars() {
        match ch {
            '\'' => out.push_str("''"),
            '%' | '_' | '[' => {
                out.push('[');
                out.push(ch);
                out.push(']');
            }
            '*' => out.push('%'),
            '?' => out.push('_'),
            _ => out.push(ch),
        }
    }
    if !wildcard {
        out.push('%');
    }
    out
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn sql_is_filename_only(sql: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    upper.contains("SYSTEM.FILENAME")
        && !upper.contains("CONTAINS(")
        && !upper.contains("FREETEXT(")
        && !upper.contains("SYSTEM.SEARCH.CONTENTS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_keeps_spaces_and_chinese_and_skips_body() {
        let sql = filename_like_sql("季度 报告");
        assert!(sql.contains("LIKE '%季度 报告%'"));
        assert!(sql_is_filename_only(&sql));
        assert!(sql.contains("TOP 50"));
        assert!(sql.contains("System.ItemType"));
        assert!(!sql.contains("CONTAINS("));
        assert!(!sql.contains("FREETEXT("));
    }

    #[test]
    fn wildcards_map_to_like_and_literals_are_escaped() {
        assert_eq!(like_pattern("a?c"), "a_c");
        assert_eq!(like_pattern("pre*suf"), "pre%suf");
        assert_eq!(like_pattern("100%_["), "%100[%][_][[]%");
        assert_eq!(like_pattern("it's"), "%it''s%");
        let sql = filename_like_sql("lanworkprobe?mid.txt");
        assert!(sql.contains("TOP 50"));
        assert!(sql.contains("LIKE 'lanworkprobe_mid.txt'"));
    }

    #[test]
    fn content_predicates_are_not_filename_only() {
        assert!(!sql_is_filename_only(
            "SELECT System.FileName WHERE CONTAINS('token')"
        ));
        assert!(!sql_is_filename_only(
            "SELECT System.FileName WHERE FREETEXT('token')"
        ));
        assert!(!sql_is_filename_only(
            "SELECT System.Search.Contents, System.FileName"
        ));
    }
}
