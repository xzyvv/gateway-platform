//! 总线管理器（§8.3 + §A.3 缺陷修正）
//!
//! 每条总线一个 mpsc 通道 + 一个独立 task，采集请求排队执行，
//! 避免 RS485 半双工冲突。所有协议（Modbus/DLT645/CJT188）共用。
//!
//! 支持：
//! - 写/读超时分离（写用短超时，防止串口流控异常拖死总线）
//! - RS485 DE/RE 方向控制（BusConfig.de_pin）
//! - Modbus ASCII 模式（BusConfig.modbus_mode）

pub(crate) mod frame_buffer;
pub mod serial;
pub mod transport;

pub use frame_buffer::FrameBuffer;

pub use serial::{SerialHandle, SerialManager};
pub use transport::{BusDeps, GpioOutput, SerialOpener};

use tokio::sync::{mpsc, oneshot};

use crate::config::{BusConfig, OpcUaClientConfig};
use crate::engine::metrics::Metrics;
use crate::south::protocol::CollectorError;
use std::sync::Arc;

/// 一次总线事务：写请求 + 读响应
pub struct BusTransaction {
    pub request: Vec<u8>,
    pub response_tx: oneshot::Sender<Result<Vec<u8>, CollectorError>>,
    /// 期望读取的 Modbus 寄存器数（用于帧层 byte_count 对照）
    pub expected_count: Option<u16>,
    /// 调试直发模式（P0-1）：原样写帧、原样收字节。
    ///
    /// 与采集事务的关键差异——**不做协议分帧、不做 CRC 校验、不重试**：
    /// 调试工位需要看到设备真实回了什么（含 CRC 错帧、异常帧、半截帧），
    /// 而分帧器会静默丢弃校验失败的字节使上层只能看到「无应答」，
    /// 重试则会重发请求、破坏报文回放的逐帧对比。仅在 [`BusManager::raw_transaction`]
    /// 中置 true；采集路径恒为 false，行为与历史完全一致。
    pub raw: bool,
    /// 调试直发模式的单帧等待超时（ms）；`None` 时沿用 `BusConfig.timeout_ms`
    pub timeout_ms: Option<u64>,
}

