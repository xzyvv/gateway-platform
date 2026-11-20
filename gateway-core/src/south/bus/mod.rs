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

use std::time::Duration;

use crate::config::{BusConfig, OpcUaClientConfig};
use crate::engine::metrics::Metrics;
use crate::south::protocol::CollectorError;
use std::sync::Arc;

/// 失败重试次数的运行期兜底（BusConfig 未显式配置时使用）。
///
/// 与 `BusConfig` 的 serde 默认值一致（见 `tests::config_defaults_match_shared_retry_constants`
/// 的契约断言）。串口与 TCP 两条总线 task 的 retry 循环共用本常量，避免
/// 「配置默认 / 运行期兜底」两处硬编码漂移。
pub(crate) const DEFAULT_RETRY_COUNT: u32 = 2;

/// 重试间隔的运行期兜底（ms），与 `DEFAULT_RETRY_COUNT` 同源。
pub(crate) const DEFAULT_RETRY_DELAY_MS: u64 = 200;

/// 应答等待预算在「总线重试最坏耗时」之上保留的调度余量。
const REPLY_MARGIN: Duration = Duration::from_millis(100);

/// 单笔总线事务在最坏情况下占用总线 task 的时长（不含调用侧排队等待）：
/// 首试 + `retry_count` 次重试，每次最多 `timeout`，重试之间 sleep `retry_delay`。
///
/// 默认配置下 = 3×T + 2×200ms = `3T+400ms`，与 DLT645/CJT188 采集器
/// 「`io_timeout × 3`（首试 + 2 次重试）」的口径一致。
///
/// 这是**应答等待预算的唯一来源**：`BusManager::transaction` 必须按本函数
/// 推导出的预算等待应答。原实现硬编码 `2T+100ms`，小于重试预算，导致
/// 重试刚成功的应答在回传途中就被上层判超时丢弃——静默丢数（原 #107）。
pub(crate) fn bus_retry_budget(
    timeout: Duration,
    retry_count: u32,
    retry_delay: Duration,
) -> Duration {
    let attempts = retry_count.saturating_add(1);
    timeout
        .saturating_mul(attempts)
        .saturating_add(retry_delay.saturating_mul(retry_count))
}

/// 控制（写）事务在并发池中保留的槽位数。
///
/// 8 槽（默认）→ 留 2；2~3 槽 → 留 1；1 槽 → 不留（否则普通事务永远无法准入）。
/// 唯一来源：`TransactionPool` 用它算普通事务准入上限，
/// `BusManager::read_concurrency` 用它算采集侧并发信号量，
/// 两处必须一致，否则采集会按「总容量」开并发，多出来的那部分必然拿到 Busy。
pub(crate) fn reserved_control_slots(max_concurrent: usize) -> usize {
    match max_concurrent {
        0 | 1 => 0,
        2 | 3 => 1,
        _ => 2,
    }
}

/// 估算一次 Modbus 读请求的响应帧字节数（含 slave/fc/byte_count/CRC）。
///
/// 只处理读功能码（01/02 位、03/04 寄存器）；写响应固定 8 字节、异常帧 5 字节，
/// 都由调用方的固定超时覆盖，不需要按帧长放大。
fn estimated_response_bytes(request: &[u8], expected_count: Option<u16>) -> Option<usize> {
    let fc = *request.get(1)?;
    let count = usize::from(expected_count?);
    match fc {
        0x01 | 0x02 => Some(5 + count.div_ceil(8)),
        0x03 | 0x04 => Some(5 + count * 2),
        _ => None,
    }
}

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
    /// 调用方入队时刻：总线 task 取出执行时据此记录排队等待时长。
    pub enqueued_at: std::time::Instant,
    /// 事务优先级：控制（写）优先于采集（读）。
    pub priority: BusPriority,
    /// 是否允许传输层自动重发。
    ///
    /// `true`（读/采集）：失败后可按 `retry_count` 重发，重发是安全的。
    ///
    /// `false`（写）：**禁止自动重发**。写帧一旦下发，从站可能已经执行；
    /// 若因应答丢失而重发，同一指令会被执行多次（累加/脉冲类点位直接出错）。
    /// 写路径失败时改为上报 [`CollectorError::WriteUncertain`]，把「可能已生效」
    /// 显式交给调用方判断，而不是由系统自作主张重发。
    pub retry_allowed: bool,
}

