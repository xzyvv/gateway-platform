//! `run_event_loop` 的入参集合（从 `event_loop.rs` 抽出）。
//!
//! 未放进 `event_loop/` 子目录：这些字段由同属 mqtt 模块的 `connection.rs`
//! 直接构造，放在 mqtt 模块下的兄弟文件才能让 `pub(super)` 口径保持原样。

use super::*;

/// 入站下行报文发送端槽位：Some = 订阅者已挂接可投递，None = 尚未挂接
pub(super) type IncomingTxSlot = Option<mpsc::Sender<(String, Vec<u8>)>>;

/// `run_event_loop` 的入参集合：所有共享状态通过 Arc 传入，装配点唯一，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub(super) struct EventLoopInputs {
    pub(super) eventloop: EventLoop,
    pub(super) shutdown: Arc<Notify>,
    /// r48-MQTT1：关停标志（与 shutdown 配合，见 mod.rs::wait_shutdown）
    pub(super) stopping: Arc<std::sync::atomic::AtomicBool>,
    pub(super) pending_ack: PendingAckQueue,
    pub(super) incoming_tx: Arc<parking_lot::Mutex<IncomingTxSlot>>,
    /// f5: 期望订阅表，ConnAck 成功后据此重新订阅（clean_session=true 会清空订阅）
    pub(super) desired_subscriptions: Arc<parking_lot::Mutex<Vec<String>>>,
    pub(super) connected: Arc<std::sync::atomic::AtomicBool>,
    pub(super) outbox: Option<Arc<AsyncMutex<OutboxStore>>>,
    pub(super) replay_in_progress: Arc<std::sync::atomic::AtomicBool>,
    /// P1-4：孤儿 PubAck 计数（与 MqttClient 字段共享同一原子）
    pub(super) orphan_acks: Arc<std::sync::atomic::AtomicU64>,
    /// P1-4：槽位 ack_id 生成器（replay 路径共用）
    pub(super) ack_id_gen: Arc<std::sync::atomic::AtomicU64>,
    /// P1-4：publish 串行信号量（replay 路径同样遵守）
    pub(super) publish_semaphore: Arc<tokio::sync::Semaphore>,
    /// P1-5：数据队列句柄（replay 成功路径需同步 ack 队列行，防双通道重复投递）
    pub(super) queue: Arc<DataQueue>,
    pub(super) config: MqttConfig,
    pub(super) client: AsyncClient,
    /// R2：指标实例，用于统计 ConnAck 被拒次数（与 MqttClient 共享同一 Arc）
    pub(super) metrics: Arc<crate::engine::metrics::Metrics>,
}
