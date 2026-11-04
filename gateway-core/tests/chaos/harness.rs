//! 故障注入测试 harness（T1.3）
//!
//! 所有"测试桩"必须是真实组件（真 TCP、真 MBAP、真 MQTT 协议），
//! mock 仅在覆盖不到真实路径时用，且必须明确标注。
//!
//! 每个场景用 `#[tokio::test]` 标记，独立运行；用 `--test-threads=1`
//! 串行化避免共享资源互踩。

#![allow(dead_code)]

use std::sync::Arc;
#[cfg(feature = "opcua-server")]
use std::time::Duration;

use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::model::DataPoint;
use gateway_core::north::mqtt::outbox::{OutboxStore, OutgoingMsg};
use gateway_core::queue::DataQueue;

/// 构造一个真实 DataQueue（memory 后端）。
pub async fn real_memory_queue() -> Arc<DataQueue> {
    let storage = Arc::new(gateway_core::queue::MemoryStorage::new());
    let queue = DataQueue::with_storage(storage).expect("memory queue init");
    Arc::new(queue)
}

/// 构造一个真实 SQLite 后端 DataQueue，DB 落在 tempdir。
///
/// 关键点：每个测试放进独立 tempdir，避免 SQLite 文件 lock
/// 与上次测试残留数据相互干扰。返回 (queue, tempdir) —— tempdir
/// 必须在测试结束前一直存活，否则 SQLite 句柄关闭前文件被删会报错。
pub fn real_sqlite_queue() -> (DataQueue, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("queue.sqlite");
    let metrics = Arc::new(Metrics::new());
    let queue = DataQueue::new(db_path.to_str().unwrap(), metrics).expect("sqlite queue init");
    (queue, dir)
}

/// 构造一个 DataPoint。chaos 场景只需要 id/value/quality 区分优先级。
pub fn make_dp(id: &str, value: f64, quality: gateway_core::engine::model::Quality) -> DataPoint {
    let now = DataPoint::now();
    DataPoint {
        point_id: id.into(),
        device_id: "dev1".into(),
        value,
        raw_value: value,
        raw_type: "u16".into(),
        quality,
        quality_reason: "".into(),
        value_text: None,
        timestamp: now,
        source_ts: now,
        stale_count: 0,
    }
}

/// 构造一个真实 OutboxStore（独立 SQLite 文件，落 tempdir）。
///
/// 关键点：每次测试用独立 tempdir + db 文件，避免 SQLite lock/残留数据
/// 干扰。返回 (store, tempdir) —— tempdir 必须活到测试结束。
pub fn real_outbox() -> (OutboxStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("outbox tempdir");
    let db_path = dir.path().join("outbox.sqlite");
    let store = OutboxStore::open(&db_path).expect("outbox open");
    (store, dir)
}

/// 构造一个 OutgoingMsg。OutboxStore 关心 data_id/data_ids/topic/payload/qos/protocol；
/// 场景测试用 data_id 作为唯一标识（与队列 id 对应），topic/payload 可任意。
pub fn make_outbox_msg(data_id: i64, topic: &str, payload: &[u8]) -> OutgoingMsg {
    OutgoingMsg {
        data_id,
        data_ids: vec![data_id],
        topic: topic.to_string(),
        payload: payload.to_vec(),
        qos: 1,
        protocol: "mqtt".to_string(),
    }
}

// ============================================================
// Modbus TCP 真从站桩
// ============================================================

/// 真实 Modbus TCP 测试桩：起本地 TcpListener + 一组从站寄存器，
/// 接受连接后读 MBAP 帧、按 FC 回响应，可被配置为"读到 N 个请求后关连接"
/// 模拟断线。**不是 mock**——它就是真 TCP + 真 MBAP 协议编解码。
pub struct ModbusTestServer {
    pub addr: String,
    _listener: Arc<tokio::net::TcpListener>,
    _accept_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    _shutdown: tokio::sync::watch::Sender<bool>,
    registers: Arc<parking_lot::Mutex<Vec<u16>>>,
    drop_after: Arc<parking_lot::Mutex<Option<usize>>>,
    request_count: Arc<parking_lot::Mutex<usize>>,
    /// C1.3 网络切换场景：false 时对收到的请求「丢弃不回」并关闭连接，
    /// 模拟承载网络中断（TCP 端口仍在监听，但流量被吞）。true 时正常应答。
    network_ok: Arc<std::sync::atomic::AtomicBool>,
}

impl ModbusTestServer {
    pub async fn start() -> std::io::Result<Self> {
        let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await?);
        let addr = listener.local_addr()?.to_string();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let registers = Arc::new(parking_lot::Mutex::new(vec![0u16; 1024]));
        let drop_after = Arc::new(parking_lot::Mutex::new(None));
        let request_count = Arc::new(parking_lot::Mutex::new(0usize));
        let network_ok = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let listener_for_task = listener.clone();
        let regs_for_task = registers.clone();
        let drop_for_task = drop_after.clone();
        let count_for_task = request_count.clone();
        let net_for_task = network_ok.clone();
        let handle = tokio::spawn(async move {
            run_modbus_accept_loop(
                listener_for_task,
                regs_for_task,
                drop_for_task,
                count_for_task,
                net_for_task,
                shutdown_rx,
            )
            .await;
        });
        Ok(Self {
            addr,
            _listener: listener,
            _accept_handle: parking_lot::Mutex::new(Some(handle)),
            _shutdown: shutdown_tx,
            registers,
            drop_after,
            request_count,
            network_ok,
        })
    }

    pub fn set_register(&self, idx: usize, val: u16) {
        let mut regs = self.registers.lock();
        if idx < regs.len() {
            regs[idx] = val;
        }
    }

    pub fn set_drop_after(&self, n: usize) {
        *self.drop_after.lock() = Some(n);
    }

    pub fn request_count(&self) -> usize {
        *self.request_count.lock()
    }

    /// C1.3 网络切换场景：`false` = 承载网络中断（请求被吞、连接被关）；
    /// `true` = 网络恢复（正常应答，客户端自动重连后即可继续采集）。
    pub fn set_network_ok(&self, ok: bool) {
        self.network_ok
            .store(ok, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for ModbusTestServer {
    fn drop(&mut self) {
        let _ = self._shutdown.send(true);
        if let Some(h) = self._accept_handle.lock().take() {
            h.abort();
        }
    }
}

async fn run_modbus_accept_loop(
    listener: Arc<tokio::net::TcpListener>,
    registers: Arc<parking_lot::Mutex<Vec<u16>>>,
    drop_after: Arc<parking_lot::Mutex<Option<usize>>>,
    request_count: Arc<parking_lot::Mutex<usize>>,
    network_ok: Arc<std::sync::atomic::AtomicBool>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => { if *shutdown_rx.borrow() { break; } }
            res = listener.accept() => {
                let (mut socket, _) = match res { Ok(x) => x, Err(_) => break };
                let r = registers.clone();
                let d = drop_after.clone();
                let c = request_count.clone();
                let net = network_ok.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    loop {
                        if let Some(n) = *d.lock() { if *c.lock() >= n { return; } }
                        let mut header = [0u8; 7];
                        match socket.read_exact(&mut header).await { Ok(_) => {}, Err(_) => return }
                        let tx_id = u16::from_be_bytes([header[0], header[1]]);
                        if u16::from_be_bytes([header[2], header[3]]) != 0 { return; }
                        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
                        let mut pdu = vec![0u8; length.saturating_sub(1)];
                        if !pdu.is_empty() && socket.read_exact(&mut pdu).await.is_err() { return; }
                        // C1.3 网络切换：断网期间「吞掉请求」并关闭连接，让客户端走重连路径
                        if !net.load(std::sync::atomic::Ordering::SeqCst) {
                            return;
                        }
                        let fc = pdu.first().copied().unwrap_or(0);
                        let response_pdu = match fc {
                            0x03 if pdu.len() >= 5 => {
                                let addr = u16::from_be_bytes([pdu[1], pdu[2]]) as usize;
                                let qty = u16::from_be_bytes([pdu[3], pdu[4]]) as usize;
                                let regs = r.lock();
                                let end = (addr + qty).min(regs.len());
                                let mut bytes = Vec::with_capacity(qty * 2);
                                for i in addr..end { bytes.extend_from_slice(&regs[i].to_be_bytes()); }
                                while bytes.len() < qty * 2 { bytes.extend_from_slice(&0u16.to_be_bytes()); }
                                let mut resp = vec![0x03, (qty * 2) as u8];
                                resp.extend_from_slice(&bytes);
                                resp
                            }
                            _ => vec![0x80 | fc, 0x01],
                        };
                        let resp_len = (1 + response_pdu.len()) as u16;
                        let mut out = Vec::with_capacity(7 + 1 + response_pdu.len());
                        out.extend_from_slice(&tx_id.to_be_bytes());
                        out.extend_from_slice(&0u16.to_be_bytes());
                        out.extend_from_slice(&resp_len.to_be_bytes());
                        out.push(pdu.first().copied().unwrap_or(0));
                        out.extend_from_slice(&response_pdu);
                        if socket.write_all(&out).await.is_err() { return; }
                        *c.lock() += 1;
                    }
                });
            }
        }
    }
}

