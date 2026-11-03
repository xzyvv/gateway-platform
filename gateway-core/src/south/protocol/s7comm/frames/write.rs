//! 写侧帧：Write Var（单/多 item）构建与响应校验（决策表 #11 / #13）。
use super::*;

/// 构建 S7 Write Var 请求（决策表 #11）
pub(crate) fn build_write_var(
    area: u8,
    db_number: u16,
    byte_offset: u32,
    bit_offset: u8,
    data_type: S7DataType,
    value: f64,
    pdu_ref: u16,
) -> CollectorResult<Vec<u8>> {
    let value_bytes = encode_value(value, data_type)?;
    let transport_size = data_type.transport_size();
    let num_elements = if data_type == S7DataType::Bit {
        1u16
    } else {
        value_bytes.len() as u16
    };
    let bit_address = byte_offset * 8 + bit_offset as u32;
    let addr_bytes = [
        ((bit_address >> 16) & 0xFF) as u8,
        ((bit_address >> 8) & 0xFF) as u8,
        (bit_address & 0xFF) as u8,
    ];
    let mut param = Vec::with_capacity(14);
    param.push(FUNC_WRITE);
    param.push(0x01);
    param.push(0x12);
    param.push(0x0A);
    param.push(0x10);
    param.push(transport_size);
    param.extend_from_slice(&num_elements.to_be_bytes());
    param.extend_from_slice(&db_number.to_be_bytes());
    param.push(area);
    param.extend_from_slice(&addr_bytes);
    let mut data = Vec::with_capacity(5 + value_bytes.len());
    data.push(0x00);
    data.push(transport_size);
    data.extend_from_slice(&(value_bytes.len() as u16).to_be_bytes());
    data.extend_from_slice(&value_bytes);
    if data.len() % 2 != 0 {
        data.push(0x00);
    }
    let mut frame = Vec::with_capacity(4 + 3 + 10 + param.len() + data.len());
    frame.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]);
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]);
    frame.push(0x32);
    frame.push(PDU_JOB);
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.extend_from_slice(&pdu_ref.to_be_bytes());
    frame.extend_from_slice(&(param.len() as u16).to_be_bytes());
    frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
    frame.extend_from_slice(&param);
    frame.extend_from_slice(&data);
    let total_len = frame.len() as u16;
    frame[2] = (total_len >> 8) as u8;
    frame[3] = total_len as u8;
    Ok(frame)
}

/// 校验 S7 Write Var 响应（0x05 Ack-Data）
pub(crate) fn parse_write_response(frame: &[u8]) -> CollectorResult<()> {
    // Bug: S7 Ack-Data 响应头是 12 字节（10 字节基础头 + Error class/code 各 1 字节），
    // 与请求 Job 的 10 字节不同。按 10 解析会把 s7[10]（Error class=0x00）当成功能码，
    // 使真实 PLC 响应恒报 `unexpected S7 function: 0x00`。
    if frame.len() < 3 + 12 + 2 {
        return Err(CollectorError::InvalidFrame(
            "S7 write response too short".into(),
        ));
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
                "S7 write error response: class=0x{:02X} code=0x{:02X}",
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
    if param_len < 2 {
        return Err(CollectorError::InvalidFrame(
            "S7 write parameter too short".into(),
        ));
    }
    // 参数段起点 = 12（10 字节基础头 + 2 字节 Error class/code）
    let function = s7[12];
    if function != FUNC_WRITE {
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected S7 function: 0x{:02X} (expected 0x{:02X})",
            function, FUNC_WRITE
        )));
    }
    if data_len == 0 {
        return Ok(());
    }
    let data_start = 12 + param_len;
    if s7.len() < data_start + data_len {
        return Err(CollectorError::InvalidFrame(
            "S7 write response data truncated".into(),
        ));
    }
    let return_code = s7[data_start];
    if return_code != RET_OK {
        return Err(CollectorError::InvalidFrame(format!(
            "S7 write failed (return code 0x{:02X})",
            return_code
        )));
    }
    Ok(())
}

