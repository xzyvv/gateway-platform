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
// C-3 + R-3: 帧提取的期望字节数必须区分「位」与「寄存器」
//
// FC01/FC02 读的是位（线圈/离散输入），响应字节数 = ceil(n / 8)；
// FC03/FC04 读寄存器，响应字节数 = n * 2。旧实现一律按 n * 2 校验，
// 且线圈路径恒传 None 使该 bug 长期休眠。R-3 已让 read_coils_with_retry
// 传 Some(count)，此处用例锁定「位公式」与「寄存器公式」互不串味。
// =========================================================================

/// 构造合法 Modbus RTU 响应帧：[slave][fc][byte_count][data...][crc_lo][crc_hi]
fn modbus_response(slave: u8, fc: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![slave, fc, data.len() as u8];
    f.extend_from_slice(data);
    let crc = crate::engine::codec::crc::crc16(&f);
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

/// R-4：FC21(0x15 写文件记录) 响应为变长回显，按 byte_count 定帧，
/// 且重同步不能吞掉紧随其后的正常帧。
#[test]
fn extract_modbus_frame_fc21_uses_byte_count() {
    use crate::south::bus::FrameBuffer;

    // 数据段 = ref_type(2) + file_num(2) + record_len(2) + data(2*1) = 8 字节
    let data = [0x00, 0x06, 0x00, 0x01, 0x00, 0x01, 0x12, 0x34];
    let frame = modbus_response(0x01, 0x15, &data);
    assert_eq!(frame.len(), 3 + 8 + 2);

    let mut fb = FrameBuffer::new();
    fb.push(&frame);
    let got = super::extract_modbus_frame(&mut fb, None);
    assert_eq!(
        got.as_deref(),
        Some(frame.as_slice()),
        "FC21 应按 byte_count 定帧"
    );

    // 后面紧跟一帧正常 FC03，验证没有被 FC21 的重同步逻辑吞掉
    let next = modbus_response(0x01, 0x03, &[0x00, 0x2A]);
    fb.push(&next);
    let got2 = super::extract_modbus_frame(&mut fb, Some(1));
    assert_eq!(got2.as_deref(), Some(next.as_slice()));
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
use crate::south::protocol::modbus::transaction::TransactionPool;
use crate::south::protocol::Collector;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// =========================================================================
// 第四批#1: 池满时短等待退避（原先池满即 Busy，瞬时拥塞直接失败）
// =========================================================================

#[tokio::test]
async fn pool_waits_for_slot_instead_of_failing_immediately() {
    let pool = TransactionPool::new(1);
    // 1 槽池：先占满
    let (tx_id, _rx) = pool
        .acquire_slot(false, Duration::from_millis(50), 1)
        .await
        .unwrap();

    // 池满 + 等待预算耗尽 → 仍应返回 Busy（语义不变，只是多等了一会儿）
    let t0 = std::time::Instant::now();
    let r = pool.acquire_slot(false, Duration::from_millis(20), 1).await;
    assert!(r.is_err(), "池满且等待预算耗尽必须返回 Busy");
    assert!(
        t0.elapsed() >= Duration::from_millis(20),
        "必须真的等待了预算时长，而不是立刻失败：{:?}",
        t0.elapsed()
    );

    // 槽位释放后，等待者应能立刻拿到（而不是自己失败）
    let pool2 = pool.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        pool2.deliver(tx_id, 1, vec![]).await;
    });
    let t1 = std::time::Instant::now();
    let (tx2, _rx2) = pool
        .acquire_slot(false, Duration::from_millis(500), 1)
        .await
        .expect("槽位释放后应能拿到槽位");
    assert!(
        t1.elapsed() >= Duration::from_millis(20),
        "应当等到槽位释放才返回：{:?}",
        t1.elapsed()
    );
    assert_ne!(tx2, tx_id, "tx_id 不能复用");
}

#[tokio::test]
async fn pool_control_transaction_reserves_slots_under_pressure() {
    // 8 槽池：普通事务上限 6，控制事务上限 8
    let pool = TransactionPool::new(8);
    let mut held = Vec::new();
    for _ in 0..6 {
        held.push(
            pool.acquire_slot(false, Duration::from_millis(10), 1)
                .await
                .unwrap(),
        );
    }
    // 普通事务再来一个：必须等不到（上限 6 已满）
    assert!(pool
        .acquire_slot(false, Duration::from_millis(20), 1)
        .await
        .is_err());
    // 控制事务仍有保留槽可用
    assert!(pool
        .acquire_slot(true, Duration::from_millis(20), 1)
        .await
        .is_ok());
}

