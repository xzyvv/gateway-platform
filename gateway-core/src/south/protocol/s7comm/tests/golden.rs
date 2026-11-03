//! Golden 报文回归（决策表 #10）
//!
//! 请求帧逐字节与 `tests/fixtures/golden/s7comm/*.hex` 比对；响应样本做解码断言。
use super::*;

/// S7Comm Write Var（FUNC_WRITE 0x05）请求：DB1.DBD0 ← REAL 23.5，pdu_ref 钉死为 1
#[test]
fn golden_s7comm_write_var_db1_dbd0_real() {
    let f = frames::build_write_var(AREA_DB, 1, 0, 0, S7DataType::Real, 23.5, 1)
        .expect("构建 S7 Write Var 请求失败");
    crate::south::protocol::golden::assert_golden("s7comm/write_var_db1_dbd0_real.hex", &f);
}

/// S7Comm Write Var 成功响应（0x05 Ack-Data，return code 0xFF）必须判为写成功
///
/// 注意：与仓库既有约定一致，`parse_*_response` 的输入为「COTP DT(3) + S7 头」，
/// 不含 4 字节 TPKT（由调用方剥离）。
#[test]
fn golden_s7comm_write_resp_ok_decode() {
    let frame = s7_write_ack_body();
    crate::south::protocol::golden::assert_golden("s7comm/write_resp_ok.hex", &frame);
    let loaded = crate::south::protocol::golden::load("s7comm/write_resp_ok.hex");
    frames::parse_write_response(&loaded).expect("golden 样本必须被判为写成功响应");
}

/// Read SZL 请求帧逐字节与 golden 样本比对（SZL-ID 0x0011，Index 1，pdu_ref 1）
#[test]
fn golden_s7comm_szl_request_component_id() {
    let f = frames::build_szl_request(SZL_ID_COMPONENT_ID, 1, 1);
    crate::south::protocol::golden::assert_golden("s7comm/szl_request_component_id.hex", &f);
}
