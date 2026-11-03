//! 多 item Read Var 测试（决策表 #13：S7-1200/1500 优化块）。
use super::*;

use std::time::Duration;

use crate::south::protocol::Collector;

// =====================================================================
// 决策表 #13：S7-1200/1500 优化块 = 多 item Read/Write Var
// =====================================================================

/// 多 item Read Var 请求：单次 PDU 携带 N 个 VariableSpecification（12 字节/项）
///
/// 验证：3 个 DB Real 点位 → Parameter 长度 = 2 + 3×12 = 38、item_count = 3
/// 协议层固定头（TPKT + COTP DT + S7 header）= 4 + 3 + 10 = 17，
/// 总帧长 = 17 + 38 = 55 字节。
#[test]
fn test_build_read_var_multi_3_items() {
    let items = vec![
        ItemSpec {
            area: AREA_DB,
            db_number: 1,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        },
        ItemSpec {
            area: AREA_DB,
            db_number: 2,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        },
        ItemSpec {
            area: AREA_DB,
            db_number: 3,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        },
    ];
    let frame = frames::build_read_var_multi(&items, 0x0007);
    // TPKT(4) + COTP DT(3) + S7 header(10) + parameter(2 + 3*12 = 38)
    assert_eq!(frame.len(), 4 + 3 + 10 + 38);
    // TPKT length
    let tpkt_len = u16::from_be_bytes([frame[2], frame[3]]);
    assert_eq!(tpkt_len, 4 + 3 + 10 + 38);
    // PDU reference 位于 [11..13]
    let pdu_ref = u16::from_be_bytes([frame[11], frame[12]]);
    assert_eq!(pdu_ref, 0x0007);
    // Parameter length 位于 [13..15]
    let param_len = u16::from_be_bytes([frame[13], frame[14]]);
    assert_eq!(param_len, 38);
    // Data length = 0 (read) 位于 [15..17]
    let data_len = u16::from_be_bytes([frame[15], frame[16]]);
    assert_eq!(data_len, 0);
    // function + item count 位于 [17..19]
    assert_eq!(frame[17], FUNC_READ);
    assert_eq!(frame[18], 3);
}

/// 多 item Read Var item 字节序与单 item 相同
/// （同区域/同 DB 号/同数据类型应产出与 build_read_var 一致的 VariableSpecification 段）
#[test]
fn test_multi_item_per_item_matches_single_item() {
    let single = frames::build_read_var(AREA_DB, 1, 0, 0, S7DataType::Real, 0x0042);
    let multi = frames::build_read_var_multi(
        &[ItemSpec {
            area: AREA_DB,
            db_number: 1,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        }],
        0x0042,
    );
    // 两函数第一个 VariableSpecification 段均从 [19..31] 开始（function + item_count + 12B）
    assert_eq!(&multi[19..31], &single[19..31]);
}

/// 多 item Read Var 响应：全成功 → 按顺序返回 Ok(value_bytes)
#[test]
fn test_parse_read_response_multi_all_ok() {
    let value1 = [0x00, 0x00, 0xA8, 0x41]; // 浮点 21.0 大端
    let value2 = [0x00, 0x00, 0x70, 0x41]; // 浮点 15.0
                                           // 构造响应：COTP(3) + S7 header(12) + param(2) + data
                                           // data = item1(4 + 4B) + item2(4 + 4B)
    let mut resp = vec![0x02, 0xF0, 0x80];
    resp.push(0x32);
    resp.push(PDU_ACK);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&[0x00, 0x01]); // PDU ref
    resp.extend_from_slice(&(2u16).to_be_bytes()); // param len
    let data_len = (4 + value1.len()) + (4 + value2.len());
    resp.extend_from_slice(&(data_len as u16).to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    resp.push(FUNC_READ);
    resp.push(2); // item count
    for v in [&value1[..], &value2[..]] {
        resp.push(RET_OK);
        resp.push(TS_BYTE);
        resp.extend_from_slice(&(v.len() as u16).to_be_bytes());
        resp.extend_from_slice(v);
    }
    let items = [
        ItemSpec {
            area: AREA_DB,
            db_number: 1,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        },
        ItemSpec {
            area: AREA_DB,
            db_number: 2,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        },
    ];
    let parsed = frames::parse_read_response_multi(&resp, &items).expect("parse ok");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].as_ref().unwrap(), &value1);
    assert_eq!(parsed[1].as_ref().unwrap(), &value2);
}

