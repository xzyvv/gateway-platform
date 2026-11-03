//! DLT645 / CJT188 共享帧逻辑（组帧 / 解帧 / 提取）
//!
//! 两者帧结构高度相似（均以 0x68 起始、0x16 结束、CS 累加和校验），
//! 仅 DLT645 多一个 0x68 第二头字节、地址 6 字节且 1997 版地址高字节在前。
//! 本模块提供共用的组帧、解帧与提取函数，各协议 frames.rs 调用并指定差异。

use crate::engine::metrics::Metrics;
use crate::south::bus::FrameBuffer;
use crate::south::protocol::{CollectorError, CollectorResult};

use super::util::checksum8;

/// DLT645 / CJT188 共用组帧：`0x68 [addr] [0x68] C L DATA CS 0x16`
///
/// - `second_header`：DLT645 为 true（地址后补第二个 0x68），CJT188 为 false
/// - `addr_reversed`：DLT645-1997 地址传输高字节在前，需反转 6 字节地址
/// - `tag`：日志前缀（`DLT645` / `DLT645-1997` / `CJT188`），用于错误消息
///
/// 数据区不做 +0x33 偏移，由调用方按版本预先处理（写路径约定）。
/// 数据过长（> 255，LEN 为单字节）或反转地址长度非 6 时返回空 `Vec`。
pub(crate) fn build_68_frame(
    addr: &[u8],
    second_header: bool,
    addr_reversed: bool,
    ctrl: u8,
    data: &[u8],
    tag: &str,
) -> Vec<u8> {
    if data.len() > 0xFF {
        tracing::error!(
            "{tag} frame data length {} exceeds protocol max 255, frame dropped",
            data.len()
        );
        return Vec::new();
    }
    let mut frame = Vec::with_capacity(12 + data.len());
    frame.push(0x68);
    if addr_reversed {
        if addr.len() != 6 {
            tracing::error!(
                "{tag} expects 6-byte address, got {}, frame dropped",
                addr.len()
            );
            return Vec::new();
        }
        let mut be = [0u8; 6];
        for i in 0..6 {
            be[i] = addr[5 - i];
        }
        frame.extend_from_slice(&be);
    } else {
        frame.extend_from_slice(addr);
    }
    if second_header {
        frame.push(0x68);
    }
    frame.push(ctrl);
    frame.push(data.len() as u8);
    frame.extend_from_slice(data);
    // 校验和：从第一个 68 到 CS 前
    frame.push(checksum8(&frame));
    frame.push(0x16);
    frame
}

