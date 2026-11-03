//! 端到端测试：真实 TCP 从站桩上的惰性建链、自愈重连、批次建链与写通路。
use super::*;

use std::time::Duration;

use crate::south::protocol::Collector;

// ============================================================
// C-13: 真实 TCP 桩端到端测试（read 惰性建链）
//
// 桩为独立实现的最小 S7 从站（字节手写，不复用生产 frames 构造响应），
// 真实走 TcpStream + Collector 生产路径。
// ============================================================

/// C-13 核心契约：调用方从不调用 `connect()`，首次 `read` 必须自动建链并成功
#[tokio::test]
async fn c13_read_lazily_connects_over_tcp() {
    let value = 42.5f32.to_be_bytes().to_vec();
    let addr = spawn_s7_stub(vec![S7StubAction::Serve], value, TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let p = s7_point("p1", "DB1.DBD0", "real");

    assert!(!c.is_connected(), "新建采集器应为未连接状态");

    // 关键：全程不调用 connect()——旧实现会直接返回 Err("S7 not connected")
    let dp = c.read(&p).await.expect("惰性建链 + 读取应成功");
    assert_eq!(dp.raw_type, "s7comm");
    assert_eq!(dp.point_id, "p1");
    assert!((dp.raw_value - 42.5).abs() < 0.01, "got {}", dp.raw_value);
    assert!(c.is_connected(), "读取后应处于已连接状态");

    // 二次读取复用同一连接（桩每连接持续服务）
    let dp2 = c.read(&p).await.expect("二次读取应成功");
    assert!((dp2.raw_value - 42.5).abs() < 0.01, "got {}", dp2.raw_value);
}

/// C-13 自愈：读到断链后清链，下一次 `read` 自动重连
#[tokio::test]
async fn c13_reconnect_after_connection_drop() {
    let value = 7.0f32.to_be_bytes().to_vec();
    // 第 1 次连接：握手后立即断开；第 2 次连接：正常服务
    let addr = spawn_s7_stub(
        vec![S7StubAction::CloseAfterHandshake, S7StubAction::Serve],
        value,
        TS_BYTE,
    )
    .await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let p = s7_point("p1", "DB1.DBD0", "real");

    // 首次读取：链路被断开 → 失败，且被标记为未连接
    let first = c.read(&p).await;
    assert!(first.is_err(), "断链后首次读取应失败");
    assert!(!c.is_connected(), "应被标记为未连接");

    // 再次读取：自动重连，成功
    let dp = c.read(&p).await.expect("自动重连后读取应成功");
    assert!((dp.raw_value - 7.0).abs() < 0.01, "got {}", dp.raw_value);
    assert!(c.is_connected(), "重连后应处于已连接状态");
}

// ============================================================
// 批次建链优化契约（read_batch 整批共享一次建链）
// ============================================================

/// 契约一：整批点位只建立**一次**连接（逐点 read 复用同一链路）
#[tokio::test]
async fn batch_read_connects_once() {
    let value = 42.5f32.to_be_bytes().to_vec();
    let (addr, conns, _writes) =
        spawn_s7_stub_counted(vec![S7StubAction::Serve], value, TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let pts = vec![
        s7_point("p1", "DB1.DBD0", "real"),
        s7_point("p2", "DB2.DBD0", "real"),
        s7_point("p3", "DB3.DBD0", "real"),
    ];

    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 3);
    for r in &rs {
        let dp = r.as_ref().expect("批次内每个点位都应读取成功");
        assert!((dp.raw_value - 42.5).abs() < 0.01, "got {}", dp.raw_value);
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
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let pts = vec![
        s7_point("p1", "DB1.DBD0", "real"),
        s7_point("p2", "DB2.DBD0", "real"),
    ];

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

/// E2E：write 惰性建链写入成功，且连接可被后续 read 复用
#[tokio::test]
async fn write_var_over_tcp_reuses_connection() {
    let value = 42.5f32.to_be_bytes().to_vec();
    let addr = spawn_s7_stub(vec![S7StubAction::Serve], value, TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let p = s7_point("p1", "DB1.DBD0", "real");

    assert!(!c.is_connected(), "新建采集器应为未连接状态");
    // 全程不调用 connect()——write 应惰性建链并成功
    c.write(&p, 123.5).await.expect("惰性建链 + 写入应成功");
    assert!(c.is_connected(), "写入后应处于已连接状态");

    let dp = c.read(&p).await.expect("写入后读取应成功");
    assert!((dp.raw_value - 42.5).abs() < 0.01, "got {}", dp.raw_value);
}

/// E2E：write_multi 覆盖默认扇出——整批合并为一次多 item Write Var 事务，
/// 坏点位单独失败，结果与入参逐位对应
#[tokio::test]
async fn write_multi_merges_into_single_transaction() {
    let value = 0.0f32.to_be_bytes().to_vec();
    let (addr, conns, writes) =
        spawn_s7_stub_counted(vec![S7StubAction::Serve], value, TS_BYTE).await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let p1 = s7_point("p1", "DB1.DBD0", "real");
    let p2 = s7_point("p2", "MW0", "word");
    let bad = s7_point("bad", "INVALID", "word"); // 地址解析失败，不应拖累同批

    let pts = vec![(&p1, 1.5), (&p2, 100.0), (&bad, 1.0)];
    let rs = c.write_multi(&pts).await;
    assert_eq!(rs.len(), 3, "结果必须与入参等长");
    assert!(rs[0].is_ok(), "p1 应成功: {:?}", rs[0]);
    assert!(rs[1].is_ok(), "p2 应成功: {:?}", rs[1]);
    assert!(rs[2].is_err(), "坏地址应单独报错，而不是整批失败");
    assert_eq!(conns.load(Ordering::SeqCst), 1, "批量写应共享一次建链");
    assert_eq!(
        writes.load(Ordering::SeqCst),
        1,
        "两个合法点位应合并成一次 Write Var 事务（默认扇出会是 2 次）"
    );
}
