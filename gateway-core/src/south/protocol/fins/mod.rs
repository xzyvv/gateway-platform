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
use crate::engine::model::PointDataType;
use crate::south::protocol::{Collector, CollectorError, CollectorResult, ResultSlots};

pub(crate) mod frames;
use frames::{
    build_fins_frame_request, build_node_addr_request, build_read_frame, parse_address,
    parse_node_addr_response, parse_read_response, parse_udp_target, read_tcp_response,
    CMD_RESPONSE, MRC_MEMORY_READ, SRC_MEMORY_READ,
};

mod collector;

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
    /// G-08: 指标句柄（工厂注入；None 时写不确定只映射不计数）
    metrics: Option<std::sync::Arc<crate::engine::metrics::Metrics>>,
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
            metrics: None,
        }
    }

    /// 注入指标句柄（生产路径：`registry::factories::create_fins`）。
    pub fn with_metrics(
        mut self,
        metrics: std::sync::Arc<crate::engine::metrics::Metrics>,
    ) -> Self {
        self.metrics = Some(metrics);
        self
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

    /// 缺陷 #7：与 `read_batch` 同语义（一次惰性建链 + 逐点 read），逐点结果立即写共享槽，组超时 drop 不丢已完成结果。
    pub(crate) async fn read_batch_points_with_slots(
        &self,
        points: &[PointConfig],
        slots: ResultSlots,
    ) {
        if points.is_empty() {
            return;
        }
        if !self.is_connected() {
            if let Err(e) = self.connect().await {
                let reason = e.to_string();
                let mut guard = slots.lock().unwrap_or_else(|e| e.into_inner());
                for slot in guard.iter_mut() {
                    *slot = Some(Err(CollectorError::IoError(reason.clone())));
                }
                return;
            }
        }
        for (i, p) in points.iter().enumerate() {
            slots.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(self.read(p).await);
        }
    }
}
