//! 欧姆龙 FINS 协议单元测试 + 真实 FINS/TCP 桩端到端测试

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

use super::frames::*;
use super::{mode_from_variant, FinsCollector, FinsMode};
use crate::config::PointConfig;
use crate::engine::model::PointDataType;
use crate::south::protocol::Collector;

// ============================================================
// 纯函数单元测试
// ============================================================

#[test]
fn test_parse_address_areas() {
    let d = parse_address("D100").expect("D100");
    assert_eq!(d.word_code, 0x82);
    assert_eq!(d.bit_code, 0x02);
    assert_eq!(d.address, 100);

    let cio = parse_address("CIO0").expect("CIO0");
    assert_eq!(cio.word_code, 0xB0);
    assert_eq!(cio.address, 0);

    let w = parse_address("W200").expect("W200");
    assert_eq!(w.word_code, 0xB1);
    assert_eq!(w.address, 200);
}

#[test]
fn test_parse_address_errors() {
    assert!(parse_address("").is_err());
    assert!(parse_address("ZZ10").is_err());
    assert!(parse_address("D").is_err());
}

#[test]
fn test_build_node_addr_request_layout() {
    let req = build_node_addr_request(0);
    assert_eq!(req.len(), 20);
    assert_eq!(&req[0..4], b"FINS");
    assert_eq!(u32::from_be_bytes([req[4], req[5], req[6], req[7]]), 12);
    assert_eq!(u32::from_be_bytes([req[8], req[9], req[10], req[11]]), 0);
    assert_eq!(&req[16..20], &[0x00, 0x00, 0x00, 0x00]);
}

#[test]
fn test_build_read_frame_layout() {
    let fr = build_read_frame(0x01, 0x00, 0x82, 100, 2);
    assert_eq!(fr.len(), 17);
    assert_eq!(fr[0], 0x80); // ICF
    assert_eq!(fr[4], 0x01); // DA1 = 服务端节点
    assert_eq!(fr[7], 0x00); // SA1 = 客户端节点
    assert_eq!(fr[10], MRC_MEMORY_READ);
    assert_eq!(fr[11], SRC_MEMORY_READ);
    assert_eq!(fr[12], 0x82); // DM 字存储区码
    assert_eq!(u16::from_be_bytes([fr[13], fr[14]]), 100); // 地址大端
    assert_eq!(u16::from_be_bytes([fr[15], fr[16]]), 2); // 计数大端
}

#[test]
fn test_build_read_frame_count() {
    let fr = build_read_frame(0x01, 0x00, 0xB0, 5, 1);
    // 存储区码 + 地址(2) + 计数(2) 位于尾部
    let n = fr.len();
    assert_eq!(u16::from_be_bytes([fr[n - 2], fr[n - 1]]), 1);
    assert_eq!(u16::from_be_bytes([fr[n - 4], fr[n - 3]]), 5);
    assert_eq!(fr[n - 5], 0xB0);
}

#[test]
fn test_parse_read_response_ok_and_error() {
    // 构造成功应答：头(10) + MRC SRC + MRES SRES + data
    let mut fr = vec![0xC0, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01];
    fr.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]);
    fr.extend_from_slice(&[0x12, 0x34]);
    let data = parse_read_response(&fr, 0x01, 0x01).expect("ok");
    assert_eq!(data, vec![0x12, 0x34]);

    // 结束码非零
    let mut bad = vec![0xC0, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01];
    bad.extend_from_slice(&[0x01, 0x01, 0x00, 0x40]);
    assert!(parse_read_response(&bad, 0x01, 0x01).is_err());
}

#[test]
fn test_decode_encode_words_big_endian() {
    assert_eq!(
        decode_words(&[0x12, 0x34], PointDataType::U16),
        Some(0x1234 as f64)
    );
    let f = decode_words(&[0x3F, 0xC0, 0x00, 0x00], PointDataType::F32).expect("f32");
    assert!((f - 1.5).abs() < 1e-6);

    let bytes = encode_words(0x1234 as f64, PointDataType::U16);
    assert_eq!(bytes, vec![0x12, 0x34]);
    let bytes = encode_words(1.5, PointDataType::F32);
    assert_eq!(bytes, vec![0x3F, 0xC0, 0x00, 0x00]);
}

