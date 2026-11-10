//! 三菱 MC 协议帧处理（QnA 3E / 4E 二进制帧 + A 兼容 1E 二进制帧，构建 / 解析 / 解码）
//!
//! 支持三种帧格式，经统一变体字段 `bus.variant` 选择（决策表 #16 / #24）：
//! - `3e`（默认）：QnA 兼容 3E 帧，TCP/UDP 二进制，请求子头 `0x5000`，响应子头 `0xD000`
//! - `4e`：QnA 兼容 4E 帧 = 3E 帧 + 2 字节序列号，请求子头 `0x5400`，响应子头 `0xD400` 并回显序列号
//! - `1e`：A 兼容 1E 帧（老款 FX/A 系列），首字节即功能码，无长度字段（长度由点数推导）
//!
//! 3E 请求帧结构（批量读，21 字节）：
//! ```text
//! 50 00 | 网络No | PC No | 模块IO(2) | 站No | 数据长度(2,LE) | 定时器(2,LE)
//!        | 命令(2,LE) | 子命令(2,LE) | 软元件码(1) | 首地址(3,LE) | 点数(2,LE)
//! ```
//! 数据长度 = 从「定时器」到帧尾的字节数 = 12 + 附加数据长度。
//!
//! 4E 请求帧 = 3E 帧在子头后插入 2 字节序列号（数据长度字段含义不变）：
//! ```text
//! 54 00 | 序列号(2,LE) | 网络No | PC No | 模块IO(2) | 站No | 数据长度(2,LE) | （其余同 3E）
//! ```
//! 3E 响应：`D0 00 | 网络No | PC No | 模块IO(2) | 站No | 数据长度(2,LE) | 结束代码(2,LE) | 数据…`
//! 4E 响应：`D4 00 | 序列号(2,LE) | 网络No | PC No | 模块IO(2) | 站No | 数据长度(2,LE) | 结束代码(2,LE) | 数据…`
//!
//! 1E 请求帧（A 兼容，读/写均无长度字段，长度由点数推导）：
//! ```text
//! 功能码(1) | PLC号(1) | 定时器(2,LE) | 首地址(4,LE) | 软元件码(2,LE) | 点数(2,LE) | [写数据…]
//! ```
//! 1E 响应：`(功能码|0x80)(1) | 状态码(1) | 数据…`；状态码 0 表示正常。
//!
//! 字元件（D/W/R/TN/CN 等）按字为单位，每点 2 字节小端；
//! 位元件（X/Y/M/B 等）按位为单位，每 2 点打包 1 字节（首点在高半字节）。

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::engine::model::PointDataType;
use crate::south::protocol::{CollectorError, CollectorResult};

/// MC 帧格式（决策表 #16，经统一变体字段选择）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McFrameFormat {
    /// QnA 兼容 3E 帧（默认）
    E3,
    /// QnA 兼容 4E 帧（3E + 2 字节序列号）
    E4,
    /// A 兼容 1E 帧（老款 FX/A 系列）
    E1,
}

impl McFrameFormat {
    /// 由统一变体取值解析帧格式；缺省/未知一律回退 3E。
    pub(crate) fn from_variant(variant: Option<&str>) -> Self {
        match variant.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("4e") => McFrameFormat::E4,
            Some("1e") => McFrameFormat::E1,
            _ => McFrameFormat::E3,
        }
    }
}

/// 3E 请求子头（QnA 兼容；响应为请求首字节置最高位 → `0xD000`）
pub(crate) const SUBHEADER_REQ_3E: [u8; 2] = [0x50, 0x00];
/// 3E 响应子头
pub(crate) const SUBHEADER_RESP_3E: [u8; 2] = [0xD0, 0x00];
/// 4E 请求子头
pub(crate) const SUBHEADER_REQ_4E: [u8; 2] = [0x54, 0x00];
/// 4E 响应子头
pub(crate) const SUBHEADER_RESP_4E: [u8; 2] = [0xD4, 0x00];
/// 批量读命令
pub(crate) const CMD_BATCH_READ: u16 = 0x0401;
/// 批量写命令
pub(crate) const CMD_BATCH_WRITE: u16 = 0x1401;
/// 子命令：字单位
pub(crate) const SUB_WORD: u16 = 0x0000;
/// 子命令：位单位
pub(crate) const SUB_BIT: u16 = 0x0001;
/// 3E 固定头长度（子头2 + 网络1 + PC1 + IO2 + 站1 + 长度2）
const HEADER_LEN_3E: usize = 9;
/// 4E 固定头长度（3E 头 + 2 字节序列号）
const HEADER_LEN_4E: usize = 11;

