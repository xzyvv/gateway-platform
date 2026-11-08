//! 实时广播总线（RealtimeBus）契约。
//!
//! # 为什么放在 engine
//!
//! `RealtimeBus` 是"处理管道 → 各消费端（北向 fan-out / WebSocket / 报警引擎）"
//! 之间的广播契约。它原先定义在 `pipeline` 模块里，于是 `engine`（报警引擎要订阅它）
//! 与 `pipeline` 形成双向依赖：
//!
//! ```text
//! engine  → pipeline  (报警引擎订阅 RealtimeBus)
//! pipeline → engine  (管道发布 DataPoint)
//! ```
//!
//! 与 `engine::ports` 消除 `engine ↔ north` / `engine ↔ queue` 循环的做法一致：
//! **把双方都需要的契约放到中立层，由依赖方引用**。总线是基础设施契约而非管道实现细节，
//! 因此下沉到 `engine`，`pipeline` 侧保留 `pub use` 重导出以兼容既有调用点
//! （`gateway_core::pipeline::RealtimeBus` 仍可用）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::engine::model::DataPoint;

/// 实时总线消费者类别。
///
/// 不同消费端的 Lagged 语义不同（北向 fan-out 掉队 = 上行缺口；WS 掉队 =
/// 看板掉点；报警掉队 = 限值判定漏批），统一在总线上按类别累计，
/// 便于运维区分「哪个下游拖慢了广播」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusConsumerKind {
    North,
    WebSocket,
    Alarm,
}

/// 各类消费者的累计丢批次快照（供状态接口暴露）。
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaggedCounts {
    pub north: u64,
    pub websocket: u64,
    pub alarm: u64,
}

/// 实时数据广播总线
///
/// 处理管道把"边缘过滤+转换之后、即将入队"的点发布到这里，
/// Web Server 的 WebSocket 端点 `subscribe()` 拿到 Receiver 后推给前端。
/// ConversionManager 也订阅此总线，将数据 fan-out 到所有北向 connector。
///
/// 容量由 `WebConfig::realtime_buffer` 控制（默认 1024）。
/// 消费慢时广播返回 Lagged，调用方应丢弃而非 panic。
pub struct RealtimeBus {
    tx: broadcast::Sender<Arc<Vec<DataPoint>>>,
    /// 按消费者类别累计的 Lagged 丢批次（单调递增）
    lagged_north: AtomicU64,
    lagged_websocket: AtomicU64,
    lagged_alarm: AtomicU64,
}

impl RealtimeBus {
    /// 创建实时广播通道
    pub fn new(capacity: usize) -> Arc<Self> {
        let cap = capacity.max(16);
        let (tx, _rx) = broadcast::channel(cap);
        Arc::new(Self {
            tx,
            lagged_north: AtomicU64::new(0),
            lagged_websocket: AtomicU64::new(0),
            lagged_alarm: AtomicU64::new(0),
        })
    }

    /// 发布一批点（处理管道调用）
    pub fn publish(&self, points: Arc<Vec<DataPoint>>) {
        let _ = self.tx.send(points);
    }

    /// 订阅（WebSocket handler 调用）
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Vec<DataPoint>>> {
        self.tx.subscribe()
    }

    /// 当前订阅者数
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// 上报某个消费端发生的 Lagged 丢批，返回该类别累计值。
    ///
    /// 所有消费端（北向 / WS / 报警）在 `RecvError::Lagged` 分支统一调用，
    /// 避免各消费端各写一份计数、状态接口只能看到北向一家。
    pub fn note_lagged(&self, kind: BusConsumerKind, dropped: u64) -> u64 {
        let slot = match kind {
            BusConsumerKind::North => &self.lagged_north,
            BusConsumerKind::WebSocket => &self.lagged_websocket,
            BusConsumerKind::Alarm => &self.lagged_alarm,
        };
        slot.fetch_add(dropped, Ordering::Relaxed)
            .saturating_add(dropped)
    }

    /// 各类消费者累计丢批次快照
    pub fn lagged_counts(&self) -> LaggedCounts {
        LaggedCounts {
            north: self.lagged_north.load(Ordering::Relaxed),
            websocket: self.lagged_websocket.load(Ordering::Relaxed),
            alarm: self.lagged_alarm.load(Ordering::Relaxed),
        }
    }
}