/// 起真 Modbus TCP 从站 + 真实 `TransactionPool`。
///
/// 直接引用 `south::protocol::modbus::transaction`（协议插件），故随 `modbus`
/// feature 编译；协议裁剪构建下无该模块，整个函数一并剥离。
#[cfg(feature = "modbus")]
pub async fn real_modbus_pool() -> (
    std::sync::Arc<gateway_core::south::protocol::modbus::transaction::TransactionPool>,
    ModbusTestServer,
) {
    let server = ModbusTestServer::start()
        .await
        .expect("modbus test server bind");
    let stream = tokio::net::TcpStream::connect(&server.addr)
        .await
        .expect("connect modbus test server");
    let pool =
        gateway_core::south::protocol::modbus::transaction::TransactionPool::with_stream(8, stream);
    (pool, server)
}

// ============================================================
// MQTT 3.1.1 真 broker 桩
// ============================================================

pub type ReceivedPublish = (String, Vec<u8>, u16, u8);
type ReceivedBuffer = Arc<parking_lot::Mutex<Vec<ReceivedPublish>>>;

pub struct MqttBroker {
    pub addr: String,
    listener: Arc<tokio::net::TcpListener>,
    shutdown: tokio::sync::watch::Sender<bool>,
    _handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub received: ReceivedBuffer,
    /// 收到 N 个 PUBLISH 后主动关闭当前连接（None = 永不主动断）。
    /// 关闭后 broker 仍监听端口，可接受新连接（重连场景）。
    /// 通过 `set_drop_after(n)` 设置，setter 会同时写 self.drop_after
    /// 和后台共享的 _drop_after_arc（同一 Arc 解引用）。
    ///
    /// 计数语义：**跨连接全局累加**（与 `ModbusTestServer::request_count`
    /// 一致），且**只触发一次**——首次达到阈值时断开当前连接，之后重连
    /// 进来的连接正常服务。
    ///
    /// 为何必须"只触发一次"：若按连接重置计数，每次重连后的第 1 条
    /// PUBLISH 都会触发断连，客户端永远等不到 PUBACK，publish 失败路径
    /// 把数据 release 回队列 → 队列永不排空（场景 26 的排空断言必然失败）。
    /// 一次性语义才能复现真实的"瞬时网络抖动"：断一次 → 重连 → 重发成功。
    pub drop_after: Arc<parking_lot::Mutex<Option<usize>>>,
    /// 断连是否已经触发过（保证 drop_after 只生效一次）。
    pub drop_fired: Arc<std::sync::atomic::AtomicBool>,
    /// 已接受连接数（用于"重连后第二次连接是否建立"断言）
    pub connection_count: Arc<parking_lot::Mutex<usize>>,
    _drop_after_arc: Arc<parking_lot::Mutex<Option<usize>>>,
    _connection_count_arc: Arc<parking_lot::Mutex<usize>>,
}

impl MqttBroker {
    pub async fn start() -> std::io::Result<Self> {
        let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await?);
        let addr = listener.local_addr()?.to_string();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let received: ReceivedBuffer = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let drop_after_arc = Arc::new(parking_lot::Mutex::new(None));
        let drop_fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let connection_count_arc = Arc::new(parking_lot::Mutex::new(0usize));
        let listener_clone = listener.clone();
        let received_clone = received.clone();
        let drop_after_clone = drop_after_arc.clone();
        let drop_fired_clone = drop_fired.clone();
        let count_clone = connection_count_arc.clone();
        let handle = tokio::spawn(async move {
            run_mqtt_accept(
                listener_clone,
                shutdown_rx,
                received_clone,
                drop_after_clone,
                drop_fired_clone,
                count_clone,
            )
            .await;
        });
        Ok(Self {
            addr,
            listener,
            shutdown: shutdown_tx,
            _handle: parking_lot::Mutex::new(Some(handle)),
            received,
            // self.drop_after / self.connection_count 持有 Arc clone，
            // 与 _drop_after_arc / _connection_count_arc（task 持有）共享同一份
            // Mutex。set_drop_after 写 self.drop_after，task 立即看到。
            drop_after: drop_after_arc.clone(),
            connection_count: connection_count_arc.clone(),
            drop_fired,
            _drop_after_arc: drop_after_arc,
            _connection_count_arc: connection_count_arc,
        })
    }

    pub fn set_drop_after(&self, n: usize) {
        *self.drop_after.lock() = Some(n);
    }

    /// 已处理 PUBLISH 计数（跨连接累加）
    pub fn publish_count(&self) -> usize {
        self.received.lock().len()
    }

    /// 等 broker 接受至少 n 个 TCP 连接（重连场景用）
    pub async fn wait_for_connections(&self, n: usize, timeout: std::time::Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if *self.connection_count.lock() >= n {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        false
    }

    /// 等 broker 收到至少 n 个 PUBLISH（跨连接累加）
    pub async fn wait_for_publishes(&self, n: usize, timeout: std::time::Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if self.received.lock().len() >= n {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        false
    }
}

impl Drop for MqttBroker {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn run_mqtt_accept(
    listener: Arc<tokio::net::TcpListener>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    received: ReceivedBuffer,
    drop_after: Arc<parking_lot::Mutex<Option<usize>>>,
    drop_fired: Arc<std::sync::atomic::AtomicBool>,
    connection_count: Arc<parking_lot::Mutex<usize>>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => { if *shutdown_rx.borrow() { break; } }
            res = listener.accept() => {
                let (socket, _) = match res { Ok(x) => x, Err(_) => break };
                *connection_count.lock() += 1;
                let r = received.clone();
                let d = drop_after.clone();
                let f = drop_fired.clone();
                tokio::spawn(async move {
                    let _ = handle_mqtt_connection(socket, r, d, f).await;
                });
            }
        }
    }
}

async fn handle_mqtt_connection(
    mut socket: tokio::net::TcpStream,
    received: ReceivedBuffer,
    drop_after: Arc<parking_lot::Mutex<Option<usize>>>,
    drop_fired: Arc<std::sync::atomic::AtomicBool>,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    loop {
        let (header_end, pkt_type, pkt_flags, remaining_len) =
            match read_mqtt_header(&mut socket, &mut buf, &mut tmp).await {
                Ok(x) => x,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(e),
            };
        let body = &buf[header_end..header_end + remaining_len];
        match pkt_type {
            1 => {
                let connack = [0x20, 0x02, 0x00, 0x00];
                socket.write_all(&connack).await?;
            }
            3 => {
                let qos = (pkt_flags >> 1) & 0x03;
                if body.len() < 2 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "PUBLISH body too short",
                    ));
                }
                let topic_len = u16::from_be_bytes([body[0], body[1]]) as usize;
                if body.len() < 2 + topic_len {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "PUBLISH topic truncated",
                    ));
                }
                let topic = std::str::from_utf8(&body[2..2 + topic_len])
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
                    .to_string();
                let mut offset = 2 + topic_len;
                let packet_id = if qos > 0 {
                    if body.len() < offset + 2 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "PUBLISH packet_id missing",
                        ));
                    }
                    let id = u16::from_be_bytes([body[offset], body[offset + 1]]);
                    offset += 2;
                    id
                } else {
                    0
                };
                let payload = body[offset..].to_vec();
                received.lock().push((topic, payload, packet_id, qos));
                if qos == 1 {
                    let puback = [0x40, 0x02, (packet_id >> 8) as u8, packet_id as u8];
                    socket.write_all(&puback).await?;
                }
                // drop_after 触发：直接关连接（不主动 DISCONNECT，让客户端感知 EOF）
                //
                // 计数语义：**跨连接全局**——`received` 缓冲长度即 broker 累计
                // 收到的 PUBLISH 数（与 ModbusTestServer 的 request_count 一致）。
                // 只触发**一次**（drop_fired）：模拟"瞬时网络抖动"——断一次后，
                // 重连进来的连接正常服务，客户端才能收到 PUBACK 让数据出队。
                //
                // 若按连接重置计数（每次重连后第 1 条 PUBLISH 都断），客户端
                // 永远等不到 PUBACK → publish 失败路径把数据 release 回队列
                // → 队列永不排空（场景 26 的排空断言必然失败）。
                let total_publishes = received.lock().len();
                if let Some(n) = *drop_after.lock() {
                    if total_publishes >= n
                        && !drop_fired.swap(true, std::sync::atomic::Ordering::SeqCst)
                    {
                        drop(socket);
                        return Ok(());
                    }
                }
            }
            14 => return Ok(()),
            _ => {}
        }
        buf.clear();
    }
}