/// 响应 unit_id 与请求的从站地址不一致 → 该事务按 Malformed 失败。
///
/// 多从站共享一个 TCP 网关时，tx_id 只能证明「回的是我们发起的」，证明不了
/// 「回的是我们要的那个从站」。串帧若被放行，A 的寄存器值会落到 B 的点位上，
/// 形成静默错读数——比超时难发现得多。
#[tokio::test]
async fn pool_rejects_response_with_mismatched_unit_id() {
    use crate::south::protocol::modbus::transaction::TransactionError;

    let pool = TransactionPool::new(2);
    let (tx_id, rx) = pool
        .acquire_slot(false, Duration::from_millis(50), 7)
        .await
        .unwrap();

    pool.deliver(tx_id, 9, vec![0x03, 0x02, 0x00, 0x01]).await;

    match rx.await.unwrap() {
        Err(TransactionError::Malformed(msg)) => {
            assert!(msg.contains("unit_id"), "错误文案要指明 unit_id：{msg}");
        }
        other => panic!("串帧必须拒绝，实际返回: {other:?}"),
    }
}

/// unit_id 一致时正常投递：不能把正常响应误判成串帧。
#[tokio::test]
async fn pool_accepts_response_with_matching_unit_id() {
    let pool = TransactionPool::new(2);
    let (tx_id, rx) = pool
        .acquire_slot(false, Duration::from_millis(50), 7)
        .await
        .unwrap();

    pool.deliver(tx_id, 7, vec![0x03, 0x02, 0x00, 0x2A]).await;

    assert_eq!(
        rx.await.unwrap().unwrap(),
        vec![0x03, 0x02, 0x00, 0x2A],
        "unit_id 匹配的响应必须原样交给调用方"
    );
}

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
// R-1 / R-3 端到端：真实 TCP 池 + RTU↔MBAP 转换，验证写异常语义与线圈定帧
// =========================================================================

/// R-1：从站拒绝写（FC06 回 0x86/0x02 异常帧）时必须保留 ModbusException。
///
/// 回归点：旧实现在 trait_impl 里用 `resp.len() < 8` 判回显，5 字节异常帧
/// 会被误判成 `InvalidFrame("write response mismatch")`，异常码彻底丢失，
/// 现场无法区分「设备拒绝写」与「总线噪声」。
#[tokio::test]
async fn write_exception_frame_maps_to_modbus_exception() {
    use crate::south::protocol::CollectorError;

    let addr = spawn_modbus_tcp_stub(vec![0x86, 0x02]).await;
    let c = super::ModbusCollector::new("modbus_write_exc", modbus_tcp_bus(addr, 1000));
    let point = mk_typed_point("U16"); // fc=6 → FC06 写单寄存器

    let err = c.write(&point, 1.0).await.expect_err("异常帧必须返回错误");
    assert!(
        matches!(err, CollectorError::ModbusException(0x02)),
        "异常码必须原样返回，实际 {err:?}"
    );
}

/// R-3：线圈读传入 expected_count 后，合法的 16 点 FC01 响应必须能正常解析。
///
/// 回归点：FC01/FC02 的期望字节数是 ceil(n/8)；若定帧沿用 n*2 公式，
/// 合法响应会被判非法并逐字节丢弃 → 批量线圈恒 NoReply、单点线圈正常，
/// 现场极难定位。
#[tokio::test]
async fn coil_read_with_expected_count_parses_valid_response() {
    // FC01 响应：byte_count=2，0xFF 0x00 → 前 8 点 ON、后 8 点 OFF
    let addr = spawn_modbus_tcp_stub(vec![0x01, 0x02, 0xFF, 0x00]).await;
    let c = super::ModbusCollector::new("modbus_coil_read", modbus_tcp_bus(addr, 1000));

    let coils = c
        .read_coils_with_retry(1, 0x01, 0, 16)
        .await
        .expect("合法 FC01 响应应解析成功");
    assert_eq!(coils.len(), 16);
    assert!(coils[..8].iter().all(|b| *b), "前 8 点应为 ON");
    assert!(coils[8..].iter().all(|b| !*b), "后 8 点应为 OFF");
}

// =========================================================================
// 写结果未知：写帧已下发但应答丢失
// =========================================================================

/// 假从站：只收不回，并统计收到的请求帧数（连接可被重开，循环 accept）。
async fn spawn_silent_modbus_stub() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let frames = Arc::new(AtomicUsize::new(0));
    let counter = frames.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let counter = counter.clone();
            tokio::spawn(async move {
                loop {
                    let mut header = [0u8; 7];
                    if stream.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let length = u16::from_be_bytes([header[4], header[5]]) as usize;
                    let body_len = length.saturating_sub(1);
                    let mut body = vec![0u8; body_len];
                    if body_len > 0 && stream.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    // 故意不回复：模拟「写帧已下发，但应答丢失」
                }
            });
        }
    });
    (addr, frames)
}

