//! 串口总线任务循环
//!
//! 包含串口模式下的总线请求处理循环、重连逻辑和帧提取。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::frame_buffer::FrameBuffer;
use crate::config::BusConfig;
use crate::engine::metrics::Metrics;
use crate::south::protocol::codec::{ascii_to_rtu, rtu_to_ascii};
use crate::south::protocol::CollectorError;

use super::frame_extract::extract_frame;
use super::transport::{BusDeps, GpioOutput};
use super::{BusTransaction, SerialHandle, DEFAULT_RETRY_COUNT, DEFAULT_RETRY_DELAY_MS};

// 后台自愈重连（热拔插）独立成模块：主循环只管「取事务 → 执行 → 回结果」，
// 端口掉了怎么自愈是另一组关注点。见子模块内注释。
mod reconnect;
use reconnect::{ensure_background_reconnect, lock_slot, AliveGuard, ReconnectCtx};

/// NoReply/ChecksumError 连续达到该次数后发一帧自检帧，区分设备级与端口级故障。
const PROBE_AFTER_SILENT: u32 = 10;

/// 由自检帧驱动的端口重开最小间隔。
///
/// 防止「总线上的从站全都掉线」时每次自检都关端口重开——重开本身会让同总线
/// 所有设备短暂拿到 BusClosed，需要节流。
const MIN_REOPEN_INTERVAL: Duration = Duration::from_secs(60);

/// Modbus ASCII 接收缓冲硬上限（字节）。
///
/// Modbus RTU ADU 最大 256 字节，ASCII 编码后每字节变 2 个 hex 字符，
/// 再加起始 `:`、2 字节 LRC 与 CRLF，合法帧最长约 517 字节。取 1024
/// 留出约一倍余量（容忍少量前导噪声 / 粘连帧），同时对异常设备持续发
/// 无换行数据做硬约束——原实现用裸 `Vec<u8>` 无上限累积，可撑爆内存。
///
/// 同一进程内的 RTU 分帧缓冲已有 `FrameBuffer::MAX_FRAME_BYTES`(2048) 保护，
/// 但 ASCII 分支的 `ascii_buf` 是独立缓冲，此前没有任何上限（原 #112）。
const ASCII_BUF_LIMIT: usize = 1024;

/// ASCII 缓冲超限后的收缩：保留最后一个 `:` 及其后的字节，返回丢弃的字节数。
///
/// 溢出点通常落在「垃圾前缀 + 一个刚开始接收的半帧」上。整段 `clear()` 会把这段
/// 半帧一并丢掉，随后到达的续帧缺头，LRC 永远对不上，表现为「设备明明在回，却
/// 一直 CRC 错」。保留最后一个起始符即可让续帧接上。
///
/// 约束：保留段若仍超 `ASCII_BUF_LIMIT`，说明这个「帧」本身长到不可能合法
/// （合法 ASCII 帧最多约 515 字节），此时按垃圾处理，整段丢弃。
fn shrink_ascii_buf(buf: &mut Vec<u8>) -> usize {
    let keep_from = buf
        .iter()
        .rposition(|&b| b == b':')
        .filter(|&pos| buf.len() - pos <= ASCII_BUF_LIMIT);
    let dropped = keep_from.unwrap_or(buf.len());
    match keep_from {
        Some(pos) => {
            buf.drain(..pos);
        }
        None => buf.clear(),
    }
    dropped
}

/// 构造端口自检帧：Modbus RTU FC03 最小读（addr=0, count=1）。
///
/// 帧格式与协议实现共用同一份定义（`protocol::modbus::build_read_request`），
/// 避免总线层再抄一份帧构造导致与协议层漂移。
///
/// 返回 `None` 表示「本构建无法构造自检帧」——modbus 协议被 feature gate
/// 剥离时协议模块不存在。此时不存在 modbus 从站，自检分支不可达，
/// 按「无已知良好从站」保守处理（不动端口）。
#[cfg(feature = "modbus")]
fn build_probe_frame(slave: u8) -> Option<Vec<u8>> {
    Some(crate::south::protocol::modbus::build_read_request(
        slave, 0x03, 0, 1,
    ))
}

/// modbus 协议被 feature gate 剥离时的占位实现（见 modbus 版文档）。
#[cfg(not(feature = "modbus"))]
fn build_probe_frame(_slave: u8) -> Option<Vec<u8>> {
    None
}