/// 构建 S7 多 item Write Var 请求（决策表 #13）
///
/// `items.len()` ≤ 255；每个 item 的 `value_le` 字节数应与 `spec.data_type.byte_size()`
/// 一致（位 1 字节、Byte 1 字节、Word 2 字节、Real 4 字节……）。
pub(crate) fn build_write_var_multi(items: &[WriteItem<'_>], pdu_ref: u16) -> Vec<u8> {
    let n = items.len();
    assert!(n <= u8::MAX as usize, "S7 Write Var item count > 255");

    // Parameter = function(1) + item_count(1) + N × 12B VariableSpec
    let mut param = Vec::with_capacity(2 + 12 * n);
    param.push(FUNC_WRITE);
    param.push(n as u8);
    for it in items {
        param.extend_from_slice(&it.spec.to_variable_spec());
    }

    // Data：每个 item：return_code(1) + transport_size(1) + length(2) + value(N) + 偶对齐
    let mut data =
        Vec::with_capacity(4 * n + items.iter().map(|it| it.value_le.len()).sum::<usize>());
    for it in items {
        data.push(0x00); // return code（请求占位）
        data.push(it.spec.data_type.transport_size());
        data.extend_from_slice(&(it.value_le.len() as u16).to_be_bytes());
        data.extend_from_slice(it.value_le);
        if data.len() % 2 != 0 {
            data.push(0x00);
        }
    }

    let mut frame = Vec::with_capacity(4 + 3 + 10 + param.len() + data.len());
    frame.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]);
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]);
    frame.push(0x32);
    frame.push(PDU_JOB);
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.extend_from_slice(&pdu_ref.to_be_bytes());
    frame.extend_from_slice(&(param.len() as u16).to_be_bytes());
    frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
    frame.extend_from_slice(&param);
    frame.extend_from_slice(&data);

    let total_len = frame.len() as u16;
    frame[2] = (total_len >> 8) as u8;
    frame[3] = total_len as u8;
    frame
}

/// 解析 S7 多 item Write Var 响应（决策表 #13）
///
/// 多 item 写响应 parameter = function(1) + item_count(1)；data 段为空或
/// 仅含 item_count 个 return_code。本函数只检查整体错误，任一 item return
/// code 非 0xFF 视为失败；返回首个失败项的索引与 code（便于上层日志定位）。
pub(crate) fn parse_write_response_multi(
    frame: &[u8],
    expected_count: usize,
) -> CollectorResult<()> {
    // Bug: Ack-Data 响应头 12 字节（10 基础 + Error class/code）。
    if frame.len() < 3 + 12 + 2 {
        return Err(CollectorError::InvalidFrame(
            "S7 write response too short".into(),
        ));
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
                "S7 multi-write error response: class=0x{:02X} code=0x{:02X}",
                err_class, err_code
            )));
        }
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected PDU type: 0x{:02X}",
            pdu_type
        )));
    }
    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    // Bug: 参数段起点 = 12（10 基础头 + 2 字节 Error class/code）
    let data_start = 12 + param_len;
    if s7.len() < data_start + data_len {
        return Err(CollectorError::InvalidFrame(
            "S7 write response data truncated".into(),
        ));
    }
    if param_len < 2 {
        return Err(CollectorError::InvalidFrame(
            "S7 write parameter too short".into(),
        ));
    }
    let function = s7[12];
    if function != FUNC_WRITE {
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected S7 function: 0x{:02X} (expected 0x{:02X})",
            function, FUNC_WRITE
        )));
    }
    let item_count = s7[13] as usize;
    if item_count != expected_count {
        return Err(CollectorError::InvalidFrame(format!(
            "S7 multi-write response item count {} != request {}",
            item_count, expected_count
        )));
    }
    if data_len == 0 {
        // 部分 CPU 写响应 data 段为空，按整体成功处理（对齐 parse_write_response 宽松判定）
        return Ok(());
    }
    if data_len < expected_count {
        return Err(CollectorError::InvalidFrame(format!(
            "S7 multi-write data length {data_len} < item count {expected_count}"
        )));
    }
    for i in 0..expected_count {
        let rc = s7[data_start + i];
        if rc != RET_OK {
            return Err(CollectorError::InvalidFrame(format!(
                "S7 multi-write item {i} failed (return code 0x{rc:02X})"
            )));
        }
    }
    Ok(())
}