/// 1E 功能码：批量位读取
pub(crate) const FC_BATCH_BIT_READ: u8 = 0x00;
/// 1E 功能码：批量字读取
pub(crate) const FC_BATCH_WORD_READ: u8 = 0x01;
/// 1E 功能码：批量位写入
pub(crate) const FC_BATCH_BIT_WRITE: u8 = 0x02;
/// 1E 功能码：批量字写入
pub(crate) const FC_BATCH_WORD_WRITE: u8 = 0x03;

/// 单帧最大字元件点数（3E/4E）：数据区上限 960 字节（含 2 字节结束代码）→ (960-2)/2 = 479 字
pub(crate) const MAX_WORDS_PER_FRAME: u16 = 479;
/// 单帧最大位元件点数（3E/4E）：位数据 2 点/字节 → (960-2)*2 = 1916 位
pub(crate) const MAX_BITS_PER_FRAME: u16 = 1916;
/// 1E 单帧上限：ICC 驱动手册标注 1E 帧字数上限 256（字元件）；位元件按点数同口径保守取 256
pub(crate) const MAX_UNITS_PER_FRAME_1E: u16 = 256;

/// 解析后的 MC 软元件（设备）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct McDevice {
    /// 软元件码（3E 单字节设备代码）
    pub code: u8,
    /// 首地址（十进制编号）
    pub head: u32,
    /// 是否为位元件
    pub is_bit: bool,
}

/// 软元件前缀 -> (代码, 是否位元件)。按前缀长度降序匹配（先试 2 字符再 1 字符）。
fn device_from_prefix(prefix: &str) -> Option<(u8, bool)> {
    let p = prefix.to_ascii_uppercase();
    Some(match p.as_str() {
        "SM" => (0x91, true),
        "SD" => (0xA9, false),
        "ZR" => (0xB0, false),
        "TN" => (0xC2, false),
        "CN" => (0xC5, false),
        "X" => (0x9C, true),
        "Y" => (0x9D, true),
        "M" => (0x90, true),
        "L" => (0x92, true),
        "F" => (0x93, true),
        "V" => (0x94, true),
        "B" => (0xA0, true),
        "D" => (0xA8, false),
        "W" => (0xB4, false),
        "R" => (0xAF, false),
        "Z" => (0xCC, false),
        "T" => (0xC2, false), // 定时器当前值（TN）
        "C" => (0xC5, false), // 计数器当前值（CN）
        _ => return None,
    })
}

/// 3E 单字节软元件码 → 1E 双字节软元件码（A 兼容）。
///
/// 1E 软元件码为双字节 ASCII：设备符号字母（大写）置于高字节，单字符符号低字节补
/// `0x20`（空格）。例：D → `0x4420`（'D'+' '）、M → `0x4D20`、X → `0x5820`、TN → `0x544E`。
/// 来源：ICC "Mitsubishi MELSEC Client Driver Manual" Table 1（3E/1E 双列软元件码）。
/// 3E 码 `0xC2`（T/TN）归一为 TN、`0xC5`（C/CN）归一为 CN。
/// 返回 `None` 表示该软元件在 A 系列 1E 帧中不可寻址（该表标注 N/A）。
pub(crate) fn code_1e_from_code_3e(code: u8) -> Option<u16> {
    Some(match code {
        0x9C => 0x5820, // X
        0x9D => 0x5920, // Y
        0x90 => 0x4D20, // M
        0xA0 => 0x4220, // B
        0xA8 => 0x4420, // D
        0xB4 => 0x5720, // W
        0xC2 => 0x544E, // TN（含 T 别名）
        0xC5 => 0x434E, // CN（含 C 别名）
        _ => return None,
    })
}