async fn read_mqtt_header(
    socket: &mut tokio::net::TcpStream,
    buf: &mut Vec<u8>,
    tmp: &mut [u8],
) -> std::io::Result<(usize, u8, u8, usize)> {
    use tokio::io::AsyncReadExt;
    while buf.is_empty() {
        let n = socket.read(tmp).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof before header",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let first = buf[0];
    let pkt_type = (first >> 4) & 0x0F;
    let pkt_flags = first & 0x0F;
    let mut remaining_len: usize = 0;
    let mut multiplier: usize = 1;
    let mut consumed = 1;
    loop {
        while buf.len() <= consumed {
            let n = socket.read(tmp).await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof reading remaining_len",
                ));
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let b = buf[consumed];
        consumed += 1;
        remaining_len += ((b & 0x7F) as usize) * multiplier;
        if (b & 0x80) == 0 {
            break;
        }
        multiplier *= 128;
    }
    let needed = consumed + remaining_len;
    while buf.len() < needed {
        let n = socket.read(tmp).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof before body",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    Ok((consumed, pkt_type, pkt_flags, remaining_len))
}

pub fn make_mqtt_config(broker_addr: &str, topic: &str) -> gateway_core::config::MqttConfig {
    use gateway_core::config::MqttConfig;
    MqttConfig {
        enabled: true,
        broker: format!("tcp://{}", broker_addr),
        topic: topic.to_string(),
        publish_interval: 1,
        qos: 1,
        keep_alive: Some(30),
        ack_timeout_secs: Some(2),
        username: None,
        password: None,
        compression: None,
        compression_threshold: None,
        resume_rate_limit: None,
        payload_template: None,
        ..Default::default()
    }
}

// ============================================================
// BusManager 串口总线（无硬件 seam 时的最深真实路径）
// ============================================================

/// 构造"串口缺失"的真实 BusManager：serial=None + 不存在的端口名。
///
/// Windows CI 无虚拟串口对、SerialHandle 无注入 seam，串口总线在 CI 上
/// 能走到的最深真实路径是 S-3 热插拔重开：每笔事务 try_reopen_serial →
/// open 失败 → 硬编码 2s 退避 → IoError("serial handle unavailable,
/// reopen failed")。总线 task 不死，mpsc 串行化 / 通道背压 / 连续失败
/// 阈值重连逻辑全部真实触达。
///
/// 时序预算（场景断言边界按此设定）：
/// - 每笔失败事务 ≈ 2s（reopen 退避，run_serial_bus_task 硬编码）
/// - 连续 IoError ≥ 3 次（reconnect_threshold）后，error-handling 分支
///   在回响应前再触发一次 reopen（+2s），即第 3 笔起的失败事务 ≈ 4s
pub fn dead_serial_bus_manager(cap: usize) -> (gateway_core::south::bus::BusManager, Arc<Metrics>) {
    let bus = gateway_core::config::BusConfig {
        port_name: "chaos_no_such_port".to_string(),
        channel_capacity: Some(cap),
        ..Default::default()
    };
    let metrics = Arc::new(Metrics::new());
    let bm = gateway_core::south::bus::BusManager::spawn(&bus, None, metrics.clone());
    (bm, metrics)
}

// ============================================================
// Scheduler 真路径桩：StubCollector + 完整 Pipeline
// ============================================================

/// 桩采集器：read 立即返回成功 DataPoint，不触网不触串口。
///
/// 目的是让 Scheduler::start/reload 走完整生产路径
/// （spawn_collector_task → supervision_loop → read_batch → handle_results
/// → ProcessingPipeline），而不是跳过采集层测空壳。
pub struct StubCollector {
    bus: String,
}

impl StubCollector {
    pub fn new(bus: &str) -> Self {
        Self {
            bus: bus.to_string(),
        }
    }
}

impl gateway_core::south::protocol::Collector for StubCollector {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn bus_name(&self) -> &str {
        &self.bus
    }

    fn read<'a>(
        &'a self,
        point: &'a gateway_core::config::PointConfig,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = gateway_core::south::protocol::CollectorResult<DataPoint>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(DataPoint::good(
                &point.id,
                point.device_id.as_deref().unwrap_or(&point.bus),
                42.0,
                42.0,
                "f32",
            ))
        })
    }
}

/// 构造运行中的 Scheduler：StubCollector + 完整 ProcessingPipeline
/// （EdgeProcessor + 内存 DataQueue + Metrics）。
///
/// 返回 (scheduler, queue)，queue 用于断言数据确实流经管道落队。
pub async fn real_scheduler(
    bus: &str,
    points: Vec<gateway_core::config::PointConfig>,
) -> (
    Arc<gateway_core::south::scheduler::Scheduler>,
    Arc<DataQueue>,
) {
    use gateway_core::pipeline::{
        ConvertEngine, EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline,
    };
    use gateway_core::south::protocol::Collector;
    use gateway_core::south::scheduler::Scheduler;
    use std::collections::HashMap;

    let queue = real_memory_queue().await;
    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: ConvertEngine::empty(),
        virtual_points: gateway_core::pipeline::VirtualPointEngine::empty(),
        queue: queue.clone(),
        realtime_bus: None,
        buffer: 1024,
        metrics: Arc::new(Metrics::new()),
    });

    let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
    collectors.insert(
        bus.to_string(),
        Arc::new(StubCollector::new(bus)) as Arc<dyn Collector>,
    );

    let scheduler = Arc::new(Scheduler::new(
        collectors,
        points,
        pipeline,
        Arc::new(Metrics::new()),
    ));
    (scheduler, queue)
}

/// 构造最小合法 PointConfig（interval=1s，其余默认）
pub fn make_point(id: &str, bus: &str) -> gateway_core::config::PointConfig {
    gateway_core::config::PointConfig {
        id: id.to_string(),
        bus: bus.to_string(),
        protocol: "stub".to_string(),
        interval: 1,
        ..Default::default()
    }
}

// ============================================================
// 串口总线桩（seam 注入）：可编程串口流 + DE 记录 + 重开注入
// ============================================================
//
// 这几个桩配合 `gateway_core::south::bus::transport` 的注入 seam
// （BusDeps / SerialOpener / GpioOutput / SerialHandle::from_stream）使用，
// 使**真实**的 `run_serial_bus_task` + `do_transaction` 能在无物理串口的
// 环境下被驱动：帧提取、+0x33 偏移、重试、阈值重连、DE 时序全部走生产代码。
//
// 它们不是"替代真实路径的 mock"——总线任务循环、协议编解码均为生产实现，
// 桩只扮演"物理串口 + 表计"这一不可在 CI 复现的角色。

use std::collections::VecDeque;
use std::task::{Context, Poll};

use gateway_core::south::bus::{BusDeps, GpioOutput, SerialHandle, SerialOpener};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// 从站对一笔请求的应答描述。
#[derive(Clone)]
pub struct MockReply {
    /// 要回给总线的字节
    pub bytes: Vec<u8>,
    /// 每次 poll_read 最多交付字节数（0 = 一次给完）
    pub frag: usize,
    /// true = 永不回复（模拟设备静默）
    pub silent: bool,
}

impl MockReply {
    pub fn whole(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            frag: 0,
            silent: false,
        }
    }

    pub fn fragmented(bytes: Vec<u8>, frag: usize) -> Self {
        Self {
            bytes,
            frag,
            silent: false,
        }
    }

    pub fn silent() -> Self {
        Self {
            bytes: Vec::new(),
            frag: 0,
            silent: true,
        }
    }
}

/// 从站行为：按请求产出应答。
pub trait MockBehavior: Send + Sync {
    fn reply(&self, request: &[u8], tx_index: usize) -> MockReply;
}

/// 预定脚本型从站：每次被请求（=每次 do_transaction 的 flush）依次取用
/// 脚本中的应答，脚本耗尽后用 `fallback`。
///
/// 典型用法：脚本首项为 [`MockReply::silent`]（第一次静默），次项为正确帧
/// （重试成功）；或用 [`corrupt_cs`](dlt645_response) 制造校验错。
pub struct CannedSlave {
    replies: parking_lot::Mutex<VecDeque<MockReply>>,
    fallback: MockReply,
    calls: parking_lot::Mutex<usize>,
}

impl CannedSlave {
    /// 每次都以同一应答回复。
    pub fn always(reply: MockReply) -> Self {
        Self {
            replies: parking_lot::Mutex::new(VecDeque::new()),
            fallback: reply,
            calls: parking_lot::Mutex::new(0),
        }
    }

    /// 依次取用 `script`，耗尽后回落到 `fallback`。
    pub fn scripted(script: Vec<MockReply>, fallback: MockReply) -> Self {
        Self {
            replies: parking_lot::Mutex::new(script.into()),
            fallback,
            calls: parking_lot::Mutex::new(0),
        }
    }

    /// 被请求次数（含静默/校验错等异常应答）。
    pub fn calls(&self) -> usize {
        *self.calls.lock()
    }
}

impl MockBehavior for CannedSlave {
    fn reply(&self, _request: &[u8], _tx_index: usize) -> MockReply {
        *self.calls.lock() += 1;
        self.replies
            .lock()
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone())
    }
}

/// 串口流桩的共享状态。
#[derive(Default)]
struct MockPortState {
    write_acc: Vec<u8>,
    /// 每次 flush 落定的完整请求（按 flush 顺序，即每个事务尝试一次）
    requests: Vec<Vec<u8>>,
    flush_count: usize,
    pending: Vec<u8>,
    pending_pos: usize,
    frag: usize,
    silent: bool,
}

/// 可编程异步串口流桩：写侧记录请求，读侧按 [`MockBehavior`] 交付应答。
#[derive(Clone)]
pub struct MockPort {
    st: Arc<parking_lot::Mutex<MockPortState>>,
    behavior: Arc<dyn MockBehavior>,
}

impl MockPort {
    pub fn new(behavior: Arc<dyn MockBehavior>) -> Self {
        Self {
            st: Arc::new(parking_lot::Mutex::new(MockPortState::default())),
            behavior,
        }
    }