// ============================================================
// 真实 FINS/TCP 桩端到端测试
// ============================================================

const STUB_SERVER_NODE: u8 = 0x01;

#[derive(Clone)]
enum FinsReply {
    /// 存储区读应答，带数据（大端字序列）
    Read(Vec<u8>),
    /// 存储区写应答（无数据）
    Write,
    /// 结束码非零的读应答
    EndCode(u8, u8),
    /// CPU 单元运行控制应答（src, mres, sres）
    CpuControl(u8, u8, u8),
    /// 不回复直接断开
    Drop,
}

fn tcp_wrap(command: u32, payload: &[u8]) -> Vec<u8> {
    let mut f = b"FINS".to_vec();
    f.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    f.extend_from_slice(&command.to_be_bytes());
    f.extend_from_slice(&0u32.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

fn fins_frame_resp(mrc: u8, src: u8, mres: u8, sres: u8, data: &[u8]) -> Vec<u8> {
    let mut fr = vec![
        0xC0,
        0x00,
        0x02,
        0x00,
        0x00, // DA1 = 客户端节点
        0x00,
        0x00,
        STUB_SERVER_NODE, // SA1 = 服务端节点
        0x00,
        0x01,
    ];
    fr.extend_from_slice(&[mrc, src, mres, sres]);
    fr.extend_from_slice(data);
    fr
}

/// 启动 FINS/TCP 从站桩；`replies` 按 FINS 事务序取用（跨连接共享，耗尽用最后一项）。
async fn spawn_fins_stub(replies: Vec<FinsReply>) -> String {
    spawn_fins_stub_counted(replies).await.0
}

/// 同 `spawn_fins_stub`，额外返回已接受的连接次数（用于验证"批次只建链一次"）。
async fn spawn_fins_stub_counted(replies: Vec<FinsReply>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local").to_string();
    let tx_idx = Arc::new(AtomicUsize::new(0));
    let conn_count = Arc::new(AtomicUsize::new(0));
    let conn_ret = conn_count.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            conn_count.fetch_add(1, Ordering::SeqCst);
            let tx_idx = tx_idx.clone();
            let replies = replies.clone();
            tokio::spawn(async move {
                // 握手：读节点地址请求
                let mut head = [0u8; 8];
                if sock.read_exact(&mut head).await.is_err() {
                    return;
                }
                let len = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
                let mut body = vec![0u8; len];
                if sock.read_exact(&mut body).await.is_err() {
                    return;
                }
                // 回节点地址应答
                let resp = tcp_wrap(0x0000_0002, &[0x00, 0x00, 0x00, STUB_SERVER_NODE]);
                if sock.write_all(&resp).await.is_err() {
                    return;
                }
                // 循环处理 FINS 事务
                loop {
                    let mut h = [0u8; 8];
                    if sock.read_exact(&mut h).await.is_err() {
                        return;
                    }
                    let l = u32::from_be_bytes([h[4], h[5], h[6], h[7]]) as usize;
                    let mut b = vec![0u8; l];
                    if sock.read_exact(&mut b).await.is_err() {
                        return;
                    }
                    let idx = tx_idx.fetch_add(1, Ordering::SeqCst);
                    let reply = replies
                        .get(idx)
                        .cloned()
                        .unwrap_or_else(|| replies.last().cloned().unwrap_or(FinsReply::Drop));
                    let out = match reply {
                        FinsReply::Read(data) => {
                            tcp_wrap(0x0000_0002, &fins_frame_resp(0x01, 0x01, 0x00, 0x00, &data))
                        }
                        FinsReply::Write => {
                            tcp_wrap(0x0000_0002, &fins_frame_resp(0x01, 0x02, 0x00, 0x00, &[]))
                        }
                        FinsReply::EndCode(m, s) => {
                            tcp_wrap(0x0000_0002, &fins_frame_resp(0x01, 0x01, m, s, &[]))
                        }
                        FinsReply::CpuControl(src, mres, sres) => tcp_wrap(
                            0x0000_0002,
                            &fins_frame_resp(MRC_CPU_CONTROL, src, mres, sres, &[]),
                        ),
                        FinsReply::Drop => return,
                    };
                    if sock.write_all(&out).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, conn_ret)
}

fn point(id: &str, address: &str, data_type: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: "fins-bus".to_string(),
        protocol: "fins".to_string(),
        address: address.to_string(),
        data_type: Some(data_type.to_string()),
        interval: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_diag_reads_dm0_over_tcp() {
    let addr = spawn_fins_stub(vec![FinsReply::Read(vec![0x00, 0x2A])]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let msg = c.diag().await;
    assert!(msg.contains("建链成功"), "{msg}");
    assert!(msg.contains("DM0 回读值=42"), "{msg}");
}

#[tokio::test]
async fn test_diag_reports_unreachable_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local").to_string();
    drop(listener);
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_millis(300), None);
    let msg = c.diag().await;
    assert!(msg.contains("建链/节点握手失败"), "{msg}");
}

#[tokio::test]
async fn test_e2e_read_word_over_tcp() {
    let addr = spawn_fins_stub(vec![FinsReply::Read(vec![0x12, 0x34])]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");
    let dp = c.read(&p).await.expect("read ok");
    assert_eq!(dp.raw_value, 0x1234 as f64);
    assert_eq!(dp.point_id, "p1");
    // 复用连接
    let dp2 = c.read(&p).await.expect("read ok 2");
    assert_eq!(dp2.raw_value, 0x1234 as f64);
    assert!(c.is_connected());
}

#[tokio::test]
async fn test_e2e_read_float_over_tcp() {
    let addr = spawn_fins_stub(vec![FinsReply::Read(vec![0x3F, 0xC0, 0x00, 0x00])]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("pf", "D200", "f32");
    let dp = c.read(&p).await.expect("read ok");
    assert!((dp.raw_value - 1.5).abs() < 1e-6, "got {}", dp.raw_value);
}

#[tokio::test]
async fn test_e2e_write_over_tcp() {
    let addr = spawn_fins_stub(vec![FinsReply::Write]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");
    c.write(&p, 1234.0).await.expect("write ok");
}

#[tokio::test]
async fn test_error_end_code_is_rejected() {
    let addr = spawn_fins_stub(vec![FinsReply::EndCode(0x00, 0x40)]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");
    let err = c.read(&p).await.expect_err("should fail");
    assert!(
        matches!(err, crate::south::protocol::CollectorError::InvalidFrame(_)),
        "unexpected: {err:?}"
    );
}

#[tokio::test]
async fn test_reconnect_after_handshake_drop() {
    // 第 1 次事务掉线；第 2 次正常
    let addr = spawn_fins_stub(vec![FinsReply::Drop, FinsReply::Read(vec![0xAB, 0xCD])]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");
    let first = c.read(&p).await;
    assert!(first.is_err(), "first read should fail");
    assert!(!c.is_connected());
    let dp = c.read(&p).await.expect("reconnected read ok");
    assert_eq!(dp.raw_value, 0xABCD as f64);
}

// ============================================================
// 批次建链优化契约（read_batch 整批共享一次建链）
// ============================================================

/// 契约一：整批点位只建立**一次**连接（逐点 read 复用同一链路）
#[tokio::test]
async fn batch_read_connects_once() {
    let (addr, conns) = spawn_fins_stub_counted(vec![FinsReply::Read(vec![0x12, 0x34])]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let pts = vec![
        point("p1", "D100", "u16"),
        point("p2", "D200", "u16"),
        point("p3", "D300", "u16"),
    ];

    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 3);
    for r in &rs {
        let dp = r.as_ref().expect("批次内每个点位都应读取成功");
        assert_eq!(dp.raw_value, 0x1234 as f64);
    }
    assert_eq!(conns.load(Ordering::SeqCst), 1, "整批应只建链一次");
}

/// 契约二：设备不可达时整批返回**同因错误**（不再逐点重复等待连接超时）
#[tokio::test]
async fn batch_read_same_error_when_device_unreachable() {
    // 绑定后立即释放监听端口，使后续 connect 必然被拒（快速失败）
    let addr = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().expect("local").to_string()
    };
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let pts = vec![point("p1", "D100", "u16"), point("p2", "D200", "u16")];

    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 2);
    let mut msgs = Vec::new();
    for r in rs {
        let e = r.expect_err("设备不可达时整批应失败");
        assert!(
            matches!(e, crate::south::protocol::CollectorError::IoError(_)),
            "unexpected: {e:?}"
        );
        msgs.push(e.to_string());
    }
    assert_eq!(msgs[0], msgs[1], "整批应为同因错误");
    assert!(!c.is_connected(), "建链失败后不应标记为已连接");
}

// ============================================================
// CPU 单元运行控制（RUN 0401 / STOP 0402）
// ============================================================

/// CPU 运行控制地址解析（大小写不敏感；CPU: 前缀可选）
#[test]
fn test_parse_cpu_control_address() {
    assert_eq!(parse_cpu_control("CPU:RUN"), Some(CpuControl::Run));
    assert_eq!(parse_cpu_control("cpu:run"), Some(CpuControl::Run));
    assert_eq!(parse_cpu_control("RUN"), Some(CpuControl::Run));
    assert_eq!(parse_cpu_control(" CPU:STOP "), Some(CpuControl::Stop));
    assert_eq!(parse_cpu_control("stop"), Some(CpuControl::Stop));
    // 非控制地址不应被误判（走普通存储区读写）
    assert_eq!(parse_cpu_control("D100"), None);
    assert_eq!(parse_cpu_control("RUNNING"), None);
    assert_eq!(parse_cpu_control(""), None);
}

/// CPU 运行控制帧字节布局：RUN 带 2 字节模式参数，STOP 无参数
#[test]
fn test_build_cpu_control_frame_layout() {
    let run = build_cpu_control_frame(0x01, 0x00, CpuControl::Run);
    assert_eq!(run.len(), 14, "RUN 帧 = 头(10) + MRC/SRC(2) + 模式(2)");
    assert_eq!(&run[10..12], &[MRC_CPU_CONTROL, SRC_CPU_RUN]);
    assert_eq!(
        u16::from_be_bytes([run[12], run[13]]),
        CPU_RUN_MODE_STANDARD
    );

    let stop = build_cpu_control_frame(0x01, 0x00, CpuControl::Stop);
    assert_eq!(stop.len(), 12, "STOP 帧 = 头(10) + MRC/SRC(2)");
    assert_eq!(&stop[10..12], &[MRC_CPU_CONTROL, SRC_CPU_STOP]);
}

/// 端到端：CPU:RUN 经统一点位写路径下发，桩回 0401 成功应答
#[tokio::test]
async fn test_e2e_cpu_run_over_tcp() {
    let addr = spawn_fins_stub(vec![FinsReply::CpuControl(SRC_CPU_RUN, 0x00, 0x00)]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("cpu", "CPU:RUN", "u16");
    c.write(&p, 1.0).await.expect("RUN 指令应下发成功");
}

/// 端到端：CPU:STOP 下发，桩回 0402 成功应答
#[tokio::test]
async fn test_e2e_cpu_stop_over_tcp() {
    let addr = spawn_fins_stub(vec![FinsReply::CpuControl(SRC_CPU_STOP, 0x00, 0x00)]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("cpu", "CPU:STOP", "u16");
    c.write(&p, 1.0).await.expect("STOP 指令应下发成功");
}

/// 端到端：CPU 控制结束码非零必须判失败（不得静默成功）
#[tokio::test]
async fn test_e2e_cpu_control_end_code_rejected() {
    // SRES=0x02（如"运行模式冲突"）应导致写入失败
    let addr = spawn_fins_stub(vec![FinsReply::CpuControl(SRC_CPU_RUN, 0x00, 0x02)]).await;
    let c = FinsCollector::new(addr, "fins-bus".into(), Duration::from_secs(5), None);
    let p = point("cpu", "CPU:RUN", "u16");
    let err = c.write(&p, 1.0).await.expect_err("结束码非零应失败");
    assert!(
        matches!(err, crate::south::protocol::CollectorError::InvalidFrame(_)),
        "unexpected: {err:?}"
    );
}

// ============================================================
// #14 FINS/UDP 模式 + EM 区契约
// ============================================================

#[test]
fn test_parse_address_em_area() {
    let em = parse_address("EM10").expect("EM10");
    assert_eq!(em.word_code, 0xA0);
    assert_eq!(em.bit_code, 0x20);
    assert_eq!(em.address, 10);
    let e = parse_address("E7").expect("E7");
    assert_eq!(e.word_code, 0xA0);
    assert_eq!(e.address, 7);
}

#[test]
fn test_mode_from_variant() {
    assert_eq!(mode_from_variant(Some("udp")), FinsMode::Udp);
    assert_eq!(mode_from_variant(Some(" UDP ")), FinsMode::Udp);
    assert_eq!(mode_from_variant(Some("tcp")), FinsMode::Tcp);
    assert_eq!(mode_from_variant(None), FinsMode::Tcp);
}

#[test]
fn test_parse_udp_target_node() {
    let (a, n) = parse_udp_target("192.168.1.20:9600");
    assert_eq!(a, "192.168.1.20:9600");
    assert_eq!(n, Some(20), "IPv4 末位八位组作为缺省节点号");

    let (a, n) = parse_udp_target("10.0.0.5:9600?node=77");
    assert_eq!(a, "10.0.0.5:9600", "显式 node 应从连接目标中剥离");
    assert_eq!(n, Some(77));

    let (_, n) = parse_udp_target("localhost:9600");
    assert_eq!(n, None, "非 IPv4 且无显式 node 时无法推导");
}

/// FINS/UDP 从站桩：数据报即 FINS 帧，收到后按回复直接回发应答 FINS 帧。
async fn spawn_fins_udp_stub(replies: Vec<FinsReply>) -> String {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind udp");
    let addr = socket.local_addr().expect("local").to_string();
    let idx = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        loop {
            let (n, peer) = match socket.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(_) => break,
            };
            if n < 14 {
                continue;
            }
            let i = idx.fetch_add(1, Ordering::SeqCst);
            let reply = replies
                .get(i)
                .cloned()
                .unwrap_or_else(|| replies.last().cloned().unwrap_or(FinsReply::Drop));
            let out = match reply {
                FinsReply::Read(data) => fins_frame_resp(0x01, 0x01, 0x00, 0x00, &data),
                FinsReply::Write => fins_frame_resp(0x01, 0x02, 0x00, 0x00, &[]),
                FinsReply::EndCode(m, s) => fins_frame_resp(0x01, 0x01, m, s, &[]),
                FinsReply::CpuControl(src, mres, sres) => {
                    fins_frame_resp(MRC_CPU_CONTROL, src, mres, sres, &[])
                }
                FinsReply::Drop => continue,
            };
            let _ = socket.send_to(&out, peer).await;
        }
    });
    addr
}

#[tokio::test]
async fn test_e2e_read_word_over_udp() {
    let addr = spawn_fins_udp_stub(vec![FinsReply::Read(vec![0x12, 0x34])]).await;
    let c = FinsCollector::new(
        addr,
        "fins-bus".into(),
        Duration::from_secs(5),
        Some("udp".into()),
    );
    let p = point("p1", "D100", "u16");
    let dp = c.read(&p).await.expect("udp read ok");
    assert_eq!(dp.raw_value, 0x1234 as f64);
    assert!(c.is_connected());
    // 复用同一 UDP 套接字
    let dp2 = c.read(&p).await.expect("udp read ok 2");
    assert_eq!(dp2.raw_value, 0x1234 as f64);
}

#[tokio::test]
async fn test_e2e_write_over_udp() {
    let addr = spawn_fins_udp_stub(vec![FinsReply::Write]).await;
    let c = FinsCollector::new(
        addr,
        "fins-bus".into(),
        Duration::from_secs(5),
        Some("udp".into()),
    );
    let p = point("p1", "D100", "u16");
    c.write(&p, 4321.0).await.expect("udp write ok");
}

/// UDP 缺少可用目标节点号时应报配置错误，且不得标记为已连接。
#[tokio::test]
async fn test_udp_missing_node_is_config_error() {
    let c = FinsCollector::new(
        "localhost:9600".into(),
        "fins-bus".into(),
        Duration::from_secs(1),
        Some("udp".into()),
    );
    let p = point("p1", "D100", "u16");
    let err = c.read(&p).await.expect_err("should fail");
    assert!(
        matches!(
            err,
            crate::south::protocol::CollectorError::InvalidConfig(_)
        ),
        "unexpected: {err:?}"
    );
    assert!(!c.is_connected());
}

// =========================================================================
// P0-3：黄金报文（Golden）
// -------------------------------------------------------------------------
// FINS 的字节级基线：TCP 报头（"FINS" 魔数 + 大端长度/命令）+ FINS 帧头
// （ICF/RSV/GCT/DNA/DA1/DA2/SNA/SA1/SA2/SID）+ MRC/SRC + 参数。
// 这些字段任何一处字节序或常量写错，现场表现都是「连上了但读不到数」，
// 没有字节基线时只能靠换一台 PLC 试。样本同时覆盖「TCP 封装」与「裸 FINS 帧」
// 两种形态，因为 FINS/UDP 模式直接发后者。
//
// 刷新方式：$env:GOLDEN_UPDATE='1'; cargo test -p gateway-core --lib golden
// =========================================================================

#[test]
fn golden_fins_node_addr_request() {
    let f = build_node_addr_request(0x01);
    crate::south::protocol::golden::assert_golden("fins/node_addr_request.hex", &f);
}

#[test]
fn golden_fins_read_dm_words_request() {
    // DM 字区（0x82）、起始地址 0x0000、读 2 项
    let fins = build_read_frame(0x01, 0x02, 0x82, 0x0000, 0x0002);
    crate::south::protocol::golden::assert_golden("fins/read_dm_words_fins_frame.hex", &fins);
    let tcp = build_fins_frame_request(&fins);
    crate::south::protocol::golden::assert_golden("fins/read_dm_words_tcp_request.hex", &tcp);
}

#[test]
fn golden_fins_write_dm_words_request() {
    // 写 1 个字：值 0x1234（大端字序列）
    let data = encode_words(0x1234 as f64, PointDataType::U16);
    let fins = build_write_frame(0x01, 0x02, 0x82, 0x0000, 0x0001, &data);
    let tcp = build_fins_frame_request(&fins);
    crate::south::protocol::golden::assert_golden("fins/write_dm_words_tcp_request.hex", &tcp);
}

#[test]
fn golden_fins_cpu_run_request() {
    let fins = build_cpu_control_frame(0x01, 0x02, CpuControl::Run);
    let tcp = build_fins_frame_request(&fins);
    crate::south::protocol::golden::assert_golden("fins/cpu_run_tcp_request.hex", &tcp);
}

/// 读应答样本（不含 TCP 报头）：FINS 帧头 10 字节 + MRC/SRC + MRES/SRES + 数据
///
/// 断言不止「字节没变」，还要「样本自己能解出正确值」——否则样本冻结的可能是
/// 一份谁都解析不出来的垃圾。
#[test]
fn golden_fins_read_dm_words_response_decodes() {
    let mut fins = vec![
        0x80, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x01, 0x00,
        0x01, // 帧头（应答方向 DA/SA 互换）
    ];
    fins.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]); // MRC/SRC=01/01，结束码 0000
    fins.extend_from_slice(&[0x12, 0x34]); // 1 个字的读回数据
    crate::south::protocol::golden::assert_golden("fins/read_dm_words_response.hex", &fins);

    let loaded = crate::south::protocol::golden::load("fins/read_dm_words_response.hex");
    let payload = parse_read_response(&loaded, MRC_MEMORY_READ, SRC_MEMORY_READ)
        .expect("golden 读应答必须可解析");
    assert_eq!(payload, vec![0x12, 0x34]);
    assert_eq!(parse_read_end_code(&loaded).expect("结束码可读"), 0);
    assert_eq!(
        decode_words(&payload, PointDataType::U16),
        Some(0x1234 as f64),
        "读回数据应按大端解码为 0x1234"
    );
}

// ============================================================
// 畸形帧负例：截断 / 命令码不匹配 / 结束码非零 / 长度越界
// ============================================================

fn valid_read_response() -> Vec<u8> {
    let mut fins = vec![0x80, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x01, 0x00, 0x01];
    fins.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]); // MRC/SRC=01/01，结束码 0000
    fins.extend_from_slice(&[0x12, 0x34]);
    fins
}

#[test]
fn malformed_fins_read_response_is_rejected_without_panic() {
    use crate::south::protocol::CollectorError;

    let full = valid_read_response();
    assert!(parse_read_response(&full, MRC_MEMORY_READ, SRC_MEMORY_READ).is_ok());

    // 帧头（14 字节）任意截断都必须报错
    for cut in 0..14 {
        assert!(
            parse_read_response(&full[..cut], MRC_MEMORY_READ, SRC_MEMORY_READ).is_err(),
            "截断到 {cut} 字节应被拒绝"
        );
        assert!(
            parse_read_end_code(&full[..cut]).is_err(),
            "结束码读取也应拒绝 {cut} 字节"
        );
    }

    // MRC / SRC 与请求不一致（串包 / 错帧）
    assert!(parse_read_response(&full, 0x02, SRC_MEMORY_READ).is_err());
    assert!(parse_read_response(&full, MRC_MEMORY_READ, 0x02).is_err());
    assert!(parse_read_end_code(&{
        let mut f = full.clone();
        f[10] = 0x02;
        f
    })
    .is_err());

    // 结束码非零仍按帧错误上抛，但 parse_read_end_code 应如实返回原始码
    let mut bad_end = full.clone();
    bad_end[12] = 0x11;
    bad_end[13] = 0x02;
    match parse_read_response(&bad_end, MRC_MEMORY_READ, SRC_MEMORY_READ) {
        Err(CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("end code"), "应说明结束码异常: {msg}");
        }
        other => panic!("非零结束码应报错，实际: {other:?}"),
    }
    assert_eq!(parse_read_end_code(&bad_end).expect("结束码可读"), 0x1102);

    // 数据区截断（帧头完整但数据不足）不得 panic
    for cut in 14..full.len() {
        let _ = parse_read_response(&full[..cut], MRC_MEMORY_READ, SRC_MEMORY_READ);
    }
}

#[test]
fn malformed_fins_node_addr_response_is_rejected() {
    for len in 0..4 {
        let payload = vec![0u8; len];
        assert!(
            parse_node_addr_response(&payload).is_err(),
            "{len} 字节的节点地址应答应被拒绝"
        );
    }
    assert_eq!(
        parse_node_addr_response(&[0x00, 0x00, 0x00, 0x07]).expect("4 字节应可解析"),
        0x07
    );
}
