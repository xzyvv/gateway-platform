//! C-12 长连接 keepalive 常量契约与 Drop 资源释放回归
use super::*;

// ============================================================
// C-12 修复：长连接 keepalive 常量契约
// ============================================================

/// TESTFR U-frame 控制域：b0/b1 恒为 1（区分 U 与 I/S 格式），
/// 功能位 one-hot —— b6 = act(0x40) → 0x43，b7 = con(0x80) → 0x83。
/// 旧实现把 con(0x83) 当 act 发出（server 不会对 con 回 con），是必须纠正的协议错误。
#[test]
fn c12_testfr_act_and_con_are_distinct_bytes() {
    assert_eq!(
        TESTFR_ACT_CF,
        [0x43, 0x00, 0x00, 0x00],
        "TESTFR act cf 必须为 0x43"
    );
    assert_eq!(
        TESTFR_CON_CF,
        [0x83, 0x00, 0x00, 0x00],
        "TESTFR con cf 必须为 0x83"
    );
    assert_ne!(
        TESTFR_ACT_CF[0], TESTFR_CON_CF[0],
        "act 与 con 不能是同一字节"
    );
}

/// U-frame 控制域低 2 位恒为 1，是 U 格式（与 I 格式 b0=0 / S 格式 b0=1,b1=0）的判据
#[test]
fn c12_uframe_encoding_matches_iec104_spec() {
    for cf in [STARTDT_ACT, TESTFR_ACT_CF, TESTFR_CON_CF] {
        assert_eq!(cf[0] & 0x03, 0x03, "U-frame 控制域低 2 位必须恒为 1");
    }
    // STARTDT act = 0x07（b2 置位）、STARTDT con = 0x0B（b3 置位）
    assert_eq!(STARTDT_ACT[0], 0x07);
}

/// keepalive 周期必须小于 IEC 104 t1（默认 15s），且响应超时小于探测周期
/// 避免本端还没响应 server 端就关连接
// 契约守门：断言对象就是常量取值范围本身，常量断言属预期，不简化
#[allow(clippy::assertions_on_constants)]
#[test]
fn c12_keepalive_timing_within_iec104_spec() {
    assert!(
        KEEPALIVE_INTERVAL < Duration::from_secs(15),
        "keepalive interval must be < 15s (IEC 104 t1 default), got {:?}",
        KEEPALIVE_INTERVAL
    );
    assert!(
        KEEPALIVE_RESPONSE_TIMEOUT < KEEPALIVE_INTERVAL,
        "response timeout must be < keepalive interval, got {:?} vs {:?}",
        KEEPALIVE_RESPONSE_TIMEOUT,
        KEEPALIVE_INTERVAL
    );
    // 容忍次数 ≥ 2：单次抖动不应触发重连（避免连接抖动）
    assert!(
        KEEPALIVE_MAX_CONSECUTIVE_FAILURES >= 2,
        "single probe failure must not tear down the link"
    );
}

/// build_uframe + TESTFR 控制域应生成 IEC 104 规范的 6 字节 U-frame
#[test]
fn c12_build_testfr_frames() {
    assert_eq!(
        build_uframe(TESTFR_ACT_CF),
        vec![0x68, 0x04, 0x43, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        build_uframe(TESTFR_CON_CF),
        vec![0x68, 0x04, 0x83, 0x00, 0x00, 0x00]
    );
}

// ============================================================
// 修复(审查#B4) 运行时回归：drop 必须中止 keepalive 并立即释放 TCP
// ============================================================
//
// 修复前：热重载整体替换 collectors 时不会调用 disconnect()，keepalive supervisor
//         持有 `stream` 的 Arc clone，TCP 流永不释放——旧连接持续占用（每次热重载
//         堆积一份，并可能互相偷帧）。
// 修复后：Drop 中 abort supervisor 句柄并 take 出 TcpStream（drop 即关连接）。
//
// 观测点：假从站侧读到 EOF（对端已关闭）。KEEPALIVE_INTERVAL=10s 远大于 2s 观测
//         窗口，故无 Drop 时窗口内不会出现关闭事件。

#[tokio::test]
async fn drop_closes_tcp_and_aborts_keepalive() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地假从站端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(fake_iec104_slave(listener, closed_tx));

    let collector = Iec104Collector::new(
        addr,
        "bus_iec104_drop".to_string(),
        Duration::from_millis(500),
        2,
    );
    collector.connect().await.expect("假从站握手应成功");
    assert!(
        collector.connected.load(Ordering::Relaxed),
        "建链成功后 connected 应为 true"
    );

    // 等价热重载整体替换采集器：直接 drop（不调用 disconnect）
    let connected = collector.connected.clone();
    drop(collector);

    tokio::time::timeout(Duration::from_secs(2), closed_rx)
        .await
        .expect("drop 后假从站应在 2s 内观测到 TCP 关闭（否则 keepalive/TCP 泄漏）")
        .expect("关闭通知通道不应被丢弃");
    assert!(
        !connected.load(Ordering::Relaxed),
        "drop 后 connected 必须置 false"
    );

    server.abort();
}
