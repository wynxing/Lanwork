//! 搜索结果的组内排序和 20 条名额。
//!
//! 可见顺序见产品规格「搜索」。[`super::query_prepared`] 仍返回全部命中，不在这里截断。
//! 使用频率由调用方传入。次数如何增减尚未规定，这里不保存。

use std::cmp::Ordering;
use std::collections::HashMap;

use super::query::{Hit, HitKind};

/// 界面最多显示的搜索结果条数。
pub const DISPLAY_LIMIT: usize = 20;
/// 补位之前，每一组先取的条数上限。
pub const GROUP_FIRST_TAKE: usize = 8;

/// 调用方用来比较两条命中。实现必须只看命中本身和自己持有的数据，不能在这里读盘或做拼音转换。
pub trait GroupOrder {
    fn cmp(&self, a: &Hit, b: &Hit) -> Ordering;
}

/// 使用次数。只在同一种命中类型内比较，数值大的在前。
///
/// 匹配引擎不保存次数，也不规定它如何增减。持有候选项的服务在规则写进产品规格之前不落盘。
pub trait FrequencySource {
    fn frequency(&self, id: u64) -> u64;
}

impl<F> FrequencySource for F
where
    F: Fn(u64) -> u64,
{
    fn frequency(&self, id: u64) -> u64 {
        self(id)
    }
}

/// 全部候选项的使用次数都是 0。同一种命中类型内不再改相对顺序。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZeroFrequency;

impl FrequencySource for ZeroFrequency {
    fn frequency(&self, _id: u64) -> u64 {
        0
    }
}

/// 产品规格里的组内比较：先命中类型，再使用频率。
pub struct MatchKindOrder<F> {
    pub frequency: F,
}

impl<F: FrequencySource> GroupOrder for MatchKindOrder<F> {
    fn cmp(&self, left: &Hit, right: &Hit) -> Ordering {
        hit_kind_rank(left.kind)
            .cmp(&hit_kind_rank(right.kind))
            .then_with(|| {
                self.frequency
                    .frequency(right.id)
                    .cmp(&self.frequency.frequency(left.id))
            })
    }
}

/// 完全匹配、前缀、全拼、首字母、子串、模糊。数值越小越靠前。
#[must_use]
pub const fn hit_kind_rank(kind: HitKind) -> u8 {
    match kind {
        HitKind::Exact => 0,
        HitKind::Prefix => 1,
        HitKind::Pinyin => 2,
        HitKind::Initial => 3,
        HitKind::Substring => 4,
        HitKind::Fuzzy => 5,
    }
}

/// 按 `order` 稳定重排。
pub fn sort_hits(hits: &mut [Hit], order: &impl GroupOrder) {
    hits.sort_by(|left, right| order.cmp(left, right));
}

/// 一组内的显示顺序。
///
/// 每个候选项 id 只留一条，命中类型取最前的一种；同类型保留输入里先出现的那条。
/// 然后按命中类型排序。同类型内频率高的在前。类型和频率都相同，则保持第一次出现的相对顺序。
/// 分数不参与排序。
#[must_use]
pub fn rank_hits(hits: &[Hit], frequency: impl FrequencySource) -> Vec<Hit> {
    let mut selected = dedupe_best_kind(hits);
    sort_hits(&mut selected, &MatchKindOrder { frequency });
    selected
}

fn dedupe_best_kind(hits: &[Hit]) -> Vec<Hit> {
    let mut position: HashMap<u64, usize> = HashMap::with_capacity(hits.len());
    let mut selected: Vec<Hit> = Vec::new();
    for hit in hits {
        if let Some(&index) = position.get(&hit.id) {
            if hit_kind_rank(hit.kind) < hit_kind_rank(selected[index].kind) {
                selected[index] = *hit;
            }
        } else {
            position.insert(hit.id, selected.len());
            selected.push(*hit);
        }
    }
    selected
}

/// 已经排好序的一组结果。分组顺序见产品规格「搜索」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedGroup<T> {
    pub group: SearchGroup,
    pub items: Vec<T>,
}

/// 搜索结果分组。声明顺序就是显示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SearchGroup {
    /// 有效 `http` / `https` 时才有。调用方不放入时，这一组不占名额。
    Browser,
    /// 应用。
    Application,
    /// 未完成待办。
    Todo,
    /// 不在回收站中的便签。
    Note,
    /// 文件。
    File,
    /// 文件夹。
    Folder,
}

const SEARCH_GROUPS: [SearchGroup; 6] = [
    SearchGroup::Browser,
    SearchGroup::Application,
    SearchGroup::Todo,
    SearchGroup::Note,
    SearchGroup::File,
    SearchGroup::Folder,
];

impl SearchGroup {
    const fn index(self) -> usize {
        match self {
            Self::Browser => 0,
            Self::Application => 1,
            Self::Todo => 2,
            Self::Note => 3,
            Self::File => 4,
            Self::Folder => 5,
        }
    }
}

/// 把已经排好的各组收成最多 [`DISPLAY_LIMIT`] 条。
///
/// 每组先取至多 [`GROUP_FIRST_TAKE`] 条。名额还有剩余时，按组顺序把前面组剩下的结果取完，再取后面的组。
/// 同一组入选的条目挨在一起，并保持传入时的先后。组内顺序由调用方用 [`rank_hits`] 排好。
/// 同一个 [`SearchGroup`] 出现多次时，条目按传入顺序接在后面。
#[must_use]
pub fn allocate_display<T: Clone>(groups: &[RankedGroup<T>]) -> Vec<(SearchGroup, T)> {
    let mut buckets: [Vec<T>; 6] = std::array::from_fn(|_| Vec::new());
    for grouped in groups {
        buckets[grouped.group.index()].extend(grouped.items.iter().cloned());
    }

    let mut take = [0usize; 6];
    let mut total = 0usize;
    for index in 0..SEARCH_GROUPS.len() {
        if total >= DISPLAY_LIMIT {
            break;
        }
        let next = buckets[index]
            .len()
            .min(GROUP_FIRST_TAKE)
            .min(DISPLAY_LIMIT - total);
        take[index] = next;
        total += next;
    }
    for index in 0..SEARCH_GROUPS.len() {
        if total >= DISPLAY_LIMIT {
            break;
        }
        let extra = buckets[index]
            .len()
            .saturating_sub(take[index])
            .min(DISPLAY_LIMIT - total);
        take[index] += extra;
        total += extra;
    }

    let mut out = Vec::with_capacity(total);
    for (index, bucket) in buckets.into_iter().enumerate() {
        let group = SEARCH_GROUPS[index];
        let count = take[index];
        for item in bucket.into_iter().take(count) {
            out.push((group, item));
        }
    }
    out
}
