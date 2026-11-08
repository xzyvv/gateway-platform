//! BACnet MSTP 协议测试

use super::*;
use crate::south::bus::frame_buffer::FrameBuffer;

use crate::engine::metrics::Metrics;

fn test_metrics() -> Metrics {
    Metrics::new()
}
// ===== CRC 测试 =====

#[test]
fn test_crc8_header_known_value() {
    // 头部 [0x00, 0x00, 0x00, 0x00, 0x00]
    let crc = crc8_header(&[0x00, 0x00, 0x00, 0x00, 0x00]);
    // 计算：init=0xFF, 5 个 0x00 字节
    // 每个 0x00 不改变 crc 的值（除了 8 轮移位）
    // 第一个 0x00 后: 0xA0 (从之前的手算)
    // 后续 4 个 0x00 会继续移位
    assert_ne!(crc, 0x00); // 不为零
    assert_ne!(crc, 0xFF); // 不等于初始值
}

#[test]
fn test_crc8_header_empty() {
    // 空输入应返回初始值
    assert_eq!(crc8_header(&[]), 0xFF);
}

#[test]
fn test_crc8_header_consistency() {
    // 相同输入应得到相同输出
    let data = [0x04, 0x0A, 0x01, 0x00, 0x05];
    assert_eq!(crc8_header(&data), crc8_header(&data));
}

#[test]
fn test_crc16_data_empty() {
    // 空输入：init 0xFFFF, 取反 → 0x0000
    assert_eq!(crc16_data(&[]), 0x0000);
}

#[test]
fn test_crc16_data_known() {
    // "123456789" 的标准 CRC-16/Modbus = 0x4B37
    // BACnet MS/TP 对结果取反（~crc），所以 ~0x4B37 = 0xB4C8
    let data = b"123456789";
    assert_eq!(crc16_data(data), 0xB4C8);
}

#[test]
fn test_crc16_data_single_byte() {
    let crc = crc16_data(&[0x00]);
    // 非零结果
    assert_ne!(crc, 0);
}

// ===== 帧构建测试 =====

#[test]
fn test_build_mstp_frame_no_data() {
    // 无数据帧（如 Token 帧）
    let frame = build_mstp_frame(FRAME_TOKEN, 5, 1, &[]);
    assert_eq!(frame.len(), 8);
    assert_eq!(&frame[0..2], &PREAMBLE);
    assert_eq!(frame[2], FRAME_TOKEN);
    assert_eq!(frame[3], 5); // dest
    assert_eq!(frame[4], 1); // src
    assert_eq!(&frame[5..7], &[0x00, 0x00]); // length = 0
                                             // 头 CRC
    assert_eq!(frame[7], crc8_header(&frame[2..7]));
}

#[test]
fn test_build_mstp_frame_with_data() {
    let data = vec![0x01, 0x02, 0x03, 0x04];
    let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 10, 0, &data);
    // 前导码(2) + 头(5) + 头CRC(1) + data(4) + 数据CRC(2) = 14
    assert_eq!(frame.len(), 14);
    assert_eq!(&frame[0..2], &PREAMBLE);
    assert_eq!(frame[2], FRAME_DATA_NOT_EXPECTING_REPLY);
    assert_eq!(frame[3], 10);
    assert_eq!(frame[4], 0);
    assert_eq!(u16::from_be_bytes([frame[5], frame[6]]), 4);
    // 头 CRC
    assert_eq!(frame[7], crc8_header(&frame[2..7]));
    // 数据
    assert_eq!(&frame[8..12], &data[..]);
    // 数据 CRC (low byte first, then high byte)
    let expected_crc = crc16_data(&data);
    assert_eq!(frame[12], (expected_crc & 0xFF) as u8);
    assert_eq!(frame[13], (expected_crc >> 8) as u8);
}

#[test]
fn test_build_mstp_frame_max_data() {
    let data = vec![0xFF; MAX_DATA_LEN];
    let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &data);
    // 前导码(2) + 头(5) + 头CRC(1) + data(512) + 数据CRC(2) = 522
    assert_eq!(frame.len(), 522);
    assert_eq!(
        u16::from_be_bytes([frame[5], frame[6]]),
        MAX_DATA_LEN as u16
    );
}

