//! Modbus TCP 总线任务循环
//!
//! 通过 TCP 连接与 Modbus TCP 设备通信。
//!
//! P1: 接入 TransactionPool 实现单 socket 多事务并发。
//! - 主循环收到 BusTransaction 后 spawn 独立 task 提交到 pool，不阻塞主循环
//! - pool 内部用 Modbus TCP transaction ID 区分响应归属，单 socket 上同时 in-flight 多个事务
//! - 串行模式：N 个事务总耗时 = sum(t_i)；并发模式：N 个事务总耗时 = max(t_i)
//!
//! 保留：MBAP 帧封装/解封（在 pool 内完成）、指数退避重连、retry_count 对 Timeout/Busy 的重试。

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::config::{BusConfig, ReconnectBackoffConfig};
use crate::engine::metrics::Metrics;
use crate::engine::util::crc::crc16;
use crate::south::protocol::modbus::transaction::{
    ModbusRequest, TransactionError, TransactionPool,
};
use crate::south::protocol::CollectorError;

use super::BusTransaction;

/// Modbus TCP 总线 task：通过 TCP 连接与 Modbus TCP 设备通信
///
/// 帧格式转换：
/// - 入站 BusTransaction.request = RTU [slave][fc][data...][crc_lo][crc_hi]
/// - 解析为 ModbusRequest { slave_id, function_code, pdu=data, deadline } 提交 pool
/// - pool 响应 PDU = [fc][data...]（unit_id 之后的所有字节）
/// - 封装回 RTU = [slave][pdu_resp...][crc_lo][crc_hi] 回复 caller
///
/// S3: 重连指数退避（1s -> 2s -> 5s -> 10s -> 30s），避免网络故障时反复 connect 打满 CPU
/// P1: 单 socket 多事务并发——每个 BusTransaction spawn 一个独立 task 提交到 pool，
///     主循环不阻塞等待响应，可继续接收下一个 tx。pool 内 reader_loop 按 tx_id 路由响应。
pub(crate) async fn bus_task_tcp(
    bus_config: BusConfig,
    tcp_addr: String,
    mut rx: mpsc::Receiver<BusTransaction>,
    metrics: Arc<Metrics>,
) {
    let name = bus_config.port_name.clone();
    let timeout = Duration::from_millis(bus_config.timeout_ms);
    let retry_count = bus_config.retry_count.unwrap_or(2);
    let retry_delay = Duration::from_millis(bus_config.retry_delay_ms.unwrap_or(200));
    // 下限 1：与 `south/bus/mod.rs` 的 `.max(1)` 保持一致。
    // 若为 0，`TransactionPool::with_stream(0, …)` 后每个事务都判定 `active.len() >= 0`
    // 而恒返回 Busy，该 TCP 总线会静默全线失效。
    let max_concurrent = bus_config.tcp_concurrent.unwrap_or(8).max(1);

    // S3: 重连退避序列由 bus_config.reconnect_backoff 参数化（默认 1/2/5/10/30 秒、无抖动）
    let mut reconnect_failures: u32 = 0;
    // 是否已经成功连接过：仅用于区分「首次连接」与「断线重连」，后者计入重连次数
    let mut ever_connected = false;

    // P1: 共享 pool 引用。None 表示未连接或已断开。
    // 主循环负责（重）连接与构造 pool；spawn 的 submit task 在 Disconnected/Io 时
    // 将 pool 置 None，触发主循环下次循环重连。
    let pool_slot: Arc<Mutex<Option<Arc<TransactionPool>>>> = Arc::new(Mutex::new(None));
    let mut in_flight: Vec<JoinHandle<()>> = Vec::new();

    while let Some(tx) = rx.recv().await {
        // P0-1 调试直发：走一条独立短连接，原样收发。
        //
        // 为什么不复用下方 pool：pool 只接受「RTU → ModbusRequest → MBAP」转换后的
        // 请求，任意原始帧（回放录制的 MBAP 帧、DL/T645 帧等）无法承载；
        // 且调试会话按契约独占该总线（采集占用时上层直接返回 bus_busy），
        // 因此额外开一条短连接不会与采集争用同一 socket 的事务号空间。
        if tx.raw {
            let frame_timeout = tx.timeout_ms.map(Duration::from_millis).unwrap_or(timeout);
            let res = raw_tcp_exchange(&tcp_addr, &tx.request, frame_timeout).await;
            if tx.response_tx.send(res).is_err() {
                tracing::debug!("modbus tcp {}: raw response channel closed", tcp_addr);
            }
            continue;
        }
        // P1+S3: 懒连接 / 断线重连（带指数退避）
        // 关键修复：sleep 与 connect 移出 lock 作用域，避免持锁 await 导致主循环
        // 在指数退避期间（最长 30s）卡死，spawn 的 submit task 也无法置 pool=None。
        let need_reconnect = pool_slot.lock().await.is_none();
        if need_reconnect {
            // S3: 重连前先 sleep 退避（不持锁）
            if reconnect_failures > 0 {
                // 默认 jitter_ratio=0.0 时无抖动，与历史硬编码序列等价；
                // 仅在配置了正抖动比例时才叠加随机抖动。
                let delay_secs = if bus_config.reconnect_backoff.jitter_ratio > 0.0 {
                    bus_config
                        .reconnect_backoff
                        .delay_secs(reconnect_failures, ReconnectBackoffConfig::time_rand01())
                } else {
                    bus_config
                        .reconnect_backoff
                        .base_delay_secs(reconnect_failures)
                };
                let backoff = Duration::from_secs_f64(delay_secs);
                tracing::warn!(
                    "modbus tcp {} reconnect attempt {} (backoff {:?})",
                    tcp_addr,
                    reconnect_failures,
                    backoff
                );
                tokio::time::sleep(backoff).await;
            }
            // connect（不持锁，避免阻塞 spawn task 设置 pool=None）
            // Bug C14: connect 无超时——TCP SYN 被丢弃（设备掉电 / 网线半插 / 对端无响应）时
            // connect 会长期挂起（Linux 默认约 2 分钟），期间循环无法进入退避与重试，
            // 采集对该总线全线 stall。加 10s 超时，超时按 connect 失败处理走退避。
            let connected =
                match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&tcp_addr))
                    .await
                {
                    Ok(res) => res.map_err(|e| e.to_string()),
                    Err(_) => Err("connect timeout after 10s".to_string()),
                };
            match connected {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    let pool = TransactionPool::with_stream(max_concurrent, s);
                    // 仅在赋值时短暂持锁
                    *pool_slot.lock().await = Some(pool);
                    reconnect_failures = 0;
                    if ever_connected {
                        metrics.inc_bus_reconnect(&name);
                    }
                    ever_connected = true;
                    tracing::info!("modbus tcp connected to {}", tcp_addr);
                }
                Err(e) => {
                    reconnect_failures = reconnect_failures.saturating_add(1);
                    tracing::warn!(
                        "modbus tcp connect {} failed (attempt {}): {}",
                        tcp_addr,
                        reconnect_failures,
                        e
                    );
                    if tx
                        .response_tx
                        .send(Err(CollectorError::IoError(e.to_string())))
                        .is_err()
                    {
                        tracing::warn!(
                            "modbus tcp {}: response channel closed (caller canceled)",
                            tcp_addr
                        );
                    }
                    continue;
                }
            }
        }

        // 取当前 pool 的 Arc clone（不持锁跨 await）
        let pool = match pool_slot.lock().await.clone() {
            Some(p) => p,
            None => {
                // 理论不可达：上面已确保 Some。兜底处理。
                let _ = tx
                    .response_tx
                    .send(Err(CollectorError::IoError("pool not available".into())));
                continue;
            }
        };
        let pool_slot_clone = pool_slot.clone();
        // 取 slave_id 用于构造 RTU 响应帧
        let slave_id = tx.request.first().copied().unwrap_or(0);

        // P1: spawn 并发处理该 tx
        // Bug-R2 防御性分析：spawn_catch_unwind 包裹的 future panic 时会被吸收，
        // 外层 future 正常 resolve——但 panic 时的栈展开会触发所有 owned 值的
        // Drop，包括 `tx.response_tx`（oneshot::Sender）。Sender drop 会让
        // 对端 `resp_rx.await` 立即返回 `RecvError` → caller 端 `mod.rs:129` 的
        // `map_err(|_| CollectorError::BusClosed)` 把它转成 `BusClosed`。
        //
        // 因此**任何 panic 路径**都不会让 caller 永久 hang——caller 至少能在
        // 自身 timeout 内拿到 `BusClosed`/`Timeout` 错误并向上汇报。
        // 当前三处显式 `response_tx.send()` 仅是为 caller 提供更精确的语义
        // （disconnected / parse_rtu 错误等），不依赖它们也能保证 caller 不挂死。
        let handle = crate::engine::supervision::spawn_catch_unwind("tcp_txn", async move {
            // 解析 RTU -> ModbusRequest
            let req = match parse_rtu_request(&tx.request, timeout) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.response_tx.send(Err(e));
                    return;
                }
            };

            // retry 循环（仅对 Timeout/Busy 重试；Disconnected/Io 触发重连）
            let mut attempt: u32 = 0;
            loop {
                match pool.submit(req.clone()).await {
                    Ok(pdu_resp) => {
                        let rtu = build_rtu_response(slave_id, &pdu_resp);
                        let _ = tx.response_tx.send(Ok(rtu));
                        return;
                    }
                    Err(TransactionError::Timeout(_)) | Err(TransactionError::Busy)
                        if attempt < retry_count =>
                    {
                        attempt += 1;
                        tokio::time::sleep(retry_delay).await;
                    }
                    Err(TransactionError::Disconnected) | Err(TransactionError::Io(_)) => {
                        // 触发主循环重连。
                        // 修复(审查#B5)：仅当槽位仍指向本任务使用的 pool 时才清空。
                        // 旧连接上的在途事务其 Disconnected/Io 可能在数百 ms 后才返回，
                        // 此时主循环往往已重连并写入**新** pool；原实现无条件置 None，
                        // 会把刚建好的新连接一并丢弃 → 反复重连（重连风暴）。
                        // 用 Arc::ptr_eq 做连接代次校验，只清理属于本代次的 pool。
                        let mut slot = pool_slot_clone.lock().await;
                        if slot.as_ref().is_some_and(|p| Arc::ptr_eq(p, &pool)) {
                            *slot = None;
                        }
                        drop(slot);
                        let _ = tx
                            .response_tx
                            .send(Err(CollectorError::IoError("disconnected".into())));
                        return;
                    }
                    Err(e) => {
                        let _ = tx.response_tx.send(Err(map_txn_err(e)));
                        return;
                    }
                }
            }
        });
        in_flight.push(handle);

        // 清理已完成的 in_flight handles（避免 Vec 无限增长）
        in_flight.retain(|h| !h.is_finished());
    }

    // shutdown：abort 所有 in-flight，关闭 pool 的 reader
    for h in in_flight.drain(..) {
        h.abort();
    }
    let guard = pool_slot.lock().await;
    if let Some(pool) = guard.as_ref() {
        pool.shutdown();
    }
    tracing::info!("modbus tcp task {} ended", name);
}

