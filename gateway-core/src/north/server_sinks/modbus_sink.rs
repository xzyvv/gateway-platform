//! Modbus Server sink：将 RealtimeBus 数据推送到 Modbus 寄存器空间。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::engine::shutdown::ShutdownToken;
use crate::north::server::RegisterMapping;
use crate::north::server::Server;

use super::base::SinkCore;

/// Modbus Server sink：将 DataPoint 转为寄存器值
///
/// Modbus sink 内部 task 与外部共享寄存器映射表（build_mappings 调用前映射为空，apply_registers 跳过）。
/// Modbus sink 与 OPC UA sink 不同之处在于：build_mappings 必须在 sink.send 前调用一次，
/// 因此 sink 字段用 Arc<RwLock> 让内部 task 与外部 build_mappings 共享。
/// 其余生命周期（channel / 背压 / 停机）由 [`SinkCore`] 统一承载。
pub struct ModbusSink {
    core: Arc<SinkCore>,
    /// 寄存器映射表（point_id → register_addr），Arc 共享给内部 task
    mappings: Arc<RwLock<Vec<RegisterMapping>>>,
    /// Modbus server 句柄：`health()` 据此反映 server 真实运行状态，
    /// 避免 server 未启动 / 已停止时仍上报 connected=true（假健康 + 静默丢数据）。
    server: Arc<crate::north::server::modbus_server::ModbusServer>,
}

impl ModbusSink {
    pub fn new(
        server: Arc<crate::north::server::modbus_server::ModbusServer>,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Arc<Self> {
        Self::with_shutdown(server, ShutdownToken::new(), metrics)
    }

    pub fn with_shutdown(
        server: Arc<crate::north::server::modbus_server::ModbusServer>,
        shutdown: ShutdownToken,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Arc<Self> {
        let mappings = Arc::new(RwLock::new(Vec::<RegisterMapping>::new()));
        let mappings_for_task = mappings.clone();
        // server 需要同时被内部 task（写寄存器）与 health()（读运行状态）使用，故 clone Arc。
        let server_for_apply = server.clone();

        // 差异点：消费一批点位 → 按当前映射表写入 Modbus 寄存器空间。
        //
        // Bug 187 (N17): apply 现在必须返回 Result，并要求构造时传入 on_failure。
        // 失败条件 = server.is_running()==false（Modbus TCP listener 未在 listen、
        // 或者端口被外部进程抢占、或者 modbus 进程自身 panic 后 supervisor 还没拉起）。
        let server_for_recovery = server.clone();
        let core = SinkCore::new(
            "modbus",
            shutdown,
            metrics,
            move |points| {
                if !server_for_apply.is_running() {
                    // Bug 187: server 未运行，写入无意义（外部主站读不到任何寄存器），
                    // 触发 on_failure 重连。
                    return Err(());
                }
                let mappings = mappings_for_task.read();
                if !mappings.is_empty() {
                    let values: HashMap<String, f64> = points
                        .iter()
                        .map(|p| (p.point_id.clone(), p.value))
                        .collect();
                    server_for_apply.apply_registers(&mappings, &values);
                }
                Ok(())
            },
            move || {
                tracing::warn!(
                    target: "north.server_sink",
                    "modbus sink: triggering server restart (on_failure)"
                );
                tokio::task::block_in_place(|| {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.block_on(async {
                            let _ = server_for_recovery.stop().await;
                            if let Err(e) = server_for_recovery.start().await {
                                tracing::error!(
                                    target: "north.server_sink",
                                    "modbus server restart failed: {} (后续重试仍会失败)",
                                    e
                                );
                            }
                        });
                    } else {
                        tracing::error!(
                            target: "north.server_sink",
                            "modbus on_failure: no tokio runtime in scope, cannot restart"
                        );
                    }
                });
            },
        );
        Arc::new(Self {
            core,
            mappings,
            server,
        })
    }

    /// 根据点位配置生成寄存器映射
    pub fn build_mappings(&self, points: &[PointConfig], base_register: u16) {
        let mut mappings = Vec::with_capacity(points.len());
        let mut auto_addr = base_register;
        for p in points.iter() {
            let addr = p.modbus_register.unwrap_or_else(|| {
                let a = auto_addr;
                auto_addr += 1;
                a
            });
            mappings.push(RegisterMapping::from_point(p, addr));
        }
        *self.mappings.write() = mappings;
    }

    /// sink 标识（用于 metrics key，与 send() 内部一致）。
    /// 注意：NorthConnector::name() 返回 "modbus_sink"（连接器 SPI 名称），
    /// 本方法返回 "modbus"（历史 metrics key，保持不变）。
    pub fn name(&self) -> &'static str {
        self.core.name()
    }

    /// 投递一批采集数据到 Modbus 通道（非阻塞 try_send）。
    ///
    /// 1.2: 原为 NorthboundSink trait 方法，现为 inherent 方法。
    /// NorthConnector::publish_batch 内部调用此方法。
    pub fn send(&self, points: Vec<DataPoint>) {
        self.core.send(points);
    }
}

// ============================================================================
// 1.2: NorthConnector 统一北向 SPI 实现
// ============================================================================
// 不导入 NorthConnector（use），避免与 NorthboundSink::name() 方法歧义。
// 使用全路径 crate::north::NorthConnector 引用 trait。

#[async_trait::async_trait]
impl crate::north::NorthConnector for ModbusSink {
    fn name(&self) -> &str {
        "modbus_sink"
    }

    async fn publish_batch(&self, batch: &[DataPoint]) -> crate::engine::error::GatewayResult<()> {
        // 复用 inherent send()（try_send，非阻塞语义）。
        self.send(batch.to_vec());
        Ok(())
    }

    fn health(&self) -> crate::north::ConnectorHealth {
        // 真实反映 Modbus server 运行状态（与 OpcUaSink 一致）：server 未运行
        // （未启动/已停止）时外部主站读不到寄存器数据，应报告 not connected，
        // 避免监控误判为健康。原实现恒返回 connected=true，属假健康。
        let running = self.server.is_running();
        crate::north::ConnectorHealth {
            connected: running,
            last_error: if running {
                None
            } else {
                Some("modbus server not running".to_string())
            },
        }
    }

    fn is_enabled(&self) -> bool {
        // sink 存在即表示已启用（创建时已检查 config.modbus_server.enabled）
        true
    }
}
