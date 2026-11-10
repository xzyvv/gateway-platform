use super::frames::*;
use super::*;
use crate::config::PointConfig;
use crate::engine::model::Quality;
use crate::south::protocol::common::codec::{decode_bcd_value, encode_bcd_value};
use crate::south::protocol::Collector;

// =========================================================================
// B3：读通信地址诊断
// =========================================================================

#[test]
fn diag_read_address_di_v2004_is_di_plus_ser() {
    let di = build_read_address_di(Cjt188Version::V2004).expect("构造失败");
    assert_eq!(di, vec![0xA0, 0x18, DEFAULT_SER]);
}

#[test]
fn diag_read_address_di_vendor7_is_reversed_and_ciphered() {
    let di = build_read_address_di(Cjt188Version::Vendor7).expect("构造失败");
    assert_eq!(di.len(), 7);
    // 配置序 A0 18 00 00 00 00 00 → 线上低字节在前 → 全体 +0x33
    assert_eq!(di[..5], [0x33, 0x33, 0x33, 0x33, 0x33]);
    assert_eq!(di[5], 0x18u8.wrapping_add(0x33));
    assert_eq!(di[6], 0xA0u8.wrapping_add(0x33));
}

#[test]
fn diag_address_formats_14_digit_meter_address() {
    // 配置书写顺序 "00000001234567" 的线上形态（低字节在前），与 parse_bcd_address 一致
    let addr_low_first = parse_bcd_address("00000001234567", 7).expect("地址解析失败");
    assert_eq!(
        addr_low_first,
        vec![0x67, 0x45, 0x23, 0x01, 0x00, 0x00, 0x00]
    );
    let data = vec![0xA0, 0x18, DEFAULT_SER]
        .into_iter()
        .chain(addr_low_first)
        .collect::<Vec<u8>>();
    let formatted = format_cjt_address(&data, Cjt188Version::V2004.response_prefix_len());
    assert_eq!(formatted.as_deref(), Some("00000001234567"));
}

#[test]
fn diag_address_returns_none_when_payload_short() {
    let data = vec![0xA0, 0x18, DEFAULT_SER, 0x01];
    assert!(format_cjt_address(&data, Cjt188Version::V2004.response_prefix_len()).is_none());
}

// =========================================================================
// Golden 报文回归（决策表 #10）
//
// 帧逐字节与 `tests/fixtures/golden/cjt188/*.hex` 比对；响应样本做解码断言。
// =========================================================================

/// V2004 方言读数据请求：CTRL=0x01，数据区 = DI(2)+SER(1)，不加 0x33 偏移
#[test]
fn golden_cjt188_v2004_read_request() {
    let di = parse_cjt188_data_id_v2004("901F").expect("V2004 DI 解析失败");
    let mut data = di;
    data.push(DEFAULT_SER);
    let addr = parse_bcd_address("00000001234567", 7).expect("地址解析失败");
    let f = build_cjt188_frame(&addr, 0x01, &data);
    crate::south::protocol::golden::assert_golden("cjt188/v2004_read_request.hex", &f);
}

/// V2004 写通信地址：CTRL=0x15，数据区 = DI(A0 18)+SER+7 字节 BCD 新地址
#[test]
fn golden_cjt188_v2004_write_address() {
    let addr = parse_bcd_address("00000001234567", 7).expect("地址解析失败");
    let new_addr = encode_bcd_value(1234567890.0, 7).expect("BCD 编码失败");
    let mut data = vec![0xA0, 0x18, DEFAULT_SER];
    data.extend_from_slice(&new_addr);
    let f = build_cjt188_frame(&addr, 0x15, &data);
    crate::south::protocol::golden::assert_golden("cjt188/v2004_write_address.hex", &f);
}

/// V2004 响应解码：数据区必须原样解析（标准方言无 -0x33 偏移）
#[test]
fn golden_cjt188_v2004_response_decode() {
    let addr = parse_bcd_address("00000001234567", 7).expect("地址解析失败");
    let payload = vec![0x90, 0x1F, 0x00, 0x12, 0x34];
    let body = build_cjt188_frame(&addr, 0x81, &payload);
    crate::south::protocol::golden::assert_golden("cjt188/v2004_response.hex", &body);

    let metrics = crate::engine::metrics::Metrics::new();
    let loaded = crate::south::protocol::golden::load("cjt188/v2004_response.hex");
    let decoded = parse_cjt188_response_v(&loaded, &addr, &metrics, Cjt188Version::V2004)
        .expect("golden V2004 响应样本必须可解析");
    assert_eq!(decoded, payload, "V2004 方言不得做 -0x33 偏移");
}

