//! 欧姆龙 FINS 协议帧处理（FINS/TCP，构建 / 解析 / 解码）
//!
//! FINS/TCP 在 FINS 帧外层加 16 字节 TCP 报头：
//! ```text
//! "FINS"(4) | 长度(4,大端) | 命令(4,大端) | 错误码(4,大端) | 负载…
//! ```
//! - 命令 `0x00000000`：节点地址数据发送请求
//! - 命令 `0x00000001`：FINS 帧发送请求
//! - 命令 `0x00000002`：以上请求的应答
//! - 长度 = 命令 + 错误码 + 负载的字节数
//!
//! FINS 帧结构（无 TCP 报头，FINS/UDP 时直接使用）：
//! ```text
//! ICF RSV GCT DNA DA1 DA2 SNA SA1 SA2 SID | MRC SRC | 参数…
//! ```
//! 存储区读（MRC/SRC = 0x01/0x01）参数：存储区码(1) + 起始地址(2,大端) + 项数(2,大端)
//! 响应：MRC SRC MRES SRES 数据…
//!
//! 多字节数值字段统一大端；存储区码为单字节（如 DM 字区 0x82）。

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::south::protocol::{CollectorError, CollectorResult};

/// FINS/TCP 报头魔数
pub(crate) const FINS_TCP_MAGIC: &[u8; 4] = b"FINS";
/// 命令：节点地址数据发送请求
pub(crate) const CMD_NODE_ADDR_SEND: u32 = 0x0000_0000;
/// 命令：FINS 帧发送请求
pub(crate) const CMD_FINS_FRAME_SEND: u32 = 0x0000_0001;
/// 命令：以上请求的应答
pub(crate) const CMD_RESPONSE: u32 = 0x0000_0002;

/// MRC：存储区读
pub(crate) const MRC_MEMORY_READ: u8 = 0x01;
/// SRC：存储区读
pub(crate) const SRC_MEMORY_READ: u8 = 0x01;
/// MRC：存储区写
pub(crate) const MRC_MEMORY_WRITE: u8 = 0x01;
/// SRC：存储区写
pub(crate) const SRC_MEMORY_WRITE: u8 = 0x02;

/// 存储区类型：同时给出字/位访问码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FinsArea {
    /// 字访问存储区码
    pub word_code: u8,
    /// 位访问存储区码
    pub bit_code: u8,
    /// 起始地址
    pub address: u16,
}

/// 存储区前缀 -> (字码, 位码)
fn area_from_prefix(prefix: &str) -> Option<(u8, u8)> {
    let p = prefix.to_ascii_uppercase();
    Some(match p.as_str() {
        "DM" | "D" => (0x82, 0x02),  // DM 数据区
        "CIO" | "C" => (0xB0, 0x30), // CIO 区
        "WR" | "W" => (0xB1, 0x31),  // 工作区
        "HR" | "H" => (0xB2, 0x32),  // 保持区
        "AR" | "A" => (0xB3, 0x33),  // 辅助区
        "EM" | "E" => (0xA0, 0x20),  // 扩展数据区（当前存储体）
        _ => return None,
    })
}

/// 拆分 FINS/UDP 目标地址，返回（纯 `host:port`，目标节点号 DA1）。
///
/// 地址形如 `host:port`，可选追加查询串 `?node=NN`：
/// - 显式 `node=NN`（0-255）优先；
/// - 未给定时按欧姆龙以太网惯例取 IPv4 地址的末位八位组（无显式 `node` 且非 IPv4 时返回 `None`，
///   由上层报配置错误，避免静默使用无效节点号）。
pub(crate) fn parse_udp_target(addr: &str) -> (String, Option<u8>) {
    let (host_part, query) = match addr.split_once('?') {
        Some((h, q)) => (h, q),
        None => (addr, ""),
    };
    let mut node: Option<u8> = None;
    for pair in query.split('&') {
        if let Some(v) = pair.trim().strip_prefix("node=") {
            if let Ok(n) = v.trim().parse::<u8>() {
                node = Some(n);
            }
        }
    }
    if node.is_none() {
        // 剥离端口取 host；IPv6 字面量不做末位推导（欧姆龙惯例仅适用 IPv4）
        let host = host_part
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(host_part)
            .trim();
        if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
            node = Some(ip.octets()[3]);
        }
    }
    (host_part.trim().to_string(), node)
}

