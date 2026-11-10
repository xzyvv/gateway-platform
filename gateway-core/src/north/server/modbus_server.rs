//! Modbus TCP 从站实现（T3.1）
//!
//! 监听 502 端口，把 source_point_id 映射到寄存器地址，
//! 响应主站的 FC01（读线圈）/ FC02（读离散输入）/ FC03（读保持寄存器）/
//! FC04（读输入寄存器）/ FC06（写单寄存器）/ FC16（写多寄存器）。
//!
//! 位读（FC01/FC02）与寄存器共用同一地址空间：线圈地址 N 取寄存器 N 的最低位
//! （`value & 1 != 0`），用于把布尔点位以位语义暴露给主站。
//!
//! MBAP header (7 bytes):
//!   transaction_id(2) + protocol_id(2) + length(2) + unit_id(1)
//! PDU:
//!   function_code(1) + data(...)

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::{wait_shutdown_notify, ModbusServerConfig, NorthServer, RegisterMapping};
use crate::engine::error::{GatewayError, GatewayResult};

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
        }
    }

    /// 设置寄存器值（采集 task 写入，server 读取）
    pub fn set_register(&self, addr: u16, value: u16) {
        self.registers.write().insert(addr, value);
    }

    /// 批量设置寄存器值
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

    /// 解析 endpoint (tcp://0.0.0.0:502 -> "0.0.0.0:502")
    pub(crate) fn bind_addr(endpoint: &str) -> &str {
        endpoint.strip_prefix("tcp://").unwrap_or(endpoint)
    }

    /// 处理单个 TCP 连接：循环读取 MBAP+PDU，返回响应
    async fn handle_connection(
        mut sock: tokio::net::TcpStream,
        registers: Arc<RwLock<HashMap<u16, u16>>>,
        running: Arc<AtomicBool>,
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

            // 3. 处理请求，生成响应 PDU
            let resp_pdu = Self::handle_request(function_code, &pdu[1..], &registers);

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

    /// 根据 function_code 分发请求
    pub(crate) fn handle_request(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        match fc {
            FC_READ_COILS | FC_READ_DISCRETE => Self::handle_read_bits(fc, data, registers),
            FC_READ_HOLDING | FC_READ_INPUT => Self::handle_read(fc, data, registers),
            FC_WRITE_SINGLE => Self::handle_write_single(fc, data, registers),
            FC_WRITE_MULTIPLE => Self::handle_write_multiple(fc, data, registers),
            _ => Self::exception(fc, EX_ILLEGAL_FUNCTION),
        }
    }

    /// FC01/FC02: 读线圈/离散输入
    /// 请求 data: starting_addr(2) + quantity(2)
    /// 响应 PDU: fc(1) + byte_count(1) + bits(ceil(quantity/8))
    ///
    /// 与寄存器共用地址空间：线圈地址 N 对应寄存器 N 的最低位。
    pub(crate) fn handle_read_bits(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 4 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let starting_addr = u16::from_be_bytes([data[0], data[1]]);
        let quantity = u16::from_be_bytes([data[2], data[3]]);

        // Modbus 规范：FC01/FC02 quantity 范围 1-2000
        if quantity == 0 || quantity > 2000 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let byte_count = (quantity as usize).div_ceil(8);
        let mut values = vec![0u8; byte_count];
        let reg_map = registers.read();
        for i in 0..quantity as usize {
            let addr = starting_addr.saturating_add(i as u16);
            // 寄存器不存在视为断开（0），与 FC03 缺省 0 的语义一致
            let on = reg_map.get(&addr).is_some_and(|v| v & 1 != 0);
            if on {
                values[i / 8] |= 1 << (i % 8);
            }
        }
        drop(reg_map);

        let mut resp = Vec::with_capacity(2 + values.len());
        resp.push(fc);
        resp.push(byte_count as u8);
        resp.extend_from_slice(&values);
        resp
    }

    /// FC03/FC04: 读保持/输入寄存器
    /// 请求 data: starting_addr(2) + quantity(2)
    /// 响应 PDU: fc(1) + byte_count(1) + values(quantity*2)
    pub(crate) fn handle_read(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 4 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let starting_addr = u16::from_be_bytes([data[0], data[1]]);
        let quantity = u16::from_be_bytes([data[2], data[3]]);

        // Modbus 规范：FC03/FC04 quantity 范围 1-125
        if quantity == 0 || quantity > 125 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let reg_map = registers.read();
        let mut values = Vec::with_capacity(quantity as usize * 2);
        for i in 0..quantity {
            let addr = starting_addr.saturating_add(i);
            let val = reg_map.get(&addr).copied().unwrap_or(0);
            values.extend_from_slice(&val.to_be_bytes());
        }
        drop(reg_map);

        let mut resp = Vec::with_capacity(2 + values.len());
        resp.push(fc);
        resp.push(values.len() as u8);
        resp.extend_from_slice(&values);
        resp
    }

    /// FC06: 写单个寄存器
    /// 请求 data: addr(2) + value(2)
    /// 响应 PDU: fc(1) + addr(2) + value(2) （回显请求）
    pub(crate) fn handle_write_single(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 4 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let addr = u16::from_be_bytes([data[0], data[1]]);
        let value = u16::from_be_bytes([data[2], data[3]]);

        registers.write().insert(addr, value);

        let mut resp = Vec::with_capacity(5);
        resp.push(fc);
        resp.extend_from_slice(&addr.to_be_bytes());
        resp.extend_from_slice(&value.to_be_bytes());
        resp
    }

    /// FC16: 写多个寄存器
    /// 请求 data: addr(2) + quantity(2) + byte_count(1) + values(quantity*2)
    /// 响应 PDU: fc(1) + addr(2) + quantity(2) （回显请求头和数量）
    ///
    /// 整批写入在一次写锁内完成，避免主站读到半更新状态。
    pub(crate) fn handle_write_multiple(
        fc: u8,
        data: &[u8],
        registers: &Arc<RwLock<HashMap<u16, u16>>>,
    ) -> Vec<u8> {
        if data.len() < 5 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let starting_addr = u16::from_be_bytes([data[0], data[1]]);
        let quantity = u16::from_be_bytes([data[2], data[3]]);
        let byte_count = data[4] as usize;

        // Modbus 规范：FC16 quantity 范围 1-123（123*2+5 = 251 字节 PDU 上限）
        if quantity == 0 || quantity > 123 {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }
        // byte_count 必须与 quantity 严格一致，否则视为非法报文
        if byte_count != quantity as usize * 2 || data.len() < 5 + byte_count {
            return Self::exception(fc, EX_ILLEGAL_DATA_VALUE);
        }

        let mut reg_map = registers.write();
        for i in 0..quantity as usize {
            let off = 5 + i * 2;
            let value = u16::from_be_bytes([data[off], data[off + 1]]);
            // 批量跨地址上界时按 u16 回绕，避免越界 panic
            reg_map.insert(starting_addr.wrapping_add(i as u16), value);
        }
        drop(reg_map);

        let mut resp = Vec::with_capacity(5);
        resp.push(fc);
        resp.extend_from_slice(&starting_addr.to_be_bytes());
        resp.extend_from_slice(&quantity.to_be_bytes());
        resp
    }

    /// 构建异常响应 PDU: (fc | 0x80) + exception_code
    pub(crate) fn exception(fc: u8, code: u8) -> Vec<u8> {
        vec![fc | 0x80, code]
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
                                            crate::engine::supervision::spawn_catch_unwind("modbus_server_conn", async move {
                                                // permit 随连接任务结束释放，连接数回落
                                                let _permit = permit;
                                                Self::handle_connection(sock, regs, run).await;
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
}
