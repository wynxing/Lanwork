//! 悬浮窗内容最上方的来源行。不调用窗口 API。
//!
//! 格式是「类型 · 名称」。间隔号是 U+00B7，两边各一个空格。
//! 过长由界面省略，这里不截断。

/// 来源行里的间隔号，两边带空格。
pub const SOURCE_GAP: &str = " \u{00B7} ";

/// 收纳悬浮窗的来源。`name` 是分组名；分组已经不在，或名称没有可见字符时，只有「收纳」。
#[must_use]
pub fn shelf_source(name: Option<&str>) -> String {
    match name.and_then(first_visible_line) {
        Some(name) => format!("收纳{SOURCE_GAP}{name}"),
        None => "收纳".to_owned(),
    }
}

/// 便签悬浮窗的来源。名称按顺序取：标题里第一个有字的行，否则正文里第一个有字的行，否则「无标题」。
#[must_use]
pub fn note_source(title: &str, body: &str) -> String {
    let name = first_visible_line(title)
        .or_else(|| first_visible_line(body))
        .unwrap_or("无标题");
    format!("便签{SOURCE_GAP}{name}")
}

/// 待办悬浮窗列出全部清单，没有单独的名称。来源行就是「待办」。
#[must_use]
pub fn todo_source() -> &'static str {
    "待办"
}

/// 第一个去掉首尾空白后还有字的行。不改这一行中间的空格。
fn first_visible_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_source_uses_the_group_name_and_updates_when_it_changes() {
        assert_eq!(shelf_source(Some("单片机")), "收纳 \u{00B7} 单片机");
        assert_eq!(shelf_source(Some("电路")), "收纳 \u{00B7} 电路");
        assert_eq!(shelf_source(Some("  收纳  ")), "收纳 \u{00B7} 收纳");
        assert_eq!(shelf_source(Some("第一行\n第二行")), "收纳 \u{00B7} 第一行");
        assert_eq!(shelf_source(None), "收纳");
        assert_eq!(shelf_source(Some("   ")), "收纳");
        assert_eq!(shelf_source(Some("\n")), "收纳");
    }

    #[test]
    fn note_source_prefers_title_then_first_body_line() {
        assert_eq!(note_source("会议", "正文第一行"), "便签 \u{00B7} 会议");
        assert_eq!(
            note_source("  标题  \n第二行", "正文"),
            "便签 \u{00B7} 标题"
        );
        assert_eq!(note_source("", "第一行\n第二行"), "便签 \u{00B7} 第一行");
        assert_eq!(
            note_source("   ", "\n\n  草稿  \n后文"),
            "便签 \u{00B7} 草稿"
        );
        assert_eq!(note_source("", ""), "便签 \u{00B7} 无标题");
        assert_eq!(note_source(" \n ", " \n "), "便签 \u{00B7} 无标题");
        assert!(note_source("会议", "很长").contains('\u{00B7}'));
        assert!(!note_source("会议", "正文").contains('•'));
    }

    #[test]
    fn todo_source_has_no_item_name() {
        assert_eq!(todo_source(), "待办");
        assert!(!todo_source().contains('\u{00B7}'));
    }
}
