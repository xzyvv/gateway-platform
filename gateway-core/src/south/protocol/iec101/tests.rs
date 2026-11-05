//! IEC 60870-5-101 单元测试 + 内存串口桩端到端测试
//!
//! 端到端部分通过 `tokio::io::duplex` 注入内存串口对，驱动**真实**
//! `BusManager`（串行任务）+ 真实 `Iec101Collector`。设备侧桩为独立手写实现
//! （按请求帧结构分派、手工构造响应帧），作为交叉验证 oracle，不复用生产构造逻辑。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::frames::*;
use super::Iec101Collector;
use crate::config::{BusConfig, PointConfig};
use crate::engine::metrics::Metrics;
use crate::engine::model::Quality;
use crate::south::bus::{BusDeps, BusManager, FrameBuffer, SerialHandle};
use crate::south::protocol::Collector;

// ============================================================
// 纯函数单元测试
// ============================================================

#[test]
fn test_build_fixed_frame_layout() {
    let f = reset_link(1);
    assert_eq!(f.len(), 5);
    assert_eq!(f[0], 0x10);
    assert_eq!(f[1], 0x40); // PRM=1，功能码 0
    assert_eq!(f[2], 1);
    assert_eq!(f[3], 0x41); // (0x40 + 0x01) mod 256
    assert_eq!(f[4], 0x16);
}

#[test]
fn test_request_class2_control() {
    let f = request_class2(1, true);
    assert_eq!(f[1], 0x7B); // PRM|FCV|FCB|功能码11
    assert_eq!(f[1] & 0x0F, 0x0B);
    let f2 = request_class2(1, false);
    assert_eq!(f2[1] & 0x20, 0); // FCB=0
}

#[test]
fn test_interrogation_frame_layout() {
    let f = interrogation(1, 1, true, 2);
    assert_eq!(f[0], 0x68);
    assert_eq!(f[1], f[2]); // 两个长度字节相等
    assert_eq!(f[3], 0x68);
    assert_eq!(f[4], 0x73); // PRM|FCV|FCB|用户数据
    assert_eq!(f[5], 1);
    assert_eq!(f.len(), 6 + f[1] as usize);
    assert_eq!(*f.last().unwrap(), 0x16);
    // 校验和 = body(C..ASDU) 之和
    let l = f[1] as usize;
    let cs = f[4..4 + l].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    assert_eq!(cs, f[4 + l]);
}

#[test]
fn test_parse_frame_fixed() {
    let f = reset_link(1);
    match parse_frame(&f).unwrap() {
        Ft12Frame::Fixed { control, addr } => {
            assert_eq!(control, 0x40);
            assert_eq!(addr, 1);
        }
        other => panic!("expected fixed, got {other:?}"),
    }
}

#[test]
fn test_parse_frame_variable() {
    let asdu = vec![14u8, 1, 20, 0, 1, 0, 0x64, 0, 0, 0, 0, 0xC0, 0x3F, 0];
    let f = build_variable_frame(0x08, 1, &asdu);
    match parse_frame(&f).unwrap() {
        Ft12Frame::Variable {
            control,
            addr,
            asdu: got,
        } => {
            assert_eq!(control, 0x08);
            assert_eq!(addr, 1);
            assert_eq!(got, asdu);
        }
        other => panic!("expected variable, got {other:?}"),
    }
}

#[test]
fn test_parse_frame_checksum_error() {
    let mut f = reset_link(1);
    f[3] ^= 0xFF; // 破坏校验和
    assert!(matches!(
        parse_frame(&f),
        Err(crate::south::protocol::CollectorError::ChecksumError)
    ));
}

#[test]
fn test_parse_frame_ack_and_unknown() {
    assert_eq!(parse_frame(&[0xE5]).unwrap(), Ft12Frame::Ack);
    assert!(parse_frame(&[0x99]).is_err());
    assert!(parse_frame(&[0x10, 0x40]).is_err()); // 固定帧长度不足
}

#[test]
fn test_extract_single_ack() {
    let mut fb = FrameBuffer::new();
    fb.push(&[0xE5]);
    assert_eq!(extract_iec101_frame(&mut fb, None), Some(vec![0xE5]));
    assert!(fb.is_empty());
}