    /// 已发出的全部完整请求（按 flush 顺序）。
    pub fn requests(&self) -> Vec<Vec<u8>> {
        self.st.lock().requests.clone()
    }

    /// flush 次数（= 总线发起的写事务尝试次数）。
    pub fn flush_count(&self) -> usize {
        self.st.lock().flush_count
    }
}

impl AsyncWrite for MockPort {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.st.lock().write_acc.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        // 先取出本次请求并释放锁，再调用 behavior（避免脚本内部触达同一把锁）
        let (req, idx) = {
            let mut st = self.st.lock();
            let req = std::mem::take(&mut st.write_acc);
            let idx = st.flush_count;
            st.requests.push(req.clone());
            st.flush_count += 1;
            (req, idx)
        };
        let reply = self.behavior.reply(&req, idx);
        let mut st = self.st.lock();
        st.pending = reply.bytes;
        st.pending_pos = 0;
        st.frag = reply.frag;
        st.silent = reply.silent;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for MockPort {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut st = self.st.lock();
        if st.silent {
            return Poll::Pending;
        }
        let remaining = st.pending.len().saturating_sub(st.pending_pos);
        if remaining == 0 {
            return Poll::Pending;
        }
        let per_call = if st.frag > 0 { st.frag } else { remaining };
        let n = remaining.min(per_call).min(buf.remaining());
        let start = st.pending_pos;
        buf.put_slice(&st.pending[start..start + n]);
        st.pending_pos += n;
        Poll::Ready(Ok(()))
    }
}

/// 记录每次 DE 电平变化的 GPIO 桩。
pub struct RecordingGpio {
    events: parking_lot::Mutex<Vec<(u32, bool)>>,
}

impl RecordingGpio {
    pub fn new() -> Self {
        Self {
            events: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn events(&self) -> Vec<(u32, bool)> {
        self.events.lock().clone()
    }

    /// DE 是否以"拉低"收尾（每次事务结束都应回到接收态）。
    pub fn ends_low(&self) -> bool {
        self.events
            .lock()
            .last()
            .map(|(_, on)| !*on)
            .unwrap_or(false)
    }
}

impl Default for RecordingGpio {
    fn default() -> Self {
        Self::new()
    }
}

impl GpioOutput for RecordingGpio {
    fn set(&self, pin: u32, on: bool) -> gateway_core::engine::error::GatewayResult<()> {
        self.events.lock().push((pin, on));
        Ok(())
    }
}

/// 串口打开器桩：可配置"前 N 次失败"，其余按预置端口依次成功。
pub struct MockOpener {
    inner: parking_lot::Mutex<MockOpenerInner>,
}

struct MockOpenerInner {
    ports: VecDeque<MockPort>,
    fail_first: usize,
    attempts: usize,
}

impl MockOpener {
    pub fn new() -> Self {
        Self {
            inner: parking_lot::Mutex::new(MockOpenerInner {
                ports: VecDeque::new(),
                fail_first: 0,
                attempts: 0,
            }),
        }
    }

    /// 前 `n` 次 open 直接失败（模拟热拔插期间的打开失败）。
    pub fn fail_first(&self, n: usize) {
        self.inner.lock().fail_first = n;
    }

    /// 追加一个可供后续 open 返回的端口。
    pub fn push_port(&self, port: MockPort) {
        self.inner.lock().ports.push_back(port);
    }

    /// open 被调用次数。
    pub fn attempts(&self) -> usize {
        self.inner.lock().attempts
    }
}

impl Default for MockOpener {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialOpener for MockOpener {
    fn open(&self, _bus: &gateway_core::config::BusConfig) -> std::io::Result<SerialHandle> {
        let mut inner = self.inner.lock();
        inner.attempts += 1;
        if inner.fail_first > 0 {
            inner.fail_first -= 1;
            return Err(std::io::Error::other("mock opener: injected open failure"));
        }
        match inner.ports.pop_front() {
            Some(port) => Ok(SerialHandle::from_stream(port)),
            None => Err(std::io::Error::other("mock opener: no port available")),
        }
    }
}

/// 串口总线测试环境：持有桩句柄 + 注入依赖。
pub struct MockBusEnv {
    pub port: MockPort,
    pub opener: Arc<MockOpener>,
    pub gpio: Arc<RecordingGpio>,
}

impl MockBusEnv {
    /// 串口已在外部打开（serial=Some）；opener 无预置端口。
    pub fn opened(port: MockPort) -> Self {
        Self {
            port,
            opener: Arc::new(MockOpener::new()),
            gpio: Arc::new(RecordingGpio::new()),
        }
    }

    /// 串口启动时未打开（serial=None）；重开走 opener，前 `fail_first` 次失败。
    pub fn reopening(port: MockPort, fail_first: usize) -> Self {
        let opener = Arc::new(MockOpener::new());
        opener.fail_first(fail_first);
        opener.push_port(port.clone());
        Self {
            port,
            opener,
            gpio: Arc::new(RecordingGpio::new()),
        }
    }

    pub fn deps(&self) -> BusDeps {
        BusDeps {
            opener: self.opener.clone(),
            gpio: self.gpio.clone(),
        }
    }

    /// 以注入依赖创建真实 BusManager。
    pub fn spawn(
        &self,
        bus: &gateway_core::config::BusConfig,
        serial: Option<SerialHandle>,
        metrics: Arc<Metrics>,
    ) -> gateway_core::south::bus::BusManager {
        gateway_core::south::bus::BusManager::spawn_with(bus, serial, metrics, self.deps())
    }
}

/// 构造一条面向串口桩的总线配置（短超时，便于故障场景快速收敛）。
pub fn mock_bus_config(protocol: &str) -> gateway_core::config::BusConfig {
    gateway_core::config::BusConfig {
        port_name: "mock-tty".to_string(),
        protocol: protocol.to_string(),
        timeout_ms: 200,
        retry_count: Some(0),
        retry_delay_ms: Some(20),
        de_pin: Some(4),
        channel_capacity: Some(8),
        ..Default::default()
    }
}

// ---- 协议帧构造（手工字节，独立于生产编码，可交叉验证） ----

/// 构造 DLT645 响应帧。`data` 为**未编码**数据区（数据标识 + 值），
/// 本函数统一 +0x33；`corrupt_cs` 时故意写错校验和。
///
/// 帧格式：68 | ADDR(6) | 68 | C | L | DATA | CS | 16
pub fn dlt645_response(addr6: &[u8], data: &[u8], corrupt_cs: bool) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.push(0x68);
    frame.extend_from_slice(addr6);
    frame.push(0x68);
    // C=0x81：响应帧（bit7=1）且非异常（bit6=0）
    frame.push(0x81);
    frame.push(data.len() as u8);
    for &b in data {
        frame.push(b.wrapping_add(0x33));
    }
    let cs: u8 = frame.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    frame.push(if corrupt_cs { cs.wrapping_add(1) } else { cs });
    frame.push(0x16);
    frame
}

/// 独立实现的 CRC-16/MODBUS（poly 0xA001，init 0xFFFF，低字节在前）。
///
/// 刻意不复用生产的 `gateway_core::engine::util::crc::crc16`——作为交叉校验
/// oracle，避免"用被测量自身验证被测量"的自洽错误。
pub fn modbus_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// 构造 Modbus RTU 读保持寄存器（FC03）响应帧。
///
/// 帧格式：SLAVE | FC(0x03) | BYTE_CNT | DATA(大端 u16…) | CRC_LO | CRC_HI。
/// `corrupt_crc` 时故意写错 CRC，用于验证坏帧被拒（不被误判为有效响应）。
pub fn modbus_rtu_fc03_response(slave: u8, registers: &[u16], corrupt_crc: bool) -> Vec<u8> {
    let mut frame = vec![slave, 0x03, (registers.len() * 2) as u8];
    for r in registers {
        frame.extend_from_slice(&r.to_be_bytes());
    }
    let crc = modbus_crc16(&frame);
    let (lo, hi) = ((crc & 0xFF) as u8, (crc >> 8) as u8);
    if corrupt_crc {
        frame.push(lo ^ 0xFF);
        frame.push(hi);
    } else {
        frame.push(lo);
        frame.push(hi);
    }
    frame
}

/// 构造 Modbus RTU 写命令的**标准回显响应**（从站侧视角）。
///
/// 按 Modbus RTU 规范：
/// - FC05/FC06：响应为请求的完整 8 字节回显（含请求自身 CRC）；
/// - FC15/FC16：响应仅前 6 字节（slave/fc/addr/qty）与其 CRC——
///   请求第 6-7 字节是 byteCount+数据，不会出现在响应中。
///
/// 用于驱动 `ModbusCollector::write` / `write_multi` 的响应校验路径。
pub fn modbus_rtu_write_echo_response(request: &[u8]) -> Vec<u8> {
    assert!(
        request.len() >= 8,
        "modbus write request too short: {request:02X?}"
    );
    match request[1] {
        // FC05/FC06：完整 8 字节回显
        0x05 | 0x06 => request[..8].to_vec(),
        // FC15/FC16：前 6 字节 + 其 CRC
        0x0F | 0x10 => {
            let mut frame = request[..6].to_vec();
            let crc = modbus_crc16(&frame);
            frame.push((crc & 0xFF) as u8);
            frame.push((crc >> 8) as u8);
            frame
        }
        fc => panic!("unsupported modbus write function code {fc:#04x}"),
    }
}

/// 构造 CJT188 响应帧（地址 7 字节，无第二个 0x68 头）。
pub fn cjt188_response(addr7: &[u8], data: &[u8], corrupt_cs: bool) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.push(0x68);
    frame.extend_from_slice(addr7);
    frame.push(0x81);
    frame.push(data.len() as u8);
    for &b in data {
        frame.push(b.wrapping_add(0x33));
    }
    let cs: u8 = frame.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    frame.push(if corrupt_cs { cs.wrapping_add(1) } else { cs });
    frame.push(0x16);
    frame
}

// ============================================================
// S7comm 真从站桩（ISO-on-TCP：TPKT + COTP + S7Comm）
// ============================================================
//
// 与 ModbusTestServer 同类：起真 TcpListener，讲真 TPKT/COTP/S7Comm 三层协议，
// 完整走通 `S7CommCollector::connect`（COTP 连接 + Setup Communication 协商）
// 与 `read`（Read Var 请求 + Ack-Data 解析）。**不是 mock**——它就是真 TCP +
// 真 S7Comm 编解码。
//
// 帧构造独立于生产 `frames.rs`（手写字节），可作为交叉验证的 oracle；
// 响应体布局严格对齐 `frames::parse_read_response` 的解析约定。

/// S7 从站故障模式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum S7Fault {
    /// 正常：COTP 连接确认 + Setup 协商 + Read Var 应答
    None,
    /// COTP 连接被拒（回 PDU type≠0xD0）→ connect() 返回 IoError
    ConnectReject,
    /// COTP 通过但 Setup Communication 被拒（回错误 PDU type 0x83）→ connect() 返回 IoError
    SetupReject,
    /// 握手正常，但收到第一笔 Read Var 时直接关连接（模拟请求中断线）
    DropOnRead,
}

