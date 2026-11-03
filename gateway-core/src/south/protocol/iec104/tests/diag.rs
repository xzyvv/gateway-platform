//! P4：IEC104 连通性诊断（TCP 建链 + STARTDT + 总召唤）
//!
//! 诊断不能只看 TCP 是否可连——现场大量「TCP 通但采不到数」是 STARTDT 未确认
//! 或公共地址/IOA 配错导致总召唤空返回。这里用最小从站桩验证两种结果：
//! 1. 总召唤正常 → 报告返回的 IOA 数量；
//! 2. 总召唤无应答 → 如实报告「TCP 已连接但无数据」，不冒充成功。
use super::*;

/// 假从站：完成 STARTDT 握手 + 对时确认，然后按 `data_asdu` 决定是否回应总召唤。
///
/// `data_asdu = Some(...)` 时先回一帧数据、再回总召唤激活终止；
/// `None` 时对总召唤保持沉默（模拟从站不接受总召唤）。
async fn fake_slave(
    listener: tokio::net::TcpListener,
    data_asdu: Option<Vec<u8>>,
    keep_alive: Duration,
) {
    let Ok((mut stream, _)) = listener.accept().await else {
        return;
    };
    // 1) STARTDT act → con
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    if stream
        .write_all(&build_uframe([0x0B, 0x00, 0x00, 0x00]))
        .await
        .is_err()
    {
        return;
    }
    // 2) 建链对时 → 回一帧 I 帧确认（内容不参与断言）
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    let ack = build_iframe(0, 1, &[0x64, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00])
        .expect("I-frame 构造失败");
    if stream.write_all(&ack).await.is_err() {
        return;
    }
    // 3) 总召唤
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    if let Some(asdu) = data_asdu {
        // N(S)=1 的数据帧 + N(S)=2 的总召唤激活终止
        let data = build_iframe(1, 2, &asdu).expect("数据 I-frame 构造失败");
        if stream.write_all(&data).await.is_err() {
            return;
        }
        let term = build_iframe(2, 2, &[0x64, 0x01, 0x0A, 0x00, 0x01, 0x00])
            .expect("终止 I-frame 构造失败");
        if stream.write_all(&term).await.is_err() {
            return;
        }
    }
    // 保持连接，避免对端读帧拿到 EOF 提前结束诊断窗口
    tokio::time::sleep(keep_alive).await;
}

/// 一帧 M_ME_NA_1（类型 9）数据：CA=1，IOA=1001，归一化值 0，质量 Good
fn normalized_measure_asdu() -> Vec<u8> {
    vec![
        0x09, 0x01, 0x14, 0x00, // 类型 / 1 个对象 / COT=20（响应总召唤）
        0x01, 0x00, // 公共地址 CA=1（2 字节）
        0xE9, 0x03, 0x00, // IOA=1001（3 字节小端）
        0x00, 0x00, 0x00, // 归一化值 + QDS（Good）
    ]
}

#[tokio::test]
async fn diag_reports_connected_and_ioa_count() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假从站端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    let server = tokio::spawn(fake_slave(
        listener,
        Some(normalized_measure_asdu()),
        Duration::from_millis(500),
    ));

    let collector = Iec104Collector::new(
        addr,
        "bus_iec104_diag".to_string(),
        Duration::from_millis(800),
        2,
    );

    let detail = collector.diag().await;
    assert!(
        detail.starts_with("iec104@bus_iec104_diag:"),
        "诊断须带协议与总线标识，实际: {detail}"
    );
    assert!(detail.contains("TCP 已连接"), "实际: {detail}");
    assert!(
        detail.contains("1 个 IOA"),
        "总召唤应报告 IOA 数量，实际: {detail}"
    );
    assert!(
        detail.contains("STARTDT 已确认"),
        "诊断须覆盖 STARTDT，而非只报 TCP 连通，实际: {detail}"
    );

    server.abort();
}

#[tokio::test]
async fn diag_reports_tcp_ok_but_no_data_honestly() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假从站端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    // 从站不接受总召唤：只完成握手与对时，此后沉默
    let server = tokio::spawn(fake_slave(listener, None, Duration::from_millis(500)));

    let collector = Iec104Collector::new(
        addr,
        "bus_iec104_silent".to_string(),
        Duration::from_millis(250),
        2,
    );

    let detail = collector.diag().await;
    assert!(detail.contains("TCP 已连接"), "实际: {detail}");
    assert!(
        detail.contains("未返回任何数据"),
        "总召唤无应答必须如实报告，不得冒充成功，实际: {detail}"
    );

    server.abort();
}