#[test]
fn test_extract_fixed_with_leading_noise() {
    let mut fb = FrameBuffer::new();
    fb.push(&[0xAA, 0xBB]); // 噪声
    fb.push(&reset_link(1));
    let got = extract_iec101_frame(&mut fb, None).expect("extract");
    assert_eq!(got, reset_link(1));
    assert!(fb.is_empty());
}

#[test]
fn test_extract_variable_truncated_returns_none() {
    let f = interrogation(1, 1, false, 2);
    let mut fb = FrameBuffer::new();
    fb.push(&f[..f.len() - 2]); // 截断
    assert_eq!(extract_iec101_frame(&mut fb, None), None);
}

#[test]
fn test_extract_two_frames_back_to_back() {
    let mut fb = FrameBuffer::new();
    fb.push(&[0xE5]);
    fb.push(&reset_link(1));
    assert_eq!(extract_iec101_frame(&mut fb, None), Some(vec![0xE5]));
    assert_eq!(extract_iec101_frame(&mut fb, None), Some(reset_link(1)));
    assert!(fb.is_empty());
}

// ============================================================
// 内存串口桩 + 端到端测试
// ============================================================

/// 设备桩状态（测试可跨轮次读写，用于模拟"先失败后恢复"）。
#[derive(Default)]
struct StubState {
    /// 复位链路（固定帧功能码 != 11）的回复
    reset_reply: Vec<u8>,
    /// 总召唤（可变帧）的回复
    interrogation_reply: Vec<u8>,
    /// 依次响应二级数据请求；耗尽后回单字符 ACK
    class2_queue: Vec<Vec<u8>>,
}

fn ack() -> Vec<u8> {
    vec![0xE5]
}

/// 构造测试用可变帧（含校验和），独立于生产构造逻辑。
fn test_var_frame(asdu: &[u8]) -> Vec<u8> {
    let l = (2 + asdu.len()) as u8;
    let mut body = vec![0x08u8, 1]; // 从站用户数据 + 链路地址
    body.extend_from_slice(asdu);
    let cs = body.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let mut f = vec![0x68, l, l, 0x68];
    f.extend_from_slice(&body);
    f.push(cs);
    f.push(0x16);
    f
}

/// 构造短浮点测量值 ASDU（类型 14，SQ=0，单对象，COT=20 响应站召唤）。
fn float_asdu(ioa: u32, value: f32) -> Vec<u8> {
    let mut a = vec![14u8, 0x01, 20, 0x00, 0x01, 0x00];
    a.extend_from_slice(&[ioa as u8, (ioa >> 8) as u8, (ioa >> 16) as u8]);
    a.extend_from_slice(&value.to_le_bytes());
    a.push(0x00); // 品质：Good
    a
}

/// 构造归一化测量值 ASDU（类型 9，COT=20）。
fn normalized_asdu(ioa: u32, raw: i16) -> Vec<u8> {
    let mut a = vec![9u8, 0x01, 20, 0x00, 0x01, 0x00];
    a.extend_from_slice(&[ioa as u8, (ioa >> 8) as u8, (ioa >> 16) as u8]);
    a.extend_from_slice(&raw.to_le_bytes());
    a.push(0x00);
    a
}

/// 构造总召唤激活终止 ASDU（类型 100，COT=10）。
fn termination_asdu() -> Vec<u8> {
    vec![100u8, 0x01, 10, 0x00, 0x01, 0x00, 0x14]
}

/// 从缓冲中按 FT1.2 结构取出一个完整请求帧（设备桩侧，独立实现）。
fn try_take_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    loop {
        let first = *buf.first()?;
        match first {
            0xE5 => {
                buf.drain(..1);
                return Some(vec![0xE5]);
            }
            0x10 => {
                if buf.len() < 5 {
                    return None;
                }
                let f = buf[..5].to_vec();
                buf.drain(..5);
                return Some(f);
            }
            0x68 => {
                if buf.len() < 4 {
                    return None;
                }
                let total = 6 + buf[1] as usize;
                if buf.len() < total {
                    return None;
                }
                let f = buf[..total].to_vec();
                buf.drain(..total);
                return Some(f);
            }
            _ => {
                buf.drain(..1);
            }
        }
    }
}