pub struct S7TestServer {
    pub addr: String,
    _listener: Arc<tokio::net::TcpListener>,
    _handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    _shutdown: tokio::sync::watch::Sender<bool>,
    /// 每个 Read Var 返回的 REAL 值（大端 f32）
    value: Arc<parking_lot::Mutex<f32>>,
    fault: Arc<parking_lot::Mutex<S7Fault>>,
    read_count: Arc<parking_lot::Mutex<usize>>,
}

impl S7TestServer {
    pub async fn start() -> std::io::Result<Self> {
        let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await?);
        let addr = listener.local_addr()?.to_string();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let value = Arc::new(parking_lot::Mutex::new(42.5f32));
        let fault = Arc::new(parking_lot::Mutex::new(S7Fault::None));
        let read_count = Arc::new(parking_lot::Mutex::new(0usize));
        let listener_task = listener.clone();
        let value_task = value.clone();
        let fault_task = fault.clone();
        let count_task = read_count.clone();
        let handle = tokio::spawn(async move {
            run_s7_accept_loop(
                listener_task,
                value_task,
                fault_task,
                count_task,
                shutdown_rx,
            )
            .await;
        });
        Ok(Self {
            addr,
            _listener: listener,
            _handle: parking_lot::Mutex::new(Some(handle)),
            _shutdown: shutdown_tx,
            value,
            fault,
            read_count,
        })
    }

    pub fn set_value(&self, v: f32) {
        *self.value.lock() = v;
    }

    pub fn set_fault(&self, f: S7Fault) {
        *self.fault.lock() = f;
    }

    pub fn read_count(&self) -> usize {
        *self.read_count.lock()
    }
}

impl Drop for S7TestServer {
    fn drop(&mut self) {
        let _ = self._shutdown.send(true);
        if let Some(h) = self._handle.lock().take() {
            h.abort();
        }
    }
}

/// TPKT 封装：4 字节头（0x03 0x00 + 大端总长）+ body。
fn s7_tpkt(body: Vec<u8>) -> Vec<u8> {
    let total = (body.len() + 4) as u16;
    let mut f = vec![0x03, 0x00, (total >> 8) as u8, total as u8];
    f.extend_from_slice(&body);
    f
}

/// COTP Connect Confirm（PDU type 0xD0）。
fn s7_cotp_confirm() -> Vec<u8> {
    vec![
        0x11, 0xD0, 0x00, 0x00, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A, 0xC1, 0x02, 0x01, 0x00, 0xC2,
        0x02, 0x01, 0x02,
    ]
}

/// COTP 拒绝（PDU type 0xC0≠0xD0）→ 采集器判为「连接被拒」。
fn s7_cotp_reject() -> Vec<u8> {
    vec![
        0x11, 0xC0, 0x00, 0x00, 0x00, 0x01, 0x00, 0xC0, 0x01, 0x0A, 0xC1, 0x02, 0x01, 0x00, 0xC2,
        0x02, 0x01, 0x02,
    ]
}

/// Setup Communication 响应体：COTP DT(3) + S7 头(12 = 10 基础 + Error class/code) + 参数(8)。
/// `error=true` 时 PDU type 置 0x83（保留错误位）→ 采集器判为协商被拒。
fn s7_setup_ack(error: bool) -> Vec<u8> {
    let mut b = vec![0x02, 0xF0, 0x80]; // COTP DT
    b.extend_from_slice(&[
        0x32,
        if error { 0x83 } else { 0x03 }, // Protocol ID + PDU type(Ack-Data)
        0x00,
        0x00, // Reserved
        0x00,
        0x01, // PDU Reference
        0x00,
        0x08, // Parameter length = 8
        0x00,
        0x00, // Data length = 0
        0x00,
        0x00, // Error class + Error code（Ack-Data 头共 12 字节）
    ]);
    // 参数：FUNC_SETUP + 保留 + 3 个 2 字节值（末项为协商 PDU 长度 480）
    b.extend_from_slice(&[0xF0, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0xE0]);
    b
}

/// Read Var 响应体：COTP DT(3) + S7 头(12 = 10 基础 + Error class/code) + 参数(2) + 数据(4+4)。
/// 数据段：return_code=0xFF + transport_size=0x04 + 长度(2, 大端)=4 + REAL(4, 大端)。
fn s7_read_ack_real(value: f32) -> Vec<u8> {
    let mut b = vec![0x02, 0xF0, 0x80]; // COTP DT
    b.extend_from_slice(&[
        0x32, 0x03, // Protocol ID + PDU type(Ack-Data)
        0x00, 0x00, // Reserved
        0x00, 0x01, // PDU Reference
        0x00, 0x02, // Parameter length = 2
        0x00, 0x08, // Data length = 8 (4 头 + 4 值)
        0x00, 0x00, // Error class + Error code（Ack-Data 头共 12 字节）
    ]);
    b.extend_from_slice(&[0x04, 0x01]); // 参数：FUNC_READ + item_count=1
    b.extend_from_slice(&[0xFF, 0x04, 0x00, 0x04]); // 数据头
    b.extend_from_slice(&value.to_be_bytes());
    b
}

/// 读一个 TPKT 帧，返回去除 4 字节头后的 body（EOF/短读返回 None）。
async fn s7_read_tpkt(socket: &mut tokio::net::TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 4];
    if socket.read_exact(&mut header).await.is_err() {
        return Ok(None);
    }
    let total = u16::from_be_bytes([header[2], header[3]]) as usize;
    if total < 4 {
        return Ok(None);
    }
    let mut body = vec![0u8; total - 4];
    if !body.is_empty() && socket.read_exact(&mut body).await.is_err() {
        return Ok(None);
    }
    Ok(Some(body))
}

async fn run_s7_accept_loop(
    listener: Arc<tokio::net::TcpListener>,
    value: Arc<parking_lot::Mutex<f32>>,
    fault: Arc<parking_lot::Mutex<S7Fault>>,
    read_count: Arc<parking_lot::Mutex<usize>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => { if *shutdown_rx.borrow() { break; } }
            res = listener.accept() => {
                let (mut socket, _) = match res { Ok(x) => x, Err(_) => break };
                let v = value.clone();
                let f = fault.clone();
                let c = read_count.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    loop {
                        let body = match s7_read_tpkt(&mut socket).await {
                            Ok(Some(b)) => b,
                            _ => return,
                        };
                        // S7 报文：body[3] == 0x32（Protocol ID）
                        if body.len() > 3 && body[3] == 0x32 {
                            let func = body.get(13).copied().unwrap_or(0);
                            if func == 0xF0 {
                                // Setup Communication
                                let error = matches!(*f.lock(), S7Fault::SetupReject);
                                let reply = s7_tpkt(s7_setup_ack(error));
                                if socket.write_all(&reply).await.is_err() { return; }
                            } else if func == 0x04 {
                                // Read Var
                                *c.lock() += 1;
                                if matches!(*f.lock(), S7Fault::DropOnRead) {
                                    // 请求中断线：直接关连接
                                    return;
                                }
                                let val = *v.lock();
                                let reply = s7_tpkt(s7_read_ack_real(val));
                                if socket.write_all(&reply).await.is_err() { return; }
                            }
                        } else {
                            // COTP Connect Request
                            if matches!(*f.lock(), S7Fault::ConnectReject) {
                                let reply = s7_tpkt(s7_cotp_reject());
                                let _ = socket.write_all(&reply).await;
                                return;
                            }
                            let reply = s7_tpkt(s7_cotp_confirm());
                            if socket.write_all(&reply).await.is_err() { return; }
                        }
                    }
                });
            }
        }
    }
}

