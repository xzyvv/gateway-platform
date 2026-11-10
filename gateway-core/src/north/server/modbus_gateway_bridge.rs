//! Modbus 网关桥接（TCP↔RTU）
//!
//! 命名说明：本模块原名 `modbus_passthrough`，与 `mqtt_packet_passthrough`（MQTT 报文透传）
//! 同名不同义。这里做的是「Modbus TCP 客户端 ↔ RTU 串口」的帧格式桥接，
//! 故改名 `modbus_gateway_bridge`，类型名 `ModbusPassthroughServer` → `ModbusGatewayBridgeServer`。
//!
//! 配置键 `modbus_passthrough` 与注册表键保持一致，改动会导致现场配置失效，故保留。
//!
//! 工作流程：
//! 1. 监听 TCP 端口，接收 Modbus TCP 请求（MBAP + PDU）
//! 2. 去掉 MBAP 头，取 unit_id 作为 slave_id，PDU = fc + data
//! 3. 加 CRC 构建 RTU 帧，通过 BusManager 发送到目标串口
//! 4. 接收 RTU 响应（带 CRC），去掉 CRC
//! 5. 加回 MBAP 头（保留原 transaction_id），返回 TCP 响应
//!
//! 网关不做协议解析，只做帧格式转换，半双工串口自动串行化。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::{wait_shutdown_notify, NorthServer};
use crate::config::ModbusPassthroughConfig;
use crate::engine::codec::crc::crc16;
use crate::engine::error::{GatewayError, GatewayResult};
use crate::runtime::GatewayRuntime;

/// 单次 socket 读取超时。
///
/// 修复（审查#3）：原实现两处 `read_exact` 无超时——慢速/恶意客户端只发出半个 MBAP 头
/// 或半个 PDU 就能永久占住一个连接任务（`spawn_catch_unwind` 只捕 panic，不会回收挂起
/// 的任务），连接与任务持续泄漏直至耗尽。取值对齐同目录 `modbus_server.rs` 的 Bug 37
/// 修复（30s）。
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 并发连接上限。
///
/// 修复（审查#4 同族）：原实现每次 accept 都直接 spawn 一个任务 + 一个套接字，无上限。
/// 上面的 30s 读超时只能回收空闲连接（持续喂字节即可长占），属弱缓解；
/// 超限直接拒连，不排队。
const MAX_CONNECTIONS: usize = 64;

/// Modbus TCP↔RTU 网关桥接服务
pub struct ModbusGatewayBridgeServer {
    config: ModbusPassthroughConfig,
    gateway: Arc<GatewayRuntime>,
    running: Arc<AtomicBool>,
    stop_notify: Arc<Notify>,
    /// r60-SD2：与 r48-MQTT1 同型——`stop()` 先置位再 `notify_waiters()`，
    /// accept loop 用 `wait_shutdown_notify` 立即复查并先 `enable()` waiter，
    /// 覆盖「置位与注册之间」的丢失唤醒窗口。
    stopping: Arc<AtomicBool>,
}

impl ModbusGatewayBridgeServer {
    pub fn new(config: ModbusPassthroughConfig, gateway: Arc<GatewayRuntime>) -> Arc<Self> {
        Arc::new(Self {
            config,
            gateway,
            running: Arc::new(AtomicBool::new(false)),
            stop_notify: Arc::new(Notify::new()),
            stopping: Arc::new(AtomicBool::new(false)),
        })
    }