/// 解析软元件地址，如 `"D100"` / `"M10"` / `"X1F"`(十进制) / `"ZR1000"` / `"T100"` / `"C100"`。
pub(crate) fn parse_address(s: &str) -> CollectorResult<McDevice> {
    let s = s.trim();
    if s.is_empty() {
        return Err(CollectorError::InvalidConfig(
            "mc address is empty".to_string(),
        ));
    }
    let split = s.find(|c: char| c.is_ascii_digit());
    let (prefix, num_str) = match split {
        Some(i) => (&s[..i], &s[i..]),
        None => {
            return Err(CollectorError::InvalidConfig(format!(
                "mc address missing numeric part: {s}"
            )))
        }
    };
    let (code, is_bit) = device_from_prefix(prefix).ok_or_else(|| {
        CollectorError::InvalidConfig(format!("mc unsupported device prefix: {prefix}"))
    })?;
    let num: u32 = num_str.parse().map_err(|_| {
        CollectorError::InvalidConfig(format!("mc invalid device number: {num_str}"))
    })?;
    if num > 0x00FF_FFFF {
        return Err(CollectorError::InvalidConfig(format!(
            "mc device number out of range (24-bit): {num}"
        )));
    }
    Ok(McDevice {
        code,
        head: num,
        is_bit,
    })
}

/// 追加 3E / 4E 公共请求体（自「网络号」起）。
///
/// `payload` 为「点数」字段之后的附加数据（批量读为 0 字节，批量写为待写数据）。
/// 数据长度字段含义与 3E 一致（自定时器起算），不含 4E 序列号。
#[allow(clippy::too_many_arguments)]
fn append_qna_body(
    f: &mut Vec<u8>,
    cmd: u16,
    sub: u16,
    device: &McDevice,
    points: u16,
    timer: u16,
    payload: &[u8],
    station: u8,
) {
    // 数据长度 = 定时器(2) + 命令(2) + 子命令(2) + 软元件码(1) + 首地址(3) + 点数(2) + payload
    let data_len = (12 + payload.len()) as u16;
    f.push(0x00); // 网络号
    f.push(0xFF); // PC 号
    f.extend_from_slice(&[0xFF, 0x03]); // 请求目标模块 I/O 号（本站）
    f.push(station); // 请求目标模块站号（1 字节，0..=255；本站直连时为 0）
    f.extend_from_slice(&data_len.to_le_bytes());
    f.extend_from_slice(&timer.to_le_bytes());
    f.extend_from_slice(&cmd.to_le_bytes());
    f.extend_from_slice(&sub.to_le_bytes());
    f.push(device.code);
    let head = device.head.to_le_bytes();
    f.extend_from_slice(&head[..3]); // 首地址 3 字节小端
    f.extend_from_slice(&points.to_le_bytes());
    f.extend_from_slice(payload);
}

/// 构造 3E 请求帧（请求子头 `0x5000`）。
// 建帧参数本就按协议字段摊开（cmd/sub/device/points/timer/payload/station），拆结构体只会
// 让调用点更难对照帧布局，故显式放行参数个数告警。
#[allow(clippy::too_many_arguments)]
fn build_request_3e(
    cmd: u16,
    sub: u16,
    device: &McDevice,
    points: u16,
    timer: u16,
    payload: &[u8],
    station: u8,
) -> Vec<u8> {
    let mut f = Vec::with_capacity(HEADER_LEN_3E + 12 + payload.len());
    f.extend_from_slice(&SUBHEADER_REQ_3E);
    append_qna_body(&mut f, cmd, sub, device, points, timer, payload, station);
    f
}

/// 构造 4E 请求帧（请求子头 `0x5400` + 2 字节序列号）。
#[allow(clippy::too_many_arguments)]
fn build_request_4e(
    cmd: u16,
    sub: u16,
    device: &McDevice,
    points: u16,
    timer: u16,
    payload: &[u8],
    serial: u16,
    station: u8,
) -> Vec<u8> {
    let mut f = Vec::with_capacity(HEADER_LEN_4E + 12 + payload.len());
    f.extend_from_slice(&SUBHEADER_REQ_4E);
    f.extend_from_slice(&serial.to_le_bytes());
    append_qna_body(&mut f, cmd, sub, device, points, timer, payload, station);
    f
}

/// 按帧格式构造批量读请求（字元件或位元件由 `device.is_bit` 决定；`serial` 仅 4E 使用）。
// 生产路径已改用带站号的 build_read_request_at_station（站号恒 0），
// 保留本函数供单测/黄金帧断言直接对照 3E/4E 布局。
#[allow(dead_code)]
pub(crate) fn build_read_request(
    device: &McDevice,
    points: u16,
    timer: u16,
    fmt: McFrameFormat,
    serial: u16,
) -> Vec<u8> {
    build_read_request_at_station(device, points, timer, fmt, serial, 0)
}

