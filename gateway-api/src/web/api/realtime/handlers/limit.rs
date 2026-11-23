//! 第 4 层：实时 WebSocket 并发连接上限（进程级）
//!
//! 独立成文件而不是塞进 `mod.rs`：`mod.rs` 已贴近 500 行行数预算，而这块
//! 「配置读取 + 原子槽位」是自洽的一小段逻辑，单独放也更好测。
//!
//! 每个连接都会独立序列化广播批次，慢连接还会占用有界发送缓冲；无上限时
//! 页面多开 / 恶意连接可把 CPU 与内存打满。超限在升级前直接 503，
//! 避免"先接受再断开"造成客户端重连风暴。

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::web::types::AppState;

/// 当前在线实时 WS 连接数（仅供上限判定，与 `RealtimeBus::subscriber_count`
/// 不同——后者还包含北向、报警等内部订阅者）。
static WS_CONNECTION_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 当前在线诊断日志 WS 连接数。
///
/// DD-62：日志流此前没有连接上限，页面多开 / 恶意连接会平摊广播订阅与序列化成本。
/// 与实时数据流**分开计数**（同一页面通常只开一条日志流，且日志是纯文本转发，
/// 资源画像与点位批次不同），上限复用 `web.realtime_max_subscribers`。
static LOG_WS_CONNECTION_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 尝试占用一个槽位（CAS 循环）；超限返回 false。
fn acquire(counter: &AtomicUsize, limit: usize) -> bool {
    let mut cur = counter.load(Ordering::Acquire);
    loop {
        if cur >= limit {
            return false;
        }
        match counter.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => cur = actual,
        }
    }
}

/// 实时 WS 并发连接上限，取值来自配置项 `web.realtime_max_subscribers`
/// （默认 64，范围 1..=1024，见 [`gateway_core::config::REALTIME_MAX_SUBSCRIBERS_MAX`]）。
///
/// 每次升级现取配置：改配置后对新连接立即生效，已建立的连接不受影响。
pub(super) fn realtime_max_subscribers(state: &AppState) -> usize {
    state
        .config
        .config
        .load()
        .web
        .as_ref()
        .map(|w| w.realtime_max_subscribers)
        .unwrap_or_else(gateway_core::config::default_realtime_max_subscribers)
}

/// 实时 WS 连接槽位；Drop 时归还名额。
pub(super) struct WsSlot;

impl WsSlot {
    /// `limit` 由调用方按当前配置传入（不在此处读全局，便于单测注入）。
    pub(super) fn try_acquire(limit: usize) -> Option<Self> {
        if acquire(&WS_CONNECTION_COUNT, limit) {
            Some(Self)
        } else {
            None
        }
    }
}

impl Drop for WsSlot {
    fn drop(&mut self) {
        WS_CONNECTION_COUNT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 诊断日志 WS 连接槽位；Drop 时归还名额。
pub(super) struct LogWsSlot;

impl LogWsSlot {
    pub(super) fn try_acquire(limit: usize) -> Option<Self> {
        if acquire(&LOG_WS_CONNECTION_COUNT, limit) {
            Some(Self)
        } else {
            None
        }
    }
}

impl Drop for LogWsSlot {
    fn drop(&mut self) {
        LOG_WS_CONNECTION_COUNT.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_slot_enforces_limit_and_releases_on_drop() {
        // 用极小上限验证语义，不再依赖全局常量（上限已改为配置项）
        const LIMIT: usize = 4;
        let slots: Vec<WsSlot> = (0..LIMIT)
            .map(|_| WsSlot::try_acquire(LIMIT).expect("slot within limit"))
            .collect();
        assert!(
            WsSlot::try_acquire(LIMIT).is_none(),
            "limit must reject extra"
        );
        assert_eq!(WS_CONNECTION_COUNT.load(Ordering::Acquire), LIMIT);

        drop(slots);
        assert_eq!(WS_CONNECTION_COUNT.load(Ordering::Acquire), 0);
        assert!(
            WsSlot::try_acquire(LIMIT).is_some(),
            "slot reusable after drop"
        );
    }

    /// 配置化回归：默认值必须与配置化前的硬编码一致（64）。
    #[test]
    fn realtime_limit_default_matches_previous_hardcode() {
        let web = gateway_core::config::WebConfig::default();
        assert_eq!(web.realtime_max_subscribers, 64);
    }

    /// DD-62：日志流与实时流分开计数，各自的上限互不挤占。
    #[test]
    fn log_ws_slot_is_counted_separately_from_realtime() {
        const LIMIT: usize = 2;
        let log_slots: Vec<LogWsSlot> = (0..LIMIT)
            .map(|_| LogWsSlot::try_acquire(LIMIT).expect("log slot within limit"))
            .collect();
        assert!(LogWsSlot::try_acquire(LIMIT).is_none());
        // 日志流占满不应影响实时流名额
        assert!(WsSlot::try_acquire(LIMIT).is_some());
        assert_eq!(LOG_WS_CONNECTION_COUNT.load(Ordering::Acquire), LIMIT);
        drop(log_slots);
        assert_eq!(LOG_WS_CONNECTION_COUNT.load(Ordering::Acquire), 0);
    }
}