/// 设备桩任务：按请求帧类型分派，回写手工构造的响应。
async fn run_stub(mut dev: tokio::io::DuplexStream, st: Arc<Mutex<StubState>>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 256];
    loop {
        if let Some(frame) = try_take_frame(&mut buf) {
            let reply: Option<Vec<u8>> = {
                let mut g = st.lock().unwrap();
                match frame.first().copied() {
                    Some(0x68) => Some(g.interrogation_reply.clone()),
                    Some(0x10) => {
                        if frame[1] & 0x0F == 11 {
                            Some(if g.class2_queue.is_empty() {
                                ack()
                            } else {
                                g.class2_queue.remove(0)
                            })
                        } else {
                            Some(g.reset_reply.clone())
                        }
                    }
                    _ => None,
                }
            };
            if let Some(reply) = reply {
                if dev.write_all(&reply).await.is_err() {
                    break;
                }
                let _ = dev.flush().await;
            }
            continue;
        }
        match dev.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
}

/// 启动一条注入内存串口桩的 IEC101 总线，返回总线管理器。
fn spawn_bus(st: Arc<Mutex<StubState>>) -> Arc<BusManager> {
    let (gw, dev) = tokio::io::duplex(4096);
    let serial = SerialHandle::from_stream(gw);
    let cfg = BusConfig {
        port_name: "iec101-test".to_string(),
        protocol: "iec101".to_string(),
        timeout_ms: 300,
        retry_count: Some(0),
        retry_delay_ms: Some(10),
        ..Default::default()
    };
    let bus = Arc::new(BusManager::spawn_with(
        &cfg,
        Some(serial),
        Arc::new(Metrics::new()),
        BusDeps::production(),
    ));
    tokio::spawn(run_stub(dev, st));
    bus
}

fn stub_state() -> Arc<Mutex<StubState>> {
    Arc::new(Mutex::new(StubState {
        reset_reply: ack(),
        interrogation_reply: ack(),
        class2_queue: Vec::new(),
    }))
}

fn point(id: &str, ioa: u32) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: "iec101-bus".to_string(),
        protocol: "iec101".to_string(),
        address: ioa.to_string(),
        interval: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_diag_reports_reset_and_interrogation_count() {
    let st = stub_state();
    {
        let mut g = st.lock().unwrap();
        g.class2_queue.push(test_var_frame(&float_asdu(100, 1.5)));
        g.class2_queue.push(test_var_frame(&termination_asdu()));
    }
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);

    let msg = c.diag().await;
    assert!(msg.contains("链路复位成功"), "{msg}");
    assert!(msg.contains("总召唤返回 1 个 IOA"), "{msg}");
}

#[tokio::test]
async fn test_diag_reports_no_data_honestly() {
    let st = stub_state();
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);

    let msg = c.diag().await;
    assert!(msg.contains("链路复位成功"), "{msg}");
    assert!(msg.contains("总召唤未返回任何数据"), "{msg}");
}

/// 起一条没有串口句柄的总线：模拟现场「从站掉线 / 串口配错 / 转换器掉电」。
fn dead_bus() -> Arc<BusManager> {
    let cfg = BusConfig {
        port_name: "COM9999".to_string(),
        protocol: "iec101".to_string(),
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

/// 不可达端点必须如实报错，不得伪造「链路复位成功 / 总召唤成功」。
#[tokio::test]
async fn test_diag_reports_unreachable_endpoint_instead_of_fake_success() {
    let c = Iec101Collector::new("iec101-diag-dead", dead_bus());
    let msg = c.diag().await;
    assert!(msg.contains("链路复位失败"), "{msg}");
    assert!(msg.contains("总召唤失败"), "{msg}");
    assert!(
        !msg.contains("链路复位成功"),
        "不可达端点不得报复位成功: {msg}"
    );
    assert!(
        !msg.contains("总召唤返回"),
        "不可达端点不得报总召唤成功: {msg}"
    );
}

#[tokio::test]
async fn test_e2e_read_float_via_interrogation() {
    let st = stub_state();
    {
        let mut g = st.lock().unwrap();
        g.class2_queue.push(test_var_frame(&float_asdu(100, 1.5)));
        g.class2_queue.push(test_var_frame(&termination_asdu()));
    }
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);

    let dp = c.read(&point("p1", 100)).await.expect("read ok");
    assert!((dp.raw_value - 1.5).abs() < 1e-6, "got {}", dp.raw_value);
    assert_eq!(dp.raw_type, "iec101");
    assert_eq!(dp.quality, Quality::Good);
    assert_eq!(dp.point_id, "p1");
}

#[tokio::test]
async fn test_e2e_read_normalized() {
    let st = stub_state();
    {
        let mut g = st.lock().unwrap();
        g.class2_queue
            .push(test_var_frame(&normalized_asdu(200, 32767)));
        g.class2_queue.push(test_var_frame(&termination_asdu()));
    }
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);

    let dp = c.read(&point("p2", 200)).await.expect("read ok");
    assert!((dp.raw_value - 1.0).abs() < 1e-3, "got {}", dp.raw_value);
}

