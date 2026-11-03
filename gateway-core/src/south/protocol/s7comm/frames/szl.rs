//! SZL（System Status List）读请求/响应编解码（决策表 #12：CPU 诊断）。
use super::*;

/// SZL 部分列表响应（S7 CPU functions 0x04 / Read SZL 子功能 0x01）
///
/// 字段完整暴露 SZL 头信息，供诊断输出与测试断言；[`Collector::diag`] 主要消费
/// `records`，其余字段用于在无文本记录时给出可读的头部摘要。
#[derive(Debug, Clone)]
pub(crate) struct SzlResponse {
    /// 响应的 SZL-ID
    pub szl_id: u16,
    /// 响应的 SZL-Index
    pub szl_index: u16,
    /// 部分列表长度（含 8 字节 SZL 头）
    pub list_len: u16,
    /// 部分列表记录条数
    pub record_count: u16,
    /// 按记录切分后的原始记录字节
    pub records: Vec<Vec<u8>>,
}

/// 构建 S7 Read SZL 请求（Userdata PDU：PDU type 0x07）
///
/// 报文结构（对齐 Snap7 / PLCScan 现场验证报文，固定 33 字节）：
/// `TPKT(4) + COTP DT(3) + S7 头(10) + Parameter(8) + Data(8)`。
/// Parameter 恒为 [`SZL_PARAM`]；Data 为
/// `return_code(0xFF) + transport_size(0x09) + length(0x0004) + SZL-ID(2) + SZL-Index(2)`。
pub(crate) fn build_szl_request(szl_id: u16, index: u16, pdu_ref: u16) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + 3 + 10 + SZL_PARAM.len() + 8);
    // TPKT 头（长度占位，稍后回填）
    frame.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]);
    // COTP DT
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]);
    // S7 头
    frame.push(0x32); // Protocol ID
    frame.push(PDU_USERDATA); // PDU type: Userdata
    frame.extend_from_slice(&[0x00, 0x00]); // Reserved
    frame.extend_from_slice(&pdu_ref.to_be_bytes()); // PDU Reference
    frame.extend_from_slice(&(SZL_PARAM.len() as u16).to_be_bytes()); // Parameter length = 8
    frame.extend_from_slice(&8u16.to_be_bytes()); // Data length = 8
                                                  // Parameter（8 字节，固定）
    frame.extend_from_slice(&SZL_PARAM);
    // Data：返回码 + 传输大小 + 后续数据长度(=4) + SZL-ID + SZL-Index
    frame.push(RET_OK);
    frame.push(0x09);
    frame.extend_from_slice(&4u16.to_be_bytes());
    frame.extend_from_slice(&szl_id.to_be_bytes());
    frame.extend_from_slice(&index.to_be_bytes());

    // 回填 TPKT 长度
    let total_len = frame.len() as u16;
    frame[2] = (total_len >> 8) as u8;
    frame[3] = total_len as u8;
    frame
}

/// 解析 S7 Read SZL 响应（`frame` 为 `COTP(3) + S7 头(10) + ...`，不含 TPKT）
///
/// 校验协议 ID / PDU 类型 / 返回码后，解析 Data 段头：
/// `return_code(1) + transport_size(1) + length(2) + SZL-ID(2) + SZL-Index(2)`
/// `+ partial_list_len(2) + record_count(2) + records...`
pub(crate) fn parse_szl_response(frame: &[u8]) -> CollectorResult<SzlResponse> {
    if frame.len() < 3 + 10 + 12 {
        return Err(CollectorError::InvalidFrame(
            "S7 SZL response too short".into(),
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
    if pdu_type != PDU_USERDATA {
        if (pdu_type & 0x80) != 0 {
            let err_class = if s7.len() > 10 { s7[10] } else { 0 };
            let err_code = if s7.len() > 11 { s7[11] } else { 0 };
            return Err(CollectorError::InvalidFrame(format!(
                "S7 SZL error response: class=0x{:02X} code=0x{:02X}",
                err_class, err_code
            )));
        }
        return Err(CollectorError::InvalidFrame(format!(
            "unexpected S7 PDU type: 0x{:02X} (expected 0x{:02X})",
            pdu_type, PDU_USERDATA
        )));
    }

    let param_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    let data_start = 10 + param_len;
    if s7.len() < data_start + data_len {
        return Err(CollectorError::InvalidFrame(
            "S7 SZL response data truncated".into(),
        ));
    }
    let data = &s7[data_start..data_start + data_len];
    if data.len() < 12 {
        return Err(CollectorError::InvalidFrame(
            "S7 SZL data section too short".into(),
        ));
    }
    let return_code = data[0];
    if return_code != RET_OK {
        return Err(CollectorError::InvalidFrame(format!(
            "S7 SZL failed (return code 0x{:02X})",
            return_code
        )));
    }

    let szl_id = u16::from_be_bytes([data[4], data[5]]);
    let szl_index = u16::from_be_bytes([data[6], data[7]]);
    let list_len = u16::from_be_bytes([data[8], data[9]]);
    let record_count = u16::from_be_bytes([data[10], data[11]]);
    let records = split_szl_records(&data[12..], list_len, record_count);

    Ok(SzlResponse {
        szl_id,
        szl_index,
        list_len,
        record_count,
        records,
    })
}

/// 按记录条数将记录区切成等长记录
///
/// 优先用「部分列表长度（含 8 字节 SZL 头）」反推单条记录长度；长度不可信时
/// 退化为按字节数均分。条数为 0 或无法切分时返回空。
fn split_szl_records(area: &[u8], list_len: u16, count: u16) -> Vec<Vec<u8>> {
    if count == 0 || area.is_empty() {
        return Vec::new();
    }
    let n = count as usize;
    let mut size = if (list_len as usize) > 8 {
        (list_len as usize - 8) / n
    } else {
        0
    };
    if (size == 0 || size * n > area.len()) && area.len().is_multiple_of(n) {
        size = area.len() / n;
    }
    if size == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(n);
    let mut off = 0;
    while off < area.len() && out.len() < n {
        let end = (off + size).min(area.len());
        out.push(area[off..end].to_vec());
        off += size;
    }
    out
}

/// 从 SZL 记录中提取可读 ASCII 字符串（用于组件标识的诊断摘要）
///
/// 组件标识记录以 NUL 填充保存订货号 / 模块类型 / 序列号等，此处不采用脆弱
/// 的固定偏移，而是提取所有长度 ≥ 4 的可打印 ASCII 连续片段并去重。
pub(crate) fn extract_ascii_strings(records: &[Vec<u8>]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rec in records {
        let mut cur = String::new();
        for &b in rec {
            if (0x20..=0x7E).contains(&b) {
                cur.push(b as char);
            } else {
                flush_run(&mut cur, &mut out);
            }
        }
        flush_run(&mut cur, &mut out);
    }
    out
}

/// 收尾一个 ASCII 连续片段：去空白后长度 ≥ 4 且未出现过则收录
fn flush_run(cur: &mut String, out: &mut Vec<String>) {
    let s = cur.trim();
    if s.len() >= 4 && !out.iter().any(|x| x == s) {
        out.push(s.to_string());
    }
    cur.clear();
}