/// 写事务被总线任务取消时的收尾器（ROI-20）。
///
/// `reload` 重建总线会 `abort` 旧任务。若写帧已下发、任务却在收到应答前被取消，
/// 调用方必须看到「结果未知」，而不是 `BusClosed`——写路径把后者等价于
/// 「明确失败、可安全重试」，会造成重复写（累加 / 脉冲类点位直接出错）。
///
/// 正常路径通过 [`Self::respond`] 回送并置空 sender；`Drop` 只在异常取消时兜底。
struct TxnResponder {
    response_tx: Option<tokio::sync::oneshot::Sender<Result<Vec<u8>, CollectorError>>>,
    frame_sent: Arc<AtomicBool>,
    is_write: bool,
    /// U-7：取消写计数（与总体 `write_uncertain_total` 分开，回答
    /// 「这次 reload 打断了多少写」）。
    metrics: Arc<Metrics>,
}

impl TxnResponder {
    fn respond(&mut self, result: Result<Vec<u8>, CollectorError>) {
        if let Some(tx) = self.response_tx.take() {
            let _ = tx.send(result);
        }
    }
}

impl Drop for TxnResponder {
    fn drop(&mut self) {
        let Some(tx) = self.response_tx.take() else {
            return;
        };
        if self.is_write && self.frame_sent.load(Ordering::SeqCst) {
            self.metrics.inc_reload_cancelled_writes();
            let _ = tx.send(Err(CollectorError::WriteUncertain(
                "bus task cancelled after write frame sent".into(),
            )));
        }
        // 未下发（或读事务）：sender 直接 drop，调用方按 BusClosed 明确失败处理。
    }
}

