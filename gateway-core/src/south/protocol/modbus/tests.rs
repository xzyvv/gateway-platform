//! Modbus tests
//!
//! 覆盖：
//! - Bug 186 (N16): Modbus 异常码分级测试
//! - 写多线圈/写多寄存器帧构建

use super::ModbusExceptionCode;
use crate::south::protocol::ModbusExceptionCode as PublicExceptionCode;
// =========================================================================
// Golden 报文回归（决策表 #10）
//
// 请求帧逐字节与 `tests/fixtures/golden/modbus/*.hex` 比对，锁定线上字节。
// =========================================================================

/// FC03 读保持寄存器请求（slave=1, addr=0, count=10）
#[test]
fn golden_modbus_read_req_fc03() {
    let f = super::frames::build_read_request(0x01, 0x03, 0, 10);
    crate::south::protocol::golden::assert_golden("modbus/read_req_fc03.hex", &f);
}

/// FC15 写多线圈请求（slave=1, addr=16, 4 个线圈 ON/OFF/ON/OFF）
#[test]
fn golden_modbus_write_multi_coils_fc15() {
    let coils = [true, false, true, false];
    let f = super::frames::build_write_multi_coils(0x01, 16, 4, &coils);
    crate::south::protocol::golden::assert_golden("modbus/write_multi_coils_fc15.hex", &f);
}

/// FC16 写多寄存器请求（slave=1, addr=10, 值 0x1234 / 0x5678）
#[test]
fn golden_modbus_write_multi_registers_fc16() {
    let f = super::frames::build_write_multi_registers(0x01, 10, &[0x1234, 0x5678]);
    crate::south::protocol::golden::assert_golden("modbus/write_multi_registers_fc16.hex", &f);
}

#[test]
fn test_classify_exception_config_error() {
    for code in [0x01u8, 0x02, 0x03] {
        let exc = ModbusExceptionCode::from_code(code);
        assert!(
            exc.is_config_error(),
            "code {:#04x} should be config_error",
            code
        );
        assert!(!exc.is_recoverable());
    }
}

#[test]
fn test_classify_exception_recoverable() {
    for code in [0x04u8, 0x05, 0x06, 0x08] {
        let exc = ModbusExceptionCode::from_code(code);
        assert!(
            exc.is_recoverable(),
            "code {:#04x} should be recoverable",
            code
        );
        assert!(!exc.is_config_error());
    }
}

#[test]
fn test_classify_exception_gateway_and_unknown() {
    for code in [0x0Au8, 0x0B] {
        let exc = ModbusExceptionCode::from_code(code);
        assert!(!exc.is_config_error());
        assert!(!exc.is_recoverable());
    }
    let exc = ModbusExceptionCode::from_code(0x7F);
    assert!(!exc.is_config_error());
    assert!(!exc.is_recoverable());
    assert_eq!(exc.code(), 0x7F);
}

#[test]
fn test_exception_description_is_actionable() {
    for code in [0x01u8, 0x02, 0x03, 0x04, 0x06, 0x0A, 0x0B] {
        let exc = ModbusExceptionCode::from_code(code);
        let desc = exc.description();
        assert!(!desc.is_empty());
        assert!(
            desc.contains("0x"),
            "description for {:#04x} should include hex code: {}",
            code,
            desc
        );
    }
}

#[test]
fn test_public_re_export_matches_internal() {
    use ModbusExceptionCode as Internal;
    let i = Internal::IllegalFunction;
    let p = PublicExceptionCode::IllegalFunction;
    assert_eq!(i.code(), p.code());
    assert_eq!(i.description(), p.description());
}

#[test]
fn test_build_write_multi_coils_single() {
    use crate::south::protocol::modbus::frames::build_write_multi_coils;
    let frame = build_write_multi_coils(1, 100, 1, &[true]);
    assert_eq!(frame[0], 1);
    assert_eq!(frame[1], 0x0F);
    assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 100);
    assert_eq!(u16::from_be_bytes([frame[4], frame[5]]), 1);
    assert_eq!(frame[6], 1);
    assert_eq!(frame[7], 0x01);
    assert_eq!(frame.len(), 10);
}

#[test]
fn test_build_write_multi_coils_packed() {
    use crate::south::protocol::modbus::frames::build_write_multi_coils;
    let coils = [true, false, true, false, true, false, true, false];
    let frame = build_write_multi_coils(1, 0, 8, &coils);
    assert_eq!(frame[6], 1);
    assert_eq!(frame[7], 0b01010101);
    assert_eq!(frame.len(), 10);
}

#[test]
fn test_build_write_multi_registers_single() {
    use crate::south::protocol::modbus::frames::build_write_multi_registers;
    let frame = build_write_multi_registers(1, 200, &[0x1234]);
    assert_eq!(frame[0], 1);
    assert_eq!(frame[1], 0x10);
    assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 200);
    assert_eq!(u16::from_be_bytes([frame[4], frame[5]]), 1);
    assert_eq!(frame[6], 2);
    assert_eq!(u16::from_be_bytes([frame[7], frame[8]]), 0x1234);
    assert_eq!(frame.len(), 11);
}

