//! 组内排序的接入点。
//!
//! 规格缺口 #9 第 7 项写入 product.md 之前，这里不规定完全匹配、前缀、拼音、
//! 首字母、模糊的先后，也不规定 20 条在各组之间怎么分配。

use std::cmp::Ordering;

use super::query::Hit;

/// 调用方用来比较两条命中。实现必须只看命中本身，不能在这里读盘或做拼音转换。
pub trait GroupOrder {
    fn cmp(&self, a: &Hit, b: &Hit) -> Ordering;
}

/// 规格未定前的占位。比较结果恒为相等，稳定排序后仍是查询的输出顺序。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingGroupOrder;

impl GroupOrder for PendingGroupOrder {
    fn cmp(&self, _a: &Hit, _b: &Hit) -> Ordering {
        Ordering::Equal
    }
}

/// 按 `order` 重排。`PendingGroupOrder` 不改变相对顺序。
pub fn sort_hits(hits: &mut [Hit], order: &impl GroupOrder) {
    hits.sort_by(|left, right| order.cmp(left, right));
}
