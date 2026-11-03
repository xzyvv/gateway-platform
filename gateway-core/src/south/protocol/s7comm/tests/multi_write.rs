//! 多 item Write Var 测试（决策表 #13 写侧）。
use super::*;

// ===== S7 多 item 写入通路（决策表 #13 写侧）=====
//
// X4：这三个符号（WriteItem / build_write_var_multi /
// parse_write_response_multi）在 X4 之前是 `#[allow(dead_code)]`——既进生产
// 二进制，又没有一行测试覆盖，属于「既没接线也没验证」的纯预留代码。
// X4 把它们 gate 成 `#[cfg(test)]` 后，lib test 构建反而暴露：连测试都没引用，
// dead_code 警告成立。修法不是补 allow，而是补齐测试——写侧与读侧
//（build_read_var_multi / parse_read_response_multi，各 3 个测试）对称，
// 使预留代码真正具备「可验证」资格。
//
// 反证闭环：删掉下面任一测试 → 对应符号重新报 never used / never constructed。

/// 多 item Write Var：帧头布局与 item 数/param 长度契约
#[test]
fn test_build_write_var_multi_3_items() {
    let db1 = [0x00u8, 0x00, 0xA8, 0x41]; // REAL 21.0 BE
    let db2 = [0x00u8, 0x00, 0x70, 0x41]; // REAL 15.0 BE
    let items = [
        frames::WriteItem {
            spec: ItemSpec {
                area: AREA_DB,
                db_number: 1,
                byte_offset: 0,
                bit_offset: 0,
                data_type: S7DataType::Real,
            },
            value_le: &db1,
        },
        frames::WriteItem {
            spec: ItemSpec {
                area: AREA_DB,
                db_number: 2,
                byte_offset: 0,
                bit_offset: 0,
                data_type: S7DataType::Real,
            },
            value_le: &db2,
        },
    ];
    let frame = frames::build_write_var_multi(&items, 0x0009);

    // Parameter = function(1) + item_count(1) + N*12B VariableSpec = 2 + 2*12
    let param_len = 2 + 2 * 12;
    // Data：每 item = return_code(1) + transport(1) + len(2) + value(4) = 8B
    //        两个 item 均偶对齐，无需补齐字节
    let data_len = 2 * 8;
    let total = 4 + 3 + 10 + param_len + data_len;
    assert_eq!(frame.len(), total, "整帧长度应为 TPKT+COTP+S7头+param+data");

    // TPKT length 回填
    assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), total as u16);
    // PDU reference 位于 [11..13]
    assert_eq!(u16::from_be_bytes([frame[11], frame[12]]), 0x0009);
    // Parameter / Data 长度字段
    assert_eq!(u16::from_be_bytes([frame[13], frame[14]]), param_len as u16);
    assert_eq!(u16::from_be_bytes([frame[15], frame[16]]), data_len as u16);
    // function + item_count 位于 [17..19]
    assert_eq!(frame[17], FUNC_WRITE);
    assert_eq!(frame[18], 2);
}

/// 多 item Write Var 的单项 VariableSpec 与单 item 写入一致
///
/// 与读侧 `test_multi_item_per_item_matches_single_item` 对称：同一
/// ItemSpec 在单/多 item 两条路径下必须产出相同的 12B VariableSpec，
/// 否则 PLC 侧会对同一地址解出不同 item。
#[test]
fn test_multi_write_per_item_matches_single_write() {
    let value = [0x00u8, 0x00, 0xA8, 0x41];
    let single = frames::build_write_var(AREA_DB, 1, 0, 0, S7DataType::Real, 21.0, 0x0042)
        .expect("构建单 item 写入失败");
    let multi = frames::build_write_var_multi(
        &[frames::WriteItem {
            spec: ItemSpec {
                area: AREA_DB,
                db_number: 1,
                byte_offset: 0,
                bit_offset: 0,
                data_type: S7DataType::Real,
            },
            value_le: &value,
        }],
        0x0042,
    );
    // 两个函数第一个 VariableSpec 均从 [19..31] 开始
    // （function + item_count + 12B）
    assert_eq!(
        &multi[19..31],
        &single[19..31],
        "同一 ItemSpec 的 VariableSpec 段在单/多 item 路径下必须一致"
    );
}

