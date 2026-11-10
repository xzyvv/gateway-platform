//! 欧姆龙 FINS 协议采集器（支持 FINS/TCP 与 FINS/UDP）
//!
//! 面向欧姆龙 CJ/CS/CP/NJ/NX 系列 PLC，以太网直连（默认端口 9600）。
//! 实现最小可用子集：存储区读（MRC/SRC = 0x01/0x01）+ 存储区写（0x01/0x02）
//! + CPU 单元运行控制（RUN 0x04/0x01、STOP 0x04/0x02）。
//!
//! - **FINS/TCP**（缺省 / `variant = "tcp"`）：建链后先做节点地址交换握手，取得服务端
//!   节点地址作为 DA1，再以 `FINS` TCP 报头包裹 FINS 帧收发。
//! - **FINS/UDP**（`variant = "udp"`）：无握手、无 TCP 报头，UDP 数据报即 FINS 帧本身；
//!   目标节点号 DA1 由配置给出（`tcp_addr` 追加 `?node=NN`，缺省按 IPv4 末位八位组推导，
//!   见 `frames::parse_udp_target`）。适用于跨网段 / 无 TCP 直连的现场。
//!
//! 配置：
//! - `bus.tcp_addr`: 对端地址，如 `"192.168.1.20:9600"`；UDP 模式可追加 `?node=NN`
//! - `bus.variant`: `tcp`（默认）| `udp`
//! - `point.address`: 存储区地址，如 `"D100"`（DM 字）/ `"CIO0"` / `"W200"` / `"EM10"`（扩展数据区）
//!   写操作亦可指向 CPU 控制地址 `"CPU:RUN"` / `"CPU:STOP"`（地址决定动作，value 忽略）
//! - `point.data_type`: `u16`(默认) / `i16` / `u32` / `i32` / `f32` / `u64` / `i64` / `f64` / `bit`
//!   `bit` 时按位访问存储区（计数=1）
//!
//! 模块结构：
//! - frames.rs: 帧构建 / 解析 / 解码函数 + 协议常量
//! - tests.rs: 单元测试

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;

use crate::config::PointConfig;
use crate::engine::model::{DataPoint, PointDataType, Quality};
use crate::south::protocol::{
    convert_value, reverse_convert, Collector, CollectorError, CollectorResult,
};

mod frames;
use frames::{
    build_cpu_control_frame, build_fins_frame_request, build_node_addr_request, build_read_frame,
    build_write_frame, encode_words, parse_address, parse_cpu_control, parse_node_addr_response,
    parse_read_response, parse_udp_target, parse_write_response, read_tcp_response, CMD_RESPONSE,
    MRC_MEMORY_READ, MRC_MEMORY_WRITE, SRC_MEMORY_READ, SRC_MEMORY_WRITE,
};

#[cfg(test)]
mod tests;

/// 本端节点地址（0x00 表示由服务端自动分配；UDP 下由设备自行处理）
const CLIENT_NODE: u8 = 0x00;

/// FINS 传输模式（决策表 #14，由统一变体字段 `bus.variant` 解析）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinsMode {
    /// FINS/TCP：`FINS` 报头包裹 + 建链握手（缺省）
    Tcp,
    /// FINS/UDP：无握手、无报头，UDP 数据报即 FINS 帧
    Udp,
}

/// 由统一变体字段解析传输模式：`udp`（大小写不敏感）→ UDP，其余（含 None）→ TCP。
fn mode_from_variant(variant: Option<&str>) -> FinsMode {
    match variant.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "udp" => FinsMode::Udp,
        _ => FinsMode::Tcp,
    }
}

/// 欧姆龙 FINS 采集器
pub struct FinsCollector {
    addr: String,
    /// 传输模式（由 `bus.variant` 解析）
    mode: FinsMode,
    /// UDP 目标节点号（DA1）；None 表示地址未提供且无法从 IPv4 推导
    udp_node: Option<u8>,
    stream: Mutex<Option<TcpStream>>,
    udp: Mutex<Option<UdpSocket>>,
    bus_name: String,
    connected: AtomicBool,
    /// 目标节点地址：TCP 由握手获得，UDP 取配置（DA1）
    server_node: AtomicU8,
    /// 单次请求 IO 超时（源自 bus.timeout_ms，统一南向超时来源）
    io_timeout: Duration,
}

impl FinsCollector {
    /// 构造采集器；`variant` 取自统一变体解析点 [`crate::south::bus::BusManager::variant`]（决策表 #24）。
    pub fn new(
        addr: String,
        bus_name: String,
        io_timeout: Duration,
        variant: Option<String>,
    ) -> Self {
        let mode = mode_from_variant(variant.as_deref());
        // UDP 需剥离查询串并解析目标节点号；TCP 原样使用
        let (addr, udp_node) = match mode {
            FinsMode::Udp => parse_udp_target(&addr),
            FinsMode::Tcp => (addr, None),
        };
        Self {
            addr,
            mode,
            udp_node,
            stream: Mutex::new(None),
            udp: Mutex::new(None),
            bus_name,
            connected: AtomicBool::new(false),
            server_node: AtomicU8::new(0),
            io_timeout,
        }
    }

