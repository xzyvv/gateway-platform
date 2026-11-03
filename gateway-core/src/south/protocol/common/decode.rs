//! 跨协议共享的数据解码工具
//!
//! 包含字节序重排、二进制类型解码、工程量换算（含 CT/PT 变比）和 BCD 地址解析。
//! Modbus / DLT645 / CJT188 共用同一套解码逻辑，避免实现不一致。

use crate::config::PointConfig;
use crate::engine::model::{ByteOrder, ModbusDataType, Quality};
use crate::pipeline::apply_formula;

use super::super::CollectorError;
use super::super::CollectorResult;
use super::util::{decode_bcd_value, encode_bcd_value};

/// A-1: 跨协议共享——字节序重排（ABCD/DCBA/CDAB/BADC）
///
/// Bug 118: 短输入（< 4 字节）走 passthrough 分支——这通常意味着 caller
/// 传错了字节数（如 U32 类型只读了 2 字节）。降级为 trace!：
/// U16/I16（2 字节）是 Modbus / CJT188 / DLT645 最常见点位类型，每次采样
/// 都会命中此分支。原 warn! 造成日志洪泛（1s 轮询 × 数百点位 = 每天千万条），
/// 淹没真实告警并加速磁盘磨损。错误位宽仍属 caller 配错，应在上层校验点位
/// 字节数与 data_type 一致而非在此刷日志。
pub fn reorder_bytes(bytes: &[u8], order: ByteOrder) -> Vec<u8> {
    if bytes.len() < 4 {
        tracing::trace!(
            "reorder_bytes called with {} byte(s) (< 4), passthrough",
            bytes.len()
        );
        return bytes.to_vec();
    }
    match order {
        ByteOrder::Abcd => bytes.to_vec(),
        ByteOrder::Dcba => {
            let mut v = bytes.to_vec();
            v.reverse();
            v
        }
        ByteOrder::Cdab => {
            if bytes.len() == 4 {
                vec![bytes[2], bytes[3], bytes[0], bytes[1]]
            } else if bytes.len() == 8 {
                vec![
                    bytes[6], bytes[7], bytes[4], bytes[5], bytes[2], bytes[3], bytes[0], bytes[1],
                ]
            } else {
                bytes.to_vec()
            }
        }
        ByteOrder::Badc => {
            if bytes.len() == 4 {
                vec![bytes[1], bytes[0], bytes[3], bytes[2]]
            } else if bytes.len() == 8 {
                vec![
                    bytes[1], bytes[0], bytes[3], bytes[2], bytes[5], bytes[4], bytes[7], bytes[6],
                ]
            } else {
                bytes.to_vec()
            }
        }
    }
}

/// A-1: 跨协议共享——二进制类型解码（U16/I16/U32/I32/F32/U64/I64/F64）
#[inline]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, level = "trace"))]
pub fn decode_binary_value(bytes: &[u8], dt: ModbusDataType) -> Option<(f64, &'static str)> {
    match dt {
        ModbusDataType::U16 => {
            let a: [u8; 2] = bytes.try_into().ok()?;
            Some((u16::from_be_bytes(a) as f64, "u16"))
        }
        ModbusDataType::I16 => {
            let a: [u8; 2] = bytes.try_into().ok()?;
            Some((i16::from_be_bytes(a) as f64, "i16"))
        }
        ModbusDataType::U32 => {
            let a: [u8; 4] = bytes.try_into().ok()?;
            Some((u32::from_be_bytes(a) as f64, "u32"))
        }
        ModbusDataType::I32 => {
            let a: [u8; 4] = bytes.try_into().ok()?;
            Some((i32::from_be_bytes(a) as f64, "i32"))
        }
        ModbusDataType::F32 => {
            let a: [u8; 4] = bytes.try_into().ok()?;
            Some((f32::from_be_bytes(a) as f64, "f32"))
        }
        ModbusDataType::U64 => {
            let a: [u8; 8] = bytes.try_into().ok()?;
            Some((u64::from_be_bytes(a) as f64, "u64"))
        }
        ModbusDataType::I64 => {
            let a: [u8; 8] = bytes.try_into().ok()?;
            Some((i64::from_be_bytes(a) as f64, "i64"))
        }
        ModbusDataType::F64 => {
            let a: [u8; 8] = bytes.try_into().ok()?;
            Some((f64::from_be_bytes(a), "f64"))
        }
    }
}