/// 组一个 CJT188 响应帧：68 ADDR(7) CTRL LEN DATA CS 16（无第二个 0x68 头）
fn cjt_response(addr: &[u8; 7], ctrl: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![0x68];
    f.extend_from_slice(addr);
    f.push(ctrl);
    f.push(data.len() as u8);
    f.extend_from_slice(data);
    let cs: u8 = f.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    f.push(cs);
    f.push(0x16);
    f
}

/// P3: D5=0x20（后续帧标志）应显式报错，而不是把长数据截断成错误值静默上报。
#[test]
fn parse_rejects_multi_frame_flag() {
    let addr = [0u8; 7];
    let metrics = crate::engine::metrics::Metrics::new();

    // 单帧正常应答（ctrl=0x81）
    let ok = cjt_response(&addr, 0x81, &[0x90, 0x1F]);
    assert!(parse_cjt188_response_inner(&ok, &addr, &metrics, true).is_ok());

    // 多帧应答（D5=0x20）必须报错
    let multi = cjt_response(&addr, 0x81 | 0x20, &[0x90, 0x1F]);
    match parse_cjt188_response_inner(&multi, &addr, &metrics, true) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(m)) => {
            assert!(m.contains("multi-frame"), "错误信息应说明多帧: {}", m);
        }
        other => panic!("多帧应答应报 InvalidFrame，实际 {:?}", other),
    }
}

#[test]
fn test_parse_address() {
    let bytes = crate::south::protocol::parse_bcd_address("00000001234567", 7).unwrap();
    assert_eq!(bytes.len(), 7);
}

#[test]
fn test_build_frame() {
    let addr = vec![0u8; 7];
    let data = vec![0x33; 7];
    let frame = build_cjt188_frame(&addr, 0x01, &data);
    assert_eq!(frame[0], 0x68);
    assert_eq!(*frame.last().unwrap(), 0x16);
}

/// Bug D1: 数据区超过 255 字节时必须拒绝构建，避免 LEN 字段 as u8 静默截断。
#[test]
fn test_build_frame_rejects_oversized_data() {
    let addr = vec![0u8; 7];
    let oversized = vec![0x00u8; 256];
    assert!(
        build_cjt188_frame(&addr, 0x01, &oversized).is_empty(),
        "超过 255 字节的帧应被拒绝（返回空）"
    );
    // 边界：恰好 255 字节仍可正常构建，LEN 字段为 255
    let boundary = vec![0x00u8; 255];
    let frame = build_cjt188_frame(&addr, 0x01, &boundary);
    assert_eq!(frame[0], 0x68);
    assert_eq!(frame[9], 255);
    assert_eq!(*frame.last().unwrap(), 0x16);
}

/// Bug 224: 批量读改为逐点单 DI 后，响应解析不再依赖帧内搜索 DI 字节模式。
/// 此处钉住「单 DI 响应数据域 = DI(7) + DATA」按前缀长度切分即可正确解码。
#[test]
fn single_di_response_prefix_slicing() {
    // CJT188 DI 14 位 hex -> 7 字节
    let di = parse_cjt188_data_id("901F00000100FF").unwrap();
    assert_eq!(di.len(), 7);
    let mut di_le = di.clone();
    di_le.reverse();

    // 响应数据域：DI0(7) + DATA(4)，值 = 1234
    let mut resp = Vec::new();
    resp.extend_from_slice(&di_le);
    resp.extend_from_slice(&[0x34, 0x12, 0x00, 0x00]);

    let prefix = Cjt188Version::Vendor7.response_prefix_len();
    assert_eq!(prefix, 7);
    assert_eq!(&resp[..prefix], &di_le[..], "响应前导应为低字节在前的 DI");
    assert_eq!(decode_bcd_value(&resp[prefix..]), Some(1234.0));
}

// ===== S3: 帧解析健壮性（畸形 / 截断 / 随机输入不得 panic） =====

/// 确定性 LCG：生成可复现的伪随机字节流，避免引入外部测试依赖。
fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// 构造合法 CJT188 响应帧（ctrl bit7 置位表示响应帧）。
fn valid_cjt188_response() -> Vec<u8> {
    build_cjt188_frame(&[0u8; 7], 0x81, &[0x33, 0x34])
}