/// 串口总线请求处理循环
///
/// 重连政策（C1 修复：快速失败 + 后台自愈）：
/// - `serial=None`（启动打开失败 / 热拔插 / 掉线）时**立即返回 `BusClosed`**，
///   不在业务事务路径中 sleep，避免调用方阻塞与 channel 拥堵（保留 Bug 2.2 的快速失败语义）
/// - 同时确保**后台重连任务**在跑：成功后把新句柄回填共享状态桥 `serial_state`，
///   下一笔事务即可直接使用，实现 S-3 热拔插自愈且无需重启进程
/// - 连续 8 次 IO 错误把句柄移出状态桥，交由上述自愈路径重开
/// - 修复(C-2)：`NoReply` / `ChecksumError` **不直接触发端口重开**。这两类是「设备级」
///   故障（从站掉线、地址配错、该点不存在、干扰导致 CRC 错），不是串口句柄损坏；
///   原实现连续 10 次即关闭整条 RS485 端口重开，会让同总线上的**所有**正常设备在断口
///   期间拿到 `BusClosed`。
/// - 自检帧（C-2 补充）：设备级失败累计 10 次后，向「最近一次成功通信的从站」发一帧
///   最小读（FC03 addr=0 count=1）。收到任何合法帧（含异常响应）说明端口收发链路正常，
///   静默归因于设备、不动端口；自检帧也收不到才判定端口级故障并重开，且两次
///   "自检驱动的重开"之间至少间隔 [`MIN_REOPEN_INTERVAL`]，避免反复关端口。
///   无历史成功从站时无法区分，保守不动端口。
pub(crate) async fn run_serial_bus_task(
    bus_config: BusConfig,
    serial: Option<SerialHandle>,
    mut rx: mpsc::Receiver<BusTransaction>,
    mut ctrl_rx: mpsc::Receiver<BusTransaction>,
    metrics: Arc<Metrics>,
    deps: BusDeps,
) {
    let name = bus_config.port_name.clone();
    let protocol = bus_config.protocol.clone();
    let timeout = Duration::from_millis(bus_config.timeout_ms);
    // 重试参数与 `BusManager` 的应答预算同源（DEFAULT_RETRY_* 定义在 bus/mod.rs），
    // 保证「重试循环最坏耗时」不会超出上层「等应答」预算（原 #107）。
    let retry_delay =
        Duration::from_millis(bus_config.retry_delay_ms.unwrap_or(DEFAULT_RETRY_DELAY_MS));
    let retry_count = bus_config.retry_count.unwrap_or(DEFAULT_RETRY_COUNT);
    let de_pin = bus_config.de_pin;
    // 决策表 #24：统一变体作为唯一解析点（显式 variant 优先，回退遗留 modbus_mode）。
    let modbus_mode = bus_config
        .effective_variant()
        .unwrap_or_else(|| "rtu".to_string());
    // 触发"移除句柄、交给后台自愈重开"的阈值（避免偶发 IO 振荡触发频繁重连）
    // 修复(C-2)：只有 IOError 参与该阈值判定；NoReply/ChecksumError 属设备级故障，
    // 不重开端口（原 10 次 NoReply 阈值已移除）。
    let reconnect_threshold: u32 = 8;

    // C1: 串口句柄共享状态桥。主循环在每笔事务前 `take()` 取走句柄、事务结束后放回；
    // 后台重连任务成功后仅在槽位为空时回填（避免覆盖主循环正在使用的句柄）。
    // 锁只在同步块内短暂持有，绝不跨 await，因此主循环与后台任务不会互相阻塞。
    let serial_state: Arc<StdMutex<Option<SerialHandle>>> = Arc::new(StdMutex::new(serial));
    // 单飞标志：保证同一时刻只有一个后台重连任务
    let reconnect_in_flight = Arc::new(AtomicBool::new(false));
    // 总线存活标志：主循环退出后让后台重连任务收尾，避免任务泄漏
    let bus_alive = Arc::new(AtomicBool::new(true));
    // 修复(审查#B3)：用 Drop guard 保证**任意**退出路径都置 alive=false。
    // 原实现只在主循环正常退出（rx 关闭）后执行 `bus_alive.store(false)`，而
    // `BusManager::drop` 走的是 `handle.abort()`——abort 会直接 drop 本 future，
    // 跳过循环后的语句，导致 alive 恒为 true：后台重连任务永不收尾，持续按退避
    // 反复打开串口并占用（旧总线释放后仍占着 /dev/ttyUSBx，新总线永远打不开）。
    // guard 在 future drop（含 abort）时执行 Drop，语义与「task 结束即收尾」一致。
    let _alive_guard = AliveGuard(Arc::clone(&bus_alive));
    let reconnect_ctx = ReconnectCtx {
        bus_config: bus_config.clone(),
        name: name.clone(),
        deps: deps.clone(),
        state: Arc::clone(&serial_state),
        in_flight: Arc::clone(&reconnect_in_flight),
        alive: Arc::clone(&bus_alive),
        metrics: Arc::clone(&metrics),
    };

    let mut consecutive_io_errors: u32 = 0;
    let mut consecutive_no_replies: u32 = 0;
    // 最近一次成功通信的 Modbus 从站地址：自检帧的探测目标
    let mut last_good_slave: Option<u8> = None;
    // 上次"自检判定端口损坏并重开"的时间，用于重开冷却
    let mut last_reopen_at: Option<tokio::time::Instant> = None;

    loop {
        // 控制（写）通道优先：`biased` 保证每轮先清写队列，
        // 写指令不必排在采集轮询之后（单笔采集最坏 3T+400ms）。
        let tx = tokio::select! {
            biased;
            Some(t) = ctrl_rx.recv() => t,
            t = rx.recv() => match t {
                Some(v) => v,
                None => break,
            },
        };
        if !super::prepare_dequeued(&tx, metrics.as_ref()) {
            tracing::debug!(
                "bus {}: drop stale transaction (caller canceled while queued)",
                name
            );
            continue;
        }
        // 取出手柄（锁仅在此同步块内持有，不跨 await）
        let mut handle = match lock_slot(&serial_state).take() {
            Some(h) => h,
            None => {
                // serial=None：快速失败 BusClosed（不阻塞业务路径），并确保后台重连任务在跑
                if tx.response_tx.send(Err(CollectorError::BusClosed)).is_err() {
                    tracing::warn!("bus {}: response channel closed (caller canceled)", name);
                }
                ensure_background_reconnect(&reconnect_ctx);
                continue;
            }
        };

        let mut result: Result<Vec<u8>, CollectorError>;
        let mut attempt: u32 = 0;
        // P0-1 调试直发：单次尝试、原样收字节（不分帧 / 不校验 / 不重试）。
        // 提前 `continue`，避免调试流量进入下方的生产侧故障分级（那会累计
        // 无应答/IO 计数并可能触发整条总线重开，把一次调试操作放大成采集中断）。
        if tx.raw {
            let frame_timeout = tx.timeout_ms.map(Duration::from_millis).unwrap_or(timeout);
            let raw_result = do_raw_transaction(
                &mut handle,
                &tx.request,
                frame_timeout,
                de_pin,
                deps.gpio.as_ref(),
                bus_config.baud_rate,
            )
            .await;
            if tx.response_tx.send(raw_result).is_err() {
                tracing::debug!(
                    "bus {}: raw response channel closed (caller canceled)",
                    name
                );
            }
            // 无论成功失败都把句柄放回——调试失败多为设备级（不回/CRC 错），
            // 端口本身由采集路径按原有阈值判定，这里不越权处置。
            *lock_slot(&serial_state) = Some(handle);
            continue;
        }
        // ROI-20：拆分事务字段。写路径通过 `TxnResponder` 回送结果，任务被
        // abort 且写帧已下发时由 Drop 兜底回送 WriteUncertain。
        let BusTransaction {
            request,
            response_tx,
            expected_count,
            timeout_ms,
            retry_allowed,
            frame_sent,
            ..
        } = tx;
        let mut responder = TxnResponder {
            response_tx: Some(response_tx),
            frame_sent: Arc::clone(&frame_sent),
            is_write: !retry_allowed,
            metrics: Arc::clone(&metrics),
        };
        // P1-2：本次事务的单次超时由 BusManager 按帧长/波特率算好后随事务下发
        // （大块读在低波特率下需要更长时间收帧）。拿不到时才退回总线配置值。
        let attempt_timeout = timeout_ms.map(Duration::from_millis).unwrap_or(timeout);
        loop {
            result = do_transaction(
                &mut handle,
                TransactionParams {
                    request: &request,
                    timeout: attempt_timeout,
                    protocol: &protocol,
                    expected_count,
                    de_pin,
                    modbus_mode: &modbus_mode,
                    baud: bus_config.baud_rate,
                    frame_sent: Some(&frame_sent),
                },
                deps.gpio.as_ref(),
                Some(metrics.as_ref()),
            )
            .await;
            match &result {
                Ok(_) => break,
                // 写事务（retry_allowed=false）失败后**不得重发**：
                // 写帧可能已被从站执行，重发会造成重复写（累加/脉冲类点位直接出错）。
                // 失败原样上报，由写路径转成 WriteUncertain 交给调用方判断。
                Err(e) if !retry_allowed => {
                    tracing::warn!(
                        "bus {} write transaction failed ({}), not resending (write outcome may be uncertain)",
                        name,
                        e
                    );
                    break;
                }
                Err(e)
                    if super::should_retry_transaction(e, attempt, retry_count, retry_allowed) =>
                {
                    // 修复(R-2)：控制通道有积压时放弃本次重试，立即回到主循环让写指令先下发。
                    // 重试只用于提升「偶发失败」的成功率，优先级低于现场控制指令的及时性；
                    // 否则一笔长采集事务的内部重试窗口会把写请求一起拖住
                    // （写延迟上界 = 单笔事务的 3T+400ms，biased 只解决出队顺序，解决不了在途）。
                    // 注意：这里只「不重试」，不动 ctrl_rx 里的消息，写指令仍会在下一轮
                    // select! 里被正常取出并执行。
                    if !ctrl_rx.is_empty() {
                        tracing::debug!(
                            "bus {} transaction attempt {}/{} failed ({}), control queue pending -> skip retry",
                            name,
                            attempt + 1,
                            retry_count,
                            e
                        );
                        break;
                    }
                    attempt += 1;
                    tracing::debug!(
                        "bus {} transaction attempt {}/{} failed ({}), retrying in {}ms",
                        name,
                        attempt,
                        retry_count,
                        e,
                        bus_config.retry_delay_ms.unwrap_or(DEFAULT_RETRY_DELAY_MS)
                    );
                    tokio::time::sleep(retry_delay).await;
                }
                Err(_) => break,
            }
        }

        // 故障分级判定（修复 C-2）：
        // - IOError 累计到阈值 → 端口级故障，直接丢句柄重开，无需自检
        // - NoReply/ChecksumError 累计到阈值 → 先发一帧自检帧复测端口，
        //   再区分「设备级」（只影响该从站）与「端口级」（整条总线收发链路）
        let mut keep_handle = true;
        let mut need_probe = false;
        match &result {
            Ok(_) => {
                consecutive_io_errors = 0;
                consecutive_no_replies = 0;
                // 记录最近一次成功通信的 Modbus 从站地址，作为后续自检帧的探测目标。
                // slave 0 是广播地址（从站不应答），不能作为探测目标。
                if protocol == "modbus" {
                    last_good_slave = request.first().copied().filter(|&s| s != 0);
                }
            }
            Err(CollectorError::IoError(_)) => {
                metrics.inc_serial_errors();
                consecutive_io_errors = consecutive_io_errors.saturating_add(1);
                if consecutive_io_errors >= reconnect_threshold {
                    tracing::warn!(
                        "bus {} serial may be broken ({} consecutive IO errors), removing handle for background reopen",
                        name,
                        consecutive_io_errors
                    );
                    consecutive_io_errors = 0;
                    keep_handle = false;
                }
            }
            // 设备级故障：从站掉线 / 地址配错 / 该点不存在 / 干扰致 CRC 错。
            // 不直接动端口，而是攒够次数后先自检，避免"一个坏设备拖垮整条总线"。
            Err(CollectorError::NoReply) | Err(CollectorError::ChecksumError) => {
                consecutive_no_replies = consecutive_no_replies.saturating_add(1);
                if consecutive_no_replies >= PROBE_AFTER_SILENT {
                    consecutive_no_replies = 0;
                    need_probe = true;
                }
            }
            _ => {}
        }

        // 先把结果回给调用方，再做（可能耗时一个 timeout 的）自检：
        // 自检只决定"是否重开端口"，不应拖延 caller 取响应。
        responder.respond(result);

        if need_probe {
            // 自检帧：对「最近一次成功通信的从站」做一次最小读（FC03 addr=0 count=1）。
            // 判定依据只是"有无合法响应帧"——即便从站回异常码（如非法数据地址），
            // 也证明端口收发链路正常。没有历史成功从站时无法区分，保守不动端口。
            match last_good_slave {
                Some(slave) => match build_probe_frame(slave) {
                    Some(frame) => {
                        let probe = do_transaction(
                            &mut handle,
                            TransactionParams {
                                request: &frame,
                                timeout,
                                protocol: &protocol,
                                expected_count: None,
                                de_pin,
                                modbus_mode: &modbus_mode,
                                baud: bus_config.baud_rate,
                                frame_sent: None,
                            },
                            deps.gpio.as_ref(),
                            Some(metrics.as_ref()),
                        )
                        .await;
                        match probe {
                            // 端口能收到合法帧 → 端口健康，静默是该从站自身的问题
                            Ok(_) => {
                                metrics.inc_bus_self_check_ok();
                                tracing::debug!(
                                    "bus {} self-check ok (slave {}), port healthy; silence is device-level",
                                    name,
                                    slave
                                );
                            }
                            // 自检帧也收不到任何合法帧 → 端口收发链路可疑，丢句柄重开
                            Err(e) => {
                                metrics.inc_bus_self_check_fail();
                                let now = tokio::time::Instant::now();
                                let cooled = last_reopen_at
                                    .map(|t| now.duration_since(t) >= MIN_REOPEN_INTERVAL)
                                    .unwrap_or(true);
                                if cooled {
                                    last_reopen_at = Some(now);
                                    keep_handle = false;
                                    metrics.inc_bus_port_reopen_by_self_check();
                                    tracing::warn!(
                                        "bus {} self-check to slave {} failed ({}), treating port as broken and reopening",
                                        name,
                                        slave,
                                        e
                                    );
                                } else {
                                    tracing::warn!(
                                        "bus {} self-check to slave {} failed ({}), reopen cooling down; keeping handle",
                                        name,
                                        slave,
                                        e
                                    );
                                }
                            }
                        }
                    }
                    // 本构建未编译 modbus 协议 → 无法构造自检帧，按「无已知良好从站」保守处理
                    None => {
                        metrics.inc_bus_self_check_skipped();
                        tracing::debug!(
                            "bus {} self-check skipped: modbus protocol not compiled in",
                            name
                        );
                    }
                },
                None => {
                    metrics.inc_bus_self_check_skipped();
                    tracing::debug!(
                        "bus {} self-check skipped: no known-good slave recorded yet",
                        name
                    );
                }
            }
        }

        if keep_handle {
            // 句柄放回状态桥，供下一笔事务使用
            *lock_slot(&serial_state) = Some(handle);
        } else {
            // 丢弃损坏句柄（串口随 drop 关闭），并确保后台重连任务在跑
            drop(handle);
            ensure_background_reconnect(&reconnect_ctx);
        }
    }

    // 主循环退出：`_alive_guard` 在 future 结束（正常返回或被 abort）时置
    // alive=false，让后台重连任务尽快收尾，避免任务泄漏。
    tracing::info!("bus {} task ended", name);
}

