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
    assert!(
        matches!(err, crate::south::protocol::CollectorError::InvalidFrame(_)),
        "unexpected: {err:?}"
    );
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