/// 总线事务优先级。
///
/// 定义在 engine 中立层（见 [`crate::engine::priority`]）：北向 Modbus 网关桥接
/// 也需要按功能码区分读写指令，而 `arch_deps` 门禁禁止 north 引用 `crate::south::`。
/// 此处重导出，南向调用点仍可写 `south::bus::BusPriority`。
pub use crate::engine::priority::BusPriority;

/// 总线管理器（RS485 串行化）
pub struct BusManager {
    pub name: String,
    pub tcp_addr: Option<String>,
    /// 单次请求超时（ms），源自 `BusConfig.timeout_ms`。
    ///
    /// 供**直连型采集器**（fins/mc/s7comm/bacnet/iec104 等不经总线 task 自建连接的协议）
    /// 统一取值，与串口/TCP 总线 task 的超时口径一致，避免各协议各自硬编码。
    pub timeout_ms: u64,
    /// 应答等待预算：≥ 总线 task 的重试最坏耗时（见 [`bus_retry_budget`]）。
    ///
    /// 在 `spawn_with` 时按 `BusConfig` 快照一次，使「等应答」与「总线重试」
    /// 引用同一套超时 / 重试参数，避免两处硬编码漂移（原 #107）。
    reply_budget: Duration,
    /// 读路径可用的最大并发在途事务数（见 [`BusManager::max_concurrency`]）。
    /// Modbus TCP 取自 `BusConfig.tcp_concurrent`（默认 8）；串口总线恒为 1。
    max_inflight: usize,
    /// 重试参数快照：`transaction_with_priority` 用它按「本次有效超时」重新推导
    /// 应答预算（P1-2 自适应读超时后，单次超时可能大于配置值）。
    retry_count: u32,
    retry_delay: Duration,
    /// 串口波特率；TCP 总线为 `None`（无「线上字节耗时」概念）。
    baud: Option<u32>,
    /// 是否 Modbus ASCII 变体：每个 RTU 字节会展开成 2 个十六进制字符，
    /// 线上耗时约为 RTU 的 2.2 倍，估算帧时间时按 22 bit/byte 计。
    ascii_frame: bool,
    /// 是否 Modbus 协议族。
    ///
    /// 自适应读超时靠「请求第 2 字节是功能码」反推响应帧长，这个前提只在
    /// Modbus 下成立：DLT645 / CJT188 的第 2 字节是地址字段，可能恰好等于
    /// 0x01~0x04 而被误判成读功能码，导致按错误的帧长放大超时。
    is_modbus: bool,
    /// 协议变体（决策表 #24）：由 [`BusConfig::effective_variant`] 解析后的统一取值。
    /// 采集器按此分派帧构建逻辑（如 dlt645 版本、cjt188 方言、modbus ascii）。
    variant: Option<String>,
    /// IEC104 ASDU 公共地址（CA）字节数：1 或 2（决策表 #28）。
    asdu_addr_len: u8,
    /// OPC UA Client 认证配置（P0-3）：直连型采集器无法从总线 task 取配置，
    /// 在此保存一份启动时的快照（配置变更会重建 BusManager）。
    opcua: Option<OpcUaClientConfig>,
    /// 普通通道（采集轮询 / 调试直发）。
    tx: mpsc::Sender<BusTransaction>,
    /// 控制通道（写指令）：与普通通道分离，总线 task 优先消费。
    ctrl_tx: mpsc::Sender<BusTransaction>,
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
        // 控制通道（写指令）独立于采集通道：容量与普通通道一致。
        // 写入稀疏（人工/北向下发），不会挤占采集配额；但总线 task 会优先消费它。
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<BusTransaction>(cap);
        let name = bus.port_name.clone();
        let tcp_addr = bus.tcp_addr.clone();
        let timeout_ms = bus.timeout_ms;
        // 与串口/TCP 总线 task 的 retry 循环共用同一套参数与推导函数，
        // 快照应答预算：重试成功的应答不会在回传途中被上层误判超时（原 #107）。
        let retry_count = bus.retry_count.unwrap_or(DEFAULT_RETRY_COUNT);
        let retry_delay =
            Duration::from_millis(bus.retry_delay_ms.unwrap_or(DEFAULT_RETRY_DELAY_MS));
        let reply_budget = bus_retry_budget(
            Duration::from_millis(bus.timeout_ms),
            retry_count,
            retry_delay,
        );
        // Bug 217: 记录读路径并发上限——TCP 支持单 socket 多事务并发（MBAP 事务号路由），
        // 串口为半双工物理上只能串行。采集器据此决定并发提交帧的上限，
        // 使 tcp_concurrent 真正生效，同时不破坏串口总线的物理串行化。
        let max_inflight = if tcp_addr.is_some() {
            bus.tcp_concurrent.unwrap_or(8).max(1)
        } else {
            1
        };
        let bus_config = bus.clone();
        // 只有串口总线才有「波特率 × 帧长」的线上耗时；ASCII 变体按 22bit/byte 估算。
        let baud = if tcp_addr.is_some() {
            None
        } else {
            Some(bus.baud_rate)
        };
        let ascii_frame = bus
            .effective_variant()
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case("ascii"));
        // 自适应读超时的帧长估算依赖「请求第 2 字节 = Modbus 功能码」这一前提，
        // 只在 Modbus 协议族下启用（DLT645/CJT188 的该字节是地址字段）。
        let is_modbus = bus.protocol.to_ascii_lowercase().contains("modbus");

        let handle = crate::engine::supervision::spawn_catch_unwind(
            "bus_task",
            bus_task(bus_config, serial, rx, ctrl_rx, metrics.clone(), deps),
        );

        Self {
            name,
            tcp_addr,
            timeout_ms,
            reply_budget,
            max_inflight,
            retry_count,
            retry_delay,
            baud,
            ascii_frame,
            is_modbus,
            variant: bus.effective_variant(),
            asdu_addr_len: bus.asdu_addr_len.unwrap_or(2),
            opcua: bus.opcua.clone(),
            tx,
            ctrl_tx,
            metrics,
            handle: Some(handle),
        }
    }

    /// 普通优先级事务（采集轮询 / 调试直发）。
    pub async fn transaction(
        &self,
        request: Vec<u8>,
        expected_count: Option<u16>,
    ) -> Result<Vec<u8>, CollectorError> {
        self.transaction_with_priority(request, expected_count, BusPriority::Normal)
            .await
    }

    /// 带优先级的事务下发。
    ///
    /// [`BusPriority::Control`] 的写指令进入独立控制通道，总线 task 优先消费，
    /// 不必排在采集轮询后面；排队深度 / 等待时长指标按同一口径统计。
    pub async fn transaction_with_priority(
        &self,
        request: Vec<u8>,
        expected_count: Option<u16>,
        priority: BusPriority,
    ) -> Result<Vec<u8>, CollectorError> {
        self.transaction_inner(request, expected_count, priority, true)
            .await
    }

    /// 禁止自动重发的事务下发（写路径专用）。
    ///
    /// 与 [`BusManager::transaction_with_priority`] 的唯一区别：传输层失败后
    /// **不再重发**该请求。写帧一旦下发，从站可能已经执行；应答丢失时重发
    /// 会造成重复写（累加/脉冲类点位直接出错）。
    ///
    /// 失败原样透传给写路径，由写路径转成 [`CollectorError::WriteUncertain`]，
    /// 把「可能已生效」显式上报，而不是由总线层自作主张重发。
    pub async fn transaction_no_retry(
        &self,
        request: Vec<u8>,
        expected_count: Option<u16>,
        priority: BusPriority,
    ) -> Result<Vec<u8>, CollectorError> {
        self.transaction_inner(request, expected_count, priority, false)
            .await
    }

    /// 事务下发的公共实现。
    ///
    /// `retry_allowed=false` 时：① 不重发（见 [`BusTransaction::retry_allowed`]）；
    /// ② 应答预算按「不重试」推导，避免调用方白等 `retry_count` 轮重试的时间。
    async fn transaction_inner(
        &self,
        request: Vec<u8>,
        expected_count: Option<u16>,
        priority: BusPriority,
        retry_allowed: bool,
    ) -> Result<Vec<u8>, CollectorError> {
        let started = std::time::Instant::now();
        // 抓包 A 档：tx 在入队前记录（request 尚未被 move 进 BusTransaction）
        crate::south::frame_log::record(&self.name, true, &request, None);
        // P1-2：按帧长/波特率算出本次单次尝试的有效超时（串口读大块时可能大于配置值）。
        // 该值随事务下发，并用于下面推导应答预算，保证「调用方等待」不会先于「总线 task 收帧」。
        let attempt_timeout = self.effective_attempt_timeout(&request, expected_count);
        let (resp_tx, resp_rx) = oneshot::channel();
        let msg = BusTransaction {
            request,
            response_tx: resp_tx,
            expected_count,
            raw: false,
            timeout_ms: Some(attempt_timeout.as_millis().min(u128::from(u64::MAX)) as u64),
            enqueued_at: std::time::Instant::now(),
            priority,
            retry_allowed,
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
        // 原实现硬编码 `2T+100ms`（串口路径"写一帧 + 读一帧"共用 timeout 的假设），
        // 但总线 task 的重试循环最坏要 `3T+400ms`（默认 2 次重试，每次一整个 timeout，
        // 间隔 200ms）。重试成功的应答还在回传途中，上层就判超时丢弃 → 静默丢数（原 #107）。
        // 现按 [`bus_retry_budget`] 同一来源推导，并保留固定调度余量。
        // P1-2：预算按「本次有效超时」重新推导。放宽了单次读超时后，若仍用 spawn 时
        // 按配置值快照的 budget，调用方会先超时丢弃应答（重试成功也白搭，原 #107）。
        let effective_retries = if retry_allowed { self.retry_count } else { 0 };
        let reply_deadline = bus_retry_budget(attempt_timeout, effective_retries, self.retry_delay)
            .saturating_add(REPLY_MARGIN);
        // 控制（写）走控制通道，采集轮询 / 调试直发走普通通道。
        // 两条通道共用同一套排队指标口径（深度、等待时长）。
        let sender = match priority {
            BusPriority::Control => &self.ctrl_tx,
            BusPriority::Normal => &self.tx,
        };
        let result = match tokio::time::timeout(self.io_timeout(), sender.send(msg)).await {
            Ok(Ok(())) => {
                // 已入队：排队深度 +1，由总线 task 取出时 -1（含调用方随后超时放弃的事务）
                self.metrics.inc_bus_queue_depth();
                match tokio::time::timeout(reply_deadline, resp_rx).await {
                    // 内层 Ok = 总线 task 回传了事务结果（成功或设备级失败），必须原样透传。
                    // 注意：不能写成 `inner.map_err(|_| BusClosed)`——那会把 NoReply /
                    // ChecksumError / IoError 等**业务失败**全部改标成 BusClosed，
                    // 使"设备不应答"在监控上表现成"总线断开"，后果是整条总线的
                    // 错误归因失真（C-2 之后更明显：设备级失败必须与端口级区分）。
                    Ok(Ok(inner)) => inner,
                    // 内层 Err = 应答 oneshot 的 sender 被 drop（总线 task 已退出/panic）
                    Ok(Err(_)) => Err(CollectorError::BusClosed),
                    Err(_) => Err(CollectorError::Timeout(reply_deadline)),
                }
            }
            Ok(Err(_)) => Err(CollectorError::BusClosed),
            // 入队超时 = 队列打满，与「已下发但设备没回」严格区分（BusBusy → BUSY）
            Err(_) => Err(CollectorError::BusBusy(self.io_timeout())),
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

    /// 指标句柄：供协议层上报协议级事件（如写结果未知计数）。
    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
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
            enqueued_at: std::time::Instant::now(),
            // 调试直发刻意走低优先级：它是运维排查手段，不得抢占控制（写）指令，
            // 也不应排到采集轮询前面影响实时性。
            priority: BusPriority::Normal,
            // 调试直发不重发：报文回放要求逐帧对比，重发会破坏回放序列。
            retry_allowed: false,
        };
        // 应答预算 = 单帧超时 + 100ms 调度余量（总线任务内部已按同一超时收尾，
        // 这里只需略长于它，避免把「刚好收满」的正常响应误判为超时）
        let reply_deadline = frame_timeout + std::time::Duration::from_millis(100);
        let started = std::time::Instant::now();
        let result = match tokio::time::timeout(frame_timeout, self.tx.send(msg)).await {
            Ok(Ok(())) => {
                self.metrics.inc_bus_queue_depth();
                match tokio::time::timeout(reply_deadline, resp_rx).await {
                    Ok(Ok(inner)) => inner,
                    Ok(Err(_)) => Err(CollectorError::BusClosed),
                    Err(_) => Err(CollectorError::Timeout(reply_deadline)),
                }
            }
            Ok(Err(_)) => Err(CollectorError::BusClosed),
            Err(_) => Err(CollectorError::BusBusy(frame_timeout)),
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

    /// 采集（普通优先级）事务的并发上限 = 池总容量 − 为写保留的槽位。
    ///
    /// 修复(P0-1)：采集侧原按 `max_concurrency()`（总容量）开信号量，
    /// 而 `TransactionPool` 已把普通事务的准入上限收紧到 `总容量 − 保留槽`。
    /// 两者不一致时，多出来的那部分并发必然提交即 `Busy`，形成「每轮固定
    /// 几个点白跑一次重试、重试耗尽后报总线忙」的隐性丢点。
    /// 串口半双工恒为 1（保留槽在 1 槽时也为 0，不会饿死采集）。
    pub fn read_concurrency(&self) -> usize {
        if self.tcp_addr.is_some() {
            self.max_inflight
                .saturating_sub(reserved_control_slots(self.max_inflight))
                .max(1)
        } else {
            1
        }
    }

    /// 直连型采集器的 IO 超时：与总线 `timeout_ms` 同源（`BusConfig.timeout_ms`）。
    ///
    /// 用于 fins/mc/s7comm/bacnet/iec104 等**不经总线 task、自建连接**的协议，
    /// 使「单次请求等待应答」的超时与串口/TCP 总线口径统一，现场可在配置中调整。
    pub fn io_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms)
    }

    /// 本次事务「单次尝试」的有效超时（P1-2 自适应读超时）。
    ///
    /// 配置的 `timeout_ms` 是按「小请求」的经验值定的；9600bps 下读 125 个寄存器，
    /// 响应帧约 255 字节 ≈ 265ms 线上时间，按默认 200ms 必然每帧 NoReply，
    /// 现场表现为「设备在线但采集全空」，且错误信息指向「设备不应答」误导排查。
    ///
    /// 这里按 `帧字节数 × 10bit / 波特率` 估算线上耗时，取 `max(配置值, 估算值 + 20ms)`：
    /// - 只对读功能码（01~04）生效，写响应固定 8 字节、异常帧 5 字节，无需放大；
    /// - 只对串口生效，TCP 无波特率概念；
    /// - 上限 10s，避免配置错误导致单事务永久占用总线。
    ///
    /// 放大后的值会同时用于推导弹到 [`BusManager::transaction_with_priority`] 的
    /// 应答预算，否则调用方会先于总线 task 超时（重试成功的应答被丢弃，原 #107）。
    pub(crate) fn effective_attempt_timeout(
        &self,
        request: &[u8],
        expected_count: Option<u16>,
    ) -> Duration {
        let base = self.io_timeout();
        if !self.is_modbus {
            // 非 Modbus 协议：无法从请求字节反推响应帧长，维持配置超时
            return base;
        }
        let (Some(baud), Some(bytes)) = (self.baud, estimated_response_bytes(request, expected_count))
        else {
            return base;
        };
        if baud == 0 {
            return base;
        }
        let bits_per_byte = if self.ascii_frame { 22 } else { 10 };
        let wire_ms = u64::from(bytes as u32) * bits_per_byte * 1000 / u64::from(baud);
        let need = Duration::from_millis(wire_ms.saturating_add(20));
        let effective = base.max(need).min(Duration::from_secs(10));
        if effective > base {
            tracing::debug!(
                bus = %self.name,
                configured_ms = base.as_millis(),
                effective_ms = effective.as_millis(),
                response_bytes = bytes,
                baud,
                "adaptive read timeout: configured timeout is shorter than frame time"
            );
        }
        effective
    }

    /// 应答等待预算：总线重试最坏耗时 + 固定调度余量。
    ///
    /// 必须 ≥ [`bus_retry_budget`]，否则重试成功的应答还没回传就被判超时丢弃
    /// （原 #107 静默丢数）。`spawn_with` 时已把重试参数快照进 `reply_budget`，
    /// 这里只做纯加法，便于测试以纯函数断言数值关系。
    pub(crate) fn reply_deadline(&self) -> Duration {
        self.reply_budget.saturating_add(REPLY_MARGIN)
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
    ctrl_rx: mpsc::Receiver<BusTransaction>,
    metrics: Arc<Metrics>,
    deps: BusDeps,
) {
    // Modbus TCP 模式：走 TCP 而非串口
    #[cfg(feature = "modbus")]
    if let Some(tcp_addr) = &bus_config.tcp_addr.clone() {
        tcp_task::bus_task_tcp(bus_config, tcp_addr.clone(), rx, ctrl_rx, metrics).await;
        return;
    }
    serial_task::run_serial_bus_task(bus_config, serial, rx, ctrl_rx, metrics, deps).await;
}

/// 出队后的统一前置处理：更新排队指标，并判断事务是否已被调用方放弃。
///
/// 返回 `false` 时必须**直接丢弃、不得下发**：
/// - 写指令：调用方已收到超时/失败，若照常执行会出现「API 返回失败、设备实际被写」，
///   调用方重试还会造成重复写；
/// - 采集：过期请求会白占总线时隙。
///
/// 只能拦「尚未开始发送」的事务；帧一旦写进串口就无法撤销。
fn prepare_dequeued(tx: &BusTransaction, metrics: &crate::engine::metrics::Metrics) -> bool {
    metrics.dec_bus_queue_depth();
    let wait_us = tx.enqueued_at.elapsed().as_micros().min(u64::MAX as u128) as u64;
    metrics.record_bus_queue_wait(wait_us);
    !tx.response_tx.is_closed()
}

/// 是否值得在总线 task 内立即重试。
///
/// [`CollectorError::BusBusy`] 刻意**不**列为可重试：它表示队列已满（带宽不足），
/// 立即重试只会再占用一次 `timeout` 并加剧拥堵，交由下一个采集周期自然重试更安全。
pub(crate) fn is_retryable(e: &CollectorError) -> bool {
    matches!(
        e,
        CollectorError::NoReply
            | CollectorError::IoError(_)
            | CollectorError::ChecksumError
            | CollectorError::Timeout(_)
    )
    // 注意：`CollectorError::WriteUncertain` 刻意不在其中。
    // 写结果未知 = 写帧可能已被从站执行，重试会造成重复写。
    // `BusBusy` 同理不可重试：它是「根本没发出去」，但重试在写路径上
    // 仍可能与另一笔已下发的写交错，语义上应交由调用方决定。
    // `DeviceRejected` 同样不自动重试：设备明确说了「不行」，
    // 是否重试要由调用方按业务语义决定（可能换参数、可能改人工介入）。
}

pub(crate) mod frame_extract;
pub(crate) mod serial_task;
#[cfg(feature = "modbus")]
mod tcp_task;

#[cfg(test)]
mod tests;