#[tokio::test]
async fn test_e2e_batch_read_two_points() {
    let st = stub_state();
    {
        let mut g = st.lock().unwrap();
        g.class2_queue.push(test_var_frame(&float_asdu(10, 2.5)));
        g.class2_queue.push(test_var_frame(&float_asdu(11, 3.5)));
        g.class2_queue.push(test_var_frame(&termination_asdu()));
    }
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);

    let pts = vec![point("b10", 10), point("b11", 11)];
    let res = c.read_batch(&pts).await;
    assert_eq!(res.len(), 2);
    assert!((res[0].as_ref().unwrap().raw_value - 2.5).abs() < 1e-6);
    assert!((res[1].as_ref().unwrap().raw_value - 3.5).abs() < 1e-6);
}

#[tokio::test]
async fn test_unknown_ioa_returns_no_reply() {
    let st = stub_state();
    {
        let mut g = st.lock().unwrap();
        g.class2_queue.push(test_var_frame(&float_asdu(10, 2.5)));
        g.class2_queue.push(test_var_frame(&termination_asdu()));
    }
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);

    let err = c
        .read(&point("missing", 999))
        .await
        .expect_err("should fail");
    assert!(matches!(
        err,
        crate::south::protocol::CollectorError::NoReply
    ));
}

/// 异常注入：设备回一帧校验和损坏的可变帧，采集器应报错且不 panic。
#[tokio::test]
async fn test_corrupt_frame_injection_is_rejected() {
    let st = stub_state();
    {
        let mut g = st.lock().unwrap();
        let mut bad = test_var_frame(&float_asdu(100, 1.5));
        let n = bad.len();
        bad[n - 2] ^= 0xFF; // 破坏校验和
        g.class2_queue.push(bad);
    }
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);

    let err = c.read(&point("p1", 100)).await.expect_err("should fail");
    assert!(
        matches!(
            err,
            crate::south::protocol::CollectorError::NoReply
                | crate::south::protocol::CollectorError::ChecksumError
                | crate::south::protocol::CollectorError::IoError(_)
        ),
        "unexpected: {err:?}"
    );
}

/// 恢复场景：首轮无数据（设备未就绪）→ 采集失败；补上数据后再次采集成功。
#[tokio::test]
async fn test_recovers_after_transient_empty_interrogation() {
    let st = stub_state();
    let bus = spawn_bus(st.clone());
    let c = Iec101Collector::new("iec101-bus", bus);

    // 首轮：class2_queue 为空，设备只回 ACK → 无数据 → NoReply
    let first = c.read(&point("p1", 100)).await;
    assert!(first.is_err(), "first read should fail");

    // 设备恢复：补上数据帧与终止帧
    {
        let mut g = st.lock().unwrap();
        g.class2_queue.push(test_var_frame(&float_asdu(100, 7.25)));
        g.class2_queue.push(test_var_frame(&termination_asdu()));
    }
    let dp = c.read(&point("p1", 100)).await.expect("recovered read ok");
    assert!((dp.raw_value - 7.25).abs() < 1e-6, "got {}", dp.raw_value);
}

#[tokio::test]
async fn test_invalid_ioa_config_error() {
    let st = stub_state();
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);
    let bad = point("bad", 0);
    let mut bad = bad;
    bad.address = "not-a-number".to_string();
    let err = c.read(&bad).await.expect_err("should fail");
    assert!(matches!(
        err,
        crate::south::protocol::CollectorError::InvalidConfig(_)
    ));
}