    /// 惰性建链：按传输模式分派。
    async fn ensure_connected(&self) -> CollectorResult<()> {
        match self.mode {
            FinsMode::Tcp => self.ensure_connected_tcp().await,
            FinsMode::Udp => self.ensure_connected_udp().await,
        }
    }

    /// FINS/TCP 惰性建链：TCP 连接 + 节点地址交换握手。
    async fn ensure_connected_tcp(&self) -> CollectorResult<()> {
        let mut guard = self.stream.lock().await;
        if guard.is_some() {
            self.connected.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let mut stream = TcpStream::connect(&self.addr).await.map_err(|e| {
            CollectorError::IoError(format!("fins connect {} failed: {e}", self.addr))
        })?;
        stream.set_nodelay(true).ok();

        // 握手：节点地址数据发送
        let req = build_node_addr_request(CLIENT_NODE);
        tokio::time::timeout(self.io_timeout, stream.write_all(&req))
            .await
            .map_err(|_| CollectorError::NoReply)?
            .map_err(|e| CollectorError::IoError(format!("fins send node addr: {e}")))?;

        let resp = tokio::time::timeout(self.io_timeout, read_tcp_response(&mut stream))
            .await
            .map_err(|_| CollectorError::NoReply)??;
        if resp.command != CMD_RESPONSE || resp.tcp_error != 0 {
            return Err(CollectorError::InvalidFrame(format!(
                "fins node addr handshake rejected: cmd=0x{:08X} err=0x{:08X}",
                resp.command, resp.tcp_error
            )));
        }
        let server_node = parse_node_addr_response(&resp.payload)?;
        self.server_node.store(server_node, Ordering::Relaxed);

        *guard = Some(stream);
        self.connected.store(true, Ordering::Relaxed);
        tracing::info!(
            "fins connected to {} (server_node={})",
            self.addr,
            server_node
        );
        Ok(())
    }

    /// FINS/UDP 惰性建链：绑定本地 UDP 套接字并连接目标（无握手）。
    async fn ensure_connected_udp(&self) -> CollectorResult<()> {
        let mut guard = self.udp.lock().await;
        if guard.is_some() {
            self.connected.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let node = self.udp_node.ok_or_else(|| {
            CollectorError::InvalidConfig(format!(
                "fins/udp 目标节点号缺失：请在 tcp_addr 追加 '?node=NN' 或使用 IPv4 地址（当前 '{}'）",
                self.addr
            ))
        })?;
        // UDP 无握手：目标节点号直接取配置
        self.server_node.store(node, Ordering::Relaxed);
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| CollectorError::IoError(format!("fins/udp bind failed: {e}")))?;
        socket.connect(&self.addr).await.map_err(|e| {
            CollectorError::IoError(format!("fins/udp connect {} failed: {e}", self.addr))
        })?;
        *guard = Some(socket);
        self.connected.store(true, Ordering::Relaxed);
        tracing::info!("fins/udp ready for {} (dest_node={})", self.addr, node);
        Ok(())
    }

    /// 发送 FINS 帧并返回应答 FINS 帧负载（按模式分派）。
    async fn transact(&self, fins_frame: &[u8]) -> CollectorResult<Vec<u8>> {
        self.ensure_connected().await?;
        match self.mode {
            FinsMode::Tcp => self.transact_tcp(fins_frame).await,
            FinsMode::Udp => self.transact_udp(fins_frame).await,
        }
    }

    /// FINS/TCP 事务：`FINS` 报头包裹 + 读取 TCP 应答。
    async fn transact_tcp(&self, fins_frame: &[u8]) -> CollectorResult<Vec<u8>> {
        let req = build_fins_frame_request(fins_frame);
        let mut guard = self.stream.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| CollectorError::IoError("fins not connected".into()))?;

        match tokio::time::timeout(self.io_timeout, stream.write_all(&req)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::IoError(format!("fins send: {e}")));
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::NoReply);
            }
        }

        let resp = match tokio::time::timeout(self.io_timeout, read_tcp_response(stream)).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(e);
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::NoReply);
            }
        };
        if resp.command != CMD_RESPONSE || resp.tcp_error != 0 {
            return Err(CollectorError::InvalidFrame(format!(
                "fins tcp response error: cmd=0x{:08X} err=0x{:08X}",
                resp.command, resp.tcp_error
            )));
        }
        Ok(resp.payload)
    }

    /// FINS/UDP 事务：数据报即 FINS 帧，无 TCP 报头。
    async fn transact_udp(&self, fins_frame: &[u8]) -> CollectorResult<Vec<u8>> {
        let mut guard = self.udp.lock().await;
        // 收发全程持有 socket 借用；借用结束后再按结果决定是否清链，避免对 guard 的双重借用
        let res = {
            let socket = match guard.as_mut() {
                Some(s) => s,
                None => return Err(CollectorError::IoError("fins/udp not connected".into())),
            };
            match tokio::time::timeout(self.io_timeout, socket.send(fins_frame)).await {
                Ok(Ok(_)) => {
                    let mut buf = vec![0u8; 2048];
                    match tokio::time::timeout(self.io_timeout, socket.recv(&mut buf)).await {
                        Ok(Ok(n)) => {
                            buf.truncate(n);
                            Ok(buf)
                        }
                        Ok(Err(e)) => Err(CollectorError::IoError(format!("fins/udp recv: {e}"))),
                        Err(_) => Err(CollectorError::NoReply),
                    }
                }
                Ok(Err(e)) => Err(CollectorError::IoError(format!("fins/udp send: {e}"))),
                Err(_) => Err(CollectorError::NoReply),
            }
        };
        if res.is_err() {
            // 链路级失败（IO / 超时）→ 丢弃套接字，下次惰性重建
            *guard = None;
            self.connected.store(false, Ordering::Relaxed);
        }
        res
    }

    /// 按「节点号 + 存储区 + 地址」单点探测一个字（寄存器扫描工具复用）。
    ///
    /// 应答码非 0 视为「该地址不存在/被拒」，随 `(码, None)` 返回而不是上抛成
    /// 链路故障；只有 IO/超时/命令不匹配才返回 Err。
    /// DA1 直接用调用方给的节点号：UDP 下就是配置节点；TCP 下正常应与握手得到的
    /// `server_node` 一致，若传了别的节点，帧会被路由到该节点（多数设备不应答）。
    ///
    /// 跨 crate 可见性：`gateway-api` 的寄存器扫描需要直接调用本方法，故为 `pub`。
    pub async fn probe_word(
        &self,
        node: u8,
        area: &str,
        addr: u32,
    ) -> CollectorResult<(u16, Option<i32>)> {
        let area = parse_address(&format!("{area}{addr}"))?;
        self.ensure_connected().await?;
        let frame = build_read_frame(node, CLIENT_NODE, area.word_code, area.address, 1);
        let payload = self.transact(&frame).await?;
        let end_code = frames::parse_read_end_code(&payload)?;
        if end_code != 0 {
            return Ok((end_code, None));
        }
        let data = parse_read_response(&payload, MRC_MEMORY_READ, SRC_MEMORY_READ)?;
        Ok((
            end_code,
            frames::decode_words(&data, PointDataType::U16).map(|v| v as i32),
        ))
    }
}

