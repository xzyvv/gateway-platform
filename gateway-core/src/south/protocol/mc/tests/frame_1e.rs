//! 1E（A 兼容）帧（决策表 #16）：独立响应读取路径端到端测试。
use super::*;

use std::time::Duration;

// ============================================================
// 1E（A 兼容）帧（决策表 #16）：独立响应读取路径
// ============================================================

#[tokio::test]
async fn test_e2e_1e_read_word_over_tcp() {
    let (addr, _reqs) = spawn_mc_1e_stub().await;
    let c = McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        Some("1e".into()),
    );
    let p = point("p1", "D100", "u16");
    let dp = c.read(&p).await.expect("1E 读 ok");
    assert_eq!(dp.raw_value, 100.0);
}

#[tokio::test]
async fn test_e2e_1e_read_batch_and_write() {
    let (addr, reqs) = spawn_mc_1e_stub().await;
    let c = McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        Some("1e".into()),
    );
    let pts = vec![
        point("p1", "D100", "u16"),
        point("p2", "D101", "u16"),
        point("p3", "D102", "u16"),
    ];
    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 3);
    for (i, r) in rs.iter().enumerate() {
        assert_eq!(r.as_ref().expect("ok").raw_value, (100 + i) as f64);
    }
    assert_eq!(reqs.load(Ordering::SeqCst), 1, "1E 相邻点位应合并为单帧");

    // 字写
    let p = point("w1", "D20", "u16");
    c.write(&p, 34.0).await.expect("1E 写 ok");

    // 位写（M 区，1E 可寻址）
    let pb = point("b1", "M10", "u16");
    c.write(&pb, 1.0).await.expect("1E 位写 ok");
}

#[tokio::test]
async fn test_e2e_1e_bit_read() {
    let (addr, _reqs) = spawn_mc_1e_stub().await;
    let c = McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        Some("1e".into()),
    );
    let p = point("b1", "M16", "u16");
    let dp = c.read(&p).await.expect("1E 位读 ok");
    assert_eq!(dp.raw_value, 1.0);
}

/// 1E 帧中 A 系列不可寻址的软元件（如 SM）应逐点报 InvalidConfig，且不建立连接。
#[tokio::test]
async fn test_e2e_1e_unsupported_device_reported_per_point() {
    // 绑定后立即释放端口，确保即便误连也快速失败；此处点位建帧即报错，不会发请求
    let addr = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().expect("local").to_string()
    };
    let c = McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        Some("1e".into()),
    );
    let pts = vec![point("s1", "SM10", "u16")];
    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 1);
    let e = rs[0].as_ref().expect_err("SM 在 1E 下应失败");
    assert!(
        matches!(e, crate::south::protocol::CollectorError::InvalidConfig(_)),
        "unexpected: {e:?}"
    );
}