#[test]
fn extract_cjt188_frame_hostile_input_never_panics() {
    use crate::south::bus::FrameBuffer;

    // 1) 合法帧的所有严格前缀：均不得提取出帧
    let frame = valid_cjt188_response();
    for cut in 0..frame.len() {
        let mut fb = FrameBuffer::new();
        fb.push(&frame[..cut]);
        assert!(
            extract_cjt188_frame(&mut fb, None).is_none(),
            "截断到 {cut} 字节不应提取出帧"
        );
    }

    // 2) 随机噪声 / 诱导字节 / 单字节破坏的合法帧
    let mut state: u64 = 0x0F1E_2D3C_4B5A_6978;
    for round in 0..2000u32 {
        let mut fb = FrameBuffer::new();
        let len = (lcg_next(&mut state) % 300) as usize;
        match round % 4 {
            0 => {
                let noise: Vec<u8> = (0..len).map(|_| lcg_next(&mut state) as u8).collect();
                fb.push(&noise);
            }
            1 => {
                let v = vec![0x68; len];
                fb.push(&v);
            }
            2 => {
                let v = vec![0x16; len];
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
            if extract_cjt188_frame(&mut fb, None).is_none() {
                break;
            }
        }
    }

    // 3) 超大输入：缓冲必须被硬上限约束
    let mut fb = FrameBuffer::new();
    let big = vec![0x68u8; 100_000];
    fb.push(&big);
    assert!(fb.len() <= 8192, "缓冲应被硬上限约束，实际 {}", fb.len());
    for _ in 0..256 {
        if extract_cjt188_frame(&mut fb, None).is_none() {
            break;
        }
    }
}

#[test]
fn parse_cjt188_response_hostile_input_never_panics() {
    let addr = [0u8; 7];
    let metrics = crate::engine::metrics::Metrics::new();
    let mut state: u64 = 0x1357_9BDF_0246_8ACE;
    for _ in 0..2000 {
        let len = (lcg_next(&mut state) % 40) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| lcg_next(&mut state) as u8).collect();
        let _ = parse_cjt188_response(&bytes, &addr, &metrics);
    }
    assert!(parse_cjt188_response(&[], &addr, &metrics).is_err());
    assert!(parse_cjt188_response(&[0x68, 0x00], &addr, &metrics).is_err());
    // 正向对照：合法响应帧必须成功解析
    assert!(parse_cjt188_response(&valid_cjt188_response(), &addr, &metrics).is_ok());
}

// ===== 决策表 #6：CJT188 方言（Vendor7 / V2004）=====

#[test]
fn cjt188_version_from_config_and_layout() {
    assert_eq!(Cjt188Version::from_config("2004"), Cjt188Version::V2004);
    assert_eq!(Cjt188Version::from_config(" 2004 "), Cjt188Version::V2004);
    assert_eq!(Cjt188Version::from_config("2018"), Cjt188Version::V2004);
    assert_eq!(
        Cjt188Version::from_config("vendor7"),
        Cjt188Version::Vendor7
    );
    assert_eq!(Cjt188Version::from_config(""), Cjt188Version::Vendor7);
    assert_eq!(Cjt188Version::from_config("1997"), Cjt188Version::Vendor7);

    assert_eq!(Cjt188Version::Vendor7.data_id_len(), 7);
    assert_eq!(Cjt188Version::V2004.data_id_len(), 2);
    assert_eq!(Cjt188Version::Vendor7.response_prefix_len(), 7);
    assert_eq!(Cjt188Version::V2004.response_prefix_len(), 3);
    assert!(Cjt188Version::Vendor7.uses_offset());
    assert!(!Cjt188Version::V2004.uses_offset());
}

#[test]
fn parse_cjt188_data_id_v2004_ok_and_bounds() {
    assert_eq!(
        parse_cjt188_data_id_v2004("901F").unwrap(),
        vec![0x90, 0x1F]
    );
    assert_eq!(
        parse_cjt188_data_id_v2004("A018").unwrap(),
        vec![0xA0, 0x18]
    );
    // 长度不符
    assert!(parse_cjt188_data_id_v2004("901F00").is_err());
    assert!(parse_cjt188_data_id_v2004("90").is_err());
    // 无有效 hex 字符
    assert!(parse_cjt188_data_id_v2004("ZZZZ").is_err());
}

/// V2004 响应数据区无 +0x33 偏移；Vendor7 同字节则按 -0x33 还原。
#[test]
fn parse_cjt188_response_v_offset_semantics() {
    let addr = [0u8; 7];
    let metrics = crate::engine::metrics::Metrics::new();
    let raw = [0x90u8, 0x1F, 0x00, 0x00, 0x01];
    let frame = build_cjt188_frame(&addr, 0x81, &raw);

    let v2004 = parse_cjt188_response_v(&frame, &addr, &metrics, Cjt188Version::V2004).unwrap();
    assert_eq!(v2004, raw.to_vec());

    let v7 = parse_cjt188_response_v(&frame, &addr, &metrics, Cjt188Version::Vendor7).unwrap();
    let expected7: Vec<u8> = raw.iter().map(|b| b.wrapping_sub(0x33)).collect();
    assert_eq!(v7, expected7);
}