/// 解析存储区地址，如 `"D100"` / `"DM100"` / `"CIO0"` / `"W200"`。
pub(crate) fn parse_address(s: &str) -> CollectorResult<FinsArea> {
    let s = s.trim();
    if s.is_empty() {
        return Err(CollectorError::InvalidConfig(
            "fins address is empty".to_string(),
        ));
    }
    let split = s.find(|c: char| c.is_ascii_digit());
    let (prefix, num_str) = match split {
        Some(i) => (&s[..i], &s[i..]),
        None => {
            return Err(CollectorError::InvalidConfig(format!(
                "fins address missing numeric part: {s}"
            )))
        }
    };
    let (word_code, bit_code) = area_from_prefix(prefix).ok_or_else(|| {
        CollectorError::InvalidConfig(format!("fins unsupported area prefix: {prefix}"))
    })?;
    let address: u16 = num_str.parse().map_err(|_| {
        CollectorError::InvalidConfig(format!("fins invalid address number: {num_str}"))
    })?;
    Ok(FinsArea {
        word_code,
        bit_code,
        address,
    })
}

/// 构造节点地址数据发送请求（TCP 报头 + 客户端节点地址）。
pub(crate) fn build_node_addr_request(client_node: u8) -> Vec<u8> {
    let mut f = Vec::with_capacity(20);
    f.extend_from_slice(FINS_TCP_MAGIC);
    f.extend_from_slice(&12u32.to_be_bytes()); // 长度 = cmd(4)+err(4)+node(4)
    f.extend_from_slice(&CMD_NODE_ADDR_SEND.to_be_bytes());
    f.extend_from_slice(&0u32.to_be_bytes()); // 错误码
    f.extend_from_slice(&[0x00, 0x00, 0x00, client_node]);
    f
}

/// 构造 FINS 帧发送请求（TCP 报头 + FINS 帧）。
pub(crate) fn build_fins_frame_request(fins_frame: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(16 + fins_frame.len());
    f.extend_from_slice(FINS_TCP_MAGIC);
    f.extend_from_slice(&((8 + fins_frame.len()) as u32).to_be_bytes());
    f.extend_from_slice(&CMD_FINS_FRAME_SEND.to_be_bytes());
    f.extend_from_slice(&0u32.to_be_bytes());
    f.extend_from_slice(fins_frame);
    f
}

/// FINS 帧公共头（连 MRC/SRC 之前的 10 字节）。
fn fins_header(server_node: u8, client_node: u8) -> Vec<u8> {
    vec![
        0x80,        // ICF：需要响应
        0x00,        // RSV
        0x02,        // GCT：网关计数（本地直连）
        0x00,        // DNA：目标网络
        server_node, // DA1：目标节点
        0x00,        // DA2：目标单元
        0x00,        // SNA：源网络
        client_node, // SA1：源节点
        0x00,        // SA2：源单元
        0x01,        // SID：服务 ID
    ]
}

/// 构造存储区读 FINS 帧（容纳于 TCP 报头内）。
pub(crate) fn build_read_frame(
    server_node: u8,
    client_node: u8,
    area_code: u8,
    address: u16,
    count: u16,
) -> Vec<u8> {
    let mut fr = fins_header(server_node, client_node);
    fr.push(MRC_MEMORY_READ);
    fr.push(SRC_MEMORY_READ);
    fr.push(area_code);
    fr.extend_from_slice(&address.to_be_bytes());
    fr.extend_from_slice(&count.to_be_bytes());
    fr
}

/// 构造存储区写 FINS 帧；`data_be` 为待写数据（大端字序列）。
pub(crate) fn build_write_frame(
    server_node: u8,
    client_node: u8,
    area_code: u8,
    address: u16,
    count: u16,
    data_be: &[u8],
) -> Vec<u8> {
    let mut fr = fins_header(server_node, client_node);
    fr.push(MRC_MEMORY_WRITE);
    fr.push(SRC_MEMORY_WRITE);
    fr.push(area_code);
    fr.extend_from_slice(&address.to_be_bytes());
    fr.extend_from_slice(&count.to_be_bytes());
    fr.extend_from_slice(data_be);
    fr
}