/// `do_transaction` 的入参集合：单次事务的可变参数较多且均为值/借用，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
struct TransactionParams<'a> {
    request: &'a [u8],
    timeout: Duration,
    protocol: &'a str,
    expected_count: Option<u16>,
    de_pin: Option<u32>,
    modbus_mode: &'a str,
    /// Bug 225: 总线波特率，用于按帧长推算写超时（原固定 100ms 对长帧/低速总线过短）
    baud: u32,
    /// ROI-20：写帧下发标记。事务一旦开始写帧即置 true；任务被取消时
    /// 写路径据此判断「可能已生效」。自检 / 读事务传 `None`。
    frame_sent: Option<&'a AtomicBool>,
}

/// 写超时下限（ms）：即便帧极短也保留 100ms，覆盖驱动缓冲排队与流控抖动
const MIN_WRITE_TIMEOUT_MS: u64 = 100;

/// 按「帧长 + 波特率」推算单帧写超时。
///
/// Bug 225: 原实现固定 `min(timeout, 100ms)`。100ms 在 9600bps 下只够传约 96 字节，
/// 而 Modbus FC16 批量写（100 个寄存器 ≈ 211 字节）在 9600bps 下线上时间约 220ms，
/// 在 2400/1200bps 的老表场景更会远超 100ms。此时波特率正常、硬件健康，却被判为
/// `serial write timeout`（IoError）→ 整笔事务失败并触发重试，而重试必然再次超时。
///
/// 现改为自适应：`max(100ms, 线上时间 × 3 + 50ms)`，并始终以事务总超时为上限
/// （保证一笔挂死的写不会占满/突破整个采集周期的预算）。
fn write_timeout_for(frame_len: usize, baud: u32, total: Duration) -> Duration {
    // 每字节 10 bit：1 起始位 + 8 数据位 + 1 停止位
    let bits = (frame_len.max(1) as u64).saturating_mul(10);
    // 波特率未配置 / 非法时按 300 守底，避免除零与荒谬的超大预算
    let baud = baud.max(300) as u64;
    // 向上取整的线上传输时间（ms）
    let wire_ms = bits.saturating_mul(1000).div_ceil(baud);
    // 线上时间 ×3（预留驱动缓冲与流控握手排队）+ 50ms 余量
    let budget_ms = wire_ms
        .saturating_mul(3)
        .saturating_add(50)
        .max(MIN_WRITE_TIMEOUT_MS);
    Duration::from_millis(budget_ms).min(total)
}