/// 同 [`build_read_request`]，但显式指定 3E/4E 帧头的目标站号。
///
/// 站号是 1 字节字段（0..=255）；采集路径直连本站固定用 0，
/// 只有调试面的「按站号逐点探测」需要指定其它站。
pub(crate) fn build_read_request_at_station(
    device: &McDevice,
    points: u16,
    timer: u16,
    fmt: McFrameFormat,
    serial: u16,
    station: u8,
) -> Vec<u8> {
    let sub = if device.is_bit { SUB_BIT } else { SUB_WORD };
    match fmt {
        McFrameFormat::E4 => build_request_4e(
            CMD_BATCH_READ,
            sub,
            device,
            points,
            timer,
            &[],
            serial,
            station,
        ),
        _ => build_request_3e(CMD_BATCH_READ, sub, device, points, timer, &[], station),
    }
}

/// 按帧格式构造批量写请求（字元件，`words_le` 为待写字的原始字节，小端）。
pub(crate) fn build_write_words_request(
    device: &McDevice,
    words_le: &[u8],
    timer: u16,
    fmt: McFrameFormat,
    serial: u16,
) -> Vec<u8> {
    let points = (words_le.len() / 2) as u16;
    match fmt {
        McFrameFormat::E4 => build_request_4e(
            CMD_BATCH_WRITE,
            SUB_WORD,
            device,
            points,
            timer,
            words_le,
            serial,
            0,
        ),
        _ => build_request_3e(
            CMD_BATCH_WRITE,
            SUB_WORD,
            device,
            points,
            timer,
            words_le,
            0,
        ),
    }
}

/// 按帧格式构造批量写请求（位元件，`bit_on` 决定写入 1 或 0）。
pub(crate) fn build_write_bit_request(
    device: &McDevice,
    bit_on: bool,
    timer: u16,
    fmt: McFrameFormat,
    serial: u16,
) -> Vec<u8> {
    // 位写请求数据区：1 字节，0x10 = ON，0x00 = OFF
    let payload = [if bit_on { 0x10u8 } else { 0x00u8 }];
    match fmt {
        McFrameFormat::E4 => build_request_4e(
            CMD_BATCH_WRITE,
            SUB_BIT,
            device,
            1,
            timer,
            &payload,
            serial,
            0,
        ),
        _ => build_request_3e(CMD_BATCH_WRITE, SUB_BIT, device, 1, timer, &payload, 0),
    }
}

/// 构造 1E 请求帧（A 兼容）。读/写共用，长度由点数与 payload 推导，无长度字段。
pub(crate) fn build_request_1e(
    func: u8,
    device: &McDevice,
    points: u16,
    timer: u16,
    payload: &[u8],
) -> CollectorResult<Vec<u8>> {
    let code = code_1e_from_code_3e(device.code).ok_or_else(|| {
        CollectorError::InvalidConfig(format!(
            "mc device code 0x{:02X} 不支持 1E 帧（A 系列无法寻址）",
            device.code
        ))
    })?;
    let mut f = Vec::with_capacity(12 + payload.len());
    f.push(func);
    f.push(0xFF); // PLC 号
    f.extend_from_slice(&timer.to_le_bytes());
    f.extend_from_slice(&device.head.to_le_bytes()); // 首地址 4 字节小端
    f.extend_from_slice(&code.to_le_bytes()); // 软元件码 2 字节小端
    f.extend_from_slice(&points.to_le_bytes());
    f.extend_from_slice(payload);
    Ok(f)
}

/// 读取带 `数据长度` 字段的响应体，返回（结束代码，数据区）。
///
/// `len` 为「结束代码 + 数据」的字节数；`stream` 必须已读走固定头。
async fn read_len_body(stream: &mut TcpStream, len: usize) -> CollectorResult<(u16, Vec<u8>)> {
    if len < 2 {
        return Err(CollectorError::InvalidFrame(format!(
            "mc response data length too small: {len}"
        )));
    }
    if len > 960 {
        // 3E/4E 帧单次事务数据长度上限（保守值，防止异常长度撑爆内存）
        return Err(CollectorError::InvalidFrame(format!(
            "mc response data length too large: {len}"
        )));
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| CollectorError::IoError(format!("mc read body: {e}")))?;
    let end_code = u16::from_le_bytes([body[0], body[1]]);
    Ok((end_code, body[2..].to_vec()))
}

