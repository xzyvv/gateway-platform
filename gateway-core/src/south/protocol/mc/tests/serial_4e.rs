//! 4E 序列号帧（决策表 #16）：序列号回显校验端到端测试。
use super::*;

use std::time::Duration;

// ============================================================
// 4E 序列号帧（决策表 #16）：序列号回显校验
// ============================================================

/// 4E 从站桩：校验请求子头 0x5400，回显请求序列号，按请求点数回显「每字 = 首地址 + 序号」。
async fn spawn_mc_4e_stub() -> (String, Arc<AtomicUsize>) {
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
                    let mut head = [0u8; 11];
                    if sock.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    // 4E 头：子头(2) 序列号(2) 网络(1) PC(1) IO(2) 站(1) 长度(2)
                    if head[0..2] != [0x54, 0x00] {
                        return;
                    }
                    let serial = u16::from_le_bytes([head[2], head[3]]);
                    let len = u16::from_le_bytes([head[9], head[10]]) as usize;
                    let mut body = vec![0u8; len];
                    if sock.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    if body.len() < 12 {
                        return;
                    }
                    let start = u32::from_le_bytes([body[7], body[8], body[9], 0]);
                    let points = u16::from_le_bytes([body[10], body[11]]) as u32;
                    let mut data = Vec::with_capacity(points as usize * 2);
                    for i in 0..points {
                        data.extend_from_slice(&((start + i) as u16).to_le_bytes());
                    }
                    reqs.fetch_add(1, Ordering::SeqCst);
                    // 4E 响应：D4 00 + 回显序列号 + 网络 PC IO 站 + 长度 + 结束代码 + 数据
                    let body_len = 2 + data.len();
                    let mut resp = vec![0xD4, 0x00];
                    resp.extend_from_slice(&serial.to_le_bytes());
                    resp.extend_from_slice(&[0x00, 0xFF, 0xFF, 0x03, 0x00]);
                    resp.extend_from_slice(&(body_len as u16).to_le_bytes());
                    resp.extend_from_slice(&0u16.to_le_bytes());
                    resp.extend_from_slice(&data);
                    if sock.write_all(&resp).await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                }
            });
        }
    });
    (addr, reqs)
}

#[tokio::test]
async fn test_e2e_4e_read_word_with_serial_echo() {
    let (addr, reqs) = spawn_mc_4e_stub().await;
    let c = McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        Some("4e".into()),
    );
    let pts = vec![point("p1", "D100", "u16"), point("p2", "D102", "u16")];
    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 2);
    assert_eq!(rs[0].as_ref().expect("p1 ok").raw_value, 100.0);
    assert_eq!(rs[1].as_ref().expect("p2 ok").raw_value, 102.0);
    assert_eq!(reqs.load(Ordering::SeqCst), 1, "相邻点位应合并为单帧");
}

/// 4E 序列号不匹配（从站回显了错误序列号）应判为帧错误。
#[tokio::test]
async fn test_4e_serial_mismatch_rejected() {
    // 从站回固定响应但序列号故意写错（0xFFFF），请求序列号为 0x0000
    let bad = {
        let data = [0x34u8, 0x12];
        let body_len = 2 + data.len();
        let mut f = vec![0xD4, 0x00, 0xFF, 0xFF, 0x00, 0xFF, 0xFF, 0x03, 0x00];
        f.extend_from_slice(&(body_len as u16).to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(&data);
        f
    };
    // 该桩需读 4E 头（11 字节），故用专用桩：读一次 11 字节头+体后回 bad 并保持
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            loop {
                let mut head = [0u8; 11];
                if sock.read_exact(&mut head).await.is_err() {
                    return;
                }
                let len = u16::from_le_bytes([head[9], head[10]]) as usize;
                let mut body = vec![0u8; len];
                if sock.read_exact(&mut body).await.is_err() {
                    return;
                }
                if sock.write_all(&bad).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
            }
        }
    });
    let c = McCollector::new(
        addr,
        "mc-bus".into(),
        Duration::from_secs(5),
        Some("4e".into()),
    );
    let p = point("p1", "D100", "u16");
    let err = c.read(&p).await.expect_err("序列号不匹配应失败");
    assert!(
        matches!(err, crate::south::protocol::CollectorError::InvalidFrame(_)),
        "unexpected: {err:?}"
    );
}
