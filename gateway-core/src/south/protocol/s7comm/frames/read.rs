//! 读侧帧：Read Var（单/多 item）构建与响应解析（决策表 #13）。
use super::*;

/// 构建 S7 Read Var 请求
pub(crate) fn build_read_var(
    area: u8,
    db_number: u16,
    byte_offset: u32,
    bit_offset: u8,
    data_type: S7DataType,
    pdu_ref: u16,
) -> Vec<u8> {
    let byte_size = data_type.byte_size();
    let transport_size = data_type.transport_size();
    let num_elements = if data_type == S7DataType::Bit {
        1u16
    } else {
        byte_size as u16
    };
    let bit_address = byte_offset * 8 + bit_offset as u32;
    let addr_bytes = [
        ((bit_address >> 16) & 0xFF) as u8,
        ((bit_address >> 8) & 0xFF) as u8,
        (bit_address & 0xFF) as u8,
    ];
    let mut param = Vec::with_capacity(14);
    param.push(FUNC_READ);
    param.push(0x01);
    param.push(0x12);
    param.push(0x0A);
    param.push(0x10);
    param.push(transport_size);
    param.extend_from_slice(&num_elements.to_be_bytes());
    param.extend_from_slice(&db_number.to_be_bytes());
    param.push(area);
    param.extend_from_slice(&addr_bytes);
    let param_len = param.len() as u16;
    let mut frame = Vec::with_capacity(4 + 3 + 10 + param.len());
    frame.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]);
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]);
    frame.push(0x32);
    frame.push(PDU_JOB);
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.extend_from_slice(&pdu_ref.to_be_bytes());
    frame.extend_from_slice(&param_len.to_be_bytes());
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.extend_from_slice(&param);
    let total_len = frame.len() as u16;
    frame[2] = (total_len >> 8) as u8;
    frame[3] = total_len as u8;
    frame
}

/// 从 S7 Read Var 响应中提取数据值
pub(crate) fn parse_read_response(
    frame: &[u8],
    expected_data_type: S7DataType,
) -> CollectorResult<f64> {
    // Bug: Ack-Data 响应头 12 字节（10 基础 + Error class/code），非请求的 10 字节。
    if frame.len() < 3 + 12 + 2 {
        return Err(CollectorError::InvalidFrame("S7 response too short".into()));
    }
    let s7 = &frame[3..];
    if s7.len() < 12 {
        return Err(CollectorError::InvalidFrame("S7 header too short".into()));
    }
    if s7[0] != 0x32 {
        return Err(CollectorError::InvalidFrame(format!(
            "invalid S7 protocol ID: 0x{:02X}",
            s7[0]
        )));
    }
    let pdu_type = s7[1];
    if pdu_type != PDU_ACK {
        if (pdu_type & 0x80) != 0 {
            let err_class = if s7.len() > 10 { s7[10] } else { 0 };
            let err_code = if s7.len() > 11 { s7[11] } else { 0 };
            return Err(CollectorError::InvalidFrame(format!(
                "S7 error response: class=0x{:02X} code=0x{:02X}",
                err_class, err_code
            )));
        }
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected PDU type: 0x{:02X} (expected 0x{:02X})",
            pdu_type, PDU_ACK
        )));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    // Bug: Ack-Data 响应头 12 字节 → 参数段起点为 12（旧按 10 会整个错位）
    let param_start = 12;
    let data_start = param_start + param_len;
    if s7.len() < data_start + data_len {
        return Err(CollectorError::InvalidFrame(
            "S7 response data truncated".into(),
        ));
    }
    if param_len < 2 {
        return Err(CollectorError::InvalidFrame(
            "S7 parameter too short".into(),
        ));
    }
    let function = s7[param_start];
    if function != FUNC_READ {
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected S7 function: 0x{:02X}",
            function
        )));
    }
    let item_count = s7[param_start + 1];
    if item_count != 1 {
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected S7 item count: {} (expected 1)",
            item_count
        )));
    }
    if data_len < 4 {
        return Err(CollectorError::InvalidFrame("S7 data too short".into()));
    }
    let data_section = &s7[data_start..];
    let return_code = data_section[0];
    if return_code != RET_OK {
        if return_code == RET_NOT_FOUND {
            return Err(CollectorError::InvalidConfig(format!(
                "S7 address not found (return code 0x{:02X})",
                return_code
            )));
        }
        return Err(CollectorError::InvalidFrame(format!(
            "S7 read failed (return code 0x{:02X})",
            return_code
        )));
    }
    let _transport_size = data_section[1];
    let value_data_len = u16::from_be_bytes([data_section[2], data_section[3]]) as usize;
    if data_section.len() < 4 + value_data_len {
        return Err(CollectorError::InvalidFrame(
            "S7 value data truncated".into(),
        ));
    }
    let value_bytes = &data_section[4..4 + value_data_len];
    let expected_len = expected_data_type.byte_size();
    if value_data_len < expected_len {
        return Err(CollectorError::InvalidFrame(format!(
            "S7 data length mismatch: got {} bytes, expected {} for {:?}",
            value_data_len, expected_len, expected_data_type
        )));
    }
    decode_value(value_bytes, expected_data_type)
}

