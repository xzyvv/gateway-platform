//! Modbus TCP 从站实现（T3.1）
//!
//! 监听 502 端口，把 source_point_id 映射到寄存器地址，
//! 响应主站的 FC01（读线圈）/ FC02（读离散输入）/ FC03（读保持寄存器）/
//! FC04（读输入寄存器）/ FC06（写单寄存器）/ FC16（写多寄存器）。
//!
//! 位读（FC01/FC02）与寄存器共用同一地址空间：线圈地址 N 取寄存器 N 的最低位
//! （`value & 1 != 0`），用于把布尔点位以位语义暴露给主站。
//!
//! 写语义（DD-42）：FC06/FC16 只写本进程内的寄存器镜像，**不写南向设备**，
//! 下一采集周期由 [`ModbusServer::apply_registers`] 覆盖。与 OPC UA Server 的
//! 写回设备不同，详见 `config::modbus_server::ModbusServerConfig::write_enabled`。
//!
//! MBAP header (7 bytes):
//!   transaction_id(2) + protocol_id(2) + length(2) + unit_id(1)
//! PDU:
//!   function_code(1) + data(...)

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::{
    wait_shutdown_notify, ActivePointSet, ModbusServerConfig, NorthServer, RegisterMapping,
};
use crate::engine::error::{GatewayError, GatewayResult};

mod handlers;

/// Modbus function codes
pub(crate) const FC_READ_COILS: u8 = 0x01;
pub(crate) const FC_READ_DISCRETE: u8 = 0x02;
pub(crate) const FC_READ_HOLDING: u8 = 0x03;
const FC_READ_INPUT: u8 = 0x04;
pub(crate) const FC_WRITE_SINGLE: u8 = 0x06;
pub(crate) const FC_WRITE_MULTIPLE: u8 = 0x10;

/// 并发连接上限。
///
/// 修复（审查#4）：原实现每次 accept 都直接 spawn 一个任务 + 一个套接字，没有任何
/// 上限——对端可无限建连耗尽 fd、内存与任务。30s 读超时只能回收空闲连接，持续
/// 喂字节即可长占，属弱缓解。超限时直接拒连（不排队，避免拖住 accept 循环）。
const MAX_CONNECTIONS: usize = 64;

/// Modbus exception codes
pub(crate) const EX_ILLEGAL_FUNCTION: u8 = 0x01;
pub(crate) const EX_ILLEGAL_DATA_ADDRESS: u8 = 0x02; // 写目标不在白名单（FC06/FC16）
pub(crate) const EX_ILLEGAL_DATA_VALUE: u8 = 0x03;

/// MBAP header 大小
const MBAP_HEADER_SIZE: usize = 7;

/// Modbus TCP 从站
pub struct ModbusServer {
    config: ModbusServerConfig,
    running: Arc<AtomicBool>,
    stop_notify: Arc<Notify>,
    /// r60-SD2：与 r48-MQTT1 同型——`stop()` 先置位再 `notify_waiters()`，
    /// accept loop 用 `wait_shutdown_notify` 立即复查并先 `enable()` waiter，
    /// 覆盖「置位与注册之间」的丢失唤醒窗口。
    stopping: Arc<AtomicBool>,
    /// 当前寄存器值（按地址索引）
    pub(crate) registers: Arc<RwLock<HashMap<u16, u16>>>,
    /// 可写地址白名单；cached_mappings 用于 on_points_changed 热更重算
    pub(crate) writable_addrs: Arc<RwLock<HashSet<u16>>>,
    cached_mappings: Arc<RwLock<Vec<RegisterMapping>>>,
}