// ===== 帧解析测试 =====

#[test]
fn test_parse_mstp_frame_no_data() {
    let frame = build_mstp_frame(FRAME_TOKEN, 5, 1, &[]);
    let (ftype, dest, src, data) = parse_mstp_frame(&frame, &test_metrics()).unwrap();
    assert_eq!(ftype, FRAME_TOKEN);
    assert_eq!(dest, 5);
    assert_eq!(src, 1);
    assert!(data.is_empty());
}

#[test]
fn test_parse_mstp_frame_with_data() {
    let data = vec![0x01, 0x02, 0x03, 0x04, 0x05];
    let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 10, 0, &data);
    let (ftype, dest, src, parsed_data) = parse_mstp_frame(&frame, &test_metrics()).unwrap();
    assert_eq!(ftype, FRAME_DATA_NOT_EXPECTING_REPLY);
    assert_eq!(dest, 10);
    assert_eq!(src, 0);
    assert_eq!(parsed_data, data);
}

#[test]
fn test_parse_mstp_frame_roundtrip() {
    // 多种数据组合的往返测试
    for data in [vec![], vec![0x01], vec![0x01, 0x02], vec![0xFF; 100]] {
        let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &data);
        let (_, _, _, parsed) = parse_mstp_frame(&frame, &test_metrics()).unwrap();
        assert_eq!(parsed, data);
    }
}

#[test]
fn test_parse_mstp_frame_bad_preamble() {
    let frame = vec![0x54, 0xFE, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00];
    assert!(parse_mstp_frame(&frame, &test_metrics()).is_err());
}

#[test]
fn test_parse_mstp_frame_bad_header_crc() {
    let mut frame = build_mstp_frame(FRAME_TOKEN, 5, 1, &[]);
    frame[7] = 0x00; // 破坏头 CRC
    assert!(parse_mstp_frame(&frame, &test_metrics()).is_err());
}

#[test]
fn test_parse_mstp_frame_bad_data_crc() {
    let data = vec![0x01, 0x02, 0x03];
    let mut frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &data);
    // 破坏数据 CRC
    frame[8 + data.len()] ^= 0xFF;
    assert!(parse_mstp_frame(&frame, &test_metrics()).is_err());
}

#[test]
fn test_parse_mstp_frame_too_short() {
    assert!(parse_mstp_frame(&[0x55, 0xFF, 0x04], &test_metrics()).is_err());
}

// ===== NPDU 跳过测试 =====

#[test]
fn test_skip_npdu_simple() {
    // 简单 NPDU：version=1, control=0x00, 后面是 APDU
    let data = [0x01, 0x00, 0x30, 0x01, 0x02, 0x0C];
    let apdu = BacnetCollector::skip_npdu(&data).unwrap();
    assert_eq!(apdu[0], 0x30); // ComplexACK
}

#[test]
fn test_skip_npdu_with_dnet() {
    // 带 DNET 的 NPDU
    // version=1, control=0x40 (bit 6=DNET present, bit 7=0=APDU), DNET=0x0001, DLEN=0, HopCount=0xFF
    let data = [0x01, 0x40, 0x00, 0x01, 0x00, 0xFF, 0x30, 0x01];
    let apdu = BacnetCollector::skip_npdu(&data).unwrap();
    assert_eq!(apdu[0], 0x30);
}

#[test]
fn test_skip_npdu_too_short() {
    assert!(BacnetCollector::skip_npdu(&[0x01]).is_err());
}

// ===== 配置解析测试 =====

#[test]
fn test_parse_master_addr_default() {
    assert_eq!(BacnetMstpCollector::parse_master_addr(""), 0);
    assert_eq!(BacnetMstpCollector::parse_master_addr("foo=bar"), 0);
}

#[test]
fn test_parse_master_addr_explicit() {
    assert_eq!(BacnetMstpCollector::parse_master_addr("master=5"), 5);
    assert_eq!(
        BacnetMstpCollector::parse_master_addr("max_master=127&master=3"),
        3
    );
}

// ===== 帧提取器测试 =====

