//! 搜索条前缀分类，以及应用、待办、便签共用的匹配器。
//!
//! [`classify_prefix`] 只供搜索条调用。组内排序和 20 条名额见 [`rank_hits`]、[`allocate_display`]。
//! 应用枚举、文件索引和查询调度不在这里。使用频率的增减尚未规定。

mod corpus;
mod order;
mod pinyin;
mod prefix;
mod prepare;
mod query;
mod web;

pub use corpus::benchmark_corpus;
pub use order::{
    DISPLAY_LIMIT, FrequencySource, GROUP_FIRST_TAKE, GroupOrder, MatchKindOrder, RankedGroup,
    SearchGroup, ZeroFrequency, allocate_display, hit_kind_rank, rank_hits, sort_hits,
};
pub use pinyin::{
    CHARACTER_COUNT, COVERAGE, READINGS_SHA256, SOURCE_URL, SYLLABLE_COUNT, TABLE_RESIDENT_BYTES,
    TableInfo, UNICODE_VERSION, readings, resident_bytes, table_info,
};
#[doc(inline)]
pub use prefix::{Capture, PrefixClass, classify_prefix};
pub use prepare::{FieldInput, FieldRole, MatchIndex, PreparedCandidate, prepare};
pub use query::{
    FUZZY_BASE, FUZZY_CONSECUTIVE_BONUS, FUZZY_WORD_START_BONUS, Hit, HitKind, query_prepared,
};
pub use web::WebSearchEngine;