// ============================================================
// IEC 60870-5-104 真从站桩（TCP：U 帧 STARTDT + I 帧总召唤应答）
// ============================================================
//
// 讲真 104 帧：STARTDT 确认 + 对总召唤（I 帧）回一帧 I 帧 ASDU（短浮点量测），
// 完整走通 `Iec104Collector::connect`（STARTDT 握手）与 `read`（总召唤 + ASDU 解码）。

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Iec104Fault {
    /// 正常：STARTDT 确认 + 总召唤回一帧带值的 I 帧
    None,
    /// 总召唤后不回任何数据（仅关连接）→ 采集器返回 NoReply
    NoReply,
    /// 量的品质位 QDS 置坏位（0x80）→ 数据点应标 Bad
    BadQuality,
    /// 决策表 #18：控制命令回未知信息对象地址（COT=47）→ 采集器应返回 InvalidConfig
    CommandUnknownIoa,
}

pub struct Iec104TestServer {
    pub addr: String,
    _listener: Arc<tokio::net::TcpListener>,
    _handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    _shutdown: tokio::sync::watch::Sender<bool>,
    ioa: u32,
    value: Arc<parking_lot::Mutex<f32>>,
    fault: Arc<parking_lot::Mutex<Iec104Fault>>,
}

impl Iec104TestServer {
    pub async fn start(ioa: u32) -> std::io::Result<Self> {
        let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await?);
        let addr = listener.local_addr()?.to_string();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let value = Arc::new(parking_lot::Mutex::new(230.5f32));
        let fault = Arc::new(parking_lot::Mutex::new(Iec104Fault::None));
        let listener_task = listener.clone();
        let value_task = value.clone();
        let fault_task = fault.clone();
        let handle = tokio::spawn(async move {
            run_iec104_accept_loop(listener_task, ioa, value_task, fault_task, shutdown_rx).await;
        });
        Ok(Self {
            addr,
            _listener: listener,
            _handle: parking_lot::Mutex::new(Some(handle)),
            _shutdown: shutdown_tx,
            ioa,
            value,
            fault,
        })
    }

    pub fn ioa(&self) -> u32 {
        self.ioa
    }

    pub fn set_value(&self, v: f32) {
        *self.value.lock() = v;
    }

    pub fn set_fault(&self, f: Iec104Fault) {
        *self.fault.lock() = f;
    }
}

impl Drop for Iec104TestServer {
    fn drop(&mut self) {
        let _ = self._shutdown.send(true);
        if let Some(h) = self._handle.lock().take() {
            h.abort();
        }
    }
}

/// 读一帧 104 报文，返回 body（不含 0x68 + 长度字节）。
/// body[0]==0xFF 为哨兵：表示读到非法起始字节（用于注入坏帧）。
async fn iec104_read_frame(socket: &mut tokio::net::TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    use tokio::io::AsyncReadExt;
    let mut start = [0u8; 1];
    if socket.read_exact(&mut start).await.is_err() {
        return Ok(None);
    }
    if start[0] != 0x68 {
        return Ok(Some(vec![0xFF]));
    }
    let mut len = [0u8; 1];
    if socket.read_exact(&mut len).await.is_err() {
        return Ok(None);
    }
    let mut body = vec![0u8; len[0] as usize];
    if !body.is_empty() && socket.read_exact(&mut body).await.is_err() {
        return Ok(None);
    }
    Ok(Some(body))
}

/// 构造一帧带单点短浮点量测（M_ME_NC_1, type 14）的 I 帧报文。
fn iec104_iframe_float(seq_recv: u16, ioa: u32, value: f32, quality_byte: u8) -> Vec<u8> {
    let mut asdu = vec![14u8, 0x01, 0x03, 0x00, 0x01, 0x00];
    asdu.extend_from_slice(&ioa.to_le_bytes()[..3]);
    asdu.extend_from_slice(&value.to_le_bytes());
    asdu.push(quality_byte);
    // 控制域：I 帧（bit0=0），发送序号 0，接收序号 seq_recv
    let cf = [0x00u8, 0x00, (seq_recv << 1) as u8, (seq_recv >> 7) as u8];
    let mut frame = vec![0x68, (asdu.len() + 4) as u8];
    frame.extend_from_slice(&cf);
    frame.extend_from_slice(&asdu);
    frame
}

/// 构造一帧对时应答（C_CS_NA_1, type 103, COT=7 激活确认）的 I 帧报文。
///
/// 决策表 #19：建链后采集器会先发对时命令并短暂等待任一 I 帧应答。
/// 从站回此帧即视为对时完成，且必须保持连接（不能像总召唤那样应答后关链）。
fn iec104_iframe_clock_sync(seq_recv: u16, ioa: u32) -> Vec<u8> {
    // type(1) + VSQ(1) + COT/OA(2) + CA(2) + IOA(3) + CP56Time2a(7)
    let mut asdu = vec![103u8, 0x01, 0x07, 0x00, 0x01, 0x00];
    asdu.extend_from_slice(&ioa.to_le_bytes()[..3]);
    asdu.extend_from_slice(&[0u8; 7]);
    let cf = [0x00u8, 0x00, (seq_recv << 1) as u8, (seq_recv >> 7) as u8];
    let mut frame = vec![0x68, (asdu.len() + 4) as u8];
    frame.extend_from_slice(&cf);
    frame.extend_from_slice(&asdu);
    frame
}

/// 构造一帧控制命令确认（类型 45/46/50，COT 由调用方给定）的 I 帧报文。
///
/// 决策表 #18：控制命令的肯定确认为 COT=7（激活确认）；从站拒绝时回 COT=44/45/46/47。
/// 命令对象字节从站按规范回显，此处填 0（内容不影响采集器判定）。
fn iec104_iframe_command_confirm(seq_recv: u16, type_id: u8, ioa: u32, cot: u8) -> Vec<u8> {
    // type(1) + VSQ(1) + COT/OA(2) + CA(2) + IOA(3) + 命令对象(1) = 10 字节
    let mut asdu = vec![type_id, 0x01, cot, 0x00, 0x01, 0x00];
    asdu.extend_from_slice(&ioa.to_le_bytes()[..3]);
    asdu.push(0x00);
    let cf = [0x00u8, 0x00, (seq_recv << 1) as u8, (seq_recv >> 7) as u8];
    let mut frame = vec![0x68, (asdu.len() + 4) as u8];
    frame.extend_from_slice(&cf);
    frame.extend_from_slice(&asdu);
    frame
}

async fn run_iec104_accept_loop(
    listener: Arc<tokio::net::TcpListener>,
    ioa: u32,
    value: Arc<parking_lot::Mutex<f32>>,
    fault: Arc<parking_lot::Mutex<Iec104Fault>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => { if *shutdown_rx.borrow() { break; } }
            res = listener.accept() => {
                let (mut socket, _) = match res { Ok(x) => x, Err(_) => break };
                let v = value.clone();
                let f = fault.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    loop {
                        let body = match iec104_read_frame(&mut socket).await {
                            Ok(Some(b)) => b,
                            _ => return,
                        };
                        if body.first() == Some(&0xFF) {
                            // 坏起始字节：关连接
                            return;
                        }
                        if body.len() >= 4 && body[0] & 0x01 == 0 {
                            // I 帧：按 ASDU 类型分派
                            // 决策表 #19：采集器建链后会先发一帧对时（C_CS_NA_1, type 103），
                            // 从站应以激活确认应答并保持连接；切勿当作总召唤处理
                            // （否则会误回一帧量测后关链，导致后续总召唤 NoReply）。
                            // 控制命令（决策表 #18）：45/46/50 → 回激活确认；故障模式回未知 IOA（COT=47）
                            if body.len() >= 10 && matches!(body[4], 45 | 46 | 50) {
                                let cot = match *f.lock() {
                                    Iec104Fault::CommandUnknownIoa => 47u8,
                                    _ => 7u8,
                                };
                                let cmd_ioa = u32::from_le_bytes([body[10], body[11], body[12], 0]);
                                let conf = iec104_iframe_command_confirm(1, body[4], cmd_ioa, cot);
                                if socket.write_all(&conf).await.is_err() {
                                    return;
                                }
                                continue;
                            }
                            if body.get(4) == Some(&103u8) {
                                let conf = iec104_iframe_clock_sync(1, ioa);
                                if socket.write_all(&conf).await.is_err() { return; }
                                continue;
                            }
                            // 其余 I 帧视为总召唤（type 100）→ 按故障模式应答
                            // 先拷贝出故障枚举，避免 parking_lot guard 跨 await 持有
                            let fault = *f.lock();
                            match fault {
                                Iec104Fault::NoReply => return,
                                Iec104Fault::BadQuality => {
                                    let frame = iec104_iframe_float(1, ioa, *v.lock(), 0x80);
                                    let _ = socket.write_all(&frame).await;
                                    return;
                                }
                                Iec104Fault::CommandUnknownIoa => {
                                    // 决策表 #18：该故障仅影响控制命令确认；总召唤仍按正常值应答
                                    let frame = iec104_iframe_float(1, ioa, *v.lock(), 0x00);
                                    let _ = socket.write_all(&frame).await;
                                    return;
                                }
                                Iec104Fault::None => {
                                    let frame = iec104_iframe_float(1, ioa, *v.lock(), 0x00);
                                    let _ = socket.write_all(&frame).await;
                                    return;
                                }
                            }
                        } else if body.len() >= 4 && body[0] & 0x03 == 0x03 {
                            // U 帧 STARTDT act → 回 STARTDT con
                            let con = [0x68u8, 0x04, 0x0B, 0x00, 0x00, 0x00];
                            if socket.write_all(&con).await.is_err() { return; }
                        }
                    }
                });
            }
        }
    }
}