#[test]
fn test_extract_mstp_frame_complete() {
    let mut fb = FrameBuffer::new();
    let data = vec![0x01, 0x02, 0x03];
    let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &data);
    fb.push(&frame);
    let extracted = extract_mstp_frame(&mut fb, None);
    assert!(extracted.is_some());
    let extracted = extracted.unwrap();
    assert_eq!(extracted, frame);
}

#[test]
fn test_extract_mstp_frame_partial() {
    let mut fb = FrameBuffer::new();
    let data = vec![0x01, 0x02, 0x03, 0x04, 0x05];
    let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &data);
    // 只推入半帧
    fb.push(&frame[..5]);
    assert!(extract_mstp_frame(&mut fb, None).is_none());
}

#[test]
fn test_extract_mstp_frame_with_garbage() {
    let mut fb = FrameBuffer::new();
    let data = vec![0x01, 0x02];
    let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &data);
    // 前面加垃圾字节
    let mut buf = vec![0xAA, 0xBB, 0xCC];
    buf.extend_from_slice(&frame);
    fb.push(&buf);
    let extracted = extract_mstp_frame(&mut fb, None);
    assert!(extracted.is_some());
    let extracted = extracted.unwrap();
    assert_eq!(extracted[0], 0x55); // 应从前导码开始
}

#[test]
fn test_extract_mstp_frame_bad_crc() {
    let mut fb = FrameBuffer::new();
    let mut frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &[0x01, 0x02]);
    // 破坏头 CRC
    frame[7] ^= 0xFF;
    fb.push(&frame);
    // 应跳过坏帧
    assert!(extract_mstp_frame(&mut fb, None).is_none());
}

#[test]
fn test_extract_mstp_frame_empty_data() {
    let mut fb = FrameBuffer::new();
    let frame = build_mstp_frame(FRAME_TOKEN, 5, 1, &[]);
    fb.push(&frame);
    let extracted = extract_mstp_frame(&mut fb, None);
    assert!(extracted.is_some());
}

#[test]
fn test_extract_mstp_frame_multiple() {
    let mut fb = FrameBuffer::new();
    let f1 = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &[0x01]);
    let f2 = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &[0x02, 0x03]);
    fb.push(&f1);
    fb.push(&f2);

    let e1 = extract_mstp_frame(&mut fb, None).unwrap();
    assert_eq!(e1, f1);
    let e2 = extract_mstp_frame(&mut fb, None).unwrap();
    assert_eq!(e2, f2);
}

// ===== S3: 帧解析健壮性（畸形 / 截断 / 随机输入不得 panic） =====

/// 确定性 LCG：生成可复现的伪随机字节流，避免引入外部测试依赖。
fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

#[test]
fn extract_mstp_frame_hostile_input_never_panics() {
    // 1) 合法帧的所有严格前缀：均不得提取出帧
    let frame = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &[0x01, 0x02, 0x03]);
    for cut in 0..frame.len() {
        let mut fb = FrameBuffer::new();
        fb.push(&frame[..cut]);
        assert!(
            extract_mstp_frame(&mut fb, None).is_none(),
            "截断到 {cut} 字节不应提取出帧"
        );
    }

    // 2) 随机噪声 / 诱导字节 / 单字节破坏的合法帧
    let mut state: u64 = 0x55FF_55FF_00AA_00AA;
    for round in 0..2000u32 {
        let mut fb = FrameBuffer::new();
        let len = (lcg_next(&mut state) % 300) as usize;
        match round % 4 {
            0 => {
                let noise: Vec<u8> = (0..len).map(|_| lcg_next(&mut state) as u8).collect();
                fb.push(&noise);
            }
            1 => {
                let v = vec![0x55; len];
                fb.push(&v);
            }
            2 => {
                let v = vec![0xFF; len];
                fb.push(&v);
            }
            _ => {
                let mut f = frame.clone();
                let i = (lcg_next(&mut state) as usize) % f.len();
                f[i] ^= lcg_next(&mut state) as u8;
                fb.push(&f);
            }
        }
        for _ in 0..256 {
            if extract_mstp_frame(&mut fb, None).is_none() {
                break;
            }
        }
    }

    // 3) 超大输入：缓冲必须被硬上限约束
    let mut fb = FrameBuffer::new();
    let big = vec![0x55u8; 100_000];
    fb.push(&big);
    assert!(fb.len() <= 8192, "缓冲应被硬上限约束，实际 {}", fb.len());
    for _ in 0..256 {
        if extract_mstp_frame(&mut fb, None).is_none() {
            break;
        }
    }
}