/// 解码 BCD 值并按数据类型分派（DLT645 / CJT188 共用）
///
/// - `data_type` 配置为 u16/i16/u32/i32/u64/i64/f32/f64：按字节序重排后二进制解码
/// - 否则（默认 / "bcd" / 未识别）：回退到 BCD 路径，向后兼容
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, level = "trace"))]
pub fn decode_value_by_type(point: &PointConfig, bytes: &[u8]) -> (f64, String, Quality) {
    if let Some(dt) = point.data_type_enum() {
        let ordered = reorder_bytes(bytes, point.byte_order_enum());
        match decode_binary_value(&ordered, dt) {
            Some((v, t)) => return (v, t.to_string(), Quality::Good),
            None => {
                tracing::warn!(
                    "point {} binary decode failed: data_type {:?} needs {} bytes, got {}",
                    point.id,
                    dt,
                    dt.register_count() * 2,
                    ordered.len()
                );
                return (f64::NAN, format!("{:?}_decode_err", dt), Quality::Bad);
            }
        }
    }
    match decode_bcd_value(bytes) {
        Some(v) => (v, "bcd".to_string(), Quality::Good),
        None => {
            tracing::warn!(
                "point {} BCD decode failed (illegal digit 0x0A-0x0F in {:?}), marking Bad",
                point.id,
                bytes
            );
            (f64::NAN, "bcd".to_string(), Quality::Bad)
        }
    }
}

/// 编码 f64 为写值字节并按数据类型分派（DLT645 / CJT188 共用）
///
/// 与读路径同源配置：配置二进制类型时按该类型编码后重排字节序；
/// 否则回退 BCD。返回字节长度 = `out_len`。
///
/// Bug 222: 原实现用 to_be_bytes + reorder_bytes 编码，与读路径
/// （reorder_bytes + from_be_bytes）不互逆——设备原样回显时读回值字节序错乱
/// （如写 0x1234 读回 0x3412），控制/参数回读场景会静默得到错误值。
/// 改为 to_be_bytes + reorder_bytes：四种字节序重排均为对合运算，
/// 读路径 reorder(reorder(be)) = be，保证写→读往返一致。
pub fn encode_value_by_type(point: &PointConfig, value: f64, out_len: usize) -> Option<Vec<u8>> {
    if let Some(dt) = point.data_type_enum() {
        let mut bytes: Vec<u8> = Vec::with_capacity(out_len);
        match dt {
            ModbusDataType::U16 => bytes.extend_from_slice(&(value as u16).to_be_bytes()),
            ModbusDataType::I16 => bytes.extend_from_slice(&(value as i16).to_be_bytes()),
            ModbusDataType::U32 => bytes.extend_from_slice(&(value as u32).to_be_bytes()),
            ModbusDataType::I32 => bytes.extend_from_slice(&(value as i32).to_be_bytes()),
            ModbusDataType::U64 => bytes.extend_from_slice(&(value as u64).to_be_bytes()),
            ModbusDataType::I64 => bytes.extend_from_slice(&(value as i64).to_be_bytes()),
            ModbusDataType::F32 => bytes.extend_from_slice(&(value as f32).to_be_bytes()),
            ModbusDataType::F64 => bytes.extend_from_slice(&value.to_be_bytes()),
        }
        if bytes.len() < out_len {
            // 不足部分在末尾补 0：大端表示下末尾为低位字节，补 0 不改变数值
            bytes.resize(out_len, 0);
        }
        // 按 point.byte_order 重排（与读路径走同序配置）
        let ordered = reorder_bytes(&bytes, point.byte_order_enum());
        return Some(ordered);
    }
    encode_bcd_value(value, out_len)
}

