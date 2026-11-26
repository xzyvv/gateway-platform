//! 真实 TCP 桩端到端测试：惰性建链、连接复用、断链重连、错误结束码、写通路与超时。
use super::*;

use std::time::Duration;

// ============================================================
// 真实 TCP 桩端到端测试
// ============================================================

/// 启动一个极简 MC 从站桩，返回监听地址。
///
/// 每个连接读取一次完整请求（9 字节头 + 数据长度），按 `actions`（按连接序取用，
/// 耗尽后用最后一项）处理。
async fn spawn_mc_stub(actions: Vec<StubAction>) -> String {
    spawn_mc_stub_counted(actions).await.0
}

#[tokio::test]
async fn test_diag_reads_d0_over_tcp() {
    let addr = spawn_mc_stub(vec![StubAction::Reply(mc_response(0, &[0x01, 0x00]))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let msg = c.diag().await;
    assert!(msg.contains("建链成功"), "{msg}");
    assert!(msg.contains("D0 回读值=1"), "{msg}");
}

#[tokio::test]
async fn test_diag_reports_unreachable_endpoint() {
    // 监听后立刻释放端口，连接会被拒绝
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local").to_string();
    drop(listener);
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_millis(300), None);
    let msg = c.diag().await;
    assert!(msg.contains("建链/读取失败"), "{msg}");
}

#[tokio::test]
async fn test_e2e_read_word_over_tcp() {
    let data = [0x34, 0x12]; // D100 = 0x1234
    let addr = spawn_mc_stub(vec![StubAction::Reply(mc_response(0, &data))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");
    let dp = c.read(&p).await.expect("read ok");
    assert_eq!(dp.raw_value, 0x1234 as f64);
    assert_eq!(dp.raw_type, "mc");
    assert_eq!(dp.point_id, "p1");
    // 二次读取复用同一连接
    let dp2 = c.read(&p).await.expect("read ok 2");
    assert_eq!(dp2.raw_value, 0x1234 as f64);
}

#[tokio::test]
async fn test_e2e_read_float_over_tcp() {
    let data = [0x00, 0x00, 0xC0, 0x3F]; // f32 1.5
    let addr = spawn_mc_stub(vec![StubAction::Reply(mc_response(0, &data))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let p = point("pf", "D200", "f32");
    let dp = c.read(&p).await.expect("read ok");
    assert!((dp.raw_value - 1.5).abs() < 1e-6, "got {}", dp.raw_value);
}

#[tokio::test]
async fn test_error_end_code_is_rejected() {
    let addr = spawn_mc_stub(vec![StubAction::Reply(mc_response(0xC059, &[]))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");
    let err = c.read(&p).await.expect_err("should fail");
    // ROI-25：PLC 明确回错误结束码 → DeviceRejected，而不是 DECODE
    match err {
        crate::south::protocol::CollectorError::DeviceRejected(msg) => {
            assert!(msg.contains("0xC059"), "原因需可辨识: {msg}");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn test_reconnect_after_connection_drop() {
    let data = [0x11, 0x22];
    // 第 1 次连接：读请求后断开（无响应）；第 2 次连接：正常回复
    let addr = spawn_mc_stub(vec![
        StubAction::Close,
        StubAction::Reply(mc_response(0, &data)),
    ])
    .await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");

    // 首次读取：连接被断开 → IO 错误
    let first = c.read(&p).await;
    assert!(first.is_err(), "first read should fail");
    assert!(!c.is_connected(), "should be marked disconnected");

    // 再次读取：自动重连，成功
    let dp = c.read(&p).await.expect("reconnected read ok");
    assert_eq!(dp.raw_value, 0x2211 as f64);
    assert!(c.is_connected());
}

#[tokio::test]
async fn test_write_word_over_tcp() {
    let addr = spawn_mc_stub(vec![StubAction::Reply(mc_response(0, &[]))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let mut p = point("p1", "D100", "u16");
    p.address = "D100".to_string();
    c.write(&p, 1234.0).await.expect("write ok");
}

/// 写语义：设备回了非零结束码 → 写**确定未生效** → `DeviceRejected`（明确失败，可安全重试）。
///
/// 与读路径的 `InvalidFrame` 区分开：读的失败只影响本次读；写的失败必须能判断
/// 「设备到底执行了没」，否则调用方会盲目重试造成重复写。
#[tokio::test]
async fn test_write_end_code_nonzero_is_device_rejected() {
    let addr = spawn_mc_stub(vec![StubAction::Reply(mc_response(0xC059, &[]))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let mut p = point("p1", "D100", "u16");
    p.address = "D100".to_string();
    let err = c.write(&p, 1234.0).await.expect_err("结束码非零应失败");
    assert!(
        matches!(
            err,
            crate::south::protocol::CollectorError::DeviceRejected(_)
        ),
        "unexpected: {err:?}"
    );
}

/// 写语义：设备不回（连接被关）→ 结果未知 → `WriteUncertain`（禁止自动重试）。
#[tokio::test]
async fn test_write_no_reply_is_uncertain() {
    let addr = spawn_mc_stub(vec![StubAction::Close]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let mut p = point("p1", "D100", "u16");
    p.address = "D100".to_string();
    let err = c.write(&p, 1234.0).await.expect_err("无应答应失败");
    assert!(
        matches!(
            err,
            crate::south::protocol::CollectorError::WriteUncertain(_)
        ),
        "unexpected: {err:?}"
    );
}

#[tokio::test]
async fn test_no_response_times_out() {
    let addr = spawn_mc_stub(vec![StubAction::Close]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let p = point("p1", "D100", "u16");
    let err = c.read(&p).await.expect_err("should fail");
    assert!(
        matches!(
            err,
            crate::south::protocol::CollectorError::IoError(_)
                | crate::south::protocol::CollectorError::NoReply
        ),
        "unexpected: {err:?}"
    );
}

/// 首个连接「读走请求后静默不应答」、后续连接正常应答的 MC 从站桩。
///
/// 用「静默挂起」而非直接断开，才能只触发采集器的 IO 超时分支（断开会得到 IoError）；
/// 后续连接正常应答用于验证「超时清链后下次惰性重建」。
async fn spawn_mc_silent_then_reply_stub(data: [u8; 2]) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let conns = Arc::new(AtomicUsize::new(0));
    let conns_srv = conns.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let idx = conns_srv.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                // 读走一个完整请求帧（9 字节头 + 数据长度）
                let mut head = [0u8; 9];
                if sock.read_exact(&mut head).await.is_err() {
                    return;
                }
                let len = u16::from_le_bytes([head[7], head[8]]) as usize;
                let mut body = vec![0u8; len];
                if sock.read_exact(&mut body).await.is_err() {
                    return;
                }
                if idx == 0 {
                    // 首个连接：保持打开但不应答，等采集器自己超时放弃
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    return;
                }
                if sock.write_all(&mc_response(0, &data)).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
                // 保持连接直到对端关闭
                let mut sink = [0u8; 1];
                let _ = sock.read(&mut sink).await;
            });
        }
    });
    (addr, conns)
}

/// MC-R7：单次 IO 超时取 `bus.timeout_ms`，读请求超时 → `NoReply`，并清空连接、下次惰性重建。
///
/// 现有 `test_no_response_times_out` 断言的是 `IoError(_) | NoReply`，过宽：
/// 对端断链与「对端在线但不回」是两种现场故障，必须区分。本用例只制造后者。
#[tokio::test]
async fn read_timeout_is_no_reply_and_clears_connection() {
    let (addr, conns) = spawn_mc_silent_then_reply_stub([0x11, 0x22]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_millis(150), None);
    let p = point("p1", "D100", "u16");

    let err = c.read(&p).await.expect_err("无应答应超时失败");
    assert!(
        matches!(err, crate::south::protocol::CollectorError::NoReply),
        "读超时必须精确映射为 NoReply（守护 MC-R7：超时既不是 IoError 也不是 InvalidFrame），实际 {err:?}"
    );
    assert!(
        !c.is_connected(),
        "超时后必须清空连接并标记未连接（守护 MC-R7：下次请求惰性重建）"
    );

    // 再次读取必须走新连接并成功：证明旧连接确实被清空，而不是残留半死连接
    let dp = c.read(&p).await.expect("超时清链后应能重建并读成功");
    assert_eq!(
        dp.raw_value, 0x2211 as f64,
        "重建后必须读到第二个连接的数据"
    );
    assert_eq!(
        conns.load(Ordering::SeqCst),
        2,
        "超时清链后必须重建新连接（守护 MC-R7）"
    );
}

// ============================================================
// G-T1b MC-W4 / MC-W5：写后不读回、写失败不重发
// ============================================================

/// MC-W3：写值越界必须在**上线前**拒绝，不得 `as` 静默截断成另一个值。
///
/// 70000 写 u16 若不校验会变成 4464，现场表现为「写进去了但值不对」，很难排查。
#[tokio::test]
async fn write_value_out_of_range_is_rejected_before_send() {
    let (addr, conns, reqs) =
        spawn_mc_stub_counters(vec![StubAction::Reply(mc_response(0, &[]))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let mut p = point("p1", "D100", "u16");
    p.address = "D100".to_string();

    let err = c.write(&p, 70000.0).await.expect_err("越界写值应被拒绝");
    match err {
        crate::south::protocol::CollectorError::InvalidConfig(msg) => {
            assert!(msg.contains("70000"), "原因需可辨识: {msg}");
            assert!(msg.contains("U16"), "原因需点明数据类型: {msg}");
        }
        other => panic!("越界写值应报 InvalidConfig，实际 {other:?}"),
    }
    assert_eq!(
        conns.load(Ordering::SeqCst),
        0,
        "越界值必须在上线前拒绝：不得建立连接"
    );
    assert_eq!(
        reqs.load(Ordering::SeqCst),
        0,
        "越界值必须在上线前拒绝：不得发出写帧"
    );
}

/// MC-W4：写成功后只发出一条写请求，**不得再补一次读回**。
#[tokio::test]
async fn mc_write_issues_single_request_without_readback() {
    let (addr, _conns, reqs) =
        spawn_mc_stub_counters(vec![StubAction::Reply(mc_response(0, &[]))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let mut p = point("p1", "D100", "u16");
    p.address = "D100".to_string();

    c.write(&p, 1234.0).await.expect("write ok");
    assert_eq!(
        reqs.load(Ordering::SeqCst),
        1,
        "写成功后不得再发读回（协议层不做读回，读回只在结果未知时由统一写服务触发）"
    );
}

// ============================================================
// G-T2a MC-R11：同连接流锁串行化，批量分片与诊断并发不交叉
// ============================================================

/// 请求感知 + 延迟应答的 3E 字读桩：按请求首地址与点数回显「每字 = 首地址 + 序号」，
/// 读请求后停顿数毫秒以放大并发窗口（若同连接未串行化，应答会被另一路先读走）。
async fn spawn_mc_delayed_echo_stub() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let reqs = Arc::new(AtomicUsize::new(0));
    let reqs_srv = reqs.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let reqs = reqs_srv.clone();
            tokio::spawn(async move {
                loop {
                    let mut head = [0u8; 9];
                    if sock.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    let len = u16::from_le_bytes([head[7], head[8]]) as usize;
                    let mut body = vec![0u8; len];
                    if sock.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    if body.len() < 12 {
                        return;
                    }
                    // body: 定时器(2) 命令(2) 子命令(2) 软元件码(1) 首地址(3) 点数(2)
                    let start = u32::from_le_bytes([body[7], body[8], body[9], 0]);
                    let points = u16::from_le_bytes([body[10], body[11]]) as u32;
                    let mut data = Vec::with_capacity(points as usize * 2);
                    for i in 0..points {
                        data.extend_from_slice(&((start + i) as u16).to_le_bytes());
                    }
                    reqs.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    if sock.write_all(&mc_response(0, &data)).await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                }
            });
        }
    });
    (addr, reqs)
}

/// MC-R11：同一 TCP 连接下，批量读（会自动分片）与诊断并发时，流锁必须串行化事务，
/// 任一方都不得读到对方的应答（帧不交叉）。
#[tokio::test]
async fn concurrent_batch_and_diag_do_not_cross_frames() {
    let (addr, reqs) = spawn_mc_delayed_echo_stub().await;
    let c = Arc::new(McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        None,
    ));

    // 480 个相邻字（D1000..=D1479）→ 超 479 上限，切为 2 个分片帧；
    // 值与诊断探针 D0 的预期值 0 不同，便于识别串包。
    let batch_pts: Vec<PointConfig> = (0..480)
        .map(|i| point(&format!("p{i}"), &format!("D{}", 1000 + i), "u16"))
        .collect();

    let c_batch = c.clone();
    let c_diag = c.clone();
    let (batch_rs, diag_msgs) = tokio::join!(
        async move { c_batch.read_batch(&batch_pts).await },
        async move {
            let mut msgs = Vec::new();
            for _ in 0..3 {
                msgs.push(c_diag.diag().await);
            }
            msgs
        }
    );

    assert_eq!(batch_rs.len(), 480);
    for (i, r) in batch_rs.iter().enumerate() {
        let dp = r
            .as_ref()
            .unwrap_or_else(|e| panic!("并发下 D{} 读取失败: {e:?}", 1000 + i));
        assert_eq!(
            dp.raw_value,
            (1000 + i) as f64,
            "并发下 D{} 读到了他人应答（帧交叉）",
            1000 + i
        );
    }
    for msg in &diag_msgs {
        assert!(msg.contains("D0 回读值=0"), "诊断应答被批量帧污染: {msg}");
    }
    assert_eq!(
        reqs.load(Ordering::SeqCst),
        5,
        "2 个批量分片 + 3 次诊断，共 5 次事务"
    );
}

/// MC-W5：设备明确拒绝（结束码非零）后**不得自动重发**——写帧可能已被执行。
#[tokio::test]
async fn mc_write_failure_is_not_retried() {
    let (addr, _conns, reqs) =
        spawn_mc_stub_counters(vec![StubAction::Reply(mc_response(0xC059, &[]))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let mut p = point("p1", "D100", "u16");
    p.address = "D100".to_string();

    let err = c.write(&p, 1234.0).await.expect_err("结束码非零应失败");
    assert!(matches!(
        err,
        crate::south::protocol::CollectorError::DeviceRejected(_)
    ));
    assert_eq!(
        reqs.load(Ordering::SeqCst),
        1,
        "写失败必须原样上报，不得自动重发（重发会造成重复写）"
    );
}
