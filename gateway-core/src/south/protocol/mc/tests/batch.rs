//! 协议级批量读（决策表 #7）：合并、批次建链一次、超限分片与 1E 单帧上限。
use super::*;

use std::time::Duration;

// ============================================================
// 批次建链优化契约（read_batch 整批共享一次建链）
// ============================================================

/// 契约一：整批点位只建立**一次**连接（逐点 read 复用同一链路）
#[tokio::test]
async fn batch_read_connects_once() {
    let data = [0x34, 0x12]; // D100 = 0x1234
    let (addr, conns) = spawn_mc_stub_counted(vec![StubAction::Reply(mc_response(0, &data))]).await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
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
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
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
// 决策表 #7：协议级批量读 + 超帧上限自动分片（超限回退）
// ============================================================

#[test]
fn test_plan_batches_merges_adjacent_within_gap() {
    // 相邻/小空洞（不超过 gap）点位合并进同一帧，减少事务数
    let items = [(100u32, 1u16), (101, 1), (105, 1)];
    let batches = plan_batches(&items, MAX_WORDS_PER_FRAME, 8);
    assert_eq!(batches.len(), 1, "相邻点位应合并为单帧");
    assert_eq!(batches[0].start, 100);
    assert_eq!(batches[0].units, 6, "应覆盖 D100..=D105 共 6 字");
    assert_eq!(batches[0].range, 0..3);

    // 空洞超过 gap：各自成帧，避免读取大片无谓区间
    let items = [(100u32, 1u16), (200, 1)];
    let batches = plan_batches(&items, MAX_WORDS_PER_FRAME, 8);
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].start, 100);
    assert_eq!(batches[0].units, 1);
    assert_eq!(batches[1].start, 200);
}

#[test]
fn test_plan_batches_splits_over_frame_limit() {
    // 480 个相邻 u16 点：单元数 480 超过单帧上限 479，自动分片为 2 帧
    let items: Vec<(u32, u16)> = (0..480).map(|i| (i as u32, 1u16)).collect();
    let batches = plan_batches(&items, MAX_WORDS_PER_FRAME, 8);
    assert_eq!(batches.len(), 2, "超出单帧上限应回退为多帧");
    assert!(
        batches.iter().all(|b| b.units <= MAX_WORDS_PER_FRAME),
        "每个分片都不得超过单帧上限"
    );
    assert_eq!(batches[0].units, MAX_WORDS_PER_FRAME);
    assert_eq!(batches[1].start, MAX_WORDS_PER_FRAME as u32);
    assert_eq!(batches[1].units, 1);
}

/// 请求感知的 MC 字元件从站桩：按请求的首地址与点数回显「每字 = 首地址 + 序号」，
/// 并统计收到的请求帧数（用于验证批量合并 / 超限分片）。
async fn spawn_mc_word_echo_stub() -> (String, Arc<AtomicUsize>) {
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

/// 契约三：相邻点位合并为单次事务（3 个相邻点 → 1 帧）
#[tokio::test]
async fn batch_read_merges_adjacent_into_one_transaction() {
    let (addr, reqs) = spawn_mc_word_echo_stub().await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let pts = vec![
        point("p1", "D100", "u16"),
        point("p2", "D101", "u16"),
        point("p3", "D102", "u16"),
    ];

    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 3);
    for (i, r) in rs.iter().enumerate() {
        let dp = r
            .as_ref()
            .unwrap_or_else(|e| panic!("D10{} 读取失败: {e:?}", i));
        assert_eq!(
            dp.raw_value,
            (100 + i) as f64,
            "D10{} 值应为 {}",
            i,
            100 + i
        );
    }
    assert_eq!(reqs.load(Ordering::SeqCst), 1, "相邻点位应合并为单帧事务");
}

/// 契约四：单帧超限时自动分片（480 个相邻点 → 2 帧，且无点位失败）
#[tokio::test]
async fn batch_read_splits_when_over_frame_limit() {
    let (addr, reqs) = spawn_mc_word_echo_stub().await;
    let c = McCollector::new(addr, "mc-bus".into(), Duration::from_secs(5), None);
    let pts: Vec<PointConfig> = (0..480)
        .map(|i| point(&format!("p{i}"), &format!("D{i}"), "u16"))
        .collect();

    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 480);
    for (i, r) in rs.iter().enumerate() {
        let dp = r
            .as_ref()
            .unwrap_or_else(|e| panic!("D{i} 读取失败: {e:?}"));
        assert_eq!(dp.raw_value, i as f64, "D{i} 值应为 {i}");
    }
    assert_eq!(
        reqs.load(Ordering::SeqCst),
        2,
        "480 字超单帧上限 479，应回退为 2 帧"
    );
}

/// 契约五：1E 帧单帧上限 256 → 300 个相邻点应回退为 2 帧
#[tokio::test]
async fn batch_read_1e_splits_at_256_units() {
    let (addr, reqs) = spawn_mc_1e_stub().await;
    let c = McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        Some("1e".into()),
    );
    let pts: Vec<PointConfig> = (0..300)
        .map(|i| point(&format!("p{i}"), &format!("D{i}"), "u16"))
        .collect();

    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 300);
    for (i, r) in rs.iter().enumerate() {
        let dp = r
            .as_ref()
            .unwrap_or_else(|e| panic!("D{i} 读取失败: {e:?}"));
        assert_eq!(dp.raw_value, i as f64, "D{i} 值应为 {i}");
    }
    assert_eq!(
        reqs.load(Ordering::SeqCst),
        2,
        "300 字超 1E 单帧上限 256，应回退为 2 帧"
    );
}
