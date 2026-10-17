//! 工具 id 的路径分量规则，与数据文件 id 相同，另外拒绝以 `.` 开头。
//! 以 `.` 开头会和安装时的 `.previous-`、`.incoming-` 目录混在一起，列表也会跳过。
//! 实现对照 `lanwork_core` 的 `validate_id`，这个 crate 不依赖它。

pub fn valid_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > 200
        || id == "."
        || id == ".."
        || id.starts_with('.')
        || id.starts_with(' ')
        || id.ends_with(' ')
        || id.ends_with('.')
        || reserved_windows_name(id)
        || id.chars().any(|ch| {
            matches!(ch, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*') || ch.is_control()
        })
    {
        return false;
    }
    true
}

fn reserved_windows_name(id: &str) -> bool {
    let stem = id.split('.').next().unwrap_or(id);
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// 保留前缀。工具 id 不能以 `lanwork.` 开头。比较忽略 ASCII 大小写，
/// `Lanwork.tool` 和 `LANWORK.x` 同样拒绝。
/// 拒绝时的说明仍是「id 不能以 lanwork. 开头」。
pub fn imported_prefix_forbidden(id: &str) -> bool {
    id.get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("lanwork."))
}

#[cfg(test)]
mod tests {
    use super::imported_prefix_forbidden;

    #[test]
    fn reserved_prefix_ignores_ascii_case() {
        assert!(imported_prefix_forbidden("lanwork.quota"));
        assert!(imported_prefix_forbidden("Lanwork.tool"));
        assert!(imported_prefix_forbidden("LANWORK.x"));
        assert!(imported_prefix_forbidden("LaNwOrK.Mixed"));
        assert!(!imported_prefix_forbidden("lanwork"));
        assert!(!imported_prefix_forbidden("example.lanwork"));
        assert!(!imported_prefix_forbidden("example.api-quota"));
    }
}