// 保留：验证多轮读取复用同一链路（总事务数 > 单轮），确保 FCB 翻转与轮询逻辑可用。
#[tokio::test]
async fn test_multiple_reads_share_link() {
    let st = stub_state();
    let counter = Arc::new(AtomicUsize::new(0));
    {
        let mut g = st.lock().unwrap();
        for _ in 0..2 {
            g.class2_queue.push(test_var_frame(&float_asdu(100, 1.0)));
            g.class2_queue.push(test_var_frame(&termination_asdu()));
        }
    }
    let bus = spawn_bus(st);
    let c = Iec101Collector::new("iec101-bus", bus);
    for _ in 0..2 {
        let dp = c.read(&point("p1", 100)).await.expect("read ok");
        counter.fetch_add(1, Ordering::SeqCst);
        assert!((dp.raw_value - 1.0).abs() < 1e-6);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// =========================================================================
// P0-3：黄金报文（Golden）
// -------------------------------------------------------------------------
// FT1.2 的字节级基线。这里冻结三个最容易写错、且现场无法自证的字段：
// 1) 固定帧的 CS = (C + A) mod 256（单字符帧 E5 无校验，容易混淆）；
// 2) 可变帧的两个长度字节 L 必须相等，且 L 含 C、A 本身；
// 3) ASDU 公共地址长度 1 / 2 字节两种形态（决策表 #28：现场老站常用 2 字节）。
//
// 刷新方式：$env:GOLDEN_UPDATE='1'; cargo test -p gateway-core --lib golden
// =========================================================================

#[test]
fn golden_iec101_link_control_frames() {
    crate::south::protocol::golden::assert_golden("iec101/reset_link.hex", &reset_link(1));

    let fcb0 = request_class2(1, false);
    crate::south::protocol::golden::assert_golden("iec101/request_class2_fcb0.hex", &fcb0);
    let fcb1 = request_class2(1, true);
    crate::south::protocol::golden::assert_golden("iec101/request_class2_fcb1.hex", &fcb1);

    // FCB 位必须真的翻转，否则从站会判定为重发帧而丢弃
    assert_eq!(fcb1[1] ^ fcb0[1], C_FCB);

    for rel in [
        "iec101/reset_link.hex",
        "iec101/request_class2_fcb0.hex",
        "iec101/request_class2_fcb1.hex",
    ] {
        let loaded = crate::south::protocol::golden::load(rel);
        match parse_frame(&loaded).expect("golden 固定帧必须可解析") {
            Ft12Frame::Fixed { control, addr } => {
                assert_eq!(addr, 1, "{rel}: 链路地址");
                assert_eq!(
                    loaded[1], control,
                    "{rel}: 解析出的控制域必须与线上字节一致"
                );
            }
            other => panic!("{rel}: 期望固定帧，实际 {other:?}"),
        }
    }
}

#[test]
fn golden_iec101_interrogation_addr_len_1() {
    let f = interrogation(1, 1, false, 1);
    crate::south::protocol::golden::assert_golden("iec101/interrogation_addr1.hex", &f);
    let loaded = crate::south::protocol::golden::load("iec101/interrogation_addr1.hex");
    match parse_frame(&loaded).expect("golden 可变帧必须可解析") {
        Ft12Frame::Variable {
            control,
            addr,
            asdu,
        } => {
            assert_eq!(control, f[4]);
            assert_eq!(addr, 1);
            assert_eq!(asdu, f[6..f.len() - 2], "ASDU 必须逐字节一致");
        }
        other => panic!("期望可变帧，实际 {other:?}"),
    }
}

#[test]
fn golden_iec101_interrogation_addr_len_2() {
    let f = interrogation(1, 1, true, 2);
    crate::south::protocol::golden::assert_golden("iec101/interrogation_addr2.hex", &f);
    // 2 字节公共地址比 1 字节形态多 1 字节 ASDU
    let one = interrogation(1, 1, false, 1);
    assert_eq!(
        f.len(),
        one.len() + 1,
        "公共地址 2 字节形态的帧长应比 1 字节形态多 1"
    );
}