/// 执行单次事务：写请求 -> 按协议分帧读取响应
///
/// 写超时与读超时分离：
/// - 写使用自适应短超时（见 `write_timeout_for`），防止串口流控异常永久阻塞总线，
///   同时保证长帧 / 低速总线下的正常写入不被误判超时（Bug 225）
/// - 读使用完整 `timeout`，给设备足够响应时间
///
/// DE/RE 控制：写前拉高 DE，写后（flush 完成）拉低 DE 再开始读。
async fn do_transaction(
    serial: &mut SerialHandle,
    params: TransactionParams<'_>,
    gpio: &dyn GpioOutput,
    metrics: Option<&Metrics>,
) -> Result<Vec<u8>, CollectorError> {
    let TransactionParams {
        request,
        timeout,
        protocol,
        expected_count,
        de_pin,
        modbus_mode,
        baud,
        frame_sent,
    } = params;
    // Modbus ASCII 模式：将 RTU 请求转为 ASCII 发送
    let tx_data: Vec<u8> = if protocol == "modbus" && modbus_mode == "ascii" {
        rtu_to_ascii(request)
    } else {
        request.to_vec()
    };

    // Bug C15: 事务开始前清空接收缓冲，丢弃上一事务超时 / 被中断后残留的字节，
    // 否则本次响应解析会先读到旧帧，导致 CRC / 长度错位、恒定解析失败。
    let stale = serial.clear_input().await;
    if stale > 0 {
        // 陈旧字节非 0 = 上一事务没把帧收完整（超时/被中断）。这是「CRC 错」类
        // 现象最直接的证据：只看错误码无法区分「设备回帧不完整」和「总线噪声
        // 导致 CRC 算错」，所以既要计数上报（供 Prometheus 抓趋势），也要打日志。
        if let Some(m) = metrics {
            m.inc_bus_stale_bytes(stale as u64);
        }
        tracing::warn!(
            "serial: drained {} stale byte(s) before transaction (previous frame was incomplete)",
            stale
        );
    }

    // 写请求（含 DE 方向控制与自适应写超时）。
    // ROI-20：进入写帧前先标记「已尝试下发」——write_all 中途被取消时无法
    // 证明一个字节都没发出去，保守按「结果未知」处理。
    if let Some(sent) = frame_sent {
        sent.store(true, Ordering::SeqCst);
    }
    write_frame(serial, &tx_data, de_pin, gpio, baud, timeout).await?;

    // 用 FrameBuffer 累积数据，按协议提取完整帧
    let mut fb = FrameBuffer::new();
    // Bug 80: ASCII 模式响应可能跨多次 read 到达，原实现对分片数据直接
    // ascii_to_rtu 失败后把裸 ASCII 字节 push 进 RTU 帧缓冲，
    // 被 extract_modbus_frame 逐字节丢弃，导致完整响应永远拼不出来。
    // 改为独立 ascii_buf 累积，直到收到完整的 :...\r\n 帧再转 RTU。
    let mut ascii_buf: Vec<u8> = Vec::new();
    let mut buf = [0u8; 256];
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        match tokio::time::timeout(remaining, serial.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                // Modbus ASCII 模式：将读到的 ASCII 转回 RTU 再分帧
                if protocol == "modbus" && modbus_mode == "ascii" {
                    ascii_buf.extend_from_slice(&buf[..n]);
                    // 原 #112：硬上限保护。异常设备持续发无 CRLF 数据时，
                    // ascii_buf 会无限增长直到 OOM。
                    // 超限时不再整段 clear：溢出点通常落在「垃圾前缀 + 一个刚开始的半帧」
                    // 上，全丢会让下一个 read 到的数据缺帧头，LRC 永远对不上。改为保留最后
                    // 一个 ':' 及其后的字节（该半帧最长也超不过 ASCII_BUF_LIMIT，超了说明它
                    // 本身就不可能合法），找不到 ':' 才全丢。
                    // 计数与 warn 让现场能区分「设备灌噪声」与「正常的偶发 CRC 错帧」。
                    if ascii_buf.len() > ASCII_BUF_LIMIT {
                        let overflowed = ascii_buf.len();
                        let dropped = shrink_ascii_buf(&mut ascii_buf);
                        tracing::warn!(
                            "bus serial: ASCII buffer overflow ({overflowed} bytes > {ASCII_BUF_LIMIT}), dropped {dropped} bytes, kept {} bytes",
                            ascii_buf.len()
                        );
                        if let Some(m) = metrics {
                            m.inc_ascii_frame_dropped();
                        }
                    }
                    while let Some(crlf_pos) = ascii_buf.windows(2).position(|w| w == b"\r\n") {
                        let frame_end = crlf_pos + 2;
                        // 查找帧起始符 :
                        match ascii_buf[..frame_end].iter().position(|&b| b == b':') {
                            Some(0) => {
                                // 完整 ASCII 帧：尝试转 RTU（含 LRC 校验）
                                if let Some(rtu) = ascii_to_rtu(&ascii_buf[..frame_end]) {
                                    fb.push(&rtu);
                                } // LRC 错误，丢弃此帧
                                ascii_buf.drain(..frame_end);
                            }
                            Some(c) => {
                                // 丢弃起始符前的垃圾字节
                                ascii_buf.drain(..c);
                            }
                            None => {
                                // 帧内无起始符，丢弃到 \r\n
                                ascii_buf.drain(..frame_end);
                            }
                        }
                    }
                } else {
                    fb.push(&buf[..n]);
                }
                if let Some(frame) = extract_frame(&mut fb, protocol, expected_count) {
                    return Ok(frame);
                }
            }
            Ok(Err(e)) => {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    break;
                }
                return Err(CollectorError::IoError(e.to_string()));
            }
            Err(_) => break,
        }
    }

    if let Some(frame) = extract_frame(&mut fb, protocol, expected_count) {
        return Ok(frame);
    }

    if fb.is_empty() {
        Err(CollectorError::NoReply)
    } else {
        tracing::debug!(
            "incomplete frame ({} bytes, protocol={}): {:?}",
            fb.len(),
            protocol,
            fb.peek_all()
        );
        Err(CollectorError::NoReply)
    }
}

