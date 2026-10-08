//! 已经写进产品规格的信号：长期未更新、Draft。
//!
//! 需要处理、需要 Review、CI 失败不在这里计算。
//! [`SignalExtension`] 留给 #9 第 18 项；服务不读取它的返回值，也不根据它隐藏条目。

use super::model::{DAY_MS, SnapshotItem};

/// 天数是 0、没有更新时间、或更新时间晚于 `now_ms` 时不标。
///
/// 其余情况：`now_ms - updated_at_ms >= stale_days * 86_400_000` 时为真。
#[must_use]
pub fn is_stale(updated_at_ms: Option<i64>, now_ms: i64, stale_days: u32) -> bool {
    if stale_days == 0 {
        return false;
    }
    let Some(updated_at_ms) = updated_at_ms else {
        return false;
    };
    let window = i64::from(stale_days).saturating_mul(DAY_MS);
    now_ms.saturating_sub(updated_at_ms) >= window
}

/// 第 18 项确认前的扩展点。实现可以观察条目，不能通过这个接口改列表。
pub trait SignalExtension: Send + Sync {
    fn observe(&self, item: &SnapshotItem);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_days_and_missing_time_are_not_stale() {
        assert!(!is_stale(Some(0), 30 * DAY_MS, 0));
        assert!(!is_stale(None, 30 * DAY_MS, 14));
        assert!(!is_stale(Some(100), 50, 14));
    }

    #[test]
    fn boundary_is_inclusive() {
        let updated = 1_000_000;
        let now = updated + 14 * DAY_MS;
        assert!(is_stale(Some(updated), now, 14));
        assert!(!is_stale(Some(updated), now - 1, 14));
    }
}