#[test]
fn encode_bcd_value_roundtrip_and_bounds() {
    assert_eq!(
        encode_bcd_value(1234.0, 4).unwrap(),
        vec![0x34, 0x12, 0x00, 0x00]
    );
    assert_eq!(decode_bcd_value(&[0x34, 0x12, 0x00, 0x00]), Some(1234.0));
    assert!(encode_bcd_value(-1.0, 4).is_none());
    assert!(encode_bcd_value(1.5, 4).is_none());
    // 2 字节 BCD 最大 9999，1e9 越界
    assert!(encode_bcd_value(1e9, 2).is_none());
}

/// V2004 读/写请求的数据区与整帧字节断言（DP：DI 2 字节 + SER 1 字节，无偏移）。
#[test]
fn v2004_request_frame_layout() {
    let addr = [0u8; 7];

    // 读数据（0x01）：数据区 = DI(2) + SER(1)，LEN=3
    let mut r = parse_cjt188_data_id_v2004("901F").unwrap();
    r.push(DEFAULT_SER);
    let read = build_cjt188_frame(&addr, 0x01, &r);
    assert_eq!(&read[..10], &[0x68, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x03]);
    assert_eq!(&read[10..13], &[0x90, 0x1F, 0x00]);

    // 写数据（0x04）：数据区 = DI(2) + SER(1) + 值(2)，LEN=5
    let mut w = parse_cjt188_data_id_v2004("901F").unwrap();
    w.push(DEFAULT_SER);
    w.extend_from_slice(&encode_bcd_value(1234.0, 2).unwrap());
    let write = build_cjt188_frame(&addr, 0x04, &w);
    assert_eq!(write[8], 0x04);
    assert_eq!(write[9], 0x05);
    assert_eq!(&write[10..15], &[0x90, 0x1F, 0x00, 0x34, 0x12]);
}

// ===== Bug 222: 写值编码与读路径解码必须互逆（往返一致性） =====

/// 构造编解码测试点位（仅 data_type/byte_order 相关）
fn point_for_codec(dt: &str, order: &str) -> PointConfig {
    PointConfig {
        data_type: Some(dt.to_string()),
        byte_order: Some(order.to_string()),
        ..Default::default()
    }
}

/// 全类型 × 全字节序往返：decode(encode(v)) == v。
/// 原实现（to_le_bytes + reorder）对非回文字节值会字节序错乱，
/// 本测试钉住修复后的互逆性（与 dlt645 同构）。
#[test]
fn test_encode_decode_roundtrip_all_types_all_orders() {
    let cases: &[(&str, usize, f64)] = &[
        ("u16", 2, 0x1234_u16 as f64),
        ("i16", 2, -1234.0),
        ("u32", 4, 0x12345678_u32 as f64),
        ("i32", 4, -123456789.0),
        ("u64", 8, 0x123456789A_u64 as f64),
        ("i64", 8, -1234567890123.0),
        ("f32", 4, std::f64::consts::PI),
        ("f64", 8, -std::f64::consts::E),
    ];
    for order in ["ABCD", "DCBA", "CDAB", "BADC"] {
        for (dt, len, v) in cases {
            let point = point_for_codec(dt, order);
            let encoded = encode_cjt_value(&point, *v, *len)
                .unwrap_or_else(|| panic!("encode 失败: {dt}/{order}"));
            assert_eq!(encoded.len(), *len, "{dt}/{order} 编码长度");
            let (decoded, _t, q) = decode_cjt_value(&point, &encoded);
            assert_eq!(q, Quality::Good, "{dt}/{order} 质量");
            if *dt == "f32" {
                assert!(
                    (decoded - v).abs() < 1e-6,
                    "{dt}/{order}: 写 {v} 读回 {decoded}"
                );
            } else {
                assert_eq!(decoded, *v, "{dt}/{order}: 写 {v} 读回 {decoded}");
            }
        }
    }
}

// =========================================================================
// 写路径与读路径对称（CT/PT 变比）
// =========================================================================

