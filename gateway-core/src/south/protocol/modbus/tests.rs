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

// =========================================================================
// 畸形帧负例：任意截断 / 长度字段撒谎 / CRC 错误都不得 panic，必须返回 Err
// =========================================================================

#[test]
fn malformed_modbus_read_response_is_rejected_without_panic() {
    use super::frames::{parse_read_response, rtu_frame};
    use crate::south::protocol::CollectorError;

    // 合法 FC03 响应：2 个寄存器 = 0x1234 / 0x5678
    let full = rtu_frame(&[0x01, 0x03, 0x04, 0x12, 0x34, 0x56, 0x78]);
    assert_eq!(
        parse_read_response(1, 3, 2, &full).expect("合法帧应解析成功"),
        vec![0x1234, 0x5678]
    );

    // 任意严格前缀（含 0 字节）都必须报错而不是越界读取
    for cut in 0..full.len() {
        assert!(
            parse_read_response(1, 3, 2, &full[..cut]).is_err(),
            "截断到 {cut} 字节应被拒绝"
        );
    }

    // CRC 篡改
    let mut bad_crc = full.clone();
    *bad_crc.last_mut().unwrap() ^= 0xFF;
    assert!(matches!(
        parse_read_response(1, 3, 2, &bad_crc),
        Err(CollectorError::ChecksumError)
    ));

    // 字节数字段撒谎：声明 4 字节，但调用方期望 1 个寄存器（2 字节）
    assert!(parse_read_response(1, 3, 1, &full).is_err());
    // 零长度请求显式拒绝
    assert!(parse_read_response(1, 3, 0, &full).is_err());
    // 从站 / 功能码不匹配
    assert!(parse_read_response(2, 3, 2, &full).is_err());
    assert!(parse_read_response(1, 4, 2, &full).is_err());
    // 异常响应走 ModbusException，而不是当成正常数据
    let exc = rtu_frame(&[0x01, 0x83, 0x02]);
    assert!(matches!(
        parse_read_response(1, 3, 2, &exc),
        Err(CollectorError::ModbusException(0x02))
    ));
}

#[test]
fn malformed_modbus_coils_response_is_rejected_without_panic() {
    use super::frames::{parse_coils_response, rtu_frame};
    use crate::south::protocol::CollectorError;

    let full = rtu_frame(&[0x01, 0x01, 0x01, 0b0000_0101]);
    assert_eq!(
        parse_coils_response(1, 1, 3, &full).expect("合法帧应解析成功"),
        vec![true, false, true]
    );

    for cut in 0..full.len() {
        assert!(
            parse_coils_response(1, 1, 3, &full[..cut]).is_err(),
            "截断到 {cut} 字节应被拒绝"
        );
    }

    let mut bad_crc = full.clone();
    *bad_crc.last_mut().unwrap() ^= 0xFF;
    assert!(matches!(
        parse_coils_response(1, 1, 3, &bad_crc),
        Err(CollectorError::ChecksumError)
    ));

    // 期望 0 个线圈 / 字节数与期望不符
    assert!(parse_coils_response(1, 1, 0, &full).is_err());
    assert!(parse_coils_response(1, 1, 16, &full).is_err());
    // 功能码不匹配（FC02 响应被当成 FC01）
    assert!(parse_coils_response(1, 2, 3, &full).is_err());
}

// =========================================================================
// 批量合并（batch.rs）：读合并允许空洞，写合并必须严格连续。
//
// 写路径是高风险区：FC15/FC16 写帧会覆盖 [start, start+units) 内的**全部**地址，
// 若把中间有空洞的点位并进同一帧，就会把空洞地址上原有的线圈/寄存器一起改掉，
// 属于「配置没错、线上数据却被改坏」的现场事故。以下用例锁定该不变量。
// =========================================================================

fn mk_addr_point(addr: u16, data_type: &str) -> crate::config::PointConfig {
    let mut p = mk_typed_point(data_type);
    p.address = addr.to_string();
    p.address_num = Some(addr);
    p
}