// =========================================================================
// C-3: 帧提取的期望字节数必须区分「位」与「寄存器」
//
// FC01/FC02 读的是位（线圈/离散输入），响应字节数 = ceil(n / 8)；
// FC03/FC04 读寄存器，响应字节数 = n * 2。旧实现一律按 n * 2 校验。
// 当前唯一入口 read_coils_with_retry 恒传 None，故未爆发；但一旦做"批量读线圈
// 优化"把 expected_count 传进来，合法响应会被判非法并逐字节丢弃，
// 症状是"批量线圈恒 NoReply、单点线圈正常"，现场极难定位。
// =========================================================================

/// 构造合法 Modbus RTU 响应帧：[slave][fc][byte_count][data...][crc_lo][crc_hi]
fn modbus_response(slave: u8, fc: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![slave, fc, data.len() as u8];
    f.extend_from_slice(data);
    let crc = crate::engine::util::crc::crc16(&f);
    f.push((crc & 0xFF) as u8);
    f.push((crc >> 8) as u8);
    f
}

/// C-3：FC01 位读取按 ceil(n / 8) 校验（19 个线圈 → 3 字节）
#[test]
fn extract_modbus_frame_fc01_uses_bit_count_formula() {
    use crate::south::bus::FrameBuffer;

    let frame = modbus_response(0x01, 0x01, &[0x01, 0x00, 0x00]);
    assert_eq!(frame.len(), 8);

    let mut fb = FrameBuffer::new();
    fb.push(&frame);
    let got = super::extract_modbus_frame(&mut fb, Some(19));
    assert_eq!(
        got.as_deref(),
        Some(frame.as_slice()),
        "FC01 位读取应按 ceil(n/8) 校验"
    );
}

/// C-3：FC02 离散输入同上（9 个点 → 2 字节）
#[test]
fn extract_modbus_frame_fc02_uses_bit_count_formula() {
    use crate::south::bus::FrameBuffer;

    let frame = modbus_response(0x02, 0x02, &[0xFF, 0x01]);
    let mut fb = FrameBuffer::new();
    fb.push(&frame);
    let got = super::extract_modbus_frame(&mut fb, Some(9));
    assert_eq!(got.as_deref(), Some(frame.as_slice()));
}

/// C-3：FC03 寄存器读取仍按 n * 2 校验（不能被位公式改坏）
#[test]
fn extract_modbus_frame_fc03_still_uses_register_formula() {
    use crate::south::bus::FrameBuffer;

    let frame = modbus_response(0x01, 0x03, &[0x00, 0x2A, 0x00, 0x2B]);
    let mut fb = FrameBuffer::new();
    fb.push(&frame);
    let got = super::extract_modbus_frame(&mut fb, Some(2));
    assert_eq!(
        got.as_deref(),
        Some(frame.as_slice()),
        "FC03 寄存器读取应按 n*2 校验"
    );
}

// =========================================================================
// Bug 219/220: 写路径编码（encode_register_values）与读路径解码往返一致
// =========================================================================

fn mk_typed_point(data_type: &str) -> crate::config::PointConfig {
    crate::config::PointConfig {
        id: "p_enc".into(),
        bus: "b".into(),
        protocol: "modbus".into(),
        address: "0".into(),
        address_num: Some(0),
        slave_id: Some(1),
        function_code: Some(6),
        data_type: Some(data_type.into()),
        interval: 1,
        // 与 serde 配置默认值一致（default_scale=1.0）；derive(Default) 的 0.0
        // 会被读路径 convert_value 按 raw*0+offset 归零
        scale: 1.0,
        ..Default::default()
    }
}

/// Bug 219: I16 负值必须编码为二进制补码，而不是饱和成 0
#[test]
fn test_encode_register_values_i16_negative() {
    let point = mk_typed_point("i16");
    assert_eq!(super::encode_register_values(&point, -5.0), vec![0xFFFBu16]);
    assert_eq!(
        super::encode_register_values(&point, -32768.0),
        vec![0x8000u16]
    );
    assert_eq!(
        super::encode_register_values(&point, 32767.0),
        vec![0x7FFFu16]
    );
}

/// Bug 220: 多寄存器类型必须编码出全部寄存器（f32/u32/i32 = 2 个）
#[test]
fn test_encode_register_values_multi_register() {
    // 123.5f32 = 0x42F70000
    let f32_point = mk_typed_point("f32");
    assert_eq!(
        super::encode_register_values(&f32_point, 123.5),
        vec![0x42F7u16, 0x0000]
    );
    // -123456i32 = 0xFFFE1DC0
    let i32_point = mk_typed_point("i32");
    assert_eq!(
        super::encode_register_values(&i32_point, -123_456.0),
        vec![0xFFFEu16, 0x1DC0]
    );
}