/// 电表写数据必须与读路径对称：读乘了 CT×PT，写就要除回来。
///
/// 场景：CT=30、PT=100 的功率点，寄存器里存的是二次侧值 2，
/// 读出来上报一次侧 6000。把 6000 原样写回去，寄存器必须还是 2。
#[test]
fn write_data_reverses_ratio_to_match_read_path() {
    let mut point = point_for_codec("u16", "ABCD");
    point.unit = Some("kW".into());
    point.ct_ratio = Some(30.0);
    point.pt_ratio = Some(100.0);
    point.scale = 1.0;
    point.offset = 0.0;

    let raw = 2.0f64;
    let primary = convert_value(&point, raw);
    assert!((primary - 6000.0).abs() < 1e-9, "一次侧值应为 6000");

    // 写路径：一次侧值 → 反算回原始寄存器值
    let written = reverse_convert(&point, primary);
    assert!((written - raw).abs() < 1e-9, "写入的寄存器值应为 2");

    // 落到字节上再解回来，确认整条链路自洽
    let bytes = encode_cjt_value(&point, written, 2).expect("encode");
    let (decoded, _t, q) = decode_cjt_value(&point, &bytes);
    assert_eq!(q, Quality::Good);
    assert!((decoded - raw).abs() < 1e-9);
}

// =========================================================================
// C1.5：不可达端点/无响应时，诊断必须如实报错，不得伪造「成功」
// =========================================================================

/// 用不存在的串口起一条总线：模拟现场「表未接线 / 串口配错 / 转换器掉电」。
fn dead_serial_bus(port: &str) -> std::sync::Arc<crate::south::bus::BusManager> {
    use crate::config::BusConfig;
    use crate::engine::metrics::Metrics;
    use crate::south::bus::{BusDeps, BusManager};
    use std::sync::Arc;

    let cfg = BusConfig {
        port_name: port.to_string(),
        protocol: "cjt188".into(),
        timeout_ms: 300,
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

#[tokio::test]
async fn diag_reports_unreachable_endpoint_instead_of_fake_success() {
    let c = Cjt188Collector::new(
        "cjt188_diag_dead",
        dead_serial_bus("COM9998"),
        std::sync::Arc::new(crate::engine::metrics::Metrics::new()),
    );
    let msg = c.diag().await;
    assert!(msg.contains("无响应"), "应报无响应，实际: {msg}");
    assert!(
        msg.contains("COM9998") || msg.contains("cjt188_diag_dead"),
        "应点明总线，实际: {msg}"
    );
    assert!(!msg.contains("成功"), "不可达端点不得报成功: {msg}");
}

// =========================================================================
// P2-1：补 CJT188 缺失的正向性质（原有的 hostile 测试只证明「不 panic」，
// 没证明「脏数据之后还能把合法帧找回来」）。
// =========================================================================

use crate::south::bus::FrameBuffer;
use crate::south::protocol::test_support::Lcg;

/// 性质：噪声 + 合法帧 → 必须重同步出该帧（不 panic 不等于不吞帧）。
#[test]
fn extract_cjt188_frame_resyncs_to_valid_frame_after_noise() {
    let addr = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
    let good = build_cjt188_frame(&addr, 0x91, &[0x33, 0x34]);
    let mut lcg = Lcg::new(0x434A_5431_3838_5031);
    for _ in 0..200 {
        let len = lcg.below(40);
        let noise = lcg.bytes_without_start_marker(len);
        let mut fb = FrameBuffer::new();
        fb.push(&noise);
        fb.push(&good);
        assert_eq!(
            extract_cjt188_frame(&mut fb, None).as_deref(),
            Some(good.as_slice()),
            "噪声 {noise:?} 之后必须重同步出合法帧"
        );
    }
}

/// 性质：随机地址 / 数据区 / 控制码低 5 位都必须 build → extract → parse 往返一致。
#[test]
fn cjt188_build_extract_parse_roundtrip_for_random_frames() {
    let metrics = crate::engine::metrics::Metrics::new();
    let mut lcg = Lcg::new(0x434A_5431_3838_5032);
    for round in 0..200 {
        let addr: [u8; 7] = std::array::from_fn(|_| lcg.next_u8());
        let len = lcg.below(24);
        let data = lcg.bytes(len);
        let ctrl = 0x81 | (lcg.next_u8() & 0x1F);

        let frame = build_cjt188_frame(&addr, ctrl, &data);
        let mut fb = FrameBuffer::new();
        fb.push(&frame);
        assert_eq!(
            extract_cjt188_frame(&mut fb, None).as_deref(),
            Some(frame.as_slice()),
            "第 {round} 轮：提取结果必须与构造帧逐字节一致"
        );

        let parsed = parse_cjt188_response(&frame, &addr, &metrics)
            .unwrap_or_else(|e| panic!("第 {round} 轮合法帧解析失败: {e:?}"));
        let expected: Vec<u8> = data.iter().map(|b| b.wrapping_sub(0x33)).collect();
        assert_eq!(parsed, expected, "第 {round} 轮数据区不一致");
    }
}