/// 写合并：地址出现空洞必须拆帧，不能把 (10,1)(11,2)(14,1) 并成一帧读到 12/13。
#[test]
fn write_batches_split_on_address_gap() {
    use super::merge_write_batches;

    let items = [(10u16, 1u16), (11, 2), (14, 1)];
    let batches = merge_write_batches(&items, super::MAX_WRITE_REGISTERS);
    assert_eq!(
        batches.len(),
        2,
        "14 与前一帧不连续，必须拆帧: {:?}",
        batches
    );
    assert_eq!((batches[0].start_addr, batches[0].units), (10, 3));
    assert_eq!(batches[0].range, 0..2);
    assert_eq!((batches[1].start_addr, batches[1].units), (14, 1));
    assert_eq!(batches[1].range, 2..3);
}

/// 写合并：连续项超出 FC16 单帧上限时另起一帧，任何一帧都不得超过 123。
#[test]
fn write_batches_split_when_exceeding_frame_limit() {
    use super::merge_write_batches;

    let items = [(0u16, 60u16), (60, 60), (120, 4)];
    let batches = merge_write_batches(&items, super::MAX_WRITE_REGISTERS);
    assert_eq!(batches.len(), 2);
    assert_eq!((batches[0].start_addr, batches[0].units), (0, 120));
    assert_eq!((batches[1].start_addr, batches[1].units), (120, 4));
    for b in &batches {
        assert!(
            b.units <= super::MAX_WRITE_REGISTERS,
            "写帧超过 FC16 上限: {:?}",
            b
        );
    }
}

/// 写合并：恰好用满 123 个寄存器仍应合成一帧（边界含等号）。
#[test]
fn write_batches_merge_exactly_at_limit() {
    use super::merge_write_batches;

    let items = [(5u16, 100u16), (105, 23)];
    let batches = merge_write_batches(&items, super::MAX_WRITE_REGISTERS);
    assert_eq!(batches.len(), 1);
    assert_eq!((batches[0].start_addr, batches[0].units), (5, 123));
}

/// 读合并：空洞 ≤ gap_threshold 时并成一帧，超过阈值才拆。
#[test]
fn read_batches_merge_gap_within_threshold() {
    use super::merge_batches;

    let p0 = mk_addr_point(0, "u16");
    let p2 = mk_addr_point(2, "u16");
    let p10 = mk_addr_point(10, "u16");
    let batches = merge_batches(&[&p0, &p2, &p10], 2, 125);
    assert_eq!(batches.len(), 2);
    assert_eq!((batches[0].start_addr, batches[0].count), (0, 3));
    assert_eq!(batches[0].points.len(), 2);
    assert_eq!((batches[1].start_addr, batches[1].count), (10, 1));
}

/// 读合并：不得超过 max_count，否则会读到设备映射末尾之外的地址。
#[test]
fn read_batches_never_cross_max_count() {
    use super::merge_batches;

    let p0 = mk_addr_point(0, "u16");
    // 124 + 1 恰好等于 125 上界 → 允许合并（gap 放宽到 1000，专门隔离 max_count 判定）
    let p124 = mk_addr_point(124, "u16");
    let batches = merge_batches(&[&p0, &p124], 1000, 125);
    assert_eq!(batches.len(), 1);
    assert_eq!((batches[0].start_addr, batches[0].count), (0, 125));

    // 125 + 1 = 126 越过上界 → 必须拆帧
    let p125 = mk_addr_point(125, "u16");
    let batches = merge_batches(&[&p0, &p125], 1000, 125);
    assert_eq!(batches.len(), 2);
}

/// 解码：寄存器数不足时必须返回 Bad，而不是越界 panic（脏帧/短响应）。
#[test]
fn parse_point_value_out_of_range_is_bad_not_panic() {
    use crate::engine::model::Quality;

    // f32 需要 2 个寄存器，这里只给 1 个
    let point = mk_addr_point(2, "f32");
    let dp = super::parse_point_value(&point, &[0x42F7], 0);
    assert_eq!(dp.quality, Quality::Bad);
    assert_eq!(dp.quality_reason, "modbus_decode_failed");
}