impl Collector for FinsCollector {
    fn name(&self) -> &'static str {
        "fins"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// B3：节点地址握手 + 读 DM0 一个字，报告实际回读值。
    ///
    /// FINS 的建链本身包含节点地址交换（TCP），读 DM0 再验证一次真实数据面。
    fn diag<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let area = match parse_address("D0") {
                Ok(a) => a,
                Err(e) => return format!("FINS {}：D0 地址解析失败（{e}）", self.bus_name),
            };
            if let Err(e) = self.ensure_connected().await {
                return format!("FINS {}：建链/节点握手失败（{e}）", self.bus_name);
            }
            let server_node = self.server_node.load(Ordering::Relaxed);
            let frame = build_read_frame(server_node, CLIENT_NODE, area.word_code, area.address, 1);
            let payload = match self.transact(&frame).await {
                Ok(p) => p,
                Err(e) => return format!("FINS {}：建链成功，但读 DM0 失败（{e}）", self.bus_name),
            };
            let data = match parse_read_response(&payload, MRC_MEMORY_READ, SRC_MEMORY_READ) {
                Ok(d) => d,
                Err(e) => {
                    return format!("FINS {}：建链成功，但 DM0 应答不合规（{e}）", self.bus_name)
                }
            };
            match frames::decode_words(&data, PointDataType::U16) {
                Some(v) => format!(
                    "FINS {}：建链成功，DM0 回读值={v}（server_node=0x{server_node:02X}）",
                    self.bus_name
                ),
                None => format!(
                    "FINS {}：建链成功，但 DM0 应答无法按 U16 解码（{} 字节）",
                    self.bus_name,
                    data.len()
                ),
            }
        })
    }

    fn connect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move { self.ensure_connected().await })
    }

    fn disconnect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.stream.lock().await.take();
            let _ = self.udp.lock().await.take();
            self.connected.store(false, Ordering::Relaxed);
        })
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>>
    {
        Box::pin(async move {
            let area = parse_address(&point.address)?;
            let is_bit = matches!(point.data_type.as_deref(), Some("bit") | Some("bool"));
            let (area_code, count, dt) = if is_bit {
                (area.bit_code, 1u16, None)
            } else {
                let dt = point.data_type_enum().unwrap_or(PointDataType::U16);
                (area.word_code, dt.register_count(), Some(dt))
            };

            let server_node = {
                // 先确保握手完成，server_node 才是有效的目标节点地址
                self.ensure_connected().await?;
                self.server_node.load(Ordering::Relaxed)
            };
            let frame = build_read_frame(server_node, CLIENT_NODE, area_code, area.address, count);
            let payload = self.transact(&frame).await?;
            let data = parse_read_response(&payload, MRC_MEMORY_READ, SRC_MEMORY_READ)?;

            let (raw_value, raw_type) = match dt {
                None => {
                    let byte = *data.first().ok_or_else(|| {
                        CollectorError::InvalidFrame("fins bit data empty".into())
                    })?;
                    ((byte & 0x01) as f64, "bit".to_string())
                }
                Some(dt) => {
                    let v = frames::decode_words(&data, dt).ok_or_else(|| {
                        CollectorError::InvalidFrame(format!(
                            "fins word decode failed ({} bytes for {:?})",
                            data.len(),
                            dt
                        ))
                    })?;
                    (v, "fins".to_string())
                }
            };

            let value = convert_value(point, raw_value);
            Ok(DataPoint {
                point_id: point.id.clone(),
                device_id: point.device_id.clone().unwrap_or_default(),
                value,
                raw_value,
                raw_type,
                quality: Quality::Good,
                quality_reason: String::new(),
                timestamp: DataPoint::now(),
                source_ts: 0,
                stale_count: 0,
                value_text: None,
            })
        })
    }

    /// 批量读取：整批点位共享**一次**惰性建链（对齐 IEC 104 `read_batch` 策略）。
    ///
    /// 默认实现逐点 `read`，设备不可达时每个点都会各做一次 `connect`（含 TCP 连接
    /// + 节点地址握手），叠加 N 倍 `io_timeout` 超时。这里批次开始先建链一次：
    /// - 建链失败 → 整批返回**同因错误**，不再逐点重复等待连接超时
    /// - 建链成功 → 逐点 `read`；`ensure_connected` 已短路，不会重复建链
    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut results: Vec<CollectorResult<DataPoint>> = Vec::with_capacity(points.len());
            if points.is_empty() {
                return results;
            }

            if !self.is_connected() {
                if let Err(e) = self.connect().await {
                    let msg = e.to_string();
                    return points
                        .iter()
                        .map(|_| Err(CollectorError::IoError(msg.clone())))
                        .collect();
                }
            }

            for p in points {
                results.push(self.read(p).await);
            }
            results
        })
    }
    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            // CPU 单元运行控制：地址为 CPU:RUN / CPU:STOP / RUN / STOP 时，
            // 地址决定动作（value 忽略），改发 0401/0402 命令而非存储区写。
            if let Some(ctrl) = parse_cpu_control(&point.address) {
                self.ensure_connected().await?;
                let server_node = self.server_node.load(Ordering::Relaxed);
                let frame = build_cpu_control_frame(server_node, CLIENT_NODE, ctrl);
                let payload = self.transact(&frame).await?;
                let (mrc, src) = ctrl.command_code();
                return parse_write_response(&payload, mrc, src);
            }
            let area = parse_address(&point.address)?;
            // 先建链完成节点地址握手，server_node 才有效
            self.ensure_connected().await?;
            let server_node = self.server_node.load(Ordering::Relaxed);

            let frame = if matches!(point.data_type.as_deref(), Some("bit") | Some("bool")) {
                let data = [if value != 0.0 { 0x01u8 } else { 0x00u8 }];
                build_write_frame(
                    server_node,
                    CLIENT_NODE,
                    area.bit_code,
                    area.address,
                    1,
                    &data,
                )
            } else {
                let dt = point.data_type_enum().unwrap_or(PointDataType::U16);
                // 写值与读路径对偶：北向下发的是一次侧值，需先除回变比与 scale/offset
                let data = encode_words(reverse_convert(point, value), dt);
                build_write_frame(
                    server_node,
                    CLIENT_NODE,
                    area.word_code,
                    area.address,
                    dt.register_count(),
                    &data,
                )
            };
            let payload = self.transact(&frame).await?;
            parse_write_response(&payload, MRC_MEMORY_WRITE, SRC_MEMORY_WRITE)
        })
    }
}