impl ModbusServer {
    pub fn new(config: ModbusServerConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            stop_notify: Arc::new(Notify::new()),
            // r60-SD2：与 stopping 标志配套，stop() 置位后 accept loop 立即退出，
            // 不再依赖 hot accept 循环轮询 `running` 标志（100ms 抖动）。
            stopping: Arc::new(AtomicBool::new(false)),
            registers: Arc::new(RwLock::new(HashMap::new())),
            writable_addrs: Arc::new(RwLock::new(HashSet::new())),
            cached_mappings: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 设置寄存器值（采集 task 写入，server 读取）
    pub fn set_register(&self, addr: u16, value: u16) {
        self.registers.write().insert(addr, value);
    }

    /// 批量设置寄存器值（采集 task 每个周期调用）。
    ///
    /// DD-42：这是寄存器镜像的**唯一真实来源**——主站通过 FC06/FC16 写入的值
    /// 会在本函数下一次执行时被采集值覆盖。北向 Modbus 写不会回写到设备。
    pub fn apply_registers(&self, mappings: &[RegisterMapping], values: &HashMap<String, f64>) {
        let mut regs = self.registers.write();
        for m in mappings {
            if let Some(v) = values.get(&m.source_point_id) {
                // 修复(审查#A6)：`f64 as u16` 是**饱和**转换（NaN→0、>65535→65535、
                // 负值→0），超范围时静默钳制，SCADA 读到的是被截断/归零的错误值且
                // 没有任何日志。改为显式校验：超范围/非有限值记 warn 且**不写入**，
                // 寄存器保留上一次有效值（写入错误的钳制值比保留旧值危害更大）。
                let scaled = v * m.scale + m.offset;
                if !scaled.is_finite() || scaled < 0.0 || scaled > u16::MAX as f64 {
                    tracing::warn!(
                        "modbus server: point {} scaled value {} out of u16 range (raw={}, scale={}, offset={}); register {} keeps previous value",
                        m.source_point_id, scaled, v, m.scale, m.offset, m.register_addr
                    );
                    continue;
                }
                // 通过范围校验后 `as u16` 不再触发饱和，保持原有的截断语义
                regs.insert(m.register_addr, scaled as u16);
            }
        }
    }

    /// 注入写白名单：映射中 source_point_id ∈ writable_ids 的 register_addr 集合（幂等）。
    pub fn set_writable_addrs(&self, mappings: &[RegisterMapping], writable_ids: &HashSet<String>) {
        *self.cached_mappings.write() = mappings.to_vec();
        self.rebuild_writable_addrs(writable_ids);
    }

    fn rebuild_writable_addrs(&self, writable_ids: &HashSet<String>) {
        let addrs: HashSet<u16> = self
            .cached_mappings
            .read()
            .iter()
            .filter(|m| writable_ids.contains(&m.source_point_id))
            .map(|m| m.register_addr)
            .collect();
        *self.writable_addrs.write() = addrs;
    }

    /// 解析 endpoint (tcp://0.0.0.0:502 -> "0.0.0.0:502")
    pub(crate) fn bind_addr(endpoint: &str) -> &str {
        endpoint.strip_prefix("tcp://").unwrap_or(endpoint)
    }

    /// 处理单个 TCP 连接：循环读取 MBAP+PDU，返回响应
    async fn handle_connection(
        mut sock: tokio::net::TcpStream,
        registers: Arc<RwLock<HashMap<u16, u16>>>,
        running: Arc<AtomicBool>,
        write_enabled: bool,
        writable_addrs: Arc<RwLock<HashSet<u16>>>,
    ) {
        let mut header_buf = [0u8; MBAP_HEADER_SIZE];

        loop {
            if !running.load(Ordering::Relaxed) {
                break;
            }

            // 1. 读取 MBAP header (7 bytes)
            // Bug 37: 30s timeout prevents slow/malicious clients from blocking forever
            match tokio::time::timeout(Duration::from_secs(30), sock.read_exact(&mut header_buf))
                .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
                Ok(Err(e)) => {
                    tracing::debug!("modbus server read header err: {}", e);
                    break;
                }
                Err(_) => {
                    tracing::debug!("modbus server read header timeout (30s)");
                    break;
                }
            }

            let transaction_id = u16::from_be_bytes([header_buf[0], header_buf[1]]);
            let protocol_id = u16::from_be_bytes([header_buf[2], header_buf[3]]);
            let length = u16::from_be_bytes([header_buf[4], header_buf[5]]) as usize;
            let unit_id = header_buf[6];

            if protocol_id != 0 {
                tracing::warn!("modbus server: invalid protocol_id={}", protocol_id);
                break;
            }
            if length < 2 {
                tracing::warn!("modbus server: invalid length={}", length);
                break;
            }

            // 2. 读取 PDU（length - 1 字节，因为 length 含 unit_id）
            let pdu_len = length - 1;
            let mut pdu = vec![0u8; pdu_len];
            // Bug 37: 30s timeout prevents malicious clients from withholding PDU data
            match tokio::time::timeout(Duration::from_secs(30), sock.read_exact(&mut pdu)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::debug!("modbus server read pdu err: {}", e);
                    break;
                }
                Err(_) => {
                    tracing::debug!("modbus server read pdu timeout (30s)");
                    break;
                }
            }

            let function_code = pdu[0];

            // 3. 处理请求（白名单 guard 限定在内层作用域，写响应前必定释放，不跨 await）
            let resp_pdu = {
                let writable = writable_addrs.read();
                Self::handle_request(
                    function_code,
                    &pdu[1..],
                    &registers,
                    write_enabled,
                    &writable,
                )
            };

            // 4. 构建 MBAP + PDU 响应
            let resp_pdu_len = resp_pdu.len() as u16;
            let mut resp = Vec::with_capacity(MBAP_HEADER_SIZE + resp_pdu.len());
            resp.extend_from_slice(&transaction_id.to_be_bytes());
            resp.extend_from_slice(&0u16.to_be_bytes()); // protocol_id = 0
            resp.extend_from_slice(&(resp_pdu_len + 1).to_be_bytes()); // length = pdu + unit_id
            resp.push(unit_id); // echo unit_id
            resp.extend_from_slice(&resp_pdu);

            // 5. 发送响应
            if let Err(e) = sock.write_all(&resp).await {
                tracing::debug!("modbus server write resp err: {}", e);
                break;
            }
        }
    }
}