/// 写元件类型判定与 `write` 的功能码语义一致；其余功能码视为只读。
#[test]
fn write_kind_mapping_matches_function_code_semantics() {
    use super::batch::WriteKind;

    for fc in [1u8, 5, 15] {
        assert_eq!(super::write_kind_of(fc), Some(WriteKind::Coil), "fc={fc}");
    }
    for fc in [3u8, 6, 16] {
        assert_eq!(
            super::write_kind_of(fc),
            Some(WriteKind::Register),
            "fc={fc}"
        );
    }
    for fc in [0u8, 2, 4, 20, 99] {
        assert_eq!(super::write_kind_of(fc), None, "fc={fc} 应视为只读");
    }
}

// =========================================================================
// 采集主路径（collector.rs）：批量读成功 / 异常分级 / 可恢复异常只重试一次
//
// 现场最常见的两种真实故障是「从站回了异常码」和「异常码后恢复」。
// 这里用可脚本化的 TCP 假从站跑通 read_batch_points，锁定：
// 1) 连续地址合并成一帧后，各点位偏移解析正确；
// 2) 配置类异常不重试、只计入 config_error；
// 3) 可恢复异常退避重试一次，成功后才算 Good，不计入 retry_exhausted。
// =========================================================================

/// 可脚本化的假从站：按 `script` 顺序应答。
///
/// 同一连接上可能连续来多次事务（TCP 复用），也可能每次事务重连，两种都要能服务：
/// 外层 `accept` 负责重连，内层读循环负责同一连接上的后续事务。
/// `None` 表示本次不回包并断开（模拟无应答）。
async fn spawn_modbus_tcp_script(mut script: Vec<Option<Vec<u8>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    tokio::spawn(async move {
        let mut idx = 0usize;
        while idx < script.len() {
            let (mut stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            while idx < script.len() {
                let mut header = [0u8; 7];
                if stream.read_exact(&mut header).await.is_err() {
                    break; // 客户端每次事务新建连接 → 回到 accept
                }
                let tx_id = u16::from_be_bytes([header[0], header[1]]);
                let unit_id = header[6];
                let length = u16::from_be_bytes([header[4], header[5]]) as usize;
                let body_len = length.saturating_sub(1);
                let mut body = vec![0u8; body_len];
                if body_len > 0 && stream.read_exact(&mut body).await.is_err() {
                    break;
                }
                match script[idx].take() {
                    Some(pdu) => {
                        idx += 1;
                        let mut resp = Vec::with_capacity(7 + pdu.len());
                        resp.extend_from_slice(&tx_id.to_be_bytes());
                        resp.extend_from_slice(&0u16.to_be_bytes());
                        resp.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
                        resp.push(unit_id);
                        resp.extend_from_slice(&pdu);
                        if stream.write_all(&resp).await.is_err() {
                            return;
                        }
                    }
                    None => return,
                }
            }
        }
    });
    addr
}

fn mk_poll_point(id: &str, addr: u16) -> crate::config::PointConfig {
    let mut p = mk_typed_point("u16");
    p.id = id.into();
    p.address = addr.to_string();
    p.address_num = Some(addr);
    p.function_code = Some(3);
    p.slave_id = Some(1);
    p
}

/// 连续地址合并为一帧后，每个点位必须按各自偏移取到正确的寄存器。
#[tokio::test]
async fn batch_read_reports_values_for_merged_registers() {
    use crate::engine::model::Quality;

    // 地址 0、1 连续 → 合并为一帧 FC03 count=2，回 42 / 100
    let addr = spawn_modbus_tcp_script(vec![Some(vec![0x03, 0x04, 0x00, 0x2A, 0x00, 0x64])]).await;
    let c = super::ModbusCollector::new("modbus_batch_ok", modbus_tcp_bus(addr, 1000));
    let pts = vec![mk_poll_point("p0", 0), mk_poll_point("p1", 1)];

    let out = c.read_batch_points(&pts).await;
    assert_eq!(out.len(), 2, "每个输入点位都必须有结果");
    assert!(out.iter().all(|d| d.quality == Quality::Good), "{:?}", out);
    assert_eq!(out[0].value, 42.0, "地址 0 应取 0x002A");
    assert_eq!(out[1].value, 100.0, "地址 1 应取 0x0064");
}

/// 配置类异常（0x02 非法数据地址）：不重试，只计 config_error。
#[tokio::test]
async fn batch_read_config_error_is_reported_without_retry() {
    use crate::engine::model::Quality;
    use std::sync::atomic::Ordering;

    let addr = spawn_modbus_tcp_script(vec![Some(vec![0x83, 0x02])]).await;
    let metrics = Arc::new(Metrics::new());
    let c = super::ModbusCollector::new("modbus_batch_cfg_err", modbus_tcp_bus(addr, 1000))
        .with_metrics(metrics.clone());
    let pts = vec![mk_poll_point("p0", 0)];

    let out = c.read_batch_points(&pts).await;
    assert_eq!(out[0].quality, Quality::Bad);
    assert_eq!(out[0].quality_reason, "modbus_read_failed");
    assert_eq!(
        metrics
            .protocol_exception
            .modbus_config_error
            .load(Ordering::Relaxed),
        1,
        "配置类异常应计入 config_error"
    );
    assert_eq!(
        metrics
            .protocol_exception
            .modbus_recoverable
            .load(Ordering::Relaxed),
        0,
        "配置类异常不得计入 recoverable"
    );
    assert_eq!(
        metrics
            .protocol_exception
            .modbus_retry_exhausted
            .load(Ordering::Relaxed),
        0,
        "配置类异常不应重试"
    );
}

/// 可恢复异常（0x06 从站忙）：退避重试一次，成功后必须回到 Good。
#[tokio::test]
async fn batch_read_retries_recoverable_exception_once() {
    use crate::engine::model::Quality;
    use std::sync::atomic::Ordering;

    let addr = spawn_modbus_tcp_script(vec![
        Some(vec![0x83, 0x06]),
        Some(vec![0x03, 0x02, 0x00, 0x2A]),
    ])
    .await;
    let metrics = Arc::new(Metrics::new());
    let c = super::ModbusCollector::new("modbus_batch_retry", modbus_tcp_bus(addr, 1000))
        .with_metrics(metrics.clone());
    let pts = vec![mk_poll_point("p0", 0)];

    let out = c.read_batch_points(&pts).await;
    assert_eq!(out[0].quality, Quality::Good, "{:?}", out[0]);
    assert_eq!(out[0].value, 42.0);
    assert_eq!(
        metrics
            .protocol_exception
            .modbus_recoverable
            .load(Ordering::Relaxed),
        1,
        "第一次异常应计入 recoverable"
    );
    assert_eq!(
        metrics
            .protocol_exception
            .modbus_retry_exhausted
            .load(Ordering::Relaxed),
        0,
        "重试成功不应计入 retry_exhausted"
    );
}

// =========================================================================
// P2-1：协议帧性质测试（确定性伪随机，无新增依赖）
//
// 历史缺陷 Bug 62 / 121 / 122 / 124 / C1 / C-3 全部落在 extract / parse 的
// 长度公式与校验位上。这里用「噪声重同步 / 单比特破坏必拒 / 编解码往返」
// 三条性质把它们钉住，避免再次静默吞帧或假接受。
// =========================================================================

use crate::south::bus::FrameBuffer;
use crate::south::protocol::test_support::Lcg;

/// 构造 FC03 响应帧：`[slave][03][byte_count][寄存器大端…][CRC]`
fn fc03_response(slave: u8, regs: &[u16]) -> Vec<u8> {
    let mut body = vec![slave, 0x03, (regs.len() * 2) as u8];
    for r in regs {
        body.extend_from_slice(&r.to_be_bytes());
    }
    super::frames::rtu_frame(&body)
}

/// 性质：垃圾前缀之后，提取器必须仍能重同步出后面的合法帧。
///
/// 只断言「不 panic」不够：现场 NoReply 的真实成因是**帧被吞掉**。
///
/// 噪声用 0xFF：RTU 无起始符，0xFF 在任意窗口都会被判为未知功能码而逐字节丢弃。
/// 注意这里刻意不放「半截 FC03 帧」——残帧的 byte_count 字段会被当成后续帧的长度，
/// 提取器按协议只能等待更多字节（由调用方超时清空事务缓冲兜底），属模态无起始符
/// 协议的固有折衷，不作为本性质断言。
#[test]
fn extract_modbus_frame_resyncs_to_valid_frame_after_garbage() {
    let good = fc03_response(0x11, &[0x1234, 0x5678]);

    let mut fb = FrameBuffer::new();
    fb.push(&[0xFFu8; 37]);
    fb.push(&good);

    let mut frames = Vec::new();
    for _ in 0..64 {
        match super::frames::extract_modbus_frame(&mut fb, None) {
            Some(f) => frames.push(f),
            None => break,
        }
    }
    assert_eq!(
        frames.last(),
        Some(&good),
        "噪声之后必须能重同步出合法帧，实际提取到 {} 帧",
        frames.len()
    );
}

/// 性质：合法响应的任意单比特翻转都必须被拒绝。
///
/// 覆盖从站号 / 功能码 / 字节数 / 数据区 / CRC 五个环节——任一环节漏校验
/// 都会变成「错误数据入库」，比丢帧更难发现。
#[test]
fn parse_read_response_rejects_every_single_bit_flip() {
    let frame = fc03_response(0x07, &[0x0001, 0xABCD, 0xFFFF]);
    assert_eq!(
        super::frames::parse_read_response(0x07, 0x03, 3, &frame).expect("对照帧必须可解析"),
        vec![0x0001, 0xABCD, 0xFFFF]
    );
    for byte in 0..frame.len() {
        for bit in 0..8 {
            let mut f = frame.clone();
            f[byte] ^= 1 << bit;
            assert!(
                super::frames::parse_read_response(0x07, 0x03, 3, &f).is_err(),
                "字节 {byte} 第 {bit} 位翻转后仍被接受"
            );
        }
    }
}

/// 性质：随机字节流不得 panic，且提取循环必须有界（缓冲不被无限增长）。
#[test]
fn extract_modbus_frame_random_bytes_never_panics() {
    let mut lcg = Lcg::new(0x4D4F_4442_5553_5031);
    for _ in 0..2000 {
        let len = lcg.below(300);
        let noise = lcg.bytes(len);
        let mut fb = FrameBuffer::new();
        fb.push(&noise);
        for _ in 0..256 {
            if super::frames::extract_modbus_frame(&mut fb, None).is_none() {
                break;
            }
        }
        assert!(fb.len() <= 4096, "缓冲应被硬上限约束，实际 {}", fb.len());
    }
}

/// 性质：FC01/FC02 的「每 8 点 1 字节」与 FC03/FC04 的「每点 2 字节」
/// 两种长度公式都必须自洽（C-3 的回归防线）。
#[test]
fn read_roundtrip_covers_coil_and_register_length_formulas() {
    for n in 1u16..=64 {
        let regs: Vec<u16> = (0..n).map(|i| 0xA000 ^ i).collect();
        let frame = fc03_response(0x21, &regs);
        assert_eq!(
            super::frames::parse_read_response(0x21, 0x03, n, &frame).expect("FC03 必须可解析"),
            regs
        );

        let byte_count = (n as usize).div_ceil(8);
        let mut body = vec![0x21u8, 0x01, byte_count as u8];
        body.resize(3 + byte_count, 0xFF);
        let frame = super::frames::rtu_frame(&body);
        let mut fb = FrameBuffer::new();
        fb.push(&frame);
        assert_eq!(
            super::frames::extract_modbus_frame(&mut fb, Some(n)).as_deref(),
            Some(frame.as_slice()),
            "FC01 n={n} 的位计数公式必须能取出整帧"
        );
        assert_eq!(
            super::frames::parse_coils_response(0x21, 0x01, n, &frame)
                .expect("FC01 必须可解析")
                .len(),
            n as usize
        );
    }
}