// ============================================================
// BACnet/IP 真从站桩（UDP：BVLC + NPDU + APDU ComplexACK）
// ============================================================
//
// 讲真 BACnet/IP：对收到的 ReadProperty 请求回一帧 ComplexACK（REAL 值），
// 完整走通 `BacnetCollector::connect`（UDP bind+connect）与 `read`
// （BVLC 校验 + NPDU 跳过 + APDU 解析）。

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BacnetUdpFault {
    /// 正常：回 ComplexACK + REAL 值
    None,
    /// 回 BVLC-Result（function=0x00）→ 采集器判为协议错误
    BvlcResult,
    /// 回错误 BVLC 类型（0x82）→ 采集器判为非法帧
    BadBvlcType,
    /// 不回包 → 采集器 3s 超时判为 NoReply
    Silent,
    /// 对写入回 Abort(0x70) → 采集器应判写入失败
    WriteAbort,
}

pub struct BacnetUdpTestServer {
    pub addr: String,
    _handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    _shutdown: tokio::sync::watch::Sender<bool>,
    value: Arc<parking_lot::Mutex<f32>>,
    fault: Arc<parking_lot::Mutex<BacnetUdpFault>>,
}

impl BacnetUdpTestServer {
    pub async fn start() -> std::io::Result<Self> {
        let socket = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await?);
        let addr = socket.local_addr()?.to_string();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let value = Arc::new(parking_lot::Mutex::new(21.5f32));
        let fault = Arc::new(parking_lot::Mutex::new(BacnetUdpFault::None));
        let socket_task = socket.clone();
        let value_task = value.clone();
        let fault_task = fault.clone();
        let handle = tokio::spawn(async move {
            run_bacnet_udp_loop(socket_task, value_task, fault_task, shutdown_rx).await;
        });
        Ok(Self {
            addr,
            _handle: parking_lot::Mutex::new(Some(handle)),
            _shutdown: shutdown_tx,
            value,
            fault,
        })
    }

    pub fn set_value(&self, v: f32) {
        *self.value.lock() = v;
    }

    pub fn set_fault(&self, f: BacnetUdpFault) {
        *self.fault.lock() = f;
    }
}

impl Drop for BacnetUdpTestServer {
    fn drop(&mut self) {
        let _ = self._shutdown.send(true);
        if let Some(h) = self._handle.lock().take() {
            h.abort();
        }
    }
}

async fn run_bacnet_udp_loop(
    socket: Arc<tokio::net::UdpSocket>,
    value: Arc<parking_lot::Mutex<f32>>,
    fault: Arc<parking_lot::Mutex<BacnetUdpFault>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    // 决策表 #20：桩须按服务号分派并回显请求的 invoke id——
    // WriteProperty(15)→SimpleACK，ReadPropertyMultiple(14)→逐对象 ComplexACK，
    // ReadProperty(12)→单点 ComplexACK。真从站绝不硬编码 invoke id。
    let mut buf = vec![0u8; 2048];
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => { if *shutdown_rx.borrow() { break; } }
            res = socket.recv_from(&mut buf) => {
                let (n, peer) = match res { Ok(x) => x, Err(_) => continue };
                let f = *fault.lock();
                if f == BacnetUdpFault::Silent {
                    continue;
                }
                // 定位请求 APDU：BVLC(4) + NPDU(2) 之后
                if n < 10 {
                    continue;
                }
                let req_apdu = &buf[6..n];
                let invoke = req_apdu[2];
                let service = req_apdu[3];
                let reply = match f {
                    // Silent 已在上方提前 continue，此处仅为满足穷尽性
                    BacnetUdpFault::Silent => None,
                    BacnetUdpFault::BvlcResult => {
                        Some(vec![0x81, 0x00, 0x00, 0x06, 0x00, 0x0A])
                    }
                    BacnetUdpFault::BadBvlcType => {
                        Some(vec![0x82, 0x0A, 0x00, 0x06, 0x01, 0x00])
                    }
                    BacnetUdpFault::WriteAbort => Some(wrap_bacnet_apdu(&[0x70, invoke, service])),
                    BacnetUdpFault::None => match service {
                        15 => Some(wrap_bacnet_apdu(&[0x20, invoke, 15])),
                        14 => {
                            let objs = bacnet_rpm_request_objects(req_apdu);
                            Some(bacnet_ip_rpm_ack(&objs, *value.lock(), invoke))
                        }
                        _ => Some(bacnet_ip_read_property_ack(*value.lock(), invoke)),
                    },
                };
                if let Some(p) = reply {
                    let _ = socket.send_to(&p, peer).await;
                }
            }
        }
    }
}

/// 构造 BACnet ComplexACK（ReadProperty，REAL 值）。
///
/// 布局（符合 ASHRAE 135 的 ReadProperty-ACK 编码）：
/// `0x30`(ComplexACK) + invoke + service(12)
/// + ctx tag 0(Object Identifier, len=4) + ctx tag 1(Property Identifier=85, len=1)
/// + ctx tag 3 opening(0x3E) + REAL tag(0x44) + f32(BE) + ctx tag 3 closing(0x3F)。
///
/// 注：objectIdentifier / propertyIdentifier 两段不可省略——生产解码器
/// [`parse_value_from_apdu`] 按标准布局逐段校验，缺前缀会被判 `InvalidFrame`。
pub fn bacnet_complex_ack_real(value: f32, invoke_id: u8) -> Vec<u8> {
    let mut apdu = vec![
        0x30, invoke_id, 0x0C, // ComplexACK, invoke id, service choice 12
        0x0C, 0x00, 0x00, 0x00, 0x01, // ctx tag 0: Object Identifier (analog-input:1)
        0x19, 0x55, // ctx tag 1: Property Identifier = present-value(85)
        0x3E, // ctx tag 3 opening
        0x44, // REAL (4 bytes)
    ];
    apdu.extend_from_slice(&value.to_be_bytes());
    apdu.push(0x3F); // ctx tag 3 closing
    apdu
}

/// 用 BVLC(type=0x81, Original-Unicast-NPDU=0x0A) + NPDU(0x01,0x00) 包裹 APDU 生成完整数据报。
pub fn wrap_bacnet_apdu(apdu: &[u8]) -> Vec<u8> {
    let npdu = [0x01u8, 0x00];
    let total = 4 + npdu.len() + apdu.len();
    let mut packet = vec![0x81, 0x0A, (total >> 8) as u8, total as u8];
    packet.extend_from_slice(&npdu);
    packet.extend_from_slice(apdu);
    packet
}

/// 构造完整 BACnet/IP ReadProperty 响应包（回显请求 invoke id）。
pub fn bacnet_ip_read_property_ack(value: f32, invoke_id: u8) -> Vec<u8> {
    wrap_bacnet_apdu(&bacnet_complex_ack_real(value, invoke_id))
}

/// 从 RPM 请求 APDU 中提取被请求对象的 Object ID 列表（每个对象一条 ReadAccessSpecification）。
pub fn bacnet_rpm_request_objects(apdu: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 4usize; // 跳过 Confirmed-Request 头(0x00,0x02,invoke,service)
    while i + 5 <= apdu.len() {
        if apdu[i] == 0x0C {
            out.push(u32::from_be_bytes([
                apdu[i + 1],
                apdu[i + 2],
                apdu[i + 3],
                apdu[i + 4],
            ]));
            i += 5;
        } else {
            i += 1;
        }
    }
    out
}

/// 构造 BACnet/IP ReadPropertyMultiple-ACK（对每个请求对象回 Present_Value=value）。
pub fn bacnet_ip_rpm_ack(objects: &[u32], value: f32, invoke_id: u8) -> Vec<u8> {
    let mut apdu = vec![0x30, invoke_id, 0x0E];
    for oid in objects {
        apdu.push(0x0C);
        apdu.extend_from_slice(&oid.to_be_bytes());
        apdu.push(0x1E); // opening tag 1: listOfResults
        apdu.extend_from_slice(&[0x29, 0x55]); // propertyIdentifier[2]=Present_Value
        apdu.push(0x4E); // opening tag 4: propertyValue
        apdu.push(0x44); // REAL(4)
        apdu.extend_from_slice(&value.to_be_bytes());
        apdu.push(0x4F); // closing tag 4
        apdu.push(0x1F); // closing tag 1
    }
    wrap_bacnet_apdu(&apdu)
}

// ============================================================
// BACnet MS/TP 响应帧构造（独立于生产 frames.rs 的 oracle）
// ============================================================