/// MRC：CPU 单元控制
pub(crate) const MRC_CPU_CONTROL: u8 = 0x04;
/// SRC：CPU 单元控制 · RUN
pub(crate) const SRC_CPU_RUN: u8 = 0x01;
/// SRC：CPU 单元控制 · STOP
pub(crate) const SRC_CPU_STOP: u8 = 0x02;
/// RUN 命令的标准运行模式参数（0x0000 = 标准 RUN）
pub(crate) const CPU_RUN_MODE_STANDARD: u16 = 0x0000;

/// CPU 单元运行控制命令（FINS 0401 / 0402）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CpuControl {
    /// 切换到 RUN（MRC/SRC = 0x04/0x01，带 2 字节模式参数）
    Run,
    /// 切换到 STOP（MRC/SRC = 0x04/0x02，无参数）
    Stop,
}

impl CpuControl {
    /// 返回该命令的 (MRC, SRC)
    pub(crate) fn command_code(self) -> (u8, u8) {
        match self {
            CpuControl::Run => (MRC_CPU_CONTROL, SRC_CPU_RUN),
            CpuControl::Stop => (MRC_CPU_CONTROL, SRC_CPU_STOP),
        }
    }
}

/// 解析 CPU 运行控制地址
///
/// 接受 `"CPU:RUN"` / `"RUN"` → [`CpuControl::Run`]，
/// `"CPU:STOP"` / `"STOP"` → [`CpuControl::Stop`]（大小写不敏感）。
/// 其它地址返回 `None`（视为普通存储区读写地址）。
pub(crate) fn parse_cpu_control(address: &str) -> Option<CpuControl> {
    let upper = address.trim().to_ascii_uppercase();
    let body = upper.strip_prefix("CPU:").unwrap_or(upper.as_str());
    match body {
        "RUN" => Some(CpuControl::Run),
        "STOP" => Some(CpuControl::Stop),
        _ => None,
    }
}

/// 构造 CPU 单元运行控制 FINS 帧
///
/// RUN 在 MRC/SRC 之后追加 2 字节模式参数（大端），STOP 无参数。
pub(crate) fn build_cpu_control_frame(
    server_node: u8,
    client_node: u8,
    cmd: CpuControl,
) -> Vec<u8> {
    let mut fr = fins_header(server_node, client_node);
    let (mrc, src) = cmd.command_code();
    fr.push(mrc);
    fr.push(src);
    if cmd == CpuControl::Run {
        fr.extend_from_slice(&CPU_RUN_MODE_STANDARD.to_be_bytes());
    }
    fr
}
/// FINS/TCP 应答解析结果。
pub(crate) struct FinsTcpResponse {
    pub command: u32,
    pub tcp_error: u32,
    /// TCP 报头之后的 FINS 帧（应答时可能为节点地址，或完整 FINS 帧）
    pub payload: Vec<u8>,
}

/// 读取一个 FINS/TCP 应答帧。
pub(crate) async fn read_tcp_response(stream: &mut TcpStream) -> CollectorResult<FinsTcpResponse> {
    let mut head = [0u8; 8];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|e| CollectorError::IoError(format!("fins read header: {e}")))?;
    if &head[0..4] != FINS_TCP_MAGIC {
        return Err(CollectorError::InvalidFrame(format!(
            "fins invalid tcp magic: {:02X?}",
            &head[0..4]
        )));
    }
    let len = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if !(8..=2048).contains(&len) {
        return Err(CollectorError::InvalidFrame(format!(
            "fins tcp length out of range: {len}"
        )));
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| CollectorError::IoError(format!("fins read body: {e}")))?;
    let command = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    let tcp_error = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
    Ok(FinsTcpResponse {
        command,
        tcp_error,
        payload: body[8..].to_vec(),
    })
}

/// 从节点地址应答负载中取出「服务端节点地址」（4 字节中的低字节）。
pub(crate) fn parse_node_addr_response(payload: &[u8]) -> CollectorResult<u8> {
    if payload.len() < 4 {
        return Err(CollectorError::InvalidFrame(
            "fins node addr response too short".into(),
        ));
    }
    Ok(payload[3])
}

