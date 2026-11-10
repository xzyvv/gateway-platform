//! 协议帧编解码工具（跨协议复用，避免重复实现）
//!
//! - CRC16：Modbus RTU 帧校验（多项式 0xA001）→ 已上提至 `crate::engine::codec::crc`
//! - LRC / ASCII 转换：Modbus ASCII 模式帧工具（仅 south 串口链路使用）
//!
//! 所有协议采集器与帧提取层统一从此模块引入，
//! 消除 `bus_manager` 与 `modbus_collector` 各自定义的重复。

use crate::engine::codec::crc::crc16;

/// 累加和校验（CS）：字节累加 mod 256
///
/// DLT645 / CJT188（帧首到 CS 前）与 IEC101 FT1.2 的帧校验算法相同，
/// 统一在此实现，避免各协议各写一份后出现实现漂移。
pub(crate) fn checksum8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// 十六进制字节转数值（0-15），输入必须是 ASCII 十六进制字符
pub(crate) fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        // 非 hex 字节不应 panic，返回 None 让调用方走错误路径
        _ => None,
    }
}

/// PR-1: Modbus ASCII LRC 校验（纵向冗余校验）
///
/// LRC = -(ADDR + FUNC + DATA...) mod 256
/// 用于 ASCII 模式帧完整性校验，与 CRC16（RTU 模式）对应。
pub fn lrc8(data: &[u8]) -> u8 {
    let sum = checksum8(data);
    // LRC = -(sum) mod 256。用 wrapping_neg 而非 `-(sum as i8) as u8`：
    // 后者在 sum == 0x80（i8::MIN 的位模式）时 `-(-128)` 在 debug 构建下会 panic
    // （release 静默回绕），导致偶发、难以复现的 ASCII 模式采集崩溃。
    sum.wrapping_neg()
}

/// Modbus RTU -> ASCII 转换
///
/// ASCII 帧格式：:ADDR FUNC DATA... LRC CR LF
/// 数据区每字节转为 2 个 ASCII HEX 字符（大写）。
/// LRC = -(ADDR + FUNC + DATA...) mod 256
pub fn rtu_to_ascii(rtu: &[u8]) -> Vec<u8> {
    if rtu.len() < 2 {
        return Vec::new();
    }
    // RTU 去掉末尾 2 字节 CRC，ASCII 用 LRC
    let payload = &rtu[..rtu.len() - 2];
    let lrc = lrc8(payload);

    let mut ascii = Vec::with_capacity(1 + payload.len() * 2 + 4);
    ascii.push(b':');
    for &b in payload {
        ascii.push(hex_hi(b));
        ascii.push(hex_lo(b));
    }
    ascii.push(hex_hi(lrc));
    ascii.push(hex_lo(lrc));
    ascii.push(b'\r');
    ascii.push(b'\n');
    ascii
}

/// Modbus ASCII -> RTU 转换
///
/// 解析 ":ADDR FUNC DATA... LRC\r\n"，校验 LRC 后重组 RTU 帧（追加 CRC16）。
pub fn ascii_to_rtu(ascii: &[u8]) -> Option<Vec<u8>> {
    if ascii.len() < 9 || ascii[0] != b':' {
        return None;
    }
    // 去除 \r\n
    let body = if ascii.ends_with(b"\r\n") {
        &ascii[1..ascii.len() - 2]
    } else if ascii.last() == Some(&b'\n') {
        &ascii[1..ascii.len() - 1]
    } else {
        &ascii[1..]
    };
    if body.len() < 4 || body.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(body.len() / 2);
    for chunk in body.chunks(2) {
        let hi = hex_digit(chunk[0])?;
        let lo = hex_digit(chunk[1])?;
        bytes.push((hi << 4) | lo);
    }
    // LRC 校验
    let lrc_recv = *bytes.last()?;
    let payload = &bytes[..bytes.len() - 1];
    let lrc_calc = lrc8(payload);
    if lrc_calc != lrc_recv {
        return None;
    }
    // 追加 CRC16
    let crc = crc16(payload);
    let mut rtu = payload.to_vec();
    rtu.push((crc & 0xFF) as u8);
    rtu.push(((crc >> 8) & 0xFF) as u8);
    Some(rtu)
}

fn hex_hi(b: u8) -> u8 {
    let v = (b >> 4) & 0x0F;
    if v < 10 {
        b'0' + v
    } else {
        b'A' + (v - 10)
    }
}

fn hex_lo(b: u8) -> u8 {
    let v = b & 0x0F;
    if v < 10 {
        b'0' + v
    } else {
        b'A' + (v - 10)
    }
}

/// 解码 BCD 值（小端：低字节在前）
///
/// 每个字节高/低 4 位必须是 0-9，出现 0x0A-0x0F 视为非法返回 None
/// （上层据此标记 Bad，不再静默跳过）。DLT645 / CJT188 共用。
pub(crate) fn decode_bcd_value(bytes: &[u8]) -> Option<f64> {
    let mut value: i64 = 0;
    for &b in bytes.iter().rev() {
        let hi = (b >> 4) & 0x0F;
        let lo = b & 0x0F;
        if hi > 9 || lo > 9 {
            return None;
        }
        value = value * 100 + (hi as i64) * 10 + (lo as i64);
    }
    Some(value as f64)
}

/// 编码 f64 为 BCD 字节（小端：低字节在前）——与 [`decode_bcd_value`] 对偶。
///
/// 仅支持非负整数（负数 / NaN / 越界返回 None）。返回字节数由调用方决定
/// （不同字段长度不同：电能量 4 字节、需量 3 字节等）。DLT645 / CJT188 共用。
pub(crate) fn encode_bcd_value(value: f64, out_len: usize) -> Option<Vec<u8>> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    let mut n = value as u64;
    // 提取每位十进制数字（小端）：先最低位
    let mut digits: Vec<u8> = Vec::with_capacity(out_len * 2);
    for _ in 0..(out_len * 2) {
        digits.push((n % 10) as u8);
        n /= 10;
    }
    if n > 0 {
        // 超过目标长度能容纳的位数
        return None;
    }
    // 按字节打包（低字节在前）：每字节 = (高 nibble << 4) | 低 nibble
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let lo = digits[i * 2];
        let hi = digits[i * 2 + 1];
        out.push((hi << 4) | lo);
    }
    Some(out)
}

#[cfg(test)]
mod tests;
