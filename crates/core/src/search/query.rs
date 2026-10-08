//! 对预计算字段做比较。这里不把汉字转成拼音。

use super::pinyin;
use super::prepare::{FieldRole, PreparedCandidate, PreparedField, Token};

/// 英文模糊：每个命中字符的底分。只在 [`HitKind::Fuzzy`] 内有意义。
pub const FUZZY_BASE: u32 = 1;
/// 与上一个命中在原文里相邻时的加分。
pub const FUZZY_CONSECUTIVE_BONUS: u32 = 2;
/// 命中词首时的加分。
pub const FUZZY_WORD_START_BONUS: u32 = 2;

/// 命中方式。同一字段可以同时有多种。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitKind {
    /// 归一化原文相等，或去掉空白后相等。
    Exact,
    /// 查询是归一化原文或其去空白形式的前缀。
    Prefix,
    /// 查询是归一化原文或其去空白形式的子串。
    Substring,
    /// 按预存音节对齐，且至少用到一个汉字。
    Pinyin,
    /// 连续词段的首字母。
    Initial,
    /// 英文子序列。
    Fuzzy,
}

/// 一条命中。`score` 不能跨 [`HitKind`] 比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub id: u64,
    pub role: FieldRole,
    /// 同一候选项、同一 [`FieldRole`] 内的序号，从 0 起。
    pub field_index: u32,
    pub kind: HitKind,
    pub score: u32,
}

struct Plan {
    normalized: String,
    compact: String,
    ascii: Option<Vec<u8>>,
}

#[derive(Default)]
struct Scratch {
    current: Vec<u8>,
    next: Vec<u8>,
    end_prev: Vec<i32>,
    best_prev: Vec<i32>,
    end_cur: Vec<i32>,
    best_cur: Vec<i32>,
}

const UNSET: i32 = i32::MIN / 4;
const MISS: u8 = 0;
const PLAIN: u8 = 1;
const HAN: u8 = 2;

/// 查询一组已经预计算的候选项。
///
/// 空查询返回空列表。输出顺序是候选项和字段的输入顺序，然后是原文、全拼、
/// 首字母、模糊。这不是组内排序；组内排序用 [`super::rank_hits`]。
#[must_use]
pub fn query_prepared(candidates: &[PreparedCandidate], query: &str) -> Vec<Hit> {
    let Some(plan) = Plan::new(query) else {
        return Vec::new();
    };
    let mut scratch = Scratch::default();
    let mut hits = Vec::new();
    for candidate in candidates {
        for field in &candidate.fields {
            push_field_hits(candidate.id, field, &plan, &mut scratch, &mut hits);
        }
    }
    hits
}

impl Plan {
    fn new(query: &str) -> Option<Self> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return None;
        }
        let normalized = super::prepare::normalize(trimmed);
        if normalized.is_empty() {
            return None;
        }
        let compact: String = normalized
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        if compact.is_empty() {
            return None;
        }
        let ascii = if compact.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            Some(compact.as_bytes().to_vec())
        } else {
            None
        };
        Some(Self {
            normalized,
            compact,
            ascii,
        })
    }
}

fn push_field_hits(
    id: u64,
    field: &PreparedField,
    plan: &Plan,
    scratch: &mut Scratch,
    hits: &mut Vec<Hit>,
) {
    let score = u32::try_from(plan.normalized.chars().count()).unwrap_or(u32::MAX);
    if let Some(kind) = original_kind(field, plan) {
        hits.push(Hit {
            id,
            role: field.role,
            field_index: field.field_index,
            kind,
            score,
        });
    }
    let Some(ascii) = plan.ascii.as_deref() else {
        return;
    };
    if !field.role.allows_reading() {
        return;
    }
    if pinyin_match(field, ascii, scratch) {
        hits.push(Hit {
            id,
            role: field.role,
            field_index: field.field_index,
            kind: HitKind::Pinyin,
            score,
        });
    }
    if initial_match(field, ascii) {
        hits.push(Hit {
            id,
            role: field.role,
            field_index: field.field_index,
            kind: HitKind::Initial,
            score,
        });
    }
    if let Some(fuzzy) = fuzzy_score(field.normalized.as_bytes(), ascii, scratch) {
        hits.push(Hit {
            id,
            role: field.role,
            field_index: field.field_index,
            kind: HitKind::Fuzzy,
            score: fuzzy,
        });
    }
}

fn original_kind(field: &PreparedField, plan: &Plan) -> Option<HitKind> {
    let compact = field.compact.as_deref().unwrap_or(&field.normalized);
    if field.normalized == plan.normalized || compact == plan.compact {
        return Some(HitKind::Exact);
    }
    if field.normalized.starts_with(&plan.normalized) || compact.starts_with(&plan.compact) {
        return Some(HitKind::Prefix);
    }
    if field.normalized.contains(&plan.normalized) || compact.contains(&plan.compact) {
        return Some(HitKind::Substring);
    }
    None
}

fn pinyin_match(field: &PreparedField, query: &[u8], scratch: &mut Scratch) -> bool {
    if query.is_empty() || field.tokens.is_empty() {
        return false;
    }
    let char_count = field.normalized.chars().count();
    if query.len()
        > char_count
            .saturating_mul(6)
            .saturating_add(field.normalized.len())
    {
        return false;
    }
    let end = query.len();
    scratch.ensure_flags(end + 1);
    for start in 0..field.tokens.len() {
        if matches!(field.tokens[start], Token::Gap) {
            continue;
        }
        scratch.current[..end + 1].fill(MISS);
        scratch.current[0] = PLAIN;
        for token in &field.tokens[start..] {
            if scratch.current[end] == HAN {
                return true;
            }
            if matches!(token, Token::Gap) {
                break;
            }
            advance_token(field, token, query, scratch);
            if scratch.next[..end + 1].iter().all(|state| *state == MISS) {
                break;
            }
            std::mem::swap(&mut scratch.current, &mut scratch.next);
        }
        if scratch.current[end] == HAN {
            return true;
        }
    }
    false
}

