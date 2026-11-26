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

// ============================================================
// G-T1b MC-W4 / MC-W5：写后不读回、写失败不重发
// ============================================================

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