/// DLT645 / CJT188 共用解帧：校验 `0x68 … CS … 0x16`，返回数据区
///
/// - `second_header`：DLT645 校验地址后第二个 0x68，CJT188 不校验
/// - `addr_reversed`：DLT645-1997 回显地址高字节在前，比较前先反转
/// - `remove_offset`：数据区是否做 -0x33（DLT645-2007 / CJT188 Vendor7 为 true）
/// - `metrics`：校验和失败时递增协议解码错误计数（仅 CJT188 传入）
///
/// `expected_addr` 长度决定地址比对宽度（DLT645=6、CJT188=7）。
pub(crate) fn parse_68_response(
    response: &[u8],
    expected_addr: &[u8],
    second_header: bool,
    addr_reversed: bool,
    remove_offset: bool,
    metrics: Option<&Metrics>,
) -> CollectorResult<Vec<u8>> {
    // 最小帧长：1(68) + 地址 + [1(68)] + 1(C) + 1(L) + 0(DATA) + 1(CS) + 1(16) = 12
    if response.len() < 12 {
        return Err(CollectorError::InvalidFrame("frame too short".into()));
    }
    if response[0] != 0x68 {
        return Err(CollectorError::InvalidFrame("invalid start marker".into()));
    }
    if second_header && response[7] != 0x68 {
        return Err(CollectorError::InvalidFrame("invalid start marker".into()));
    }
    // 地址校验：V1997 回显高字节在前，先反转再与低字节在前的 expected_addr 比对
    if addr_reversed {
        let mut be = [0u8; 6];
        be.copy_from_slice(&response[1..7]);
        be.reverse();
        if &be[..] != expected_addr {
            return Err(CollectorError::InvalidFrame("address mismatch".into()));
        }
    } else {
        let end = 1 + expected_addr.len();
        if &response[1..end] != expected_addr {
            return Err(CollectorError::InvalidFrame("address mismatch".into()));
        }
    }
    let ctrl = response[8];
    if ctrl & 0x80 == 0 {
        return Err(CollectorError::InvalidFrame("not a response frame".into()));
    }
    if ctrl & 0x40 != 0 {
        return Err(CollectorError::InvalidFrame(
            "device abnormal response".into(),
        ));
    }
    // D5(0x20) = 后续帧标志。多帧应答数据跨帧续传，本实现基于「一次请求 → 一帧应答」
    // 的单帧事务模型，按单帧解析会截断长数据并静默上报错误值。显式失败优于静默出错。
    if ctrl & 0x20 != 0 {
        return Err(CollectorError::InvalidFrame(
            "multi-frame response (D5=0x20) unsupported: data would be truncated".into(),
        ));
    }
    let data_len = response[9] as usize;
    let data_start = 10;
    let data_end = data_start + data_len;
    if response.len() < data_end + 2 {
        return Err(CollectorError::InvalidFrame("frame truncated".into()));
    }
    if checksum8(&response[..data_end]) != response[data_end] {
        if let Some(m) = metrics {
            m.inc_protocol_decode_error();
        }
        return Err(CollectorError::ChecksumError);
    }
    if response[data_end + 1] != 0x16 {
        return Err(CollectorError::InvalidFrame("invalid end marker".into()));
    }
    let mut data = response[data_start..data_end].to_vec();
    if remove_offset {
        for b in data.iter_mut() {
            *b = b.wrapping_sub(0x33);
        }
    }
    Ok(data)
}

/// DLT645 / CJT188 帧提取（共用逻辑）
///
/// 帧结构：0x68 | 地址(6或7字节) | [0x68(DLT645)] | C | L | DATA | CS | 0x16
/// - DLT645: 0x68 + A0~A5(6) + 0x68 + C + L + DATA + CS + 0x16 (header=10字节)
/// - CJT188: 0x68 + A0~A6(7) + C + L + DATA + CS + 0x16 (header=10字节)
pub(crate) fn extract_dlt645_cjt188_frame(
    fb: &mut FrameBuffer,
    check_second_header: bool,
) -> Option<Vec<u8>> {
    loop {
        let buf = fb.peek_all();
        // 找起始字节 0x68
        let start = buf.iter().position(|&b| b == 0x68)?;
        if buf.len() < start + 10 {
            // 数据不足，跳过起始前的垃圾
            if start > 0 {
                let _ = fb.pop(start);
            }
            return None;
        }
        // DLT645 需校验第二个 0x68（在 start+7 位置）
        if check_second_header && buf[start + 7] != 0x68 {
            let _ = fb.pop(start + 1);
            continue;
        }
        let data_len = buf[start + 9] as usize;
        let total = 10 + data_len + 2; // header(10) + data + CS + 0x16
        if buf.len() < start + total {
            if start > 0 {
                let _ = fb.pop(start);
            }
            return None;
        }
        // 校验和 CS = 帧首到 CS 前所有字节之和 mod 256
        let cs_pos = start + 10 + data_len;
        if checksum8(&buf[start..cs_pos]) != buf[cs_pos] {
            let _ = fb.pop(start + 1);
            continue;
        }
        // 结束字节 0x16
        if buf[start + total - 1] != 0x16 {
            let _ = fb.pop(start + 1);
            continue;
        }
        return Some(fb.pop(start + total));
    }
}