/// MS/TP 头 CRC-8（多项式 0x99，初值 0xFF）。
fn mstp_crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0xFF;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x99;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// MS/TP 数据 CRC-16（多项式 0xA001，初值 0xFFFF，结果取反）。
fn mstp_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// 手工构建 MS/TP 帧（前导码 + 头 + 头CRC + 数据 + 数据CRC）。
/// 当 `corrupt_crc=true` 时数据 CRC 故意写错，用于验证帧提取器拒绝坏帧。
pub fn mstp_frame(
    frame_type: u8,
    dest_addr: u8,
    src_addr: u8,
    data: &[u8],
    corrupt_crc: bool,
) -> Vec<u8> {
    let mut frame = vec![0x55, 0xFF, frame_type, dest_addr, src_addr];
    frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
    let header_crc = mstp_crc8(&frame[2..7]);
    frame.push(header_crc);
    if !data.is_empty() {
        frame.extend_from_slice(data);
        let mut crc = mstp_crc16(data);
        if corrupt_crc {
            crc ^= 0xFFFF;
        }
        frame.push((crc & 0xFF) as u8);
        frame.push((crc >> 8) as u8);
    }
    frame
}

/// 构造 MS/TP 从站读取响应帧（DataNotExpectingReply=0x04，负载 NPDU + ComplexACK REAL）。
pub fn mstp_read_property_ack(
    value: f32,
    master_addr: u8,
    slave_addr: u8,
    corrupt_crc: bool,
) -> Vec<u8> {
    let mut data = vec![0x01u8, 0x00]; // NPDU
    data.extend_from_slice(&bacnet_complex_ack_real(value, 1));
    mstp_frame(0x04, master_addr, slave_addr, &data, corrupt_crc)
}

// ============================================================
// OPC UA Server 桩（基于 async-opcua，讲真 OPC UA 二进制协议）
// ============================================================
//
// `OpcUaTestServer` 用 async-opcua 的 `ServerBuilder` 起一个真 OPC UA 服务端
// （SecurityPolicy::None + 匿名），暴露单个 Double 变量节点。生产
// `OpcUaCollector` 作为真客户端连接并读取，完整走通 `connect`（会话建立 +
// 事件循环）与 `read`（NodeId 解析 + Read 服务 + DataValue 提取）。
// 它**不是 mock**——就是真 OPC UA 协议栈（与北向 `opcua_server` 同源 API），
// 仅用于支撑测试，不替代生产路径。
//
// 说明：北向 `opcua_server` 模块是 `pub(crate)`，集成测试不可直接复用，
// 故在 harness 内以相同 API 起真服务端；节点命名空间 index 由服务端动态分配，
// 通过 `address()` 暴露给采集器，避免硬编码 ns。

#[cfg(feature = "opcua-server")]
const OPCUA_E2E_NAMESPACE_URI: &str = "urn:gateway-e2e";

#[cfg(feature = "opcua-server")]
pub struct OpcUaTestServer {
    /// 采集器连接端点（opc.tcp://127.0.0.1:PORT）
    pub endpoint: String,
    /// 自定义命名空间 index（服务端动态分配）
    ns: u16,
    node_id: opcua::types::NodeId,
    handle: opcua::server::ServerHandle,
    _task: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[cfg(feature = "opcua-server")]
impl OpcUaTestServer {
    pub async fn start() -> std::io::Result<Self> {
        use opcua::crypto::SecurityPolicy;
        use opcua::nodes::Variable;
        use opcua::server::diagnostics::NamespaceMetadata;
        use opcua::server::node_manager::memory::{simple_node_manager, SimpleNodeManager};
        use opcua::server::{ServerBuilder, ANONYMOUS_USER_TOKEN_ID};
        use opcua::types::{DataValue, MessageSecurityMode, NodeId};

        // 取一个空闲端口
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = probe.local_addr()?.port();
        drop(probe);

        let (server, handle) = ServerBuilder::new()
            .application_name("Gateway E2E OPC UA Server")
            .application_uri("urn:gateway:e2e")
            .product_uri("urn:gateway:e2e")
            .host("127.0.0.1")
            .port(port)
            .discovery_urls(vec![format!("opc.tcp://127.0.0.1:{port}/")])
            .add_endpoint(
                "none",
                (
                    "/",
                    SecurityPolicy::None,
                    MessageSecurityMode::None,
                    &[ANONYMOUS_USER_TOKEN_ID] as &[&str],
                ),
            )
            .create_sample_keypair(true)
            .trust_client_certs(true)
            .with_node_manager(simple_node_manager(
                NamespaceMetadata {
                    namespace_uri: OPCUA_E2E_NAMESPACE_URI.to_string(),
                    ..Default::default()
                },
                "GatewayE2ENodeManager",
            ))
            .build()
            .map_err(std::io::Error::other)?;

        let ns = handle
            .get_namespace_index(OPCUA_E2E_NAMESPACE_URI)
            .ok_or_else(|| std::io::Error::other("opcua e2e namespace not found"))?;

        // 预置单变量节点 Value（Double）
        let node_id = NodeId::new(ns, "Value");
        if let Some(nm) = handle.node_managers().get_of_type::<SimpleNodeManager>() {
            {
                let mut aspace = nm.address_space().write();
                if !aspace.node_exists(&node_id) {
                    let var = Variable::new(&node_id, "Value", "Value", 0.0f64);
                    aspace.insert::<_, NodeId>(var, None);
                }
            }
            let _ = nm.set_value(
                handle.subscriptions(),
                &node_id,
                None,
                DataValue::new_now(0.0f64),
            );
        }

        let task = tokio::spawn(async move {
            let _ = server.run().await;
        });

        // 等待服务端真正监听（否则客户端首个 connect 存在竞态）
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                handle.cancel();
                task.abort();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "opcua e2e server not listening in time",
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        Ok(Self {
            endpoint: format!("opc.tcp://127.0.0.1:{port}"),
            ns,
            node_id,
            handle,
            _task: parking_lot::Mutex::new(Some(task)),
        })
    }

    /// 采集器点位地址（NodeId 字符串，ns 由服务端分配）
    pub fn address(&self) -> String {
        format!("ns={};s=Value", self.ns)
    }

    /// 更新变量值（写入地址空间，供订阅/读取）
    pub fn set_value(&self, v: f64) {
        use opcua::server::node_manager::memory::SimpleNodeManager;
        use opcua::types::DataValue;
        if let Some(nm) = self
            .handle
            .node_managers()
            .get_of_type::<SimpleNodeManager>()
        {
            let _ = nm.set_value(
                self.handle.subscriptions(),
                &self.node_id,
                None,
                DataValue::new_now(v),
            );
        }
    }
}

#[cfg(feature = "opcua-server")]
impl Drop for OpcUaTestServer {
    fn drop(&mut self) {
        self.handle.cancel();
        if let Some(t) = self._task.lock().take() {
            t.abort();
        }
    }
}

/// BACnet MS/TP 从站桩：回显请求帧里的 invoke ID（真从站行为）。
///
/// `parse_value_from_apdu` 按 invoke ID 归位校验（BACnet/IP 走 UDP 须防乱序），
/// 而采集器每发一笔请求 invoke ID 递增，固定字节的 `CannedSlave` 应答只在第一笔
/// 匹配、其后每笔都因 ID 不匹配被判 `InvalidFrame`。真从站会把请求的 invoke ID
/// 原样回填，故本桩从请求帧解析 ID 写入 ComplexACK。
pub struct MstpEchoSlave {
    value: f32,
    /// 前 `corrupt_attempts` 次应答损坏数据 CRC（验证坏帧被拒）
    corrupt_attempts: usize,
    /// true = 永不回复（模拟设备静默）
    silent: bool,
    calls: std::sync::atomic::AtomicUsize,
}

impl MstpEchoSlave {
    /// 前 `corrupt_attempts` 次应答损坏数据 CRC，其后正常（回显 invoke ID）。
    pub fn corrupting(value: f32, corrupt_attempts: usize) -> Self {
        Self {
            value,
            corrupt_attempts,
            silent: false,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 始终静默（不回任何字节）——读超时判 NoReply。
    pub fn silent() -> Self {
        Self {
            value: 0.0,
            corrupt_attempts: 0,
            silent: true,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl MockBehavior for MstpEchoSlave {
    fn reply(&self, request: &[u8], _tx_index: usize) -> MockReply {
        let call_index = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.silent {
            return MockReply::silent();
        }
        // MS/TP 帧：前导码(2)+type(1)+dest(1)+src(1)+len(2)+头CRC(1)=8 字节头，
        // 其后 NPDU(2)+APDU；APDU[2] 为 invoke ID → 帧偏移 8+2+2=12。
        let invoke_id = request.get(12).copied().unwrap_or(1);
        // 回帧方向对调：dest = 原请求 src(主站)，src = 原请求 dest(从站)。
        let dest = request.get(4).copied().unwrap_or(0);
        let src = request.get(3).copied().unwrap_or(0);
        let mut data = vec![0x01u8, 0x00]; // NPDU
        data.extend_from_slice(&bacnet_complex_ack_real(self.value, invoke_id));
        MockReply::whole(mstp_frame(
            0x04,
            dest,
            src,
            &data,
            call_index < self.corrupt_attempts,
        ))
    }
}
