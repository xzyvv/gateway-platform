//! 内存存储后端
//!
//! 纯内存实现，无持久化。适用于测试和轻量场景。
//! 所有操作 O(1) 或 O(n)（n = 队列长度），无 IO 开销。

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};

use parking_lot::Mutex;

use crate::engine::error::GatewayResult;
use crate::engine::model::DataPoint;

use super::QueueStorage;

use crate::queue::core::QueuedItem;

/// 内存中的队列项（带 in_flight 标记）
struct MemoryItem {
    item: QueuedItem,
    in_flight: bool,
}

/// 纯内存队列存储后端
pub struct MemoryStorage {
    items: Mutex<VecDeque<MemoryItem>>,
    next_id: AtomicI64,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            items: Mutex::new(VecDeque::new()),
            next_id: AtomicI64::new(1),
        }
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueStorage for MemoryStorage {
    fn enqueue_batch(&self, points: &[DataPoint]) -> GatewayResult<usize> {
        if points.is_empty() {
            return Ok(0);
        }
        let mut items = self.items.lock();
        for dp in points {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let mut qi = QueuedItem::from_datapoint(dp);
            qi.id = id;
            items.push_back(MemoryItem {
                item: qi,
                in_flight: false,
            });
        }
        Ok(points.len())
    }

    fn dequeue_batch(&self, limit: usize) -> GatewayResult<Vec<QueuedItem>> {
        let mut items = self.items.lock();
        let mut result = Vec::new();
        for mi in items.iter_mut() {
            if result.len() >= limit {
                break;
            }
            if !mi.in_flight {
                mi.in_flight = true;
                result.push(mi.item.clone());
            }
        }
        Ok(result)
    }

    fn ack_batch(&self, ids: &[i64]) -> GatewayResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        // 用 HashSet 做 O(1) 命中判定，避免 `ids.contains` 线性扫描导致的 O(n·m)
        let id_set: HashSet<i64> = ids.iter().copied().collect();
        let mut items = self.items.lock();
        items.retain(|mi| !id_set.contains(&mi.item.id));
        Ok(())
    }

    fn release_in_flight(&self, ids: &[i64]) -> GatewayResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let id_set: HashSet<i64> = ids.iter().copied().collect();
        let mut items = self.items.lock();
        for mi in items.iter_mut() {
            if id_set.contains(&mi.item.id) {
                mi.in_flight = false;
            }
        }
        Ok(())
    }

    fn cleanup_stale_in_flight(&self, _cutoff_ts: i64) -> GatewayResult<usize> {
        // 内存后端无 stale in_flight 概念（无超时风险）
        Ok(0)
    }

    fn len(&self) -> usize {
        self.items.lock().len()
    }

    fn in_flight_count(&self) -> i64 {
        self.items.lock().iter().filter(|mi| mi.in_flight).count() as i64
    }

    fn history_query(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
    ) -> GatewayResult<Vec<QueuedItem>> {
        let items = self.items.lock();
        let mut result: Vec<QueuedItem> = items
            .iter()
            .filter(|mi| {
                mi.item.point_id == point_id
                    && mi.item.timestamp >= start
                    && mi.item.timestamp <= end
            })
            .map(|mi| mi.item.clone())
            .collect();
        // 按 timestamp DESC 取 limit，然后 reverse 为 ASC（与 SQLite 实现一致）
        result.sort_by_key(|a| std::cmp::Reverse(a.timestamp));
        result.truncate(limit);
        result.reverse();
        Ok(result)
    }

    fn maintenance(&self) -> GatewayResult<()> {
        Ok(())
    }

    fn needs_maintenance(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "memory"
    }
}