/// 写事务遇到「已下发但无应答」时：
/// ① **不得自动重发**（同一指令被执行多次 = 累加/脉冲点位数据错）；
/// ② 必须上报 `WriteUncertain`（可能已生效），而不是含糊的 NoReply。
#[tokio::test]
async fn write_timeout_is_not_resent_and_reports_uncertain() {
    use crate::south::protocol::CollectorError;

    let (addr, frames) = spawn_silent_modbus_stub().await;
    // retry_count=2：若写仍被自动重发，从站会收到 3 次同一请求
    let cfg = BusConfig {
        port_name: "modbus_write_no_retry".into(),
        protocol: "modbus".into(),
        tcp_addr: Some(addr),
        timeout_ms: 200,
        retry_count: Some(2),
        retry_delay_ms: Some(0),
        ..Default::default()
    };
    let bus = Arc::new(BusManager::spawn_with(
        &cfg,
        None,
        Arc::new(Metrics::new()),
        BusDeps::production(),
    ));
    let c = super::ModbusCollector::new("modbus_write_no_retry", bus);
    let point = mk_typed_point("U16"); // fc=6 → FC06 写单寄存器

    let err = c.write(&point, 1.0).await.expect_err("从站不应答必须失败");
    assert!(
        matches!(err, CollectorError::WriteUncertain(_)),
        "写结果未知必须用 WriteUncertain 上报，实际 {err:?}"
    );

    // 给可能存在的重发留出时间窗
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        frames.load(Ordering::SeqCst),
        1,
        "写帧不得被自动重发：从站只应收到 1 次请求"
    );
}

/// 写响应回显不匹配 → 应答无法解析 → 写很可能已执行 → `WriteUncertain`。
///
/// 回归点：旧实现直接返回 `InvalidFrame("write response mismatch")`，
/// 调用方无法区分「设备明确拒绝」（可安全重试）与「应答没看懂」
/// （重试可能造成重复写）。
#[tokio::test]
async fn write_echo_mismatch_is_write_uncertain() {
    use crate::south::protocol::CollectorError;

    // 从站回一帧 FC06 响应，但寄存器地址与请求不符（错配/串帧）
    let addr = spawn_modbus_tcp_stub(vec![0x06, 0x00, 0x99, 0x99, 0x00, 0x01]).await;
    let c = super::ModbusCollector::new("modbus_write_echo", modbus_tcp_bus(addr, 1000));
    let point = mk_typed_point("U16"); // fc=6 → FC06 写单寄存器

    let err = c
        .write(&point, 1.0)
        .await
        .expect_err("回显不匹配必须返回错误");
    assert!(
        matches!(err, CollectorError::WriteUncertain(_)),
        "回显不匹配属结果未知，必须用 WriteUncertain 上报，实际 {err:?}"
    );
}

/// 不可写功能码 → 帧都没构造出来（未下发）→ 明确失败 `Unsupported`（可安全重试）。
#[tokio::test]
async fn write_unsupported_function_code_is_not_uncertain() {
    use crate::south::protocol::CollectorError;

    let addr = spawn_modbus_tcp_stub(vec![0x06, 0x00, 0x00, 0x00, 0x00, 0x01]).await;
    let c = super::ModbusCollector::new("modbus_write_fc", modbus_tcp_bus(addr, 1000));
    let mut point = mk_typed_point("U16");
    point.function_code = Some(4); // FC04 只读

    let err = c
        .write(&point, 1.0)
        .await
        .expect_err("只读功能码必须返回错误");
    assert!(
        matches!(err, CollectorError::Unsupported(_)),
        "未下发的失败不得算结果未知，实际 {err:?}"
    );
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

/// 读合并的寄存器数必须与解码宽度同源：`data_type: f32` 配上 `register_count: 1`
/// 时仍按 2 个寄存器申请，否则只读 1 个寄存器、解码判定越界，点位静默无数据。
#[test]
fn read_batches_width_follows_data_type_over_register_count() {
    use super::merge_batches;

    let mut p = mk_addr_point(0, "f32");
    p.register_count = Some(1);
    let batches = merge_batches(&[&p], 0, 125);
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0].count, 2,
        "f32 需 2 个寄存器，不能被 register_count=1 压缩"
    );
}

/// 解码：寄存器数不足时必须返回 Bad，而不是越界 panic（脏帧/短响应）。
#[test]
fn parse_point_value_out_of_range_is_bad_not_panic() {
    use crate::engine::model::Quality;

    // f32 需要 2 个寄存器，这里只给 1 个
    let point = mk_addr_point(2, "f32");
    let dp = super::parse_point_value(&point, &[0x42F7], 0);
    assert_eq!(dp.quality, Quality::Bad);
    assert_eq!(dp.quality_reason, "modbus_decode_failed:DECODE");
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
    // R-1：原因前缀保留，后缀为具体稳定错误码（随实际异常码变化，故只锁前缀）
    assert!(
        out[0].quality_reason.starts_with("modbus_read_failed:"),
        "实际: {}",
        out[0].quality_reason
    );
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
/// 覆盖从站地址 / 功能码 / 字节数 / 数据区 / CRC 五个环节——任一环节漏校验
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
