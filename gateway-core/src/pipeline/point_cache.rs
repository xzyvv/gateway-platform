//! 点位实时值缓存（虚拟点位求值的输入源）
//!
//! 虚拟点位公式引用的是**其它点位的最新值**，因此需要一份「点位 → 最近值」的
//! 进程内缓存。缓存由处理管道在每批数据落地后写入，由虚拟点位引擎读取。
//!
//! # 过期策略
//!
//! 每条缓存记录带 TTL（默认取点位采集周期的 3 倍，下限 30 秒）。采集停止或
//! 设备离线时，缓存值会自然过期，引用它的虚拟点位随之报「引用点位不可用」，
//! 而不会一直用陈旧值算出看似正常的结果。
//!
//! # 线程安全
//!
//! 写入方是处理管道（单任务），读取方是虚拟点位引擎与 Web API；用
//! `parking_lot::RwLock` 保护，读取路径无系统调用。

use std::collections::HashMap;

use parking_lot::RwLock;

/// 缓存的最小 TTL（毫秒）：即使点位周期很短，也不允许缓存瞬间过期。
const MIN_TTL_MS: i64 = 30_000;

/// 一条缓存记录
#[derive(Debug, Clone, PartialEq)]
pub struct CachedValue {
    /// 点位最近一次的工程量值
    pub value: f64,
    /// 写入时刻（UTC 毫秒）
    pub ts_ms: i64,
    /// 该记录的存活时长（毫秒）
    pub ttl_ms: i64,
}

/// 点位实时值缓存
#[derive(Debug, Default)]
pub struct PointValueCache {
    inner: RwLock<HashMap<String, CachedValue>>,
}

impl PointValueCache {
    /// 新建空缓存
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入点位最新值
    ///
    /// `interval_secs` 为该点位的采集周期（秒），TTL 取其 3 倍并以
    /// [`MIN_TTL_MS`] 为下限。
    pub fn put(&self, point_id: &str, value: f64, ts_ms: i64, interval_secs: u64) {
        if point_id.is_empty() || !value.is_finite() {
            return;
        }
        let ttl_ms = std::cmp::max(MIN_TTL_MS, interval_secs as i64 * 3 * 1000);
        self.inner.write().insert(
            point_id.to_string(),
            CachedValue {
                value,
                ts_ms,
                ttl_ms,
            },
        );
    }

    /// 读取点位最新值（已过期视为不存在）
    pub fn get(&self, point_id: &str, now_ms: i64) -> Option<f64> {
        let guard = self.inner.read();
        let rec = guard.get(point_id)?;
        if now_ms.saturating_sub(rec.ts_ms) > rec.ttl_ms {
            return None;
        }
        Some(rec.value)
    }

    /// 快照：所有**未过期**的点位值（供一次批量求值使用）
    pub fn snapshot(&self, now_ms: i64) -> HashMap<String, f64> {
        let guard = self.inner.read();
        guard
            .iter()
            .filter(|(_, rec)| now_ms.saturating_sub(rec.ts_ms) <= rec.ttl_ms)
            .map(|(k, rec)| (k.clone(), rec.value))
            .collect()
    }

    /// 清理过期记录（由处理管道周期性调用，避免点位下线后缓存无限增长）
    pub fn prune(&self, now_ms: i64) {
        let mut guard = self.inner.write();
        guard.retain(|_, rec| now_ms.saturating_sub(rec.ts_ms) <= rec.ttl_ms);
    }

    /// 缓存中的有效点位数量
    pub fn len(&self, now_ms: i64) -> usize {
        self.inner
            .read()
            .values()
            .filter(|rec| now_ms.saturating_sub(rec.ts_ms) <= rec.ttl_ms)
            .count()
    }

    /// 缓存是否为空（无任何有效记录）
    pub fn is_empty(&self, now_ms: i64) -> bool {
        self.len(now_ms) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_and_get() {
        let c = PointValueCache::new();
        c.put("p1", 12.5, 1000, 10);
        assert_eq!(c.get("p1", 1000), Some(12.5));
    }

    #[test]
    fn non_finite_and_empty_id_are_ignored() {
        let c = PointValueCache::new();
        c.put("p1", f64::NAN, 1000, 10);
        c.put("", 1.0, 1000, 10);
        assert!(c.is_empty(1000));
    }

    #[test]
    fn expired_value_is_treated_as_missing() {
        let c = PointValueCache::new();
        // interval=1s → TTL = max(30s, 3s) = 30s
        c.put("p1", 1.0, 0, 1);
        assert_eq!(c.get("p1", 29_000), Some(1.0));
        assert_eq!(c.get("p1", 31_000), None);
    }

    #[test]
    fn ttl_uses_three_times_interval_with_floor() {
        let c = PointValueCache::new();
        c.put("p1", 1.0, 0, 100);
        // TTL = 300s
        assert_eq!(c.get("p1", 299_000), Some(1.0));
        assert_eq!(c.get("p1", 301_000), None);
    }

    #[test]
    fn snapshot_excludes_expired() {
        let c = PointValueCache::new();
        c.put("p1", 1.0, 0, 1);
        c.put("p2", 2.0, 60_000, 1);
        let snap = c.snapshot(60_000);
        assert!(!snap.contains_key("p1"));
        assert_eq!(snap.get("p2"), Some(&2.0));
    }

    #[test]
    fn prune_removes_expired_only() {
        let c = PointValueCache::new();
        c.put("p1", 1.0, 0, 1);
        c.put("p2", 2.0, 60_000, 1);
        c.prune(60_000);
        assert_eq!(c.len(60_000), 1);
    }
}