/// 调试直发模式的「帧尾判定」空闲间隔。
///
/// 设备不会告知帧长，只能靠线上静默切帧。取 50ms：即便 1200bps 下（1 字节 ≈ 8.3ms）
/// 也远大于 3.5 字符间隔，不会把同一帧切成两段；相对 1500ms 的默认等待又足够短，
/// 不会让「设备已回完」的调试请求白等一整个超时。
const RAW_IDLE_GAP: Duration = Duration::from_millis(50);

/// DE（方向控制）建立时间：DE 拉高到收发器真正驱动总线所需的时间。
///
/// 典型 RS485 收发器为 0.1~1ms，少数老转换器要求更长。建立时间不足时首字节
/// 会被吃掉，表现为「写成功但从站只收到半帧 / 报 CRC 错」这类偶发问题。
/// 取 1ms 保守值；仅在外部 DE 控制时生效（自动方向收发适配器无需等待）。
pub(super) const DE_SETUP_TIME: Duration = Duration::from_millis(1);

/// Modbus RTU 帧间静默（3.5 字符）。
///
/// 3.5 字符 = 35 bit / 波特率；向上取整且至少 1ms。9600bps ≈ 4ms，
/// 115200bps ≈ 1ms。De 拉低后必须保持这段静默再开始读，否则老表会丢掉回帧首字节。
pub(super) fn rtu_inter_frame_gap(baud: u32) -> Duration {
    let gap_ms = 35_000u64.div_ceil(u64::from(baud.max(300))).max(1);
    Duration::from_millis(gap_ms)
}

