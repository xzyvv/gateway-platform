//! OPC UA Server sink：将 RealtimeBus 数据推送到 OPC UA 地址空间。

use std::sync::Arc;

use crate::engine::model::DataPoint;
use crate::engine::shutdown::ShutdownToken;
use crate::north::server::Server;

use super::base::SinkCore;

/// OPC UA Server sink
///
/// 内部 task 消费 mpsc channel，调用 `server.apply_points` 同步更新 OPC UA 地址空间。
/// ConversionManager send 时仅 try_send 到 channel，不等待。
/// 生命周期（channel / 背压 / 停机）由 [`SinkCore`] 统一承载。
pub struct OpcUaSink {
    core: Arc<SinkCore>,
    /// OPC UA server 句柄：`health()` 据此反映 server 真实运行状态，
    /// 避免 server 未启动 / 已停止时仍上报 connected=true（假健康 + 静默丢数据）。
    server: Arc<crate::north::server::opcua_server::OpcUaServer>,
}

impl OpcUaSink {
    pub fn new(
        server: Arc<crate::north::server::opcua_server::OpcUaServer>,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Arc<Self> {
        Self::with_shutdown(server, ShutdownToken::new(), metrics)
    }

    pub fn with_shutdown(
        server: Arc<crate::north::server::opcua_server::OpcUaServer>,
        shutdown: ShutdownToken,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Arc<Self> {
        // 差异点：消费一批点位 → 写入 OPC UA 地址空间。
        // server 需要同时被内部 task（写地址空间）与 health()（读运行状态）使用，故 clone Arc。
        //
        // Bug 187 (N17): apply 现在必须返回 Result，并要求构造时传入 on_failure。
        // - apply：先检查 server.is_running()；不健康时返回 Err(())，由内核触发
        //   on_failure 重连/重启并**重试一次**。
        // - on_failure：调用 server.stop() 释放句柄，再 server.start() 重建 endpoint；
        //   期间同步等待（stop/start 都是 tokio 异步），用 block_in_place 避免阻塞
        //   sink 内部 task 所在 worker 太久。
        let server_for_apply = server.clone();
        let server_for_recovery = server.clone();
        let core = SinkCore::new(
            "opcua",
            shutdown,
            metrics,
            move |points| {
                if !server_for_apply.is_running() {
                    // Bug 187: server 未运行（崩溃/被 OOM kill/启动失败），
                    // 写入无意义 → 触发 on_failure 重连。
                    return Err(());
                }
                server_for_apply.apply_points(points);
                Ok(())
            },
            move || {
                tracing::warn!(
                    target: "north.server_sink",
                    "opcua sink: triggering server restart (on_failure)"
                );
                // Bug 187: block_in_place 允许在多线程 runtime 中调用
                // .await 风格的 start/stop 而不阻塞 worker 调度。
                tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::try_current();
                    if let Ok(handle) = rt {
                        handle.block_on(async {
                            let _ = server_for_recovery.stop().await;
                            if let Err(e) = server_for_recovery.start().await {
                                tracing::error!(
                                    target: "north.server_sink",
                                    "opcua server restart failed: {} (后续重试仍会失败)",
                                    e
                                );
                            }
                        });
                    } else {
                        tracing::error!(
                            target: "north.server_sink",
                            "opcua on_failure: no tokio runtime in scope, cannot restart"
                        );
                    }
                });
            },
        );
        Arc::new(Self { core, server })
    }

    /// sink 标识（用于 metrics key，与 send() 内部一致）。
    /// 注意：NorthConnector::name() 返回 "opcua_sink"（连接器 SPI 名称），
    /// 本方法返回 "opcua"（历史 metrics key，保持不变）。
    pub fn name(&self) -> &'static str {
        self.core.name()
    }

    /// 投递一批采集数据到 OPC UA 通道（非阻塞 try_send）。
    ///
    /// 1.2: 原为 NorthboundSink trait 方法，现为 inherent 方法。
    /// NorthConnector::publish_batch 内部调用此方法。
    pub fn send(&self, points: Vec<DataPoint>) {
        self.core.send(points);
    }
}

// ============================================================================
// 1.2: NorthConnector 统一北向 SPI 实现（Phase A: additive only）
// ============================================================================

#[async_trait::async_trait]
impl crate::north::NorthConnector for OpcUaSink {
    fn name(&self) -> &str {
        "opcua_sink"
    }

    async fn publish_batch(&self, batch: &[DataPoint]) -> crate::engine::error::GatewayResult<()> {
        // 复用 inherent send()（try_send，非阻塞语义）。
        self.send(batch.to_vec());
        Ok(())
    }

    fn health(&self) -> crate::north::ConnectorHealth {
        // 真实反映 OPC UA server 运行状态：server 未运行（未启动/已停止/启动失败）时
        // 外部客户端读不到地址空间数据，应报告 not connected，避免监控误判为健康。
        // 原实现恒返回 connected=true，属假健康（server 起不来也无告警）。
        let running = self.server.is_running();
        crate::north::ConnectorHealth {
            connected: running,
            last_error: if running {
                None
            } else {
                Some("opcua server not running".to_string())
            },
        }
    }

    fn is_enabled(&self) -> bool {
        // sink 存在即表示已启用（创建时已检查 config.opcua.enabled）
        true
    }
}
