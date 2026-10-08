//! 搜索条输入的收集前缀分类。

/// 搜索条整段输入的分类。
///
/// 只供搜索条调用。面板搜索框不识别收集前缀，不得调用 [`classify_prefix`]；
/// 面板里的输入一律按搜索处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixClass<'a> {
    /// 空字符串。
    Empty,
    /// 按搜索处理。查询文本就是传入的原文，本函数不裁剪。
    Search,
    /// 以 `+` 或全角 `＋` 开头的待办收集。
    TodoCapture(Capture<'a>),
    /// 以 `/note` 开头，且后面是空白或结束的便签收集。
    NoteCapture(Capture<'a>),
}

/// 收集前缀之后的剩余文本。
///
/// 前缀后的前导空白已经去掉，尾部空白保留。剩余文本借用输入，不复制。
/// 待办收集的剩余文本原样保留日期词；便签收集的剩余文本就是标题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capture<'a> {
    remainder: &'a str,
}

impl<'a> Capture<'a> {
    /// 前缀之后的剩余文本。
    pub const fn remainder(&self) -> &'a str {
        self.remainder
    }

    /// 剩余文本含有非空白字符时可以提交。
    ///
    /// 为假时仍是收集模式，预览照常显示，但不应创建记录。
    pub fn submittable(&self) -> bool {
        self.remainder.chars().any(|c| !c.is_whitespace())
    }
}

impl PrefixClass<'_> {
    /// 是否命中收集前缀。
    ///
    /// 不可提交时也返回真。
    pub const fn is_capture(&self) -> bool {
        matches!(self, Self::TodoCapture(_) | Self::NoteCapture(_))
    }
}

/// 判断搜索条的整段输入是空输入、搜索、待办收集还是便签收集。
///
/// 不创建记录，不发起查询，也不解析日期。
///
/// - 空字符串是空输入。只有空白、但不是空字符串的输入是搜索。
/// - `+`（U+002B）或全角 `＋`（U+FF0B）在开头时是待办收集。前缀后的前导空白不进入剩余文本，后面没有文字也可以。
/// - `/note` 在开头，且后面是空白或结束时是便签收集。`/notes` 这类紧挨着的词是搜索。匹配区分大小写，不做兼容分解。
/// - 前缀必须在输入开头。开头有空白、前缀出现在中间，都是搜索。
///
/// 空白使用 Unicode `White_Space`（[`char::is_whitespace`]），包含空格、制表符、换行和全角空格。
///
/// # 例
///
/// ```
/// use lanwork_core::search::{classify_prefix, PrefixClass};
///
/// match classify_prefix("+ 提交周报") {
///     PrefixClass::TodoCapture(capture) => {
///         assert_eq!(capture.remainder(), "提交周报");
///         assert!(capture.submittable());
///     }
///     _ => panic!("expected todo capture"),
/// }
///
/// match classify_prefix("/note") {
///     PrefixClass::NoteCapture(capture) => {
///         assert_eq!(capture.remainder(), "");
///         assert!(!capture.submittable());
///         assert!(classify_prefix("/note").is_capture());
///     }
///     _ => panic!("expected note capture"),
/// }
///
/// assert!(matches!(classify_prefix(""), PrefixClass::Empty));
/// assert!(matches!(classify_prefix("微信"), PrefixClass::Search));
/// assert!(matches!(classify_prefix("/notes"), PrefixClass::Search));
/// ```
#[must_use]
pub fn classify_prefix(input: &str) -> PrefixClass<'_> {
    if input.is_empty() {
        return PrefixClass::Empty;
    }
    if let Some(rest) = strip_todo_prefix(input) {
        return PrefixClass::TodoCapture(capture(rest));
    }
    if let Some(rest) = strip_note_prefix(input) {
        return PrefixClass::NoteCapture(capture(rest));
    }
    PrefixClass::Search
}

fn capture(rest: &str) -> Capture<'_> {
    Capture {
        remainder: rest.trim_start(),
    }
}

fn strip_todo_prefix(input: &str) -> Option<&str> {
    input.strip_prefix('+').or_else(|| input.strip_prefix('＋'))
}