/// 多 item Read Var 响应：第 2 项失败 → 第 1 项 Ok，第 2 项 Err(return code)
#[test]
fn test_parse_read_response_multi_partial_failure() {
    let value1 = [0x12, 0x34, 0x56, 0x78];
    let mut resp = vec![0x02, 0xF0, 0x80, 0x32, PDU_ACK, 0x00, 0x00, 0x00, 0x01];
    resp.extend_from_slice(&(2u16).to_be_bytes());
    let data_len = (4 + value1.len()) + 4;
    resp.extend_from_slice(&(data_len as u16).to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    resp.push(FUNC_READ);
    resp.push(2);
    // item 1 OK
    resp.push(RET_OK);
    resp.push(TS_BYTE);
    resp.extend_from_slice(&(value1.len() as u16).to_be_bytes());
    resp.extend_from_slice(&value1);
    // item 2 失败（return code 0x05 = RET_NOT_FOUND）
    resp.push(RET_NOT_FOUND);
    resp.push(TS_BYTE);
    resp.extend_from_slice(&0u16.to_be_bytes());
    let items = [
        ItemSpec {
            area: AREA_DB,
            db_number: 1,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::DWord,
        },
        ItemSpec {
            area: AREA_DB,
            db_number: 999,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::DWord,
        },
    ];
    let parsed = frames::parse_read_response_multi(&resp, &items).expect("parse ok");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].as_ref().unwrap(), &value1);
    assert_eq!(parsed[1].as_ref().unwrap_err(), &RET_NOT_FOUND);
}

/// 多 item Read Var 响应：item count 不匹配 → InvalidFrame
#[test]
fn test_parse_read_response_multi_count_mismatch() {
    let mut resp = vec![0x02, 0xF0, 0x80, 0x32, PDU_ACK, 0x00, 0x00, 0x00, 0x01];
    resp.extend_from_slice(&(2u16).to_be_bytes());
    resp.extend_from_slice(&4u16.to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    resp.push(FUNC_READ);
    resp.push(1); // 服务端只回 1 项
    resp.push(RET_OK);
    resp.push(TS_BYTE);
    resp.extend_from_slice(&0u16.to_be_bytes());
    let items = [
        ItemSpec {
            area: AREA_DB,
            db_number: 1,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        },
        ItemSpec {
            area: AREA_DB,
            db_number: 2,
            byte_offset: 0,
            bit_offset: 0,
            data_type: S7DataType::Real,
        },
    ];
    let err = frames::parse_read_response_multi(&resp, &items).expect_err("count mismatch");
    let msg = format!("{err}");
    assert!(
        msg.contains("item count") || msg.contains("1"),
        "got: {msg}"
    );
}

/// 多 item Read Var 端到端：3 个 DB DWord 点位一次性读出（共享一次建链）
///
/// 桩把请求里的 item_count 解析出来后，用同一 value 重复回应 3 次（多 item
/// Read Var 标准响应格式），故 3 个点位读到的都是同一 4 字节值。
#[tokio::test]
async fn test_e2e_multi_read_three_db_dword() {
    let v1 = 0x11223344u32;
    let value = v1.to_be_bytes().to_vec();
    let (addr, conns, _writes) =
        spawn_s7_stub_counted(vec![S7StubAction::Serve], value.clone(), TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let pts = vec![
        s7_point("a", "DB1.DBD0", "dword"),
        s7_point("b", "DB2.DBD0", "dword"),
        s7_point("c", "DB3.DBD0", "dword"),
    ];
    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 3);
    for (i, r) in rs.iter().enumerate() {
        let dp = r.as_ref().unwrap_or_else(|e| panic!("pt {i} err: {e:?}"));
        let expected = u32::from_be_bytes([value[0], value[1], value[2], value[3]]);
        assert_eq!(dp.raw_value as u64, expected as u64, "pt {i}");
    }
    assert_eq!(conns.load(Ordering::SeqCst), 1, "多 item 应共享一次建链");
}

/// 多 item Read Var 端到端：某点位地址错误 → 该点位 InvalidConfig、其它点位仍读
#[tokio::test]
async fn test_e2e_multi_read_invalid_address_isolated() {
    let value = 0x01020304u32.to_be_bytes().to_vec();
    let (addr, _conns, _writes) =
        spawn_s7_stub_counted(vec![S7StubAction::Serve], value, TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let pts = vec![
        s7_point("ok1", "DB1.DBD0", "dword"),
        s7_point("bad", "INVALID", "dword"),
        s7_point("ok2", "DB2.DBD0", "dword"),
    ];
    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 3);
    assert!(rs[0].is_ok(), "ok1: {:?}", rs[0]);
    assert!(rs[1].is_err(), "bad should be err: {:?}", rs[1]);
    assert!(rs[2].is_ok(), "ok2: {:?}", rs[2]);
    // invalid address 一定不会发请求（先建帧后建链），故建链数 = 0 是允许的（视 mock 实现）
}

/// 空批次：read_batch 不建链
#[tokio::test]
async fn test_e2e_multi_read_empty() {
    let (addr, conns, _writes) =
        spawn_s7_stub_counted(vec![S7StubAction::Serve], vec![0u8; 0], TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let rs = c.read_batch(&[]).await;
    assert!(rs.is_empty());
    assert_eq!(conns.load(Ordering::SeqCst), 0, "空批不应建链");
}