/// 构建 S7 多 item Read Var 请求（决策表 #13）
///
/// `items.len()` 不得超过 255（S7 帧 item_count 字段为单字节）；超限由调用方分片。
pub(crate) fn build_read_var_multi(items: &[ItemSpec], pdu_ref: u16) -> Vec<u8> {
    let n = items.len();
    assert!(n <= u8::MAX as usize, "S7 Read Var item count > 255");

    // Parameter = function(1) + item_count(1) + N × 12B VariableSpec
    let mut param = Vec::with_capacity(2 + 12 * n);
    param.push(FUNC_READ);
    param.push(n as u8);
    for it in items {
        param.extend_from_slice(&it.to_variable_spec());
    }

    // 公共头：TPKT(4) + COTP DT(3) + S7 header(10)
    let mut frame = Vec::with_capacity(4 + 3 + 10 + param.len());
    frame.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]); // TPKT placeholder
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]); // COTP DT
    frame.push(0x32); // Protocol ID
    frame.push(PDU_JOB); // PDU type
    frame.extend_from_slice(&[0x00, 0x00]); // Reserved
    frame.extend_from_slice(&pdu_ref.to_be_bytes());
    frame.extend_from_slice(&(param.len() as u16).to_be_bytes());
    frame.extend_from_slice(&[0x00, 0x00]); // data length = 0
    frame.extend_from_slice(&param);

    let total_len = frame.len() as u16;
    frame[2] = (total_len >> 8) as u8;
    frame[3] = total_len as u8;
    frame
}

/// 解析 S7 多 item Read Var 响应（决策表 #13）
///
/// 返回与 `items` 顺序对齐的 `Vec<Result<Vec<u8>, u8>>`：成功为原始字节（位类
/// 型 1 字节、字 2/4 字节……，已剥去 return_code / transport_size / length 头），
/// 失败为 S7 ReturnCode（`RET_OK = 0xFF` 表示成功）。
///
/// 通用错误（PDU type 错误 / 帧过短 / function 不匹配 / item_count 不匹配）走
/// `Err(InvalidFrame)`，任一具体 item 错误走 `Ok(Err(code))`。
pub(crate) fn parse_read_response_multi(
    frame: &[u8],
    items: &[ItemSpec],
) -> CollectorResult<Vec<Result<Vec<u8>, u8>>> {
    // Bug: Ack-Data 响应头 12 字节（10 基础 + Error class/code）。
    if frame.len() < 3 + 12 + 2 {
        return Err(CollectorError::InvalidFrame("S7 response too short".into()));
    }
    let s7 = &frame[3..];
    if s7[0] != 0x32 {
        return Err(CollectorError::InvalidFrame(format!(
            "invalid S7 protocol ID: 0x{:02X}",
            s7[0]
        )));
    }
    let pdu_type = s7[1];
    if pdu_type != PDU_ACK {
        if (pdu_type & 0x80) != 0 {
            let err_class = if s7.len() > 10 { s7[10] } else { 0 };
            let err_code = if s7.len() > 11 { s7[11] } else { 0 };
            return Err(CollectorError::InvalidFrame(format!(
                "S7 error response: class=0x{:02X} code=0x{:02X}",
                err_class, err_code
            )));
        }
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected PDU type: 0x{:02X} (expected 0x{:02X})",
            pdu_type, PDU_ACK
        )));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    // Bug: 参数段起点 = 12（10 基础头 + 2 字节 Error class/code）
    let data_start = 12 + param_len;
    if s7.len() < data_start + data_len {
        return Err(CollectorError::InvalidFrame(
            "S7 response data truncated".into(),
        ));
    }
    if param_len < 2 {
        return Err(CollectorError::InvalidFrame(
            "S7 parameter too short".into(),
        ));
    }
    let function = s7[12];
    if function != FUNC_READ {
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected S7 function: 0x{:02X} (expected 0x{:02X})",
            function, FUNC_READ
        )));
    }
    let item_count = s7[13] as usize;
    if item_count != items.len() {
        return Err(CollectorError::InvalidFrame(format!(
            "S7 read response item count {} != request {}",
            item_count,
            items.len()
        )));
    }

    let mut out: Vec<Result<Vec<u8>, u8>> = Vec::with_capacity(item_count);
    let mut cursor = data_start;
    for (i, _it) in items.iter().enumerate() {
        if cursor + 4 > s7.len() {
            return Err(CollectorError::InvalidFrame(format!(
                "S7 multi-read: item {i} header truncated"
            )));
        }
        let rc = s7[cursor];
        let _ts = s7[cursor + 1];
        let value_len = u16::from_be_bytes([s7[cursor + 2], s7[cursor + 3]]) as usize;
        let value_start = cursor + 4;
        // S7 协议本身会对奇数 value 补 0x00；data_len 已含该 padding。
        if value_start + value_len > s7.len() {
            return Err(CollectorError::InvalidFrame(format!(
                "S7 multi-read: item {i} value truncated (need {value_len})"
            )));
        }
        let value_bytes = &s7[value_start..value_start + value_len];
        out.push(if rc == RET_OK {
            Ok(value_bytes.to_vec())
        } else {
            Err(rc)
        });
        cursor = value_start + value_len;
    }
    Ok(out)
}
