//! 读寄存器帧构造与单地址判定
//!
//! 帧构造统一走 core 单一实现；判定看「这个地址存不存在」，
//! 异常帧（非法数据地址）判 `responded=false`（与从站扫描相反）。

use crate::web::debug_bus::{verify_modbus_rtu, BusErrorKind, ExchangeOutcome, FrameCheck};
use crate::web::services::modbus_read_request_frame as build_read_request;

use super::ScanCell;

/// 读 1 个寄存器/线圈的 Modbus RTU 帧（契约 §3.1）
///
/// 帧构造统一走 core 单一实现；`addr` 按 16 位寄存器地址钳制（调用方已保证范围）。
pub(super) fn read_frame(slave_id: i32, reg_type: &str, addr: i32) -> Vec<u8> {
    let fc = match reg_type {
        "holding" => 0x03,
        "input" => 0x04,
        "coils" => 0x01,
        _ => 0x02, // discrete
    };
    build_read_request(slave_id as u8, fc, (addr & 0xFFFF) as u16, 1)
}

/// Modbus 从站地址合法域
const MODBUS_SLAVE_MIN: i32 = 1;
const MODBUS_SLAVE_MAX: i32 = 247;
/// Modbus 寄存器地址域（0..=65535）
const MODBUS_REG_ADDR_MIN: i32 = 0;
const MODBUS_REG_ADDR_MAX: i32 = 65535;

/// 四个 `reg_type` 取值在 mc / fins 上复用为「软元件区」。
///
/// JSON 契约（`ScanBody`/`ScanCell`）不变，故不能再按 Modbus 语义理解 `reg_type`：
/// - mc：`holding`→D、`input`→W、`coils`→M、`discrete`→X（三菱常用四区）
/// - fins：`holding`→D、`input`→W、`coils`→CIO、`discrete`→A
///   FINS **没有 M 区**（M 是三菱软元件，欧姆龙位软元件为 CIO/AR），
///   故 `coils` 落到最常用的 CIO 位区，`discrete` 用 AR 辅助区以便与 `coils` 区分。
pub(super) fn area_for(protocol: &str, reg_type: &str) -> Option<&'static str> {
    match (protocol, reg_type) {
        ("mc", "holding") => Some("D"),
        ("mc", "input") => Some("W"),
        ("mc", "coils") => Some("M"),
        ("mc", "discrete") => Some("X"),
        ("fins", "holding") => Some("D"),
        ("fins", "input") => Some("W"),
        ("fins", "coils") => Some("CIO"),
        ("fins", "discrete") => Some("A"),
        _ => None,
    }
}

/// 按协议分叉的参数校验（各协议的站号/节点号与地址域宽度不同，不能用 Modbus 域套用）。
pub(super) fn validate_params(
    protocol: &str,
    slave_id: i32,
    start_addr: i32,
    end_addr: i32,
) -> Result<(), String> {
    match protocol {
        "mc" => {
            if !(0..=255).contains(&slave_id) {
                return Err(format!(
                    "slave_id（MC 站号）必须在 0..=255 范围内（3E/4E 帧头站号为 1 字节），当前 {slave_id}"
                ));
            }
            if !(0..=32767).contains(&start_addr) || !(0..=32767).contains(&end_addr) {
                return Err(format!(
                    "MC 软元件地址必须在 0..=32767 范围内（D 软元件 16 位地址上限），当前 {start_addr}..={end_addr}"
                ));
            }
        }
        "fins" => {
            if !(0..=254).contains(&slave_id) {
                return Err(format!(
                    "slave_id（FINS 节点号）必须在 0..=254 范围内（节点号 0 保留给网络中继），当前 {slave_id}"
                ));
            }
            if !(0..=65535).contains(&start_addr) || !(0..=65535).contains(&end_addr) {
                return Err(format!(
                    "FINS 软元件地址必须在 0..=65535 范围内（D 区 16 位地址上限），当前 {start_addr}..={end_addr}"
                ));
            }
        }
        _ => {
            if !(MODBUS_SLAVE_MIN..=MODBUS_SLAVE_MAX).contains(&slave_id) {
                return Err(format!(
                    "slave_id 必须在 {}..={} 范围内（Modbus 合法域），当前 {}",
                    MODBUS_SLAVE_MIN, MODBUS_SLAVE_MAX, slave_id
                ));
            }
            if !(MODBUS_REG_ADDR_MIN..=MODBUS_REG_ADDR_MAX).contains(&start_addr)
                || !(MODBUS_REG_ADDR_MIN..=MODBUS_REG_ADDR_MAX).contains(&end_addr)
            {
                return Err(format!(
                    "寄存器地址必须在 {}..={} 范围内（Modbus 16 位寄存器地址），当前 {}..={}",
                    MODBUS_REG_ADDR_MIN, MODBUS_REG_ADDR_MAX, start_addr, end_addr
                ));
            }
        }
    }
    Ok(())
}