/// 写一帧（含 DE/RE 方向控制与按帧长/波特率自适应的写超时）。
///
/// 采集事务与调试直发共用，确保两条路径的物理层时序完全一致——
/// 若各写一份，DE 拉低时机一旦不同，就会出现「采集正常、调试丢首字节」这类
/// 极难复现的问题。
async fn write_frame(
    serial: &mut SerialHandle,
    data: &[u8],
    de_pin: Option<u32>,
    gpio: &dyn GpioOutput,
    baud: u32,
    timeout: Duration,
) -> Result<(), CollectorError> {
    // DE 拉高：进入发送模式
    if let Some(pin) = de_pin {
        if let Err(e) = gpio.set(pin, true) {
            tracing::warn!("set DE pin {} high failed: {}", pin, e);
        }
        // DE 建立时间：收发器从「接收」切到「发送」到总线真正可驱动需要一小段时间
        // （典型 0.1~1ms，少数老转换器要求更长）。建立时间不够时，头几个字节会被
        // 吃掉，表现为「写成功但从站只收到半帧 / 报 CRC 错」。
        // 只在外部 DE 控制时等待：自动方向收发的适配器无需此延时。
        tokio::time::sleep(DE_SETUP_TIME).await;
    }

    // 使用按帧长/波特率自适应的短写超时（Bug 225），
    // 既防止串口流控异常永久阻塞总线，又不误杀长帧在低速总线上的正常写入。
    let write_timeout = write_timeout_for(data.len(), baud, timeout);
    match tokio::time::timeout(write_timeout, serial.write(data)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            de_low(de_pin, gpio);
            return Err(CollectorError::IoError(e.to_string()));
        }
        Err(_) => {
            de_low(de_pin, gpio);
            return Err(CollectorError::IoError("serial write timeout".into()));
        }
    }

    // DE 拉低：进入接收模式（flush 已在 SerialHandle::write 中完成）
    de_low(de_pin, gpio);

    // 修复(第一批#2 R-7)：Modbus RTU 要求帧间至少 3.5 个字符的静默。
    // 部分老表在 DE 下降沿过早、紧接着就开始收时会吃掉回帧首字节，
    // 表现为「偶发 CRC 错 → 重试又成功」这类难复现问题。
    // 静默时长 = 3.5 字符 × 10 bit / 波特率，向上取整并至少 1ms
    // （9600bps ≈ 4ms，115200bps ≈ 1ms，对整体采集周期影响可忽略）。
    tokio::time::sleep(rtu_inter_frame_gap(baud)).await;

    Ok(())
}