/// 工程量值换算（分两层：工程量换算 + CT/PT 变比）
///
/// 第一层（工程量换算）：公式优先，否则 scale*raw+offset
/// 第二层（CT/PT 变比）：二次侧工程量 → 一次侧真实值
///   - ct_ratio: 电流互感器变比（如 150/5=30），None=不应用
///   - pt_ratio: 电压互感器变比（如 10000/100=100），None=不应用
pub fn convert_value(point: &PointConfig, raw: f64) -> f64 {
    // 第一层：工程量换算（原始值 → 二次侧工程量）
    let engineering = if let Some(formula) = &point.formula {
        match apply_formula(formula, raw) {
            Ok(v) => v,
            Err(_) => {
                // Bug 120: 公式执行失败时记录 warn，便于运维定位——
                // 否则会静默回退到 scale*raw+offset，掩盖公式语法错误。
                tracing::warn!(
                    "formula '{}' for point '{}' failed, fallback to scale*raw+offset",
                    formula,
                    point.id
                );
                raw * point.scale + point.offset
            }
        }
    } else {
        raw * point.scale + point.offset
    };
    // 第二层：CT/PT 变比（二次侧 → 一次侧真实值）
    let mut primary = engineering;
    if let Some(ct) = point.ct_ratio {
        primary *= ct;
    }
    if let Some(pt) = point.pt_ratio {
        primary *= pt;
    }
    // r24-DEC1（修复）：非有限值兜底——
    // 公式失败 / scale × 极端 raw / CT/PT 变比放大
    // 都会让 primary 落到 NaN / +Inf / -Inf。直接上送会让：
    //   - MQTT/Webhook payload 含 NaN（serde_json 序列化为 null，下游通常拒收）
    //   - SQLite REAL 列存 NaN，聚合/比较全部异常
    //   - alarm NaN 守卫虽跳过 trigger（r21-ALM2），但 NaN 仍上行推送
    // 决策：fail-closed 回退到 raw（raw 由协议栈直接读取，必然 finite），
    //       打 warn 留痕便于运维定位公式/变比配置错误。
    if !primary.is_finite() {
        tracing::warn!(
            "convert_value for point '{}' produced non-finite value (formula={:?}, scale={}, offset={}, ct={:?}, pt={:?}, raw={}), fall back to raw={}",
            point.id,
            point.formula,
            point.scale,
            point.offset,
            point.ct_ratio,
            point.pt_ratio,
            raw,
            raw
        );
        primary = raw;
    }
    primary
}

/// 解析 BCD 地址
///
/// Bug 119: 增加空字符串显式拒绝——空地址语义上无效，
/// 让上层尽早失败而不是 silently 返回全 0 字节序列。
pub fn parse_bcd_address(addr: &str, byte_len: usize) -> CollectorResult<Vec<u8>> {
    if addr.is_empty() {
        return Err(CollectorError::InvalidBcd("address is empty".to_string()));
    }
    if !addr.chars().all(|c| c.is_ascii_digit()) {
        return Err(CollectorError::InvalidBcd(format!(
            "address contains non-decimal characters: {}",
            addr
        )));
    }
    if addr.len() > byte_len * 2 {
        return Err(CollectorError::InvalidBcd(format!(
            "address too long: {}",
            addr
        )));
    }
    let padded = format!("{:0>width$}", addr, width = byte_len * 2);
    let padded_bytes = padded.as_bytes();
    let mut bytes = Vec::with_capacity(byte_len);
    for i in 0..byte_len {
        // 已验证全为 ASCII 数字，直接字节索引 O(1)，避免 chars().nth() 的 O(n) 遍历
        let hi = (padded_bytes[i * 2] - b'0') as u8;
        let lo = (padded_bytes[i * 2 + 1] - b'0') as u8;
        bytes.push((hi << 4) | lo);
    }
    bytes.reverse();
    Ok(bytes)
}

#[cfg(test)]
mod tests;
