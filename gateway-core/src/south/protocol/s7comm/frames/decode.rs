//! 值编解码：f64 ↔ S7 字节序列（范围校验与大端转换）。
use super::*;

/// 将 f64 目标值编码为 S7 字节序列
pub(crate) fn encode_value(value: f64, data_type: S7DataType) -> CollectorResult<Vec<u8>> {
    let bytes = match data_type {
        S7DataType::Bit => vec![if value != 0.0 { 0x01 } else { 0x00 }],
        S7DataType::Byte => vec![int_in_range(value, 0.0, u8::MAX as f64, "byte")? as u8],
        S7DataType::Word => (int_in_range(value, 0.0, u16::MAX as f64, "word")? as u16)
            .to_be_bytes()
            .to_vec(),
        S7DataType::Int => (int_in_range(value, i16::MIN as f64, i16::MAX as f64, "int")? as i16)
            .to_be_bytes()
            .to_vec(),
        S7DataType::DWord => (int_in_range(value, 0.0, u32::MAX as f64, "dword")? as u32)
            .to_be_bytes()
            .to_vec(),
        S7DataType::DInt => (int_in_range(value, i32::MIN as f64, i32::MAX as f64, "dint")? as i32)
            .to_be_bytes()
            .to_vec(),
        S7DataType::Real => (value as f32).to_be_bytes().to_vec(),
    };
    Ok(bytes)
}

fn int_in_range(value: f64, min: f64, max: f64, ty: &str) -> CollectorResult<i64> {
    if !value.is_finite() {
        return Err(CollectorError::InvalidConfig(format!(
            "non-finite value {} for S7 {}",
            value, ty
        )));
    }
    let r = value.round();
    if r < min || r > max {
        return Err(CollectorError::InvalidConfig(format!(
            "value {} out of range for S7 {} [{}..{}]",
            value, ty, min, max
        )));
    }
    Ok(r as i64)
}

/// 解码 S7 数据为 f64
pub(crate) fn decode_value(data: &[u8], data_type: S7DataType) -> CollectorResult<f64> {
    let expected = data_type.byte_size();
    if data.len() < expected {
        return Err(CollectorError::InvalidFrame(format!(
            "data too short: got {} bytes, expected {}",
            data.len(),
            expected
        )));
    }
    let val = match data_type {
        S7DataType::Bit => (data[0] & 0x01) as f64,
        S7DataType::Byte => data[0] as f64,
        S7DataType::Word => u16::from_be_bytes([data[0], data[1]]) as f64,
        S7DataType::Int => i16::from_be_bytes([data[0], data[1]]) as f64,
        S7DataType::DWord => u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as f64,
        S7DataType::DInt => i32::from_be_bytes([data[0], data[1], data[2], data[3]]) as f64,
        S7DataType::Real => f32::from_be_bytes([data[0], data[1], data[2], data[3]]) as f64,
    };
    Ok(val)
}
