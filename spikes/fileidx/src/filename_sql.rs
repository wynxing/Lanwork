//! Windows Search 只按文件名匹配的 SQL。
//!
//! 正文检索不走这里。`content_contains_sql` 只给验证用来确认正文确实进了索引。

use crate::state::clamp_limit;

pub fn filename_like_sql(query: &str, limit: usize) -> String {
    let cap = clamp_limit(limit);
    format!(
        "SELECT TOP {cap} System.ItemPathDisplay, System.FileName, System.ItemNameDisplay, System.ItemType \
         FROM SystemIndex WHERE SCOPE='file:' AND System.FileName LIKE '{pattern}' \
         ORDER BY System.FileName",
        pattern = like_pattern(query),
    )
}

/// 负对照。会命中正文，不能当作产品查询。
pub fn content_contains_sql(token: &str, limit: usize) -> String {
    let cap = clamp_limit(limit);
    let token = token.replace('\'', "''");
    format!(
        "SELECT TOP {cap} System.ItemPathDisplay, System.FileName, System.ItemNameDisplay, System.ItemType \
         FROM SystemIndex WHERE SCOPE='file:' AND CONTAINS('{token}')"
    )
}

/// 没有 `*` 或 `?` 时按文件名子串。有通配符时 `*` 是任意长度，`?` 是一个字符，不再额外包一层子串。
pub fn like_pattern(query: &str) -> String {
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

pub fn aqs_filename(query: &str) -> String {
    let mut escaped = String::new();
    for ch in query.chars() {
        if ch == '\\' || ch == '"' {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    format!("System.FileName:\"{escaped}\"")
}

pub fn sql_is_filename_only(sql: &str) -> bool {
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
    fn substring_keeps_spaces_and_chinese() {
        let sql = filename_like_sql("季度 报告", 50);
        assert!(sql.contains("LIKE '%季度 报告%'"));
        assert!(sql_is_filename_only(&sql));
        assert!(sql.contains("TOP 50"));
        assert!(!sql.contains("CONTAINS("));
    }

    #[test]
    fn wildcards_map_to_like_and_literals_are_escaped() {
        assert_eq!(like_pattern("a?c"), "a_c");
        assert_eq!(like_pattern("pre*suf"), "pre%suf");
        assert_eq!(like_pattern("100%_["), "%100[%][_][[]%");
        assert_eq!(like_pattern("it's"), "%it''s%");
        let sql = filename_like_sql("lanworkprobe?mid.txt", 80);
        assert!(sql.contains("TOP 50"));
        assert!(sql.contains("LIKE 'lanworkprobe_mid.txt'"));
        assert_eq!(crate::state::MAX_RESULTS, 50);
    }

    #[test]
    fn content_control_is_not_filename_only() {
        let sql = content_contains_sql("token", 10);
        assert!(sql.contains("CONTAINS('token')"));
        assert!(!sql_is_filename_only(&sql));
        assert_eq!(aqs_filename("a\"b\\c"), "System.FileName:\"a\\\"b\\\\c\"");
    }
}