    /// 启动透传服务：监听 TCP，循环接受连接（内部方法，由 start() 调用）
    async fn run_loop(&self) -> GatewayResult<()> {
        let listener = TcpListener::bind(&self.config.listen_addr)
            .await
            .map_err(|e| GatewayError::Io(format!("bind {}: {}", self.config.listen_addr, e)))?;
        tracing::info!(
            "modbus TCP↔RTU passthrough started on {} -> bus {}",
            self.config.listen_addr,
            self.config.target_bus
        );

        self.running.store(true, Ordering::Relaxed);
        let stop = self.stop_notify.clone();
        // r60-SD2：把 stopping 标志也 clone 进 accept loop 闭包。
        let stopping = self.stopping.clone();
        // 并发连接上限（permit 随连接任务结束释放）
        let conn_sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));

        loop {
            if !self.running.load(Ordering::Relaxed) {
                break;
            }
            tokio::select! {
                res = listener.accept() => {
                    match res {
                        Ok((stream, addr)) => {
                            match conn_sem.clone().try_acquire_owned() {
                                Ok(permit) => {
                                    tracing::debug!("modbus passthrough client connected: {}", addr);
                                    let config = self.config.clone();
                                    let gateway = self.gateway.clone();
                                    crate::engine::supervision::spawn_catch_unwind("modbus_passthrough_conn", async move {
                                        // permit 随连接任务结束释放，连接数回落
                                        let _permit = permit;
                                        handle_client(stream, &config, &gateway, READ_TIMEOUT).await;
                                    });
                                }
                                Err(_) => {
                                    // 立即断开，不做排队
                                    tracing::warn!(
                                        "modbus passthrough 连接数已达上限 {}，拒绝 {}",
                                        MAX_CONNECTIONS,
                                        addr
                                    );
                                    drop(stream);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("modbus passthrough accept error: {}", e);
                        }
                    }
                }
                _ = wait_shutdown_notify(&stop, &stopping) => {
                    tracing::info!("modbus passthrough stop signaled");
                    break;
                }
            }
        }
        Ok(())
    }
}

impl NorthServer for ModbusGatewayBridgeServer {
    fn name(&self) -> &'static str {
        // 注册表键保持 "modbus_passthrough"（配置键/配额统计依赖此字符串）
        "modbus_passthrough"
    }

    fn start<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if !self.config.enabled {
                return Err(GatewayError::Config(
                    "modbus passthrough not enabled".into(),
                ));
            }
            self.run_loop().await
        })
    }

    fn stop<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            // r60-SD2：与 r48-MQTT1 同型——先置位再 notify，避免丢失唤醒。
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

/// 处理单个 TCP 客户端连接
///
/// `read_timeout` 为单次读取超时：生产路径恒定传入 [`READ_TIMEOUT`]，显式化仅为让
/// 回归测试能注入短超时（否则验证「半帧会被超时关闭」需真等 30 秒）。
pub(crate) async fn handle_client(
    mut stream: tokio::net::TcpStream,
    config: &ModbusPassthroughConfig,
    gateway: &GatewayRuntime,
    read_timeout: Duration,
) {
    let _buf = [0u8; 260];
    loop {
        // 读取 MBAP 头（7 字节）
        let mut header = [0u8; 7];
        // 修复（审查#3）：读取必须带超时，否则客户端只发半个头就永久占住本任务
        match tokio::time::timeout(read_timeout, stream.read_exact(&mut header)).await {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break, // 客户端断开
            Err(_) => {
                tracing::debug!("passthrough read MBAP header timeout");
                break;
            }
        }

        // 解析 MBAP
        let transaction_id = u16::from_be_bytes([header[0], header[1]]);
        let protocol_id = u16::from_be_bytes([header[2], header[3]]);
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
        let unit_id = header[6];

        if protocol_id != 0 {
            tracing::debug!("invalid protocol_id: {}", protocol_id);
            break;
        }
        if !(2..=253).contains(&length) {
            tracing::debug!("invalid length: {}", length);
            break;
        }

        // 读取 PDU（length - 1 字节，因为 length 包含 unit_id）
        let pdu_len = length - 1;
        let mut pdu = vec![0u8; pdu_len];
        // 修复（审查#3）：同上，PDU 读取同样必须带超时
        match tokio::time::timeout(read_timeout, stream.read_exact(&mut pdu)).await {
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {
                tracing::debug!("passthrough read PDU timeout");
                break;
            }
        }

        // 构建 RTU 帧：unit_id + PDU + CRC
        let mut rtu = Vec::with_capacity(1 + pdu_len + 2);
        rtu.push(unit_id);
        rtu.extend_from_slice(&pdu);
        let crc = crc16(&rtu);
        rtu.push((crc & 0xFF) as u8);
        rtu.push((crc >> 8) as u8);

        // 通过 BusManager 发送到目标串口
        let timeout = Duration::from_millis(config.timeout_ms);
        let response = match gateway.bus_manager(&config.target_bus) {
            Some(bus) => {
                match tokio::time::timeout(timeout, bus.transaction(rtu, None)).await {
                    Ok(Ok(resp)) => resp,
                    Ok(Err(e)) => {
                        tracing::debug!("passthrough bus error: {}", e);
                        // 返回 Modbus 异常响应（RTU 形状，由出口 rtu_to_tcp 统一封装为 TCP）
                        build_exception_rtu(unit_id, pdu[0], 0x04)
                    }
                    Err(_) => {
                        tracing::debug!("passthrough timeout");
                        build_exception_rtu(unit_id, pdu[0], 0x0B)
                    }
                }
            }
            None => {
                tracing::warn!("passthrough target bus '{}' not found", config.target_bus);
                build_exception_rtu(unit_id, pdu[0], 0x04)
            }
        };

        // 转换 RTU 响应为 TCP 响应
        let tcp_response = rtu_to_tcp(&response, transaction_id);

        // 写回 TCP 客户端
        let _ = stream.write_all(&tcp_response).await;
    }
}

