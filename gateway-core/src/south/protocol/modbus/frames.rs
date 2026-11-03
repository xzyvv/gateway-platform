//! Modbus RTU 帧构建与响应解析

use super::{CollectorError, CollectorResult};
use crate::engine::util::crc::crc16;
use crate::south::bus::FrameBuffer;

// ===== Modbus RTU 帧处理 =====

/// 拼 RTU 帧：`prefix` + CRC16（小端）
///
/// 协议工具（参数探测 / 从站扫描 / 寄存器扫描 / 协议测试）此前在 `gateway-api`
/// 各复制了一份「裸帧 + CRC」的实现。统一到此处，避免 CRC 字节序写错时只修一处。
pub fn rtu_frame(prefix: &[u8]) -> Vec<u8> {
    let mut frame = prefix.to_vec();
    frame.extend_from_slice(&crc16(prefix).to_le_bytes());
    frame
}

/// 构建读请求帧（FC01/02/03/04 通用格式）
pub fn build_read_request(slave: u8, fc: u8, addr: u16, count: u16) -> Vec<u8> {
    let mut frame = vec![slave, fc];
    frame.extend_from_slice(&addr.to_be_bytes());
    frame.extend_from_slice(&count.to_be_bytes());
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// 构建写多线圈请求帧（FC15 = 0x0F）
///
/// 帧格式：`[slave][fc][addrH addrL][qtyH qtyL][byte_count][coils bytes...][CRClo CRChi]`
/// - `coils` 按位打包：每 bit 对应一个线圈，第 0 个线圈在字节 0 的 bit0。
/// - `byte_count` 必须是 `((count as usize) + 7) / 8`（不足字节补 0）。
/// - 返回 8 字节回显响应：写请求回显 `[slave][fc][addrH addrL][qtyH qtyL]` + CRC。
pub(crate) fn build_write_multi_coils(slave: u8, addr: u16, count: u16, coils: &[bool]) -> Vec<u8> {
    let byte_count = (count as usize).div_ceil(8);
    let mut data = Vec::with_capacity(byte_count);
    for chunk_idx in 0..byte_count {
        let mut byte = 0u8;
        for bit in 0..8 {
            let i = chunk_idx * 8 + bit;
            if i < coils.len() && coils[i] {
                byte |= 1 << bit;
            }
        }
        data.push(byte);
    }
    let mut frame = vec![slave, 0x0F];
    frame.extend_from_slice(&addr.to_be_bytes());
    frame.extend_from_slice(&count.to_be_bytes());
    frame.push(byte_count as u8);
    frame.extend_from_slice(&data);
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// 构建写多寄存器请求帧（FC16 = 0x10）
///
/// 帧格式：`[slave][fc][addrH addrL][qtyH qtyL][byte_count][regH regL...][CRClo CRChi]`
/// - `values` 为 16 位寄存器值；`count` 必须等于 `values.len()` 且 ≤ 123。
/// - 返回 8 字节回显响应。
pub(crate) fn build_write_multi_registers(slave: u8, addr: u16, values: &[u16]) -> Vec<u8> {
    let count = values.len() as u16;
    let byte_count = values.len() * 2;
    let mut frame = vec![slave, 0x10];
    frame.extend_from_slice(&addr.to_be_bytes());
    frame.extend_from_slice(&count.to_be_bytes());
    frame.push(byte_count as u8);
    for v in values {
        frame.extend_from_slice(&v.to_be_bytes());
    }
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// 解析读寄存器响应（FC03/FC04）
pub(crate) fn parse_read_response(
    slave: u8,
    fc: u8,
    expected_count: u16,
    response: &[u8],
) -> CollectorResult<Vec<u16>> {
    if response.len() < 5 {
        return Err(CollectorError::InvalidFrame("response too short".into()));
    }
    if response[0] != slave {
        return Err(CollectorError::InvalidFrame(format!(
            "slave mismatch: expected {}, got {}",
            slave, response[0]
        )));
    }
    // Bug 121: Modbus 异常响应 = 请求 fc | 0x80。
    // Rust 优先级：`==` 高于 `|`，所以原表达式 `response[1] == fc | 0x80`
    // 等价于 `(response[1] == fc) | 0x80`，即「先比较 response[1]==fc，再与 0x80 取或」。
    // 在 fc != 0 时结果相同，但 fc == 0 时语义错误。加显式括号。
    if response[1] == (fc | 0x80) {
        return Err(CollectorError::ModbusException(response[2]));
    }
    if response[1] != fc {
        return Err(CollectorError::InvalidFrame(format!(
            "function code mismatch: expected {}, got {}",
            fc, response[1]
        )));
    }
    let byte_count = response[2] as usize;
    let expected_bytes = (expected_count as usize) * 2;
    // Bug 124: expected_count == 0 → expected_bytes = 0，上游可能误发空请求。
    if expected_bytes == 0 {
        return Err(CollectorError::InvalidFrame(
            "expected_count must be > 0".into(),
        ));
    }
    if byte_count != expected_bytes {
        return Err(CollectorError::InvalidFrame(format!(
            "byte count mismatch: expected {}, got {}",
            expected_bytes, byte_count
        )));
    }
    let data_end = 3 + byte_count;
    if response.len() < data_end + 2 {
        return Err(CollectorError::InvalidFrame("response truncated".into()));
    }
    let crc_calc = crc16(&response[..data_end]);
    let crc_recv = response[data_end] as u16 | ((response[data_end + 1] as u16) << 8);
    if crc_calc != crc_recv {
        return Err(CollectorError::ChecksumError);
    }
    let mut regs = Vec::with_capacity(expected_count as usize);
    for i in 0..expected_count as usize {
        let hi = response[3 + i * 2] as u16;
        let lo = response[3 + i * 2 + 1] as u16;
        regs.push((hi << 8) | lo);
    }
    Ok(regs)
}

/// 解析读线圈/离散输入响应（FC01/FC02）
pub(crate) fn parse_coils_response(
    slave: u8,
    fc: u8,
    expected_count: u16,
    response: &[u8],
) -> CollectorResult<Vec<bool>> {
    if response.len() < 5 {
        return Err(CollectorError::InvalidFrame("response too short".into()));
    }
    if response[0] != slave {
        return Err(CollectorError::InvalidFrame(format!(
            "slave mismatch: expected {}, got {}",
            slave, response[0]
        )));
    }
    if response[1] == (fc | 0x80) {
        return Err(CollectorError::ModbusException(response[2]));
    }
    if response[1] != fc {
        return Err(CollectorError::InvalidFrame(format!(
            "function code mismatch: expected {}, got {}",
            fc, response[1]
        )));
    }
    let byte_count = response[2] as usize;
    // Bug 122: expected_count == 0 时 expected_bytes = 0，会让上游发送空帧。
    // 拒绝 0 长度请求，明确返回错误。
    let expected_bytes = (expected_count as usize).div_ceil(8);
    if expected_bytes == 0 {
        return Err(CollectorError::InvalidFrame(
            "expected_count must be > 0".into(),
        ));
    }
    if byte_count != expected_bytes {
        return Err(CollectorError::InvalidFrame(format!(
            "coil byte count mismatch: expected {}, got {}",
            expected_bytes, byte_count
        )));
    }
    let data_end = 3 + byte_count;
    if response.len() < data_end + 2 {
        return Err(CollectorError::InvalidFrame("response truncated".into()));
    }
    let crc_calc = crc16(&response[..data_end]);
    let crc_recv = response[data_end] as u16 | ((response[data_end + 1] as u16) << 8);
    if crc_calc != crc_recv {
        return Err(CollectorError::ChecksumError);
    }
    // 按位解析：第 1 个线圈在字节 0 的 bit0
    let mut bits = Vec::with_capacity(expected_count as usize);
    for i in 0..expected_count as usize {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let bit = (response[3 + byte_idx] >> bit_idx) & 0x01;
        bits.push(bit != 0);
    }
    Ok(bits)
}

/// Modbus RTU 帧提取（从 FrameBuffer 中切出完整响应帧）
///
/// Bug 62 修复：byte_count 字段仅对 fc=0x01/0x02/0x03/0x04 响应有效，
/// 其他 fc（0x05/0x06/0x0F/0x10 等）的响应布局不同——没有 count 字节，
/// 直接使用 buf[2] 作为 byte_count 会得到错误帧长度。
///
/// 按 fc 分支：
/// - fc & 0x80 != 0: 异常响应，固定 5 字节（[slave][fc+0x80][except][crc_lo][crc_hi]）
/// - fc = 0x01/0x02/0x03/0x04: 数据响应，buf[2] = byte_count
/// - 其他: 单寄存器/线圈响应，固定 6 字节（[slave][fc][data_hi][data_lo][crc_lo][crc_hi]）
pub(crate) fn extract_modbus_frame(
    fb: &mut FrameBuffer,
    expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    loop {
        let buf = fb.peek_all();
        if buf.len() < 5 {
            return None;
        }

        let fc = buf[1];

        if fc & 0x80 != 0 {
            // 异常响应：固定 5 字节
            if buf.len() < 5 {
                return None;
            }
            let crc_calc = crc16(&buf[..3]);
            let crc_recv = buf[3] as u16 | ((buf[4] as u16) << 8);
            if crc_calc == crc_recv {
                return Some(fb.pop(5));
            }
            let _ = fb.pop(1);
            continue;
        }

        let total: usize = match fc {
            0x01..=0x04 => {
                // 数据读响应：buf[2] = byte_count
                if buf.len() < 3 {
                    return None;
                }
                let byte_count = buf[2] as usize;
                if byte_count == 0 || byte_count > 253 {
                    let _ = fb.pop(1);
                    continue;
                }
                if let Some(n) = expected_count {
                    // 修复(C-3)：响应字节数公式必须区分"位"与"寄存器"。
                    //   FC01/FC02 读线圈/离散输入 → 每 8 点 1 字节，即 ceil(n / 8)
                    //   FC03/FC04 读保持/输入寄存器 → 每点 2 字节，即 n * 2
                    // 旧实现一律按 n * 2 校验。当前唯一入口 `read_coils_with_retry`
                    // 恒传 None 故未触发；一旦将来做"批量读线圈优化"把 expected_count
                    // 传进来，校验必然失败 → 逐字节丢弃重同步 → 批量线圈恒 NoReply
                    // 而单点线圈正常，现场极难定位。
                    let expected_bytes = match fc {
                        0x01 | 0x02 => (n as usize).div_ceil(8),
                        _ => (n as usize) * 2,
                    };
                    if byte_count != expected_bytes {
                        let _ = fb.pop(1);
                        continue;
                    }
                }
                3 + byte_count + 2
            }
            // Bug C1: 写响应是「请求回显」，固定 8 字节
            // [slave][fc][4 data][crc_lo][crc_hi]（FC05/06 回显地址+值，FC0F/10 回显地址+数量）
            // 原实现返回 6，导致 CRC 校验对象错位（拿数据字节当 CRC），
            // 串口写操作（FC05/06/0F/10）的响应恒被逐字节丢弃 → 永远 NoReply。
            0x05 | 0x06 | 0x0F | 0x10 | 0x16 => 8,
            // FC21(0x15 写文件记录) 响应为变长回显，暂不支持：交由默认分支丢弃重同步
            _ => {
                // 未知 fc：保守丢弃 1 字节等下一个
                let _ = fb.pop(1);
                continue;
            }
        };

        if buf.len() < total {
            return None;
        }
        let data_end = total - 2;
        let crc_calc = crc16(&buf[..data_end]);
        let crc_recv = buf[data_end] as u16 | ((buf[data_end + 1] as u16) << 8);
        if crc_calc == crc_recv {
            return Some(fb.pop(total));
        }
        let _ = fb.pop(1);
        continue;
    }
}
