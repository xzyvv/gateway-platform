//! 通用帧校验算法
//!
//! 原位于 south/protocol/common/util.rs，因 north 的 modbus server 端
//! （passthrough）同样需要 RTU 帧校验而上提至 engine 中立层，
//! 使 north 不再反向依赖 south。

/// Modbus CRC16（多项式 0xA001，低位在前）
///
/// 帧提取层需要独立做 CRC 校验以在坏帧时重新对齐；
/// modbus_collector 构建请求帧和解析响应帧同样使用；
/// north 的 modbus server 端（passthrough）构造响应帧同样使用。
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests;