/// RTU 响应 → TCP 响应（去掉 CRC，加 MBAP 头）
pub(crate) fn rtu_to_tcp(rtu: &[u8], transaction_id: u16) -> Vec<u8> {
    if rtu.len() < 3 {
        // 异常帧：slave + fc|0x80 + exception_code + crc(2)
        // 或无效帧，返回网关路径异常
        let unit_id = rtu.first().copied().unwrap_or(0xFF);
        let fc = rtu.get(1).copied().unwrap_or(0x80);
        let exc = rtu.get(2).copied().unwrap_or(0x04);
        return build_exception_response(transaction_id, unit_id, fc & 0x7F, exc);
    }

    let unit_id = rtu[0];
    // PDU = rtu[1..len-2]（去掉 slave 和 CRC）
    let pdu = &rtu[1..rtu.len() - 2];
    let length = (pdu.len() + 1) as u16; // +1 for unit_id

    let mut tcp = Vec::with_capacity(7 + pdu.len());
    tcp.extend_from_slice(&transaction_id.to_be_bytes());
    tcp.extend_from_slice(&0u16.to_be_bytes()); // protocol_id
    tcp.extend_from_slice(&length.to_be_bytes());
    tcp.push(unit_id);
    tcp.extend_from_slice(pdu);
    tcp
}

/// 构建 Modbus 异常响应
pub(crate) fn build_exception_response(
    transaction_id: u16,
    unit_id: u8,
    fc: u8,
    exception_code: u8,
) -> Vec<u8> {
    let mut tcp = Vec::with_capacity(9);
    tcp.extend_from_slice(&transaction_id.to_be_bytes());
    tcp.extend_from_slice(&0u16.to_be_bytes()); // protocol_id
    tcp.extend_from_slice(&3u16.to_be_bytes()); // length = 3 (unit_id + fc|0x80 + exc)
    tcp.push(unit_id);
    tcp.push(fc | 0x80);
    tcp.push(exception_code);
    tcp
}

/// 构建 RTU 形状的 Modbus 异常响应（由调用方的 `rtu_to_tcp` 统一封装为 TCP 帧）
///
/// 修复(审查#2)：原实现错误分支直接返回 `build_exception_response` 生成的 **TCP** 帧，
/// 又被出口的 `rtu_to_tcp` 当作 RTU 二次封装，产生畸形帧（unit_id 取到事务号高字节、
/// 长度域被算成 7），主站无法解析。改为返回 RTU 形状，走与成功分支同一条出口。
fn build_exception_rtu(unit_id: u8, fc: u8, exception_code: u8) -> Vec<u8> {
    // 末两字节为占位 CRC，会被 `rtu_to_tcp` 剥掉（len>=3 分支取 rtu[1..len-2] 作 PDU）
    vec![unit_id, fc | 0x80, exception_code, 0x00, 0x00]
}