/// 读取 3E 响应帧（子头 `0xD000`），返回（结束代码，数据区）。
pub(crate) async fn read_response_3e(stream: &mut TcpStream) -> CollectorResult<(u16, Vec<u8>)> {
    let mut head = [0u8; HEADER_LEN_3E];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|e| CollectorError::IoError(format!("mc read header: {e}")))?;
    if head[0..2] != SUBHEADER_RESP_3E {
        return Err(CollectorError::InvalidFrame(format!(
            "mc invalid 3E response subheader 0x{:02X}{:02X}",
            head[0], head[1]
        )));
    }
    read_len_body(stream, u16::from_le_bytes([head[7], head[8]]) as usize).await
}

/// 读取 4E 响应帧（子头 `0xD400` + 序列号回显校验），返回（结束代码，数据区）。
pub(crate) async fn read_response_4e(
    stream: &mut TcpStream,
    serial: u16,
) -> CollectorResult<(u16, Vec<u8>)> {
    let mut head = [0u8; HEADER_LEN_4E];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|e| CollectorError::IoError(format!("mc read header: {e}")))?;
    if head[0..2] != SUBHEADER_RESP_4E {
        return Err(CollectorError::InvalidFrame(format!(
            "mc invalid 4E response subheader 0x{:02X}{:02X}",
            head[0], head[1]
        )));
    }
    let resp_serial = u16::from_le_bytes([head[2], head[3]]);
    if resp_serial != serial {
        return Err(CollectorError::InvalidFrame(format!(
            "mc 4E serial mismatch: sent {serial}, got {resp_serial}"
        )));
    }
    read_len_body(stream, u16::from_le_bytes([head[9], head[10]]) as usize).await
}

/// 读取 1E 响应帧（无长度字段，读操作按点数推导数据长度），返回（状态码，数据区）。
///
/// 1E 响应 = `(功能码|0x80)(1) | 状态码(1) | 数据…`；写操作无数据。
pub(crate) async fn read_response_1e(
    stream: &mut TcpStream,
    func: u8,
    is_read: bool,
    is_bit: bool,
    points: u16,
) -> CollectorResult<(u16, Vec<u8>)> {
    let mut hdr = [0u8; 2];
    stream
        .read_exact(&mut hdr)
        .await
        .map_err(|e| CollectorError::IoError(format!("mc read 1E header: {e}")))?;
    let expect_fc = func | 0x80;
    if hdr[0] != expect_fc {
        return Err(CollectorError::InvalidFrame(format!(
            "mc 1E response func 0x{:02X} != expected 0x{:02X}",
            hdr[0], expect_fc
        )));
    }
    let status = hdr[1];
    let data = if is_read && status == 0 {
        // 位元件 2 点/字节，字元件 1 点 = 2 字节
        let data_len = if is_bit {
            (points as usize).div_ceil(2)
        } else {
            points as usize * 2
        };
        if data_len > 512 {
            return Err(CollectorError::InvalidFrame(format!(
                "mc 1E response data too large: {data_len}"
            )));
        }
        let mut d = vec![0u8; data_len];
        stream
            .read_exact(&mut d)
            .await
            .map_err(|e| CollectorError::IoError(format!("mc read 1E data: {e}")))?;
        d
    } else {
        Vec::new()
    };
    Ok((status as u16, data))
}

/// 将结束代码/状态码的非零值转为协议错误。
pub(crate) fn check_end_code(end_code: u16) -> CollectorResult<()> {
    if end_code == 0 {
        Ok(())
    } else {
        Err(CollectorError::InvalidFrame(format!(
            "mc end code 0x{end_code:04X}"
        )))
    }
}