impl NorthServer for ModbusServer {
    fn name(&self) -> &'static str {
        "modbus"
    }

    fn start<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if !self.config.enabled {
                return Err(GatewayError::Config("modbus server not enabled".into()));
            }
            let addr = Self::bind_addr(&self.config.endpoint).to_string();
            let listener = TcpListener::bind(&addr)
                .await
                .map_err(|e| GatewayError::Io(format!("bind {}: {}", addr, e)))?;
            tracing::info!("modbus server listening on {}", addr);

            self.running.store(true, Ordering::Relaxed);
            let stop = self.stop_notify.clone();
            let running = self.running.clone();
            // r60-SD2：把 stopping 标志也 clone 进 accept loop 闭包。
            let stopping = self.stopping.clone();
            let registers = self.registers.clone();
            let writable_addrs = self.writable_addrs.clone();
            let write_enabled = self.config.write_enabled;
            // 并发连接上限（permit 随连接任务结束释放）
            let conn_sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));

            crate::engine::supervision::spawn_catch_unwind("modbus_server_accept", async move {
                loop {
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }
                    tokio::select! {
                        res = listener.accept() => {
                            match res {
                                Ok((sock, peer)) => {
                                    match conn_sem.clone().try_acquire_owned() {
                                        Ok(permit) => {
                                            tracing::debug!("modbus server accepted {}", peer);
                                            let regs = registers.clone();
                                            let run = running.clone();
                                            let addrs = writable_addrs.clone();
                                            crate::engine::supervision::spawn_catch_unwind("modbus_server_conn", async move {
                                                // permit 随连接任务结束释放，连接数回落
                                                let _permit = permit;
                                                Self::handle_connection(sock, regs, run, write_enabled, addrs).await;
                                            });
                                        }
                                        Err(_) => {
                                            // 立即断开，不做排队
                                            tracing::warn!(
                                                "modbus server 连接数已达上限 {}，拒绝 {}",
                                                MAX_CONNECTIONS,
                                                peer
                                            );
                                            drop(sock);
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("modbus accept error: {}", e);
                                }
                            }
                        }
                        _ = wait_shutdown_notify(&stop, &stopping) => {
                            tracing::info!("modbus server stop signaled");
                            break;
                        }
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                }
            });

            Ok(())
        })
    }

    fn stop<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            // r60-SD2：与 r48-MQTT1 同型——先置位再 notify，
            // 避免 accept loop 还在 `notified().enable()` 之前的窗口
            // `notify_waiters()` 通知丢失。
            self.stopping.store(true, Ordering::SeqCst);
            self.running.store(false, Ordering::Relaxed);
            self.stop_notify.notify_waiters();
            Ok(())
        })
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn on_points_changed(&self, active: &ActivePointSet) {
        self.rebuild_writable_addrs(&active.writable_ids);
    }
}