/// 解析存储区读应答的 FINS 帧，返回数据区（大端字序列）。
///
/// 校验 MRC/SRC 与本端请求一致，并检查 MRES/SRES 结束码。
pub(crate) fn parse_read_response(
    fins_frame: &[u8],
    expect_mrc: u8,
    expect_src: u8,
) -> CollectorResult<Vec<u8>> {
    if fins_frame.len() < 14 {
        return Err(CollectorError::InvalidFrame(
            "fins read response too short".into(),
        ));
    }
    let mrc = fins_frame[10];
    let src = fins_frame[11];
    let mres = fins_frame[12];
    let sres = fins_frame[13];
    if mrc != expect_mrc || src != expect_src {
        return Err(CollectorError::InvalidFrame(format!(
            "fins response command mismatch: got {mrc:02X}/{src:02X}"
        )));
    }
    if mres != 0 || sres != 0 {
        return Err(CollectorError::InvalidFrame(format!(
            "fins end code MRES={mres:02X} SRES={sres:02X}"
        )));
    }
    Ok(fins_frame[14..].to_vec())
}

/// 取读应答的结束码（`MRES << 8 | SRES`）。
///
/// `parse_read_response` 在非 0 结束码时只给出错误文本，调试面需要原始码值
/// 才能如实展示「响应码 0xXXXX」，故单独抽一个只读结束码的入口。
/// 命令码不匹配仍按帧错误上抛（那是串包/错帧，不属于「地址不存在」）。
pub(crate) fn parse_read_end_code(fins_frame: &[u8]) -> CollectorResult<u16> {
    if fins_frame.len() < 14 {
        return Err(CollectorError::InvalidFrame(
            "fins read response too short".into(),
        ));
    }
    let mrc = fins_frame[10];
    let src = fins_frame[11];
    if mrc != MRC_MEMORY_READ || src != SRC_MEMORY_READ {
        return Err(CollectorError::InvalidFrame(format!(
            "fins response command mismatch: got {mrc:02X}/{src:02X}"
        )));
    }
    Ok(((fins_frame[12] as u16) << 8) | fins_frame[13] as u16)
}

/// 解析存储区写应答的 FINS 帧（MRC SRC MRES SRES，无数据区）。
pub(crate) fn parse_write_response(
    fins_frame: &[u8],
    expect_mrc: u8,
    expect_src: u8,
) -> CollectorResult<()> {
    if fins_frame.len() < 14 {
        return Err(CollectorError::InvalidFrame(
            "fins write response too short".into(),
        ));
    }
    let mrc = fins_frame[10];
    let src = fins_frame[11];
    let mres = fins_frame[12];
    let sres = fins_frame[13];
    if mrc != expect_mrc || src != expect_src {
        return Err(CollectorError::InvalidFrame(format!(
            "fins write response command mismatch: got {mrc:02X}/{src:02X}"
        )));
    }
    if mres != 0 || sres != 0 {
        return Err(CollectorError::InvalidFrame(format!(
            "fins write end code MRES={mres:02X} SRES={sres:02X}"
        )));
    }
    Ok(())
}

/// 按大端字序列解码为 f64（复用跨协议规范大端解码）。
pub(crate) fn decode_words(data: &[u8], dt: crate::engine::model::PointDataType) -> Option<f64> {
    let (v, _) = crate::south::protocol::decode_binary_value(data, dt)?;
    Some(v)
}

/// 将 f64 编码为大端字序列（FINS 数值大端）。
pub(crate) fn encode_words(value: f64, dt: crate::engine::model::PointDataType) -> Vec<u8> {
    use crate::engine::model::PointDataType::*;
    match dt {
        U16 => (value as u16).to_be_bytes().to_vec(),
        I16 => (value as i16).to_be_bytes().to_vec(),
        U32 => (value as u32).to_be_bytes().to_vec(),
        I32 => (value as i32).to_be_bytes().to_vec(),
        F32 => (value as f32).to_be_bytes().to_vec(),
        U64 => (value as u64).to_be_bytes().to_vec(),
        I64 => (value as i64).to_be_bytes().to_vec(),
        F64 => value.to_be_bytes().to_vec(),
    }
}
