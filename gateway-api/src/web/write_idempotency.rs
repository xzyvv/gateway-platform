//! 写请求幂等键缓存（ROI-09）
//!
//! 上游（MES / 集成方）在客户端超时后往往会整批重试。批量写不是原子的：
//! 首次请求里已经成功的点如果被重发一次，对累加型、脉冲型点位就是**重复动作**。
//!
//! 调用方为一次业务动作生成一个 `idempotency_key`，重试时带同一个 key；
//! 本缓存把首次响应体在有效期内返回给重复请求，不再真正下发。
//!
//! 边界（刻意保守）：
//! - 只缓存**成功返回**的响应体（单写被拒/参数错误不缓存，重试仍会重新校验）；
//! - 进程内缓存，重启后失效——重启意味着上一次结果已不可追溯，宁可不复用；
//! - 并发同 key 请求不保证串行（网关是单机单进程，上游重试通常在超时之后）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 幂等键有效期：覆盖上游常规重试窗口，又不会长期占用内存。
const TTL: Duration = Duration::from_secs(600);
/// 最大缓存条数：超过后先清过期项，仍超限则整体清空（宁可丢幂等，不可无限涨）。
const MAX_ENTRIES: usize = 256;

struct Entry {
    at: Instant,
    body: serde_json::Value,
}

/// 写请求幂等键 → 首次响应体。
#[derive(Default)]
pub struct WriteIdempotencyStore {
    inner: Mutex<HashMap<String, Entry>>,
}

impl WriteIdempotencyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取首次响应体；不存在或已过期返回 `None`。
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        let mut guard = self.inner.lock();
        match guard.get(key) {
            Some(entry) if entry.at.elapsed() <= TTL => Some(entry.body.clone()),
            Some(_) => {
                guard.remove(key);
                None
            }
            None => None,
        }
    }

    /// 记录首次响应体。
    pub fn put(&self, key: &str, body: serde_json::Value) {
        let mut guard = self.inner.lock();
        if guard.len() >= MAX_ENTRIES {
            guard.retain(|_, e| e.at.elapsed() <= TTL);
            if guard.len() >= MAX_ENTRIES {
                guard.clear();
            }
        }
        guard.insert(
            key.to_string(),
            Entry {
                at: Instant::now(),
                body,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_returns_same_body() {
        let store = WriteIdempotencyStore::new();
        let body = serde_json::json!({ "status": "ok", "total": 2 });
        store.put("k1", body.clone());
        assert_eq!(store.get("k1"), Some(body));
        assert_eq!(store.get("missing"), None);
    }

    #[test]
    fn overwrite_keeps_latest() {
        let store = WriteIdempotencyStore::new();
        store.put("k", serde_json::json!({ "n": 1 }));
        store.put("k", serde_json::json!({ "n": 2 }));
        assert_eq!(store.get("k"), Some(serde_json::json!({ "n": 2 })));
    }

    #[test]
    fn capacity_is_bounded() {
        let store = WriteIdempotencyStore::new();
        for i in 0..(MAX_ENTRIES + 10) {
            store.put(&format!("k{i}"), serde_json::json!({ "i": i }));
        }
        assert!(store.inner.lock().len() <= MAX_ENTRIES);
    }
}