/// 按 `dt` 解码字元件数据（每字小端，低地址在前）。
///
/// 复用跨协议 `decode_binary_value`：先按字小端拼成字节流，再整体反转为大端规范序。
pub(crate) fn decode_words(data: &[u8], dt: PointDataType) -> Option<f64> {
    // MC 字数据为「字内小端 + 低地址字在前」。整体反转即得到规范大端序
    // （首字在高位），与 decode_binary_value 的 from_be 约定一致。
    let mut bytes = data.to_vec();
    bytes.reverse();
    let (v, _) = crate::south::protocol::decode_binary_value(&bytes, dt)?;
    Some(v)
}

/// 解码位元件打包数据：`index` 为点序号（0-based），首点在高半字节。
pub(crate) fn decode_bit(data: &[u8], index: usize) -> Option<f64> {
    let byte = *data.get(index / 2)?;
    let nibble = if index.is_multiple_of(2) {
        byte >> 4
    } else {
        byte & 0x0F
    };
    Some((nibble & 0x01) as f64)
}

/// 将 f64 编码为待写字的原始字节（小端），字数为 `dt` 所需寄存器数。
pub(crate) fn encode_words(value: f64, dt: PointDataType) -> Vec<u8> {
    // 先按大端规范得到字节，再按字反转回小端
    let be = match dt {
        PointDataType::U16 => (value as u16).to_be_bytes().to_vec(),
        PointDataType::I16 => (value as i16).to_be_bytes().to_vec(),
        PointDataType::U32 => (value as u32).to_be_bytes().to_vec(),
        PointDataType::I32 => (value as i32).to_be_bytes().to_vec(),
        PointDataType::F32 => (value as f32).to_be_bytes().to_vec(),
        PointDataType::U64 => (value as u64).to_be_bytes().to_vec(),
        PointDataType::I64 => (value as i64).to_be_bytes().to_vec(),
        PointDataType::F64 => value.to_be_bytes().to_vec(),
    };
    // 大端规范序整体反转即得到 MC 期望的「字内小端 + 低地址字在前」
    let mut out = be;
    out.reverse();
    out
}

/// 一次批量读片段的规划结果（决策表 #7：协议级批量读 + 超帧上限自动分片）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchPlan {
    /// 片段起始软元件编号（字元件为字地址，位元件为位地址）
    pub start: u32,
    /// 片段需读取的单元数（字元件=字数，位元件=位数），恒 ≤ 单帧上限
    pub units: u16,
    /// 该片段覆盖的点位在输入切片中的下标区间（输入需按地址升序）
    pub range: std::ops::Range<usize>,
}

/// 规划批量读片段：把（地址, 占用单元数）按地址升序合并，超单帧上限时切分为多帧。
///
/// - `gap`：允许合并的地址空洞（字元件按字、位元件按位）。相邻或空洞 ≤ `gap` 的点位合并进
///   同一帧以减少事务数；空洞过大则各自成帧，避免读取大片无谓区间。
/// - `max_units`：单帧可读单元数上限（3E/4E 字元件 [`MAX_WORDS_PER_FRAME`]、位元件
///   [`MAX_BITS_PER_FRAME`]；1E 用 [`MAX_UNITS_PER_FRAME_1E`]）。
///
/// 返回的每个片段 `units` 恒 ≤ `max_units`，因此超出单帧能力的点位集合会自动回退为多帧读取。
/// 输入 `items` 必须已按地址升序排序。
pub(crate) fn plan_batches(items: &[(u32, u16)], max_units: u16, gap: u16) -> Vec<BatchPlan> {
    let mut out: Vec<BatchPlan> = Vec::new();
    if items.is_empty() {
        return out;
    }
    let max = max_units.max(1) as u32;
    let gap = gap as u32;
    let mut start = items[0].0;
    let mut end = start + items[0].1.max(1) as u32;
    let mut range = 0..1;
    for (i, (head, size)) in items.iter().enumerate().skip(1) {
        let head = *head;
        let cand_end = (head + (*size).max(1) as u32).max(end);
        if head <= end + gap && cand_end - start <= max {
            // 可并入当前帧
            end = cand_end;
            range.end = i + 1;
        } else {
            // 空洞过大或并入后超单帧上限 → 先落定当前帧，再以本点位另起一帧
            out.push(BatchPlan {
                start,
                units: (end - start) as u16,
                range: range.clone(),
            });
            start = head;
            end = head + (*size).max(1) as u32;
            range = i..i + 1;
        }
    }
    out.push(BatchPlan {
        start,
        units: (end - start) as u16,
        range,
    });
    out
}