/// mc / fins 单地址判定：结束码正常才算 responded，异常码与链路失败都如实写原因。
pub(super) fn judge_word_probe(
    protocol: &str,
    addr: i32,
    outcome: Result<(u16, Option<i32>), String>,
) -> (ScanCell, bool) {
    let failed = |func_code: Option<i32>, error: String| ScanCell {
        address: addr,
        responded: false,
        value: None,
        func_code,
        error: Some(error),
    };
    match outcome {
        Ok((code, Some(value))) => (
            ScanCell {
                address: addr,
                responded: true,
                value: Some(value),
                // mc 结束码正常为 0，按「填结束码」口径保留；fins 无功能码语义填 null
                func_code: if protocol == "mc" {
                    Some(code as i32)
                } else {
                    None
                },
                error: None,
            },
            false,
        ),
        Ok((code, None)) => {
            let text = if protocol == "mc" {
                format!("结束代码 0x{code:04X}")
            } else {
                format!("响应码 0x{code:04X}")
            };
            (failed(Some(code as i32), text), false)
        }
        Err(e) => (failed(None, format!("无应答或读取失败：{e}")), false),
    }
}

/// 单地址判定（契约 §3.2）：返回 (cell, 是否总线掉线)
pub(super) fn judge_cell(
    _slave_id: i32,
    reg_type: &str,
    addr: i32,
    outcome: Result<ExchangeOutcome, BusErrorKind>,
) -> (ScanCell, bool) {
    let fc_base = match reg_type {
        "holding" => 3,
        "input" => 4,
        "coils" => 1,
        _ => 2, // discrete
    };
    let empty = |error: Option<String>| ScanCell {
        address: addr,
        responded: false,
        value: None,
        func_code: None,
        error,
    };
    match outcome {
        Ok(out) => {
            let fc_byte = out.bytes.get(1).copied().unwrap_or(0);
            match verify_modbus_rtu(&out.bytes) {
                FrameCheck::Ok => {
                    // 正常响应：FC03/04 → 2 字节寄存器值；FC01/02 → 1 字节，bit0 = 该线圈值
                    let value = if reg_type == "holding" || reg_type == "input" {
                        if out.bytes.len() >= 5 && out.bytes[2] >= 2 {
                            Some(((out.bytes[3] as i32) << 8) | out.bytes[4] as i32)
                        } else {
                            None
                        }
                    } else {
                        // coils / discrete：读 1 个点，响应 1 字节，bit0 即该点值
                        out.bytes.get(3).map(|b| (b & 1) as i32)
                    };
                    (
                        ScanCell {
                            address: addr,
                            responded: true,
                            value,
                            func_code: Some(fc_base),
                            error: None,
                        },
                        false,
                    )
                }
                // 异常帧：该地址寄存器不存在或无权限 → responded=false（与从站扫描相反）
                FrameCheck::Exception { text, .. } => (
                    ScanCell {
                        address: addr,
                        responded: false,
                        value: None,
                        func_code: Some(fc_byte as i32),
                        error: Some(format!("exception:{text}")),
                    },
                    false,
                ),
                FrameCheck::Crc => (empty(Some("crc".into())), false),
            }
        }
        Err(BusErrorKind::BusOffline) => (empty(None), true),
        // 超时等帧级失败：该地址无可读寄存器或设备不应答
        Err(_) => (empty(None), false),
    }
}