fn advance_token(field: &PreparedField, token: &Token, query: &[u8], scratch: &mut Scratch) {
    let end = query.len();
    scratch.next[..end + 1].fill(MISS);
    for pos in 0..=end {
        let state = scratch.current[pos];
        if state == MISS {
            continue;
        }
        if pos == end {
            if state > scratch.next[end] {
                scratch.next[end] = state;
            }
            continue;
        }
        let rest = &query[pos..];
        match token {
            Token::Literal {
                start,
                end: lit_end,
            } => {
                let literal = &field.normalized.as_bytes()[*start as usize..*lit_end as usize];
                mark_consume(&mut scratch.next, pos, rest, literal, state);
            }
            Token::Syllables { start, count } => {
                let from = *start as usize;
                let ids = &field.syllable_ids[from..from + usize::from(*count)];
                for id in ids {
                    mark_consume(
                        &mut scratch.next,
                        pos,
                        rest,
                        pinyin::syllable(*id).as_bytes(),
                        HAN,
                    );
                }
            }
            Token::Gap => {}
        }
    }
}

fn mark_consume(next: &mut [u8], pos: usize, rest: &[u8], unit: &[u8], state: u8) {
    let consumed = if rest.starts_with(unit) {
        unit.len()
    } else if unit.starts_with(rest) {
        rest.len()
    } else {
        return;
    };
    let dest = pos + consumed;
    if state > next[dest] {
        next[dest] = state;
    }
}

fn initial_match(field: &PreparedField, query: &[u8]) -> bool {
    let tokens = &field.tokens;
    if query.is_empty() || query.len() > tokens.len() {
        return false;
    }
    let last = tokens.len() - query.len();
    'start: for start in 0..=last {
        for (offset, byte) in query.iter().enumerate() {
            if !token_initial(field, &tokens[start + offset], *byte) {
                continue 'start;
            }
        }
        return true;
    }
    false
}

fn token_initial(field: &PreparedField, token: &Token, byte: u8) -> bool {
    match token {
        Token::Gap => false,
        Token::Literal { start, .. } => {
            field.normalized.as_bytes().get(*start as usize) == Some(&byte)
        }
        Token::Syllables { start, count } => {
            let from = *start as usize;
            field.syllable_ids[from..from + usize::from(*count)]
                .iter()
                .any(|id| pinyin::syllable(*id).as_bytes().first() == Some(&byte))
        }
    }
}

fn fuzzy_score(target: &[u8], query: &[u8], scratch: &mut Scratch) -> Option<u32> {
    let target_len = target.len();
    let query_len = query.len();
    if query_len == 0 || target_len == 0 || query_len > target_len || !is_subsequence(target, query)
    {
        return None;
    }
    scratch.ensure_rows(target_len + 1);
    for index in 0..=target_len {
        scratch.best_prev[index] = 0;
        scratch.end_prev[index] = UNSET;
    }
    for &wanted in query {
        for index in 0..=target_len {
            scratch.end_cur[index] = UNSET;
            scratch.best_cur[index] = UNSET;
        }
        for index in 1..=target_len {
            if target[index - 1] != wanted {
                scratch.best_cur[index] = scratch.best_cur[index - 1].max(scratch.end_cur[index]);
                continue;
            }
            let word_start = index == 1 || !target[index - 2].is_ascii_alphanumeric();
            let base = FUZZY_BASE as i32
                + if word_start {
                    FUZZY_WORD_START_BONUS as i32
                } else {
                    0
                };
            let mut best = UNSET;
            if scratch.end_prev[index - 1] != UNSET {
                best = scratch.end_prev[index - 1] + base + FUZZY_CONSECUTIVE_BONUS as i32;
            }
            if scratch.best_prev[index - 1] != UNSET {
                best = best.max(scratch.best_prev[index - 1] + base);
            }
            scratch.end_cur[index] = best;
            scratch.best_cur[index] = scratch.best_cur[index - 1].max(best);
        }
        std::mem::swap(&mut scratch.end_prev, &mut scratch.end_cur);
        std::mem::swap(&mut scratch.best_prev, &mut scratch.best_cur);
    }
    let score = scratch.best_prev[target_len];
    if score < 0 { None } else { Some(score as u32) }
}

fn is_subsequence(target: &[u8], query: &[u8]) -> bool {
    let mut index = 0;
    for byte in target {
        if index < query.len() && *byte == query[index] {
            index += 1;
        }
    }
    index == query.len()
}

impl Scratch {
    fn ensure_flags(&mut self, len: usize) {
        if self.current.len() < len {
            self.current.resize(len, MISS);
            self.next.resize(len, MISS);
        }
    }

    fn ensure_rows(&mut self, len: usize) {
        if self.end_prev.len() < len {
            self.end_prev.resize(len, UNSET);
            self.best_prev.resize(len, UNSET);
            self.end_cur.resize(len, UNSET);
            self.best_cur.resize(len, UNSET);
        }
    }
}
