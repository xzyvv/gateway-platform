//! BACnet MSTP 帧处理（CRC、构建、解析）

use crate::south::bus::FrameBuffer;
use crate::south::protocol::{CollectorError, CollectorResult};

const PREAMBLE: [u8; 2] = [0x55, 0xFF];
pub(crate) const MAX_DATA_LEN: usize = 512;

pub fn crc8_header(data: &[u8]) -> u8 {
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

/// MS/TP 数据 CRC-16（多项式 0xA001，初始值 0xFFFF，结果取反）
pub fn crc16_data(data: &[u8]) -> u16 {
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

// ===== 帧编解码 =====

/// 构建 MS/TP 帧
pub fn build_mstp_frame(frame_type: u8, dest_addr: u8, src_addr: u8, data: &[u8]) -> Vec<u8> {
    let data_len = data.len();
    let mut frame = Vec::with_capacity(8 + data_len + if data_len > 0 { 2 } else { 0 });

    // 前导码
    frame.extend_from_slice(&PREAMBLE);

    // 头部：Type + DADDR + SADDR + Length(2 BE)
    frame.push(frame_type);
    frame.push(dest_addr);
    frame.push(src_addr);
    frame.extend_from_slice(&(data_len as u16).to_be_bytes());

    // 头 CRC（计算范围：Type + DADDR + SADDR + Length）
    let header = &frame[2..7];
    frame.push(crc8_header(header));

    // 数据 + 数据 CRC
    if data_len > 0 {
        frame.extend_from_slice(data);
        let crc = crc16_data(data);
        frame.push((crc & 0xFF) as u8); // 低字节
        frame.push((crc >> 8) as u8); // 高字节
    }

    frame
}

/// 解析 MS/TP 帧，返回 (帧类型, 目标地址, 源地址, 数据)
///
/// 输入应为以 0x55 0xFF 开头的完整帧（含前导码和 CRC）。
/// 验证 CRC 后返回数据负载。
pub fn parse_mstp_frame(
    frame: &[u8],
    metrics: &crate::engine::metrics::Metrics,
) -> CollectorResult<(u8, u8, u8, Vec<u8>)> {
    // 最小帧长：前导码(2) + 头(5) + 头CRC(1) = 8
    if frame.len() < 8 {
        return Err(CollectorError::InvalidFrame(format!(
            "MS/TP frame too short: {} bytes",
            frame.len()
        )));
    }
    // 检查前导码
    if frame[0] != PREAMBLE[0] || frame[1] != PREAMBLE[1] {
        return Err(CollectorError::InvalidFrame(format!(
            "invalid preamble: 0x{:02X} 0x{:02X}",
            frame[0], frame[1]
        )));
    }

    let frame_type = frame[2];
    let dest_addr = frame[3];
    let src_addr = frame[4];
    let data_len = u16::from_be_bytes([frame[5], frame[6]]) as usize;

    if data_len > MAX_DATA_LEN {
        return Err(CollectorError::InvalidFrame(format!(
            "data length exceeds max: {} > {}",
            data_len, MAX_DATA_LEN
        )));
    }

    // 头 CRC 校验
    let header = &frame[2..7];
    let header_crc = frame[7];
    let calc_header_crc = crc8_header(header);
    if header_crc != calc_header_crc {
        metrics.inc_protocol_decode_error();
        return Err(CollectorError::ChecksumError);
    }

    // 数据 + 数据 CRC
    if data_len == 0 {
        return Ok((frame_type, dest_addr, src_addr, Vec::new()));
    }

    let total_len = 8 + data_len + 2;
    if frame.len() < total_len {
        return Err(CollectorError::InvalidFrame(format!(
            "frame truncated: got {} bytes, expected {}",
            frame.len(),
            total_len
        )));
    }

    let data = &frame[8..8 + data_len];
    let recv_crc_lo = frame[8 + data_len] as u16;
    let recv_crc_hi = frame[8 + data_len + 1] as u16;
    let recv_crc = (recv_crc_hi << 8) | recv_crc_lo;

    let calc_crc = crc16_data(data);
    if recv_crc != calc_crc {
        metrics.inc_protocol_decode_error();
        return Err(CollectorError::ChecksumError);
    }

    Ok((frame_type, dest_addr, src_addr, data.to_vec()))
}

// NPDU 头跳过逻辑统一在 BacnetCollector::skip_npdu 中实现（bacnet.rs），供 MS/TP 和 BACnet/IP 复用

/// MS/TP 帧提取器（供 serial_task 注册使用）
///
/// 在 FrameBuffer 中搜索前导码 0x55 0xFF，提取完整 MS/TP 帧。
/// 校验头 CRC 和数据 CRC，校验失败的帧被跳过。
pub fn extract_mstp_frame(fb: &mut FrameBuffer, _expected: Option<u16>) -> Option<Vec<u8>> {
    loop {
        let buf = fb.peek_all();
        if buf.len() < 8 {
            return None;
        }

        // A8 修复：旧实现 `buf.windows(2).position(|w| w == PREAMBLE)` 会在数据字段
        // 中恰好出现 0x55 0xFF 字节对时把该位置误识别为新帧头（大数据块中概率不可忽略），
        // 导致完整帧被错位截断、值错配。改为严格按字节推进：仅当 buf 起始 2 字节就是
        // 前导码时才认为对齐，否则 pop 1 字节继续搜索。
        if buf[0] != PREAMBLE[0] || buf[1] != PREAMBLE[1] {
            let _ = fb.pop(1);
            continue;
        }

        // 重新读取（此时 buf 必以 PREAMBLE 起始）
        let buf = fb.peek_all();
        if buf.len() < 8 {
            return None;
        }

        // 读取数据长度
        let data_len = u16::from_be_bytes([buf[5], buf[6]]) as usize;
        if data_len > MAX_DATA_LEN {
            // 无效长度，跳过前导码继续搜索
            let _ = fb.pop(2);
            continue;
        }

        // 完整帧长度：前导码(2) + 头(5) + 头CRC(1) + data + 数据CRC(2, 仅当 data_len > 0)
        let total_len = 8 + data_len + if data_len > 0 { 2 } else { 0 };
        if buf.len() < total_len {
            return None;
        }

        // 校验头 CRC
        let header = &buf[2..7];
        let header_crc = buf[7];
        if crc8_header(header) != header_crc {
            // 头 CRC 校验失败，跳过前导码继续
            let _ = fb.pop(2);
            continue;
        }

        // 校验数据 CRC（如果存在）
        if data_len > 0 {
            let data = &buf[8..8 + data_len];
            let recv_lo = buf[8 + data_len] as u16;
            let recv_hi = buf[8 + data_len + 1] as u16;
            let recv_crc = (recv_hi << 8) | recv_lo;
            if crc16_data(data) != recv_crc {
                // 数据 CRC 校验失败，跳过前导码继续
                let _ = fb.pop(2);
                continue;
            }
        }

        return Some(fb.pop(total_len));
    }
}