/// 调试直发的帧尾判定空闲间隔（与串口侧 [`super::serial_task`] 同口径 50ms）。
const RAW_IDLE_GAP: Duration = Duration::from_millis(50);

/// 调试直发：与 `tcp_addr` 建一条**短连接**，原样写帧、原样收齐字节后立即断开。
///
/// 语义与 [`super::BusManager::raw_transaction`] 一致：不分帧、不校验、不重试。
/// 连接失败统一映射为 `IoError`（上层据此报 `bus_offline`），
/// 连接与读写均受 `timeout` 约束（连接额外给 5s 上限，避免 SYN 黑洞拖死调试请求）。
async fn raw_tcp_exchange(
    tcp_addr: &str,
    request: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, CollectorError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let connect_timeout = timeout
        .max(Duration::from_millis(500))
        .min(Duration::from_secs(5));
    let mut stream = match tokio::time::timeout(connect_timeout, TcpStream::connect(tcp_addr)).await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(CollectorError::IoError(e.to_string())),
        Err(_) => {
            return Err(CollectorError::IoError(format!(
                "connect timeout after {:?}",
                connect_timeout
            )))
        }
    };
    let _ = stream.set_nodelay(true);

    match tokio::time::timeout(timeout, stream.write_all(request)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(CollectorError::IoError(e.to_string())),
        Err(_) => return Err(CollectorError::Timeout(timeout)),
    }
    if let Ok(Err(e)) = tokio::time::timeout(timeout, stream.flush()).await {
        return Err(CollectorError::IoError(e.to_string()));
    }

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
        match tokio::time::timeout(wait, stream.read(&mut buf)).await {
            Ok(Ok(0)) => break, // 对端已关闭连接
            Ok(Ok(n)) => out.extend_from_slice(&buf[..n]),
            Ok(Err(e)) => return Err(CollectorError::IoError(e.to_string())),
            Err(_) => break,
        }
    }
    // 短连接用完即弃（调试语义即「一次一连接」），显式关闭避免半开堆积
    let _ = stream.shutdown().await;

    if out.is_empty() {
        Err(CollectorError::NoReply)
    } else {
        Ok(out)
    }
}