/// Bug 219/220: 写编码与读解码（parse_point_value）往返一致
#[test]
fn test_encode_register_values_roundtrip_with_read_path() {
    use crate::engine::model::Quality;
    for (dt, value) in [
        ("u16", 42.0f64),
        ("i16", -5.0),
        ("u32", 3_000_000_000.0),
        ("i32", -123_456.0),
        ("f32", 123.5),
    ] {
        let point = mk_typed_point(dt);
        let regs = super::encode_register_values(&point, value);
        let dp = super::parse_point_value(&point, &regs, 0);
        assert_eq!(dp.quality, Quality::Good, "{dt} 往返解码质量应为 Good");
        assert!(
            (dp.value - value).abs() < 1e-3,
            "{dt} 写编码/读解码往返不一致：写入 {value}，读回 {}",
            dp.value
        );
    }
}

// =========================================================================
// C2: Modbus 诊断（FC03 实读）——成功 / 异常码 / 无应答 均须如实报告
// =========================================================================

use crate::config::BusConfig;
use crate::engine::metrics::Metrics;
use crate::south::bus::{BusDeps, BusManager};
use crate::south::protocol::Collector;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 假 Modbus TCP 服务端：accept 一次，读走 MBAP 请求，按 `pdu` 回一帧 MBAP 响应。
async fn spawn_modbus_tcp_stub(pdu: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    tokio::spawn(async move {
        let (mut stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut header = [0u8; 7];
        if stream.read_exact(&mut header).await.is_err() {
            return;
        }
        let tx_id = u16::from_be_bytes([header[0], header[1]]);
        let unit_id = header[6];
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
        let body_len = length.saturating_sub(1);
        let mut body = vec![0u8; body_len];
        if body_len > 0 && stream.read_exact(&mut body).await.is_err() {
            return;
        }

        let mut resp = Vec::with_capacity(7 + pdu.len());
        resp.extend_from_slice(&tx_id.to_be_bytes());
        resp.extend_from_slice(&0u16.to_be_bytes());
        resp.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
        resp.push(unit_id);
        resp.extend_from_slice(&pdu);
        let _ = stream.write_all(&resp).await;
        // 保持连接一小会儿，避免对端重连造成竞态
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
    addr
}

fn modbus_tcp_bus(addr: String, timeout_ms: u64) -> Arc<BusManager> {
    let cfg = BusConfig {
        port_name: "modbus_diag".into(),
        protocol: "modbus".into(),
        tcp_addr: Some(addr),
        timeout_ms,
        retry_count: Some(0),
        retry_delay_ms: Some(0),
        ..Default::default()
    };
    Arc::new(BusManager::spawn_with(
        &cfg,
        None,
        Arc::new(Metrics::new()),
        BusDeps::production(),
    ))
}

/// 成功路径：从站正常回 FC03 响应 → 报告寄存器数量与回读值
#[tokio::test]
async fn diag_reports_fc03_read_success_and_register_count() {
    // MBAP 应答 PDU：fc=03, byte_count=02, value=0x002A
    let addr = spawn_modbus_tcp_stub(vec![0x03, 0x02, 0x00, 0x2A]).await;
    let c = super::ModbusCollector::new("modbus_diag", modbus_tcp_bus(addr, 1000));
    assert!(c.diag_supported(), "modbus 已覆写 diag，能力位必须为 true");

    let msg = c.diag().await;
    assert!(msg.contains("FC03 读取成功"), "{msg}");
    assert!(msg.contains("寄存器数=1"), "{msg}");
    assert!(msg.contains("回读值=42"), "{msg}");
}

/// 异常码路径：链路可达但从站拒绝 → 如实报异常码，不得说成功
#[tokio::test]
async fn diag_reports_modbus_exception_code() {
    // MBAP 应答 PDU：fc=0x83（03|0x80）, 异常码 0x02（非法数据地址）
    let addr = spawn_modbus_tcp_stub(vec![0x83, 0x02]).await;
    let c = super::ModbusCollector::new("modbus_diag_exc", modbus_tcp_bus(addr, 1000));

    let msg = c.diag().await;
    assert!(msg.contains("异常码 0x02"), "{msg}");
    assert!(!msg.contains("读取成功"), "异常答复不得报成功: {msg}");
}

/// 无应答路径：端点不可达 → 如实报链路失败，不得伪造成功
#[tokio::test]
async fn diag_reports_unreachable_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    drop(listener);

    let c = super::ModbusCollector::new("modbus_diag_dead", modbus_tcp_bus(addr, 300));
    let msg = c.diag().await;
    assert!(msg.contains("无应答/链路失败"), "{msg}");
    assert!(!msg.contains("读取成功"), "不可达端点不得报成功: {msg}");
}
