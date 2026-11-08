//! 协议转换管理器：消费北向专用无损队列并 fan-out 到所有已注册的北向 connector。
//!
//! 设计（RealtimeBus 第 5 层）：
//! - 处理管道把同一份 `Arc<Vec<DataPoint>>` 同时投递到广播总线与北向专用队列；
//! - 本管理器只消费专用队列（mpsc），队列满时背压回传到处理管道，**不丢批**；
//! - 广播总线保留给 WS / 报警 / 最新值快照等可容忍丢帧的下游。
//!
//! 优雅停机：
//! - 停机信号触发后先把队列中已积压的批次同步 fan-out 完再退出；
//! - 发送端全部 drop（`pipeline.close_north_feed()`）后 `recv()` 返回 None，
//!   队列自然排空即退出。

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;

use crate::engine::model::DataPoint;
use crate::engine::shutdown::ShutdownToken;

use super::NorthConnector;

/// 协议转换管理器
///
/// 消费北向专用队列，将采集数据分发给所有已注册的北向 connector。
pub struct ConversionManager {
    /// 北向专用队列接收端：`run()` 启动时 take 走（所有权需要），
    /// 同时保证 run 循环只可能被启动一次。
    feed: Mutex<Option<mpsc::Receiver<Arc<Vec<DataPoint>>>>>,
    /// 已注册的北向 connector 列表
    connectors: RwLock<Vec<Arc<dyn NorthConnector>>>,
    /// 关闭信号（connector task 也可监听同一个 shutdown，进程级协调停机）
    shutdown: ShutdownToken,
}

impl ConversionManager {
    /// 创建转换管理器（使用全局 ShutdownToken）
    pub fn new(feed: mpsc::Receiver<Arc<Vec<DataPoint>>>) -> Arc<Self> {
        Arc::new(Self {
            feed: Mutex::new(Some(feed)),
            connectors: RwLock::new(Vec::new()),
            shutdown: ShutdownToken::new(),
        })
    }

    /// 创建带指定 ShutdownToken 的转换管理器
    pub fn with_shutdown(
        feed: mpsc::Receiver<Arc<Vec<DataPoint>>>,
        shutdown: ShutdownToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            feed: Mutex::new(Some(feed)),
            connectors: RwLock::new(Vec::new()),
            shutdown,
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

    /// 启动转换循环：从北向专用队列接收并 fan-out 到各 connector。
    ///
    /// 退出条件：
    /// - 队列发送端全部 drop（`recv()` 返回 None）
    /// - shutdown 信号触发（先把积压批次 drain 完再退出）
    pub async fn run(&self) {
        let mut rx = match self.feed.lock().take() {
            Some(rx) => rx,
            None => {
                tracing::error!("conversion_manager: run() 被重复调用，忽略本次");
                return;
            }
        };
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.wait() => {
                    tracing::info!("conversion_manager: shutdown signal received");
                    // 停机信号可能先于最后一批数据到达：把队列里已积压的批次
                    // 同步 fan-out 出去再退出，避免最终批被停机窗口截断。
                    while let Ok(points) = rx.try_recv() {
                        self.fanout(points).await;
                    }
                    break;
                }
                batch = rx.recv() => {
                    match batch {
                        Some(points) => self.fanout(points).await,
                        None => {
                            tracing::info!("conversion_manager: north feed closed, exiting");
                            break;
                        }
                    }
                }
            }
        }
        tracing::info!("conversion_manager: fan-out loop exited");
    }

    /// 把一批点位投递给所有已启用 connector。
    ///
    /// 并发投递：各 connector 的 `publish_batch` 相互独立（MQTT 入队 /
    /// Webhook 写 outbox / sink 更新快照），串行 await 会让慢 connector
    /// 阻塞快 connector，在批次积压时放大延迟。`join_all` 在批内保持点序、
    /// 跨批仍由队列容量施加背压。
    async fn fanout(&self, points: Arc<Vec<DataPoint>>) {
        let batch = points.as_slice();
        // 先克隆 Arc 快照并立即释放读锁，避免锁 guard 跨 await 持有
        let snapshot: Vec<_> = self.connectors.read().iter().cloned().collect();
        let publishes = snapshot.iter().filter(|c| c.is_enabled()).map(|connector| {
            async move {
                // connector.publish_batch 是非阻塞语义（内部 try_send 或 enqueue），
                // 正常路径立即返回；Webhook 等写 outbox 的路径并发执行。
                if let Err(e) = connector.publish_batch(batch).await {
                    tracing::warn!(
                        target: "conversion_manager",
                        connector = connector.name(),
                        "publish_batch error: {:?}",
                        e,
                    );
                }
            }
        });
        futures::future::join_all(publishes).await;
    }
}

#[cfg(test)]
mod tests;
