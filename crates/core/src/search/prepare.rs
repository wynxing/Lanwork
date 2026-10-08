//! 候选项的预计算。拼音转换发生在这里，不发生在查询里。

use super::pinyin;

/// 字段在产品里的角色。正文不做拼音、首字母和英文模糊。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRole {
    /// 应用名、待办标题、便签标题。
    Name,
    /// 用户添加的别名，匹配方式与名称相同。
    Alias,
    /// 便签标签。
    Tag,
    /// 便签正文。只做原文匹配。
    Body,
}

impl FieldRole {
    #[must_use]
    pub const fn allows_reading(self) -> bool {
        matches!(self, Self::Name | Self::Alias | Self::Tag)
    }
}

/// 准备一个字段时的输入。文本只在 `prepare` 期间借用。
#[derive(Debug, Clone, Copy)]
pub struct FieldInput<'a> {
    pub role: FieldRole,
    pub text: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) enum Token {
    Literal { start: u32, end: u32 },
    Syllables { start: u32, count: u8 },
    Gap,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedField {
    pub(crate) role: FieldRole,
    pub(crate) field_index: u32,
    pub(crate) normalized: String,
    pub(crate) compact: Option<String>,
    pub(crate) tokens: Vec<Token>,
    pub(crate) syllable_ids: Vec<u16>,
}

/// 一个候选项的全部预计算字段。`id` 由调用方解释，引擎不规定它是应用、待办还是便签。
#[derive(Debug, Clone)]
pub struct PreparedCandidate {
    pub id: u64,
    pub(crate) fields: Vec<PreparedField>,
}

impl PreparedCandidate {
    /// 按字存放的音节 id 数量。不是多音组合展开后的字符串数。
    #[must_use]
    pub fn stored_syllable_ids(&self) -> usize {
        self.fields
            .iter()
            .map(|field| field.syllable_ids.len())
            .sum()
    }

    /// 这个候选项拥有的堆缓冲区容量，不含拼音表，不含分配器开销。
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        owned_heap(self)
    }
}

/// 同一结构的候选项集合。各来源可以共用一个，也可以各自持有再交给同一个查询函数。
#[derive(Debug, Clone, Default)]
pub struct MatchIndex {
    items: Vec<PreparedCandidate>,
}

impl MatchIndex {
    #[must_use]
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn insert(&mut self, id: u64, fields: &[FieldInput<'_>]) {
        self.items.push(prepare(id, fields));
    }

    pub fn push(&mut self, candidate: PreparedCandidate) {
        self.items.push(candidate);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn candidates(&self) -> &[PreparedCandidate] {
        &self.items
    }

    #[must_use]
    pub fn query(&self, query: &str) -> Vec<super::query::Hit> {
        super::query::query_prepared(&self.items, query)
    }

    /// 索引堆缓冲区容量之和，不含拼音表，不含分配器开销。
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.items.capacity() * std::mem::size_of::<PreparedCandidate>()
            + self.items.iter().map(owned_heap).sum::<usize>()
    }
}

#[must_use]
pub fn prepare(id: u64, fields: &[FieldInput<'_>]) -> PreparedCandidate {
    let mut counts = [0u32; 4];
    let mut prepared = Vec::with_capacity(fields.len());
    for field in fields {
        let slot = role_slot(field.role);
        let field_index = counts[slot];
        counts[slot] = counts[slot].saturating_add(1);
        prepared.push(prepare_field(field.role, field_index, field.text));
    }
    PreparedCandidate {
        id,
        fields: prepared,
    }
}

fn role_slot(role: FieldRole) -> usize {
    match role {
        FieldRole::Name => 0,
        FieldRole::Alias => 1,
        FieldRole::Tag => 2,
        FieldRole::Body => 3,
    }
}

fn prepare_field(role: FieldRole, field_index: u32, text: &str) -> PreparedField {
    let normalized = normalize(text);
    let compact_text = remove_whitespace(&normalized);
    let compact = if compact_text == normalized {
        None
    } else {
        Some(compact_text)
    };
    let (tokens, syllable_ids) = if role.allows_reading() {
        tokenize(&normalized)
    } else {
        (Vec::new(), Vec::new())
    };
    PreparedField {
        role,
        field_index,
        normalized,
        compact,
        tokens,
        syllable_ids,
    }
}

pub(crate) fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let folded = fold_width(ch);
        for lower in folded.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

fn fold_width(ch: char) -> char {
    match ch {
        '\u{3000}' => ' ',
        ch if (ch as u32) >= 0xFF01 && (ch as u32) <= 0xFF5E => {
            char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch)
        }
        ch => ch,
    }
}

fn remove_whitespace(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn tokenize(normalized: &str) -> (Vec<Token>, Vec<u16>) {
    let mut tokens = Vec::new();
    let mut syllable_ids = Vec::new();
    let mut chars = normalized.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch.is_ascii_alphanumeric() {
            let start = index;
            let mut end = index + ch.len_utf8();
            while let Some(&(next_index, next)) = chars.peek() {
                if next.is_ascii_alphanumeric() {
                    chars.next();
                    end = next_index + next.len_utf8();
                } else {
                    break;
                }
            }
            let Some(start) = u32::try_from(start).ok() else {
                break;
            };
            let Some(end) = u32::try_from(end).ok() else {
                break;
            };
            tokens.push(Token::Literal { start, end });
            continue;
        }
        let before = syllable_ids.len();
        if pinyin::push_reading_ids(ch, &mut syllable_ids) {
            let Some(start) = u32::try_from(before).ok() else {
                break;
            };
            let count = syllable_ids.len() - before;
            let Ok(count) = u8::try_from(count) else {
                break;
            };
            tokens.push(Token::Syllables { start, count });
            continue;
        }
        if is_han(ch) || ch.is_alphabetic() {
            tokens.push(Token::Gap);
        }
    }
    (tokens, syllable_ids)
}

fn is_han(ch: char) -> bool {
    let cp = ch as u32;
    matches!(cp, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFD)
}

fn owned_heap(candidate: &PreparedCandidate) -> usize {
    candidate.fields.capacity() * std::mem::size_of::<PreparedField>()
        + candidate
            .fields
            .iter()
            .map(|field| {
                field.normalized.capacity()
                    + field.compact.as_ref().map_or(0, String::capacity)
                    + field.tokens.capacity() * std::mem::size_of::<Token>()
                    + field.syllable_ids.capacity() * std::mem::size_of::<u16>()
            })
            .sum::<usize>()
}