/// 总线管理器（RS485 串行化）
pub struct BusManager {
    pub name: String,
    pub tcp_addr: Option<String>,
    /// 单次请求超时（ms），源自 `BusConfig.timeout_ms`。
    ///
    /// 供**直连型采集器**（fins/mc/s7comm/bacnet/iec104 等不经总线 task 自建连接的协议）
    /// 统一取值，与串口/TCP 总线 task 的超时口径一致，避免各协议各自硬编码。
    pub timeout_ms: u64,
    /// 读路径可用的最大并发在途事务数（见 [`BusManager::max_concurrency`]）。
    /// Modbus TCP 取自 `BusConfig.tcp_concurrent`（默认 8）；串口总线恒为 1。
    max_inflight: usize,
    /// 协议变体（决策表 #24）：由 [`BusConfig::effective_variant`] 解析后的统一取值。
    /// 采集器按此分派帧构建逻辑（如 dlt645 版本、cjt188 方言、modbus ascii）。
    variant: Option<String>,
    /// IEC104 ASDU 公共地址（CA）字节数：1 或 2（决策表 #28）。
    asdu_addr_len: u8,
    /// OPC UA Client 认证配置（P0-3）：直连型采集器无法从总线 task 取配置，
    /// 在此保存一份启动时的快照（配置变更会重建 BusManager）。
    opcua: Option<OpcUaClientConfig>,
    tx: mpsc::Sender<BusTransaction>,
    /// 单事务往返延迟埋点（决策表 #25）；与 bus_task 共用同一实例（全链路单例约束）。
    metrics: Arc<Metrics>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl BusManager {
    pub fn spawn(bus: &BusConfig, serial: Option<SerialHandle>, metrics: Arc<Metrics>) -> Self {
        Self::spawn_with(bus, serial, metrics, BusDeps::production())
    }

    /// 同 [`BusManager::spawn`]，但允许注入外部依赖（串口打开器 / GPIO 输出）。
    ///
    /// **仅供测试注入**：生产调用方应使用 [`BusManager::spawn`]。
    #[doc(hidden)]
    pub fn spawn_with(
        bus: &BusConfig,
        serial: Option<SerialHandle>,
        metrics: Arc<Metrics>,
        deps: BusDeps,
    ) -> Self {
        // Bug 149: mpsc channel 容量——默认 128，可通过 BusConfig.channel_capacity 按总线配置。
        // 慢速总线（9600bps RTU）若调度器并发提交过多请求，通道满后 caller 的
        // send().await 阻塞。如现场出现「等待 tx 通道」日志，增大此值或上层做并发限制。
        // 下限 1：tokio `mpsc::channel(0)` 会直接 panic，且此处早于 `spawn_catch_unwind`，
        // 历史配置写成 0 会导致进程启动即崩。
        let cap = bus.channel_capacity.unwrap_or(128).max(1);
        let (tx, rx) = mpsc::channel::<BusTransaction>(cap);
        let name = bus.port_name.clone();
        let tcp_addr = bus.tcp_addr.clone();
        let timeout_ms = bus.timeout_ms;
        // Bug 217: 记录读路径并发上限——TCP 支持单 socket 多事务并发（MBAP 事务号路由），
        // 串口为半双工物理上只能串行。采集器据此决定并发提交帧的上限，
        // 使 tcp_concurrent 真正生效，同时不破坏串口总线的物理串行化。
        let max_inflight = if tcp_addr.is_some() {
            bus.tcp_concurrent.unwrap_or(8).max(1)
        } else {
            1
        };
        let bus_config = bus.clone();

        let handle = crate::engine::supervision::spawn_catch_unwind(
            "bus_task",
            bus_task(bus_config, serial, rx, metrics.clone(), deps),
        );

        Self {
            name,
            tcp_addr,
            timeout_ms,
            max_inflight,
            variant: bus.effective_variant(),
            asdu_addr_len: bus.asdu_addr_len.unwrap_or(2),
            opcua: bus.opcua.clone(),
            tx,
            metrics,
            handle: Some(handle),
        }
    }

    pub async fn transaction(
        &self,
        request: Vec<u8>,
        expected_count: Option<u16>,
    ) -> Result<Vec<u8>, CollectorError> {
        let started = std::time::Instant::now();
        // 抓包 A 档：tx 在入队前记录（request 尚未被 move 进 BusTransaction）
        crate::south::frame_log::record(&self.name, true, &request, None);
        let (resp_tx, resp_rx) = oneshot::channel();
        let msg = BusTransaction {
            request,
            response_tx: resp_tx,
            expected_count,
            raw: false,
            timeout_ms: None,
        };
        // Bug 148: 若 mpsc send 失败（总线 task 已退出），oneshot sender 立即 drop，
        // resp_rx.await 会收到 RecvError → 转为 BusClosed。当前逻辑正确，但
        // 未 panic 也不泄漏 sender。注释说明此路径。
        // 修复(审查#A7)：原实现 `self.tx.send(msg).await` 无超时。总线通道打满时
        // 调用方会无限期挂起（既堵住采集 task，又只能靠上层 5s 组超时兜底而被误
        // 判为「设备超时」）。现按总线 timeout_ms 给入队加统一上限，超时返回
        // Timeout（入队失败即丢弃事务，不再让过期请求被真的下发到设备）。
        // 修复(C-1)：原先这里只给"入队"加了超时，`resp_rx.await` 完全没有超时。
        // 若总线 task 因任何原因不再应答（例如 modbus tcp 的事务任务卡在写 socket、
        // 或任务已被取消却没 drop sender），采集 task 会永久阻塞 → 上层
        // supervision_loop 卡在 task_fn() 上 → 该组所有 tick 全部停摆且只能重启。
        // 现给"等应答"补一道超时，口径取 2×io_timeout + 100ms：
        // 串口路径一次事务是"写一帧 + 读一帧"共用 timeout_ms 预算，
        // 直接复用单个 io_timeout 会把正常的长帧读写误判为超时。
        let reply_deadline = self.io_timeout() * 2 + std::time::Duration::from_millis(100);
        let result = match tokio::time::timeout(self.io_timeout(), self.tx.send(msg)).await {
            Ok(Ok(())) => match tokio::time::timeout(reply_deadline, resp_rx).await {
                // 内层 Ok = 总线 task 回传了事务结果（成功或设备级失败），必须原样透传。
                // 注意：不能写成 `inner.map_err(|_| BusClosed)`——那会把 NoReply /
                // ChecksumError / IoError 等**业务失败**全部改标成 BusClosed，
                // 使"设备不应答"在监控上表现成"总线断开"，后果是整条总线的
                // 错误归因失真（C-2 之后更明显：设备级失败必须与端口级区分）。
                Ok(Ok(inner)) => inner,
                // 内层 Err = 应答 oneshot 的 sender 被 drop（总线 task 已退出/panic）
                Ok(Err(_)) => Err(CollectorError::BusClosed),
                Err(_) => Err(CollectorError::Timeout(reply_deadline)),
            },
            Ok(Err(_)) => Err(CollectorError::BusClosed),
            Err(_) => Err(CollectorError::Timeout(self.io_timeout())),
        };
        // 决策表 #25：记录单事务往返延迟（排队 + 收发）。失败亦计入，用于错误率观测。
        // 失败原因由 `CollectorError::error_code()` 映射到统计层稳定错误码，
        // 使 Prometheus 能按 `by_reason` 区分「排队超时 / 设备无应答 / 校验错」。
        let elapsed_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        self.metrics.record_transaction_with_reason(
            result.is_ok(),
            elapsed_us,
            result.as_ref().err().map(|e| e.error_code()),
        );
        // 抓包 A 档：rx 仅在收到响应时记录（tx 已在入队前记录；失败事务呈现为
        // 「只有 tx 无 rx」，正是现场排查无响应问题时最想看到的形态）
        if let Ok(resp) = &result {
            crate::south::frame_log::record(
                &self.name,
                false,
                resp,
                Some((elapsed_us / 1000).min(i32::MAX as u64) as i32),
            );
        }
        result
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 调试直发事务（P0-1 报文编辑器 / 报文回放）：原样写帧、原样收回字节。
    ///
    /// 与 [`BusManager::transaction`] 的差异（均为调试语义必需）：
    /// 1. **不分帧、不校验**：返回设备本次真实回的全部字节。分帧器会把 CRC 错帧
    ///    逐字节丢弃，上层只能看到「无应答」，无法区分「设备没回」与「回了个坏帧」。
    /// 2. **不重试**：回放要逐帧对比请求/响应，自动重试会重发请求打乱配对。
    /// 3. **不计入采集指标**：调试流量不应污染生产侧事务延迟/成功率观测。
    ///
    /// 超时口径：`timeout_ms` 为单帧等待超时（调用方已按上限截断）；
    /// 排队等待同样受该超时约束，避免调试请求堵在通道里。
    ///
    /// 取消安全：future 被 drop 时，oneshot sender 随之释放，总线任务取到
    /// 「回传失败」只记日志并继续，不残留定时器、不占住端口。
    pub async fn raw_transaction(
        &self,
        request: Vec<u8>,
        timeout_ms: Option<u64>,
    ) -> Result<Vec<u8>, CollectorError> {
        let frame_timeout = timeout_ms
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| self.io_timeout());
        let (resp_tx, resp_rx) = oneshot::channel();
        // 抓包 A 档：调试直发同样记录（报文编辑器/回放发的帧应出现在抓包列表）；
        // 必须在 request move 进 BusTransaction 之前记录
        crate::south::frame_log::record(&self.name, true, &request, None);
        let msg = BusTransaction {
            request,
            response_tx: resp_tx,
            expected_count: None,
            raw: true,
            timeout_ms,
        };
        // 应答预算 = 单帧超时 + 100ms 调度余量（总线任务内部已按同一超时收尾，
        // 这里只需略长于它，避免把「刚好收满」的正常响应误判为超时）
        let reply_deadline = frame_timeout + std::time::Duration::from_millis(100);
        let started = std::time::Instant::now();
        let result = match tokio::time::timeout(frame_timeout, self.tx.send(msg)).await {
            Ok(Ok(())) => match tokio::time::timeout(reply_deadline, resp_rx).await {
                Ok(Ok(inner)) => inner,
                Ok(Err(_)) => Err(CollectorError::BusClosed),
                Err(_) => Err(CollectorError::Timeout(reply_deadline)),
            },
            Ok(Err(_)) => Err(CollectorError::BusClosed),
            Err(_) => Err(CollectorError::Timeout(frame_timeout)),
        };
        if let Ok(resp) = &result {
            crate::south::frame_log::record(
                &self.name,
                false,
                resp,
                Some((started.elapsed().as_millis()).min(i32::MAX as u128) as i32),
            );
        }
        result
    }

    /// 读路径可用的最大并发在途事务数。
    ///
    /// - Modbus TCP：`BusConfig.tcp_concurrent`（默认 8）。单 socket 靠 MBAP 事务号路由，
    ///   可同时 in-flight 多个事务；采集器据此并发提交帧，避免逐帧等待。
    /// - 串口总线：恒为 1（RS485 半双工，物理上必须串行）。
    pub fn max_concurrency(&self) -> usize {
        self.max_inflight
    }

    /// 直连型采集器的 IO 超时：与总线 `timeout_ms` 同源（`BusConfig.timeout_ms`）。
    ///
    /// 用于 fins/mc/s7comm/bacnet/iec104 等**不经总线 task、自建连接**的协议，
    /// 使「单次请求等待应答」的超时与串口/TCP 总线口径统一，现场可在配置中调整。
    pub fn io_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms)
    }

    /// 统一协议变体（决策表 #24），采集器按此分派帧构建逻辑。
    ///
    /// 取值来自 [`BusConfig::effective_variant`]；`None` 表示该协议无变体概念。
    pub fn variant(&self) -> Option<&str> {
        self.variant.as_deref()
    }

    /// IEC104 ASDU 公共地址（CA）字节数：1 或 2（决策表 #28），默认 2。
    pub fn asdu_addr_len(&self) -> u8 {
        self.asdu_addr_len
    }

    /// OPC UA Client 认证配置（P0-3）；`None` 表示历史匿名默认行为。
    pub fn opcua_config(&self) -> Option<&OpcUaClientConfig> {
        self.opcua.as_ref()
    }
}

impl Drop for BusManager {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// 总线处理 task（串口 / TCP 分发）
async fn bus_task(
    bus_config: BusConfig,
    serial: Option<SerialHandle>,
    rx: mpsc::Receiver<BusTransaction>,
    metrics: Arc<Metrics>,
    deps: BusDeps,
) {
    // Modbus TCP 模式：走 TCP 而非串口
    #[cfg(feature = "modbus")]
    if let Some(tcp_addr) = &bus_config.tcp_addr.clone() {
        tcp_task::bus_task_tcp(bus_config, tcp_addr.clone(), rx, metrics).await;
        return;
    }
    serial_task::run_serial_bus_task(bus_config, serial, rx, metrics, deps).await;
}

pub(crate) fn is_retryable(e: &CollectorError) -> bool {
    matches!(
        e,
        CollectorError::NoReply
            | CollectorError::IoError(_)
            | CollectorError::ChecksumError
            | CollectorError::Timeout(_)
    )
}

pub(crate) mod frame_extract;
pub(crate) mod serial_task;
#[cfg(feature = "modbus")]
mod tcp_task;

#[cfg(test)]
mod tests;