/// 调试直发：写帧后把设备回的全部字节原样收齐（不分帧、不校验）。
///
/// 收帧策略：收到首批字节后，以 [`RAW_IDLE_GAP`] 的静默作为帧尾；
/// 一个字节都没收到时则等满整个 `timeout`（与现场「设备到底回没回」的直觉一致，
/// 也让 latency_ms 反映真实等待）。
async fn do_raw_transaction(
    serial: &mut SerialHandle,
    request: &[u8],
    timeout: Duration,
    de_pin: Option<u32>,
    gpio: &dyn GpioOutput,
    baud: u32,
) -> Result<Vec<u8>, CollectorError> {
    // 与采集事务一致：先清空上一事务残留，否则会把旧帧字节当成本次响应
    let _ = serial.clear_input().await;
    write_frame(serial, request, de_pin, gpio, baud, timeout).await?;

    let mut out: Vec<u8> = Vec::new();
    let mut buf = [0u8; 256];
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        // 已收到数据 → 只等一个静默间隔判帧尾；尚未收到 → 等满剩余时间
        let wait = if out.is_empty() {
            remaining
        } else {
            remaining.min(RAW_IDLE_GAP)
        };
        match tokio::time::timeout(wait, serial.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => out.extend_from_slice(&buf[..n]),
            Ok(Err(e)) => {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    break;
                }
                return Err(CollectorError::IoError(e.to_string()));
            }
            // 静默间隔到期：帧结束（或本来就没有任何应答）
            Err(_) => break,
        }
    }

    if out.is_empty() {
        Err(CollectorError::NoReply)
    } else {
        Ok(out)
    }
}

/// 拉低 DE 引脚（接收模式）
fn de_low(de_pin: Option<u32>, gpio: &dyn GpioOutput) {
    if let Some(pin) = de_pin {
        if let Err(e) = gpio.set(pin, false) {
            tracing::warn!("set DE pin {} low failed: {}", pin, e);
        }
    }
}

#[cfg(test)]
mod tests;