// ===== 诊断（Who-Is 广播探针）端到端测试 =====

/// 构造 I-Am APDU（Device:instance，maxAPDU 1024，segmentation 3，vendor）
fn i_am_apdu(device_instance: u32, vendor_id: u16) -> Vec<u8> {
    let mut apdu = vec![
        crate::south::protocol::bacnet::APDU_UNCONFIRMED_REQUEST,
        crate::south::protocol::bacnet::SERVICE_I_AM,
    ];
    let oid =
        (crate::south::protocol::bacnet::OBJECT_DEVICE as u32) << 22 | (device_instance & 0x3FFFFF);
    apdu.extend(BacnetCollector::encode_context_tag(0, &oid.to_be_bytes()));
    apdu.extend(BacnetCollector::encode_context_tag(1, &[0x04, 0x00]));
    apdu.extend(BacnetCollector::encode_context_tag(2, &[0x03]));
    apdu.extend(BacnetCollector::encode_context_tag(
        3,
        &BacnetCollector::encode_unsigned(vendor_id as u32),
    ));
    apdu
}

/// 用内存 duplex 冒充 MS/TP 串口链路；`responder=None` 表示设备始终不回帧。
async fn fake_mstp_bus(
    bus_name: &str,
    responder: Option<Vec<u8>>,
) -> std::sync::Arc<crate::south::bus::BusManager> {
    use crate::config::BusConfig;
    use crate::south::bus::{BusDeps, BusManager, SerialHandle};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (stream, mut peer) = tokio::io::duplex(256);
    let cfg = BusConfig {
        port_name: bus_name.to_string(),
        protocol: "bacnet_mstp".into(),
        timeout_ms: 300,
        retry_count: Some(0),
        retry_delay_ms: Some(0),
        ..Default::default()
    };
    let bus = std::sync::Arc::new(BusManager::spawn_with(
        &cfg,
        Some(SerialHandle::from_stream(stream)),
        std::sync::Arc::new(test_metrics()),
        BusDeps::production(),
    ));

    tokio::spawn(async move {
        let mut buf = vec![0u8; 256];
        let _ = peer.read(&mut buf).await;
        if let Some(reply) = responder {
            let _ = peer.write_all(&reply).await;
        }
        // 保持链路存活到事务超时之后，避免提前 EOF 把「无应答」变成「链路断开」
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    });
    bus
}

#[tokio::test]
async fn diag_reports_i_am_device_instance() {
    let mut payload = BacnetCollector::build_npdu_header().to_vec();
    payload.extend_from_slice(&i_am_apdu(1001, 260));
    let reply = build_mstp_frame(FRAME_DATA_NOT_EXPECTING_REPLY, 0, 1, &payload);

    let bus = fake_mstp_bus("mstp_diag_ok", Some(reply)).await;
    let collector =
        BacnetMstpCollector::new("mstp_diag_ok", bus, 0, std::sync::Arc::new(test_metrics()));
    let msg = collector.diag().await;
    assert!(
        msg.contains("设备实例 1001"),
        "应报出 I-Am 中的设备实例号，实际: {msg}"
    );
    assert!(msg.contains("厂商 260"), "应报出厂商 ID，实际: {msg}");
}

/// 反证：设备不回帧时诊断必须报「无应答」，不得伪造成功。
#[tokio::test]
async fn diag_reports_no_reply_instead_of_fake_success() {
    let bus = fake_mstp_bus("mstp_diag_dead", None).await;
    let collector = BacnetMstpCollector::new(
        "mstp_diag_dead",
        bus,
        0,
        std::sync::Arc::new(test_metrics()),
    );
    let msg = collector.diag().await;
    assert!(msg.contains("无应答"), "应报无应答，实际: {msg}");
    assert!(msg.contains("mstp_diag_dead"), "应点明总线，实际: {msg}");
    assert!(!msg.contains("ok"), "无应答不得报成功: {msg}");
}

