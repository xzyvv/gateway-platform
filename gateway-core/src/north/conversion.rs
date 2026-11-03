//! 协议转换管理器：订阅 RealtimeBus 并 fan-out 到所有已注册的北向 connector。
//!
//! 设计：async fan-out（核心优化）
//! - ConversionManager 单 task 消费 RealtimeBus
//! - 每个 connector 自带内部 task + mpsc channel
//! - `connector.publish_batch()` 是非阻塞语义（内部 try_send / enqueue）
//! - connector 内部 task 串行处理 apply_points / apply_registers，互不阻塞
//!
//! 优雅停机：
//! - ConversionManager 任务在 broadcast sender drop 后退出
//! - connector task 监听 ShutdownToken，shutdown() 后立即 break

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::engine::shutdown::ShutdownToken;
use crate::pipeline::RealtimeBus;

use super::NorthConnector;

/// 协议转换管理器
///
/// 订阅 RealtimeBus，将采集数据分发给所有已注册的北向 connector（async fan-out）。
pub struct ConversionManager {
    /// 实时数据总线
    realtime_bus: Arc<RealtimeBus>,
    /// 已注册的北向 connector 列表
    connectors: RwLock<Vec<Arc<dyn NorthConnector>>>,
    /// 关闭信号（connector task 也可监听同一个 shutdown，进程级协调停机）
    shutdown: ShutdownToken,
    /// #6: RealtimeBus 广播溢出（Lagged）累计被丢弃的批次数（单调递增）。
    /// 持续增长说明下游（SQLite/北向）出现背压，实时 fan-out 掉队，实时推送存在缺口。
    lagged_dropped_batches: AtomicU64,
}

impl ConversionManager {
    /// 创建转换管理器（使用全局 ShutdownToken）
    pub fn new(realtime_bus: Arc<RealtimeBus>) -> Arc<Self> {
        Arc::new(Self {
            realtime_bus,
            connectors: RwLock::new(Vec::new()),
            shutdown: ShutdownToken::new(),
            lagged_dropped_batches: AtomicU64::new(0),
        })
    }

    /// 创建带指定 ShutdownToken 的转换管理器
    pub fn with_shutdown(realtime_bus: Arc<RealtimeBus>, shutdown: ShutdownToken) -> Arc<Self> {
        Arc::new(Self {
            realtime_bus,
            connectors: RwLock::new(Vec::new()),
            shutdown,
            lagged_dropped_batches: AtomicU64::new(0),
        })
    }

    /// 获取关闭信号（用于同步触发 connector task 退出）
    pub fn shutdown_token(&self) -> ShutdownToken {
        self.shutdown.clone()
    }

    /// 注册一个北向 connector
    pub fn register_connector(&self, connector: Arc<dyn NorthConnector>) {
        self.connectors.write().push(connector);
    }

    /// 当前注册的 connector 数量
    pub fn connector_count(&self) -> usize {
        self.connectors.read().len()
    }

    /// #6: 实时广播因溢出（Lagged）被丢弃的累计批次数（供运维/指标观测）。
    /// 大于 0 且持续增长 ⇒ 发生了背压性数据缺口。
    pub fn lagged_dropped_batches(&self) -> u64 {
        self.lagged_dropped_batches.load(Ordering::Relaxed)
    }

    /// N-7: 汇总所有已注册 connector 的真实健康状态。
    ///
    /// 由各 connector 的 `health()` 实现上报，而不是由 Web 层按配置猜；
    /// MQTT 断连、Webhook 最近一次发送失败、Kafka 未编译 feature 等
    /// 都会在这里如实体现，供 `/api/north/status` 排障使用。
    pub fn connector_status(&self) -> Vec<crate::north::ConnectorStatus> {
        self.connectors
            .read()
            .iter()
            .map(|c| {
                let health = c.health();
                crate::north::ConnectorStatus {
                    name: c.name().to_string(),
                    enabled: c.is_enabled(),
                    connected: health.connected,
                    last_error: health.last_error,
                }
            })
            .collect()
    }

    /// 启动转换循环：订阅 RealtimeBus 并 fan-out 到各 connector。
    ///
    /// 退出条件：
    /// - broadcast sender 全部 drop（`Err(RecvError)`）
    /// - shutdown 信号触发
    pub async fn run(&self) {
        let mut rx = self.realtime_bus.subscribe();
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.wait() => {
                    tracing::info!("conversion_manager: shutdown signal received");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(points) => {
                            // fan-out：给每个 connector.publish_batch 一份（&[DataPoint]）。
                            // publish_batch 实现为非阻塞语义（try_send / enqueue 立即返回），
                            // 不会卡住 ConversionManager。
                            //
                            // r36-CONV1: 广播项本身即 `Arc<Vec<DataPoint>>`，直接借其
                            // 切片传给各 connector，省去旧实现 `(*points).clone()` 每条
                            // 消息一次整批点位深拷贝（高频采集下的无谓分配/memcpy）。
                            let batch = points.as_slice();
                            // 先克隆 Arc 快照并立即释放读锁，避免锁 guard 跨 await 持有
                            let snapshot: Vec<_> = self.connectors.read().iter().cloned().collect();
                            for connector in snapshot {
                                // Bug 151: 跳过未启用的 connector——旧实现不加判断，
                                // 对已关闭的北向通道仍调用 publish_batch（其内部多为
                                // 静默丢弃），既浪费开销又掩盖"配置已关但不该有数据流出"
                                // 的语义。is_enabled() 反映各 connector 的配置意图。
                                if !connector.is_enabled() {
                                    continue;
                                }
                                // connector.publish_batch 是非阻塞语义（内部 try_send 或 enqueue），
                                // .await 立即返回，不会卡住 fan-out 循环。
                                if let Err(e) = connector.publish_batch(batch).await {
                                    tracing::warn!(
                                        target: "conversion_manager",
                                        connector = connector.name(),
                                        "publish_batch error: {:?}",
                                        e,
                                    );
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // #6: 广播通道满导致丢帧。旧实现仅 warn、无累计，背压期间
                            // 「静默少点」无法被观测。此处累计丢帧数并写入日志，便于运维
                            // 判断是否发生背压性数据缺口（实时推送 / 北向 fan-out 掉队）。
                            let total = self
                                .lagged_dropped_batches
                                .fetch_add(n, Ordering::Relaxed)
                                .saturating_add(n);
                            tracing::warn!(
                                target: "conversion_manager",
                                "realtime bus lagged: dropped {} frames (cumulative {}); \
                                 downstream backpressure is causing realtime data gaps",
                                n,
                                total
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("conversion_manager: broadcast closed, exiting");
                            break;
                        }
                    }
                }
            }
        }
        tracing::info!("conversion_manager: fan-out loop exited");
    }
}

#[cfg(test)]
mod tests;