/// 多 item 写响应：全部 return code = 0xFF → Ok
#[test]
fn test_parse_write_response_multi_all_ok() {
    // 前 3 字节按 parse_write_response_multi 的 `&frame[3..]` 约定跳过
    // （TPKT version + TPKT length），与既有 parse_write_response 测试一致
    let mut resp = vec![0x02, 0xF0, 0x80];
    resp.push(0x32);
    resp.push(PDU_ACK);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&0x0001u16.to_be_bytes());
    let param = 2u16; // function(1) + item_count(1)
    let data = 2u16; // 两个 return code
    resp.extend_from_slice(&param.to_be_bytes());
    resp.extend_from_slice(&data.to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    resp.push(FUNC_WRITE);
    resp.push(2);
    resp.push(RET_OK);
    resp.push(RET_OK);
    frames::parse_write_response_multi(&resp, 2).expect("两项均成功应返回 Ok");
}

/// 多 item 写响应：第 2 项 return code != 0xFF → Err 且错误消息含失败索引
#[test]
fn test_parse_write_response_multi_partial_failure() {
    // 前 3 字节按 parse_write_response_multi 的 `&frame[3..]` 约定跳过
    // （TPKT version + TPKT length），与既有 parse_write_response 测试一致
    let mut resp = vec![0x02, 0xF0, 0x80];
    resp.push(0x32);
    resp.push(PDU_ACK);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&0x0001u16.to_be_bytes());
    let param = 2u16;
    let data = 2u16;
    resp.extend_from_slice(&param.to_be_bytes());
    resp.extend_from_slice(&data.to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    resp.push(FUNC_WRITE);
    resp.push(2);
    resp.push(RET_OK);
    resp.push(RET_NOT_FOUND);
    let err = frames::parse_write_response_multi(&resp, 2).expect_err("第 2 项失败应返回 Err");
    let msg = err.to_string();
    assert!(
        msg.contains("item 1"),
        "错误消息应指出失败项索引 1，实际: {msg}"
    );
}

/// 多 item 写响应：响应 item 数与请求不符 → InvalidFrame
#[test]
fn test_parse_write_response_multi_count_mismatch() {
    // 前 3 字节按 parse_write_response_multi 的 `&frame[3..]` 约定跳过
    // （TPKT version + TPKT length），与既有 parse_write_response 测试一致
    let mut resp = vec![0x02, 0xF0, 0x80];
    resp.push(0x32);
    resp.push(PDU_ACK);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&0x0001u16.to_be_bytes());
    resp.extend_from_slice(&2u16.to_be_bytes());
    resp.extend_from_slice(&1u16.to_be_bytes()); // data 段仅 1 个 return code
    resp.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    resp.push(FUNC_WRITE);
    resp.push(1); // 只回 1 项，请求的是 2 项
    resp.push(RET_OK);
    let err = frames::parse_write_response_multi(&resp, 2).expect_err("item 数不符应 Err");
    assert!(
        err.to_string().contains("item count"),
        "错误消息应说明 item 数不符，实际: {err}"
    );
}

/// 多 item 写响应：data 段为空（部分 PLC 行为）→ 按整体成功处理
#[test]
fn test_parse_write_response_multi_empty_data_is_ok() {
    // 前 3 字节按 parse_write_response_multi 的 `&frame[3..]` 约定跳过
    // （TPKT version + TPKT length），与既有 parse_write_response 测试一致
    let mut resp = vec![0x02, 0xF0, 0x80];
    resp.push(0x32);
    resp.push(PDU_ACK);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&0x0001u16.to_be_bytes());
    resp.extend_from_slice(&2u16.to_be_bytes());
    resp.extend_from_slice(&0u16.to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    resp.push(FUNC_WRITE);
    resp.push(2);
    frames::parse_write_response_multi(&resp, 2).expect("空 data 段应按整体成功处理");
}
