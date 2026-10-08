//! 搜索条前缀分类，以及应用、待办、便签共用的匹配器。
//!
//! [`classify_prefix`] 只供搜索条调用。匹配器的组内排序等规格缺口 #9 第 7 项。
//! 应用枚举、文件索引和查询调度不在这里。

mod corpus;
mod order;
mod pinyin;
mod prefix;
mod prepare;
mod query;

pub use corpus::benchmark_corpus;
pub use order::{GroupOrder, PendingGroupOrder, sort_hits};
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