#[test]
fn parse_mstp_frame_hostile_input_never_panics() {
    let m = test_metrics();
    let mut state: u64 = 0x00FF_00FF_1234_ABCD;
    for _ in 0..2000 {
        let len = (lcg_next(&mut state) % 40) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| lcg_next(&mut state) as u8).collect();
        let _ = parse_mstp_frame(&bytes, &m);
    }
    assert!(parse_mstp_frame(&[], &m).is_err());
    assert!(parse_mstp_frame(&[0x55, 0xFF], &m).is_err());
}

// =========================================================================
// P0-3：黄金报文（Golden）
// -------------------------------------------------------------------------
// MS/TP 帧的字节级基线：前导码 55 FF + 头(Type/DADDR/SADDR/Length/CRC8) +
// 数据 + CRC16(低字节在前)。CRC 字节序（低前高后）与 CRC 计算范围（头字段不含
// 前导码、数据 CRC 只覆盖数据）是最容易写错的两处，错了表现为「总线上全是
// FrameError、一帧都收不到」，现场没有字节基线时无从判断。
//
// 说明：样本属于「编码器特征化冻结」——冻结的是当前实现的线上字节，
// 不代表已对照 BACnet 标准逐字段核验（见本模块 frame type 常量的存疑点）。
//
// 刷新方式：$env:GOLDEN_UPDATE='1'; cargo test -p gateway-core --lib golden
// =========================================================================

#[test]
fn golden_mstp_token_frame() {
    let f = build_mstp_frame(FRAME_TOKEN, 5, 1, &[]);
    crate::south::protocol::golden::assert_golden("bacnet_mstp/token_frame.hex", &f);

    let loaded = crate::south::protocol::golden::load("bacnet_mstp/token_frame.hex");
    let (ftype, dest, src, data) =
        parse_mstp_frame(&loaded, &test_metrics()).expect("golden 令牌帧必须可解析");
    assert_eq!((ftype, dest, src), (FRAME_TOKEN, 5, 1));
    assert!(data.is_empty(), "令牌帧无数据负载");
}

#[test]
fn golden_mstp_who_is_data_frame() {
    // 负载形状：NPDU 版本 1 + 控制位 0x10 + Unconfirmed-Request 服务号 Who-Is(0x08)
    let payload = [0x01_u8, 0x10, 0x08];
    let f = build_mstp_frame(FRAME_DATA_EXPECTING_REPLY, 0xFF, 0, &payload);
    crate::south::protocol::golden::assert_golden("bacnet_mstp/who_is_data_frame.hex", &f);

    let loaded = crate::south::protocol::golden::load("bacnet_mstp/who_is_data_frame.hex");
    let (ftype, dest, src, data) =
        parse_mstp_frame(&loaded, &test_metrics()).expect("golden 数据帧必须可解析");
    assert_eq!((ftype, dest, src), (FRAME_DATA_EXPECTING_REPLY, 0xFF, 0));
    assert_eq!(data, payload, "数据负载必须逐字节还原（数据 CRC 校验通过）");
}

/// 契约：帧类型常量必须与 ASHRAE 135 表 9-1 的 **wire 值**一致。
///
/// 断言刻意用字面量而不是「常量互比」：曾把 DataExpectingReply 写成 0x00
/// （Token）、DataNotExpectingReply 写成 0x01（Poll For Master），现场表现是
/// 「总线通、一帧数据都没有」。这类错误构造出的帧仍然合法，fuzz 抓不到；
/// 若测试只引用常量符号，变异测试也杀不掉——必须在字面值上钉死。
#[test]
fn mstp_frame_type_constants_match_ashrae_135_table_9_1() {
    assert_eq!(FRAME_TOKEN, 0x00, "Token");
    assert_eq!(FRAME_DATA_EXPECTING_REPLY, 0x05, "Data Expecting Reply");
    assert_eq!(
        FRAME_DATA_NOT_EXPECTING_REPLY, 0x06,
        "Data Not Expecting Reply"
    );
    assert_eq!(FRAME_REPLY_POSTPONED, 0x07, "Reply Postponed");
}