/// RTU 帧 -> ModbusRequest
/// RTU: [slave_id][fc][data...][crc_lo][crc_hi]
/// ModbusRequest.pdu = data（不含 slave_id / fc，也不含尾部 2 字节 CRC）。
fn parse_rtu_request(rtu: &[u8], deadline: Duration) -> Result<ModbusRequest, CollectorError> {
    if rtu.len() < 4 {
        return Err(CollectorError::InvalidFrame("rtu request too short".into()));
    }
    let slave_id = rtu[0];
    let function_code = rtu[1];
    // D4 修复: pdu 必须取 rtu[2..len-2]——即去掉首字节 slave_id、次字节 function_code
    // 与尾部 2 字节 CRC 后的净数据。`build_read_request` 产出
    // [slave][fc][addr_hi][addr_lo][count_hi][count_lo][crc_lo][crc_hi]，
    // 故 pdu = [addr_hi, addr_lo, count_hi, count_lo]。`build_mbap_frame` 会补回
    // [unit_id=slave][fc] 前缀，故此处既不能带入 slave/fc（否则线上 fc 重复），
    // 也不能漏掉 data 首字节（原 rtu[3..] 实现即漏掉 addr_hi，导致请求 PDU 短一字节、
    // 设备端寄存器地址/数量错位）。
    let pdu = rtu[2..rtu.len() - 2].to_vec();
    Ok(ModbusRequest {
        slave_id,
        function_code,
        pdu,
        deadline,
    })
}

/// PDU 响应 -> RTU 帧
/// 输入 pdu_resp = [fc][data...]（pool 返回的 unit_id 之后所有字节）
/// 输出 RTU = [slave_id][fc][data...][crc_lo][crc_hi]
fn build_rtu_response(slave_id: u8, pdu_resp: &[u8]) -> Vec<u8> {
    let mut rtu = Vec::with_capacity(1 + pdu_resp.len() + 2);
    rtu.push(slave_id);
    rtu.extend_from_slice(pdu_resp);
    let crc = crc16(&rtu);
    rtu.push((crc & 0xFF) as u8);
    rtu.push((crc >> 8) as u8);
    rtu
}

/// TransactionError -> CollectorError
fn map_txn_err(e: TransactionError) -> CollectorError {
    match e {
        TransactionError::Disconnected => CollectorError::IoError("disconnected".into()),
        TransactionError::Timeout(_) => CollectorError::NoReply,
        TransactionError::Malformed(s) => CollectorError::InvalidFrame(s),
        TransactionError::Busy => CollectorError::NoReply,
        TransactionError::Io(e) => CollectorError::IoError(e.to_string()),
        TransactionError::NotBound => CollectorError::IoError("pool not bound".into()),
    }
}

#[cfg(test)]
mod tests;
