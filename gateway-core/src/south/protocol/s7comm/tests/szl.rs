//! SZL 诊断通路测试（决策表 #12：Userdata PDU 0x07 / Read SZL 子功能 0x01）。
use super::*;

use std::time::Duration;

use crate::south::protocol::Collector;

// ============================================================
// 决策表 #12：SZL 诊断（Userdata PDU 0x07 / Read SZL 子功能 0x01）
// ============================================================

/// 请求帧布局契约：TPKT/COTP/S7 头/固定 Parameter/Data 各段落位正确
#[test]
fn test_build_szl_request_layout() {
    let f = frames::build_szl_request(SZL_ID_COMPONENT_ALL, 1, 1);
    assert_eq!(f.len(), 33);
    assert_eq!(&f[0..4], &[0x03, 0x00, 0x00, 0x21]); // TPKT 总长 33
    assert_eq!(&f[4..7], &[0x02, 0xF0, 0x80]); // COTP DT
    assert_eq!(f[7], 0x32); // S7 协议 ID
    assert_eq!(f[8], PDU_USERDATA); // PDU 类型 = Userdata
    assert_eq!(u16::from_be_bytes([f[13], f[14]]), 8); // Parameter length
    assert_eq!(u16::from_be_bytes([f[15], f[16]]), 8); // Data length
    assert_eq!(&f[17..25], &SZL_PARAM); // Parameter 固定 8 字节
    assert_eq!(&f[25..29], &[RET_OK, 0x09, 0x00, 0x04]); // return + ts + 后续长度 4
    assert_eq!(u16::from_be_bytes([f[29], f[30]]), SZL_ID_COMPONENT_ALL); // SZL-ID
    assert_eq!(u16::from_be_bytes([f[31], f[32]]), 1); // SZL-Index
}

/// 响应解析契约：头字段与记录切分正确
#[test]
fn test_parse_szl_response_ok() {
    let frame = s7_szl_ack_body();
    let r = frames::parse_szl_response(&frame).expect("SZL 响应应解析成功");
    assert_eq!(r.szl_id, 0x0011);
    assert_eq!(r.szl_index, 1);
    assert_eq!(r.record_count, 1);
    assert_eq!(r.list_len, 42); // 34 字节记录 + 8 字节 SZL 头
    assert_eq!(r.records.len(), 1);
    assert_eq!(r.records[0].len(), 34);

    let strs = frames::extract_ascii_strings(&r.records);
    assert!(
        strs.contains(&"6ES7 315-2AG10-0AB0".to_string()),
        "应提取到订货号，got {strs:?}"
    );
}

/// 返回码非 0xFF 必须判为失败
#[test]
fn test_parse_szl_response_bad_return_code() {
    let mut frame = s7_szl_ack_body();
    frame[21] = 0x01; // data 段首字节 = 返回码（COTP3 + S7头10 + Parameter8）
    assert!(frames::parse_szl_response(&frame).is_err());
}

/// 错误 PDU 类型（0x87 = Userdata with error）必须判为失败
#[test]
fn test_parse_szl_response_pdu_error() {
    let mut frame = s7_szl_ack_body();
    frame[4] = 0x87;
    assert!(frames::parse_szl_response(&frame).is_err());
}

/// ASCII 文本提取：忽略 < 4 字符片段并去重
#[test]
fn test_extract_ascii_strings_filters_and_dedups() {
    let recs = vec![
        b"CPU 315-2 DP\x00\x00".to_vec(),
        b"CPU 315-2 DP\x00xx".to_vec(), // 重复项，且 "xx" 过短
        vec![0x00, 0x01, 0x02],         // 无可读文本
    ];
    let strs = frames::extract_ascii_strings(&recs);
    assert_eq!(strs, vec!["CPU 315-2 DP".to_string()]);
}

/// E2E：diag 走 SZL 诊断路径，返回 CPU 订货号，且两次 SZL 共享同一次建链
#[tokio::test]
async fn diag_reports_component_identification() {
    let value = 0.0f32.to_be_bytes().to_vec();
    let (addr, conns, _writes) =
        spawn_s7_stub_counted(vec![S7StubAction::Serve], value, TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));

    let d = c.diag().await;
    assert!(d.contains("s7comm@s7-bus"), "diag={d}");
    assert!(d.contains("6ES7 315-2AG10-0AB0"), "diag={d}");
    assert_eq!(
        conns.load(Ordering::SeqCst),
        1,
        "diag 的两次 SZL 读取应共享一次建链"
    );
}