fn strip_note_prefix(input: &str) -> Option<&str> {
    let rest = input.strip_prefix("/note")?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Capture, PrefixClass, classify_prefix};

    fn todo(remainder: &str) -> PrefixClass<'_> {
        PrefixClass::TodoCapture(Capture { remainder })
    }

    fn note(remainder: &str) -> PrefixClass<'_> {
        PrefixClass::NoteCapture(Capture { remainder })
    }

    fn assert_capture(input: &str, expected: PrefixClass<'_>, submittable: bool) {
        let got = classify_prefix(input);
        assert_eq!(got, expected, "input {input:?}");
        assert!(
            got.is_capture(),
            "input {input:?} should stay in capture mode"
        );
        let capture = match got {
            PrefixClass::TodoCapture(capture) | PrefixClass::NoteCapture(capture) => capture,
            PrefixClass::Empty | PrefixClass::Search => unreachable!(),
        };
        assert_eq!(
            capture.submittable(),
            submittable,
            "input {input:?} submittable"
        );
        assert_eq!(capture.remainder(), expected_remainder(&expected));
    }

    fn expected_remainder<'a>(class: &PrefixClass<'a>) -> &'a str {
        match class {
            PrefixClass::TodoCapture(capture) | PrefixClass::NoteCapture(capture) => {
                capture.remainder()
            }
            PrefixClass::Empty | PrefixClass::Search => unreachable!(),
        }
    }

    fn assert_search(input: &str) {
        let got = classify_prefix(input);
        assert_eq!(got, PrefixClass::Search, "input {input:?}");
        assert!(!got.is_capture(), "input {input:?}");
    }

    #[test]
    fn normal_prefixes() {
        // `+ 提交周报` 是待办收集，剩余「提交周报」。
        assert_capture("+ 提交周报", todo("提交周报"), true);
        // `/note 会议纪要` 是便签收集，剩余「会议纪要」。
        assert_capture("/note 会议纪要", note("会议纪要"), true);
        // `微信` 是搜索。
        assert_search("微信");
    }

    #[test]
    fn boundary_prefixes() {
        // `＋提交` 是待办收集。全角加号后没有空白也可以。
        assert_capture("＋提交", todo("提交"), true);
        assert_capture("＋ 提交周报", todo("提交周报"), true);
        assert_capture("＋", todo(""), false);

        // `/note` 单独是便签收集，且不可提交。
        assert_capture("/note", note(""), false);
        assert_capture("/note ", note(""), false);
        assert_capture("/note   ", note(""), false);
        assert_capture("/note\t", note(""), false);

        // `+` 后只有空白时不可提交，但仍是待办收集。
        assert_capture("+", todo(""), false);
        assert_capture("+ ", todo(""), false);
        assert_capture("+   ", todo(""), false);
        assert_capture("+\t", todo(""), false);
        assert_capture("+\n", todo(""), false);
        assert_capture("+\r\n", todo(""), false);
        assert_capture("+ \t\n", todo(""), false);
        assert_capture("+　", todo(""), false);

        // 前缀后的空白忽略，标题内部和末尾的空白保留。
        assert_capture("+提交周报", todo("提交周报"), true);
        assert_capture("+  提交周报", todo("提交周报"), true);
        assert_capture("+\t提交周报", todo("提交周报"), true);
        assert_capture("+\n提交周报", todo("提交周报"), true);
        assert_capture("+　提交周报", todo("提交周报"), true);
        assert_capture("+ \n\t 提交周报", todo("提交周报"), true);
        assert_capture("+ 提交 周报", todo("提交 周报"), true);
        assert_capture("+ 提交周报 ", todo("提交周报 "), true);
        assert_capture("/note  会议纪要", note("会议纪要"), true);
        assert_capture("/note\t会议纪要", note("会议纪要"), true);
        assert_capture("/note\r\n会议纪要", note("会议纪要"), true);
        assert_capture("/note　会议纪要", note("会议纪要"), true);

        // `/notes` 这类紧挨着的词、开头不是前缀、前缀不在开头，都是搜索。
        assert_search("/notes");
        assert_search("/notes ");
        assert_search("/notes x");
        assert_search("/notebook");
        assert_search("/note+会议");
        assert_search("/note/x");
        assert_search("/Note 会议");
        assert_search("/NOTE");
        assert_search("／note 会议");
        assert_search(" +x");
        assert_search("a+b");
        assert_search("x/note 会议");
        assert_search("\n+提交");
        assert_search("　+提交");
        assert_search("　＋提交");
        assert_search("/ note");
        assert_search("note");
        assert_search("﹢提交");
        assert_search("⁺提交");

        // 只有第一个字符是待办前缀。后续的 `+` 或 `/note` 留在剩余文本里。
        assert_capture("++提交", todo("+提交"), true);
        assert_capture("+/note 会议", todo("/note 会议"), true);

        // 不解析日期。日期词留在剩余文本里。
        assert_capture("+ 明天 提交周报", todo("明天 提交周报"), true);
        assert_capture("+ 明天性计划", todo("明天性计划"), true);
        assert_capture("+明天", todo("明天"), true);

        // 空字符串是空输入。只有空白的输入不是空输入，按搜索。
        assert_eq!(classify_prefix(""), PrefixClass::Empty);
        assert!(!classify_prefix("").is_capture());
        assert_search(" ");
        assert_search("   ");
        assert_search("\n");
        assert_search("\t");
        assert_search("　");
    }

    #[test]
    fn long_input_and_newlines_do_not_panic() {
        let search = "a".repeat(10_000);
        assert_eq!(search.chars().count(), 10_000);
        assert_search(&search);

        let todo_body = "报".repeat(9_999);
        let todo_input = format!("+{todo_body}");
        assert_eq!(todo_input.chars().count(), 10_000);
        assert_capture(&todo_input, todo(&todo_body), true);

        let note_len = "/note ".chars().count();
        let note_body = "纪".repeat(10_000 - note_len);
        let note_input = format!("/note {note_body}");
        assert_eq!(note_input.chars().count(), 10_000);
        assert_capture(&note_input, note(&note_body), true);

        let blank = format!("+{}", " ".repeat(9_999));
        assert_eq!(blank.chars().count(), 10_000);
        assert_capture(&blank, todo(""), false);

        assert_capture("+提交\n周报", todo("提交\n周报"), true);
        assert_capture("/note\n会议纪要", note("会议纪要"), true);
        let mixed = format!("微信\n+待办{}", "\n".repeat(10_000));
        assert_search(&mixed);

        let plus_line = format!("+{}\n{}", "报".repeat(5_000), "x".repeat(5_000));
        match classify_prefix(&plus_line) {
            PrefixClass::TodoCapture(capture) => {
                assert!(capture.submittable());
                assert!(capture.remainder().contains('\n'));
                assert!(!capture.remainder().starts_with(char::is_whitespace));
            }
            other => panic!("expected todo capture, got {other:?}"),
        }
    }
}
