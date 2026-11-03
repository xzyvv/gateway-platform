//! 决策表 #21：Who-Is / I-Am 发现、SubscribeCOV / COVNotification 与 COV 缓存测试。

use std::sync::atomic::AtomicUsize;

use super::*;

// ============================================================
// 决策表 #21：Who-Is / I-Am 设备发现 + SubscribeCOV / COVNotification
// ============================================================

/// Golden：全局广播 Who-Is 请求 APDU 必须为 0x10 0x08
#[test]
fn golden_bacnet_who_is_broadcast() {
    let apdu = frames::build_who_is_apdu(None, None);
    crate::south::protocol::golden::assert_golden("bacnet/who_is_broadcast.hex", &apdu);
}

/// 契约：带设备实例范围的 Who-Is 请求须携带 [0]/[1] 两个 3 字节 context tag
#[test]
fn who_is_range_apdu_bytes() {
    let apdu = frames::build_who_is_apdu(Some(1), Some(100));
    assert_eq!(
        apdu,
        vec![
            0x10, 0x08, // Unconfirmed-Request, Who-Is(8)
            0x0B, 0x00, 0x00, 0x01, // ctx tag 0 (len 3): low limit 1
            0x1B, 0x00, 0x00, 0x64, // ctx tag 1 (len 3): high limit 100
        ],
        "Who-Is 范围请求须逐字节符合 BACnet 规范"
    );
}

/// 契约：I-Am 解析须还原设备实例号、maxAPDU 与厂商 ID
#[test]
fn parse_i_am_apdu_decodes() {
    let apdu = build_i_am_apdu(1, 15);
    let info = frames::parse_i_am_apdu(&apdu).expect("I-Am 应可解析");
    assert_eq!(info.device_instance, 1);
    assert_eq!(info.max_apdu, 1024);
    assert_eq!(info.vendor_id, 15);
}

/// 契约：非 Device 对象 / 非 I-Am 服务的报文必须拒绝
#[test]
fn parse_i_am_apdu_rejects_wrong_object_and_service() {
    // 以 AI:1（object type 0）冒充 Device：改写 objectIdentifier 的 4 字节
    let mut bad = build_i_am_apdu(1, 15);
    bad[3] = 0x00;
    bad[4] = 0x00;
    bad[5] = 0x00;
    bad[6] = 0x01;
    assert!(
        frames::parse_i_am_apdu(&bad).is_err(),
        "非 Device 对象应拒绝"
    );

    // 服务号不是 I-Am(0)
    let mut wrong_svc = build_i_am_apdu(1, 15);
    wrong_svc[1] = SERVICE_WHO_IS;
    assert!(
        frames::parse_i_am_apdu(&wrong_svc).is_err(),
        "非 I-Am 服务应拒绝"
    );
}

/// Golden：SubscribeCOV 请求 APDU（AI:1，invoke=3，subscriber=1，confirmed，lifetime=300）
#[test]
fn golden_bacnet_subscribe_cov_ai1() {
    let apdu = frames::build_subscribe_cov_apdu(OBJECT_ANALOG_INPUT, 1, 3, 1, true, Some(300));
    crate::south::protocol::golden::assert_golden("bacnet/subscribe_cov_ai1.hex", &apdu);
    assert_eq!(
        apdu,
        vec![
            0x00, 0x02, 0x03, 0x1B, // Confirmed-Request, invoke=3, SubscribeCOV(27)
            0x09, 0x01, // ctx tag 0 (len 1): subscriberProcessIdentifier=1
            0x1C, 0x00, 0x00, 0x00, 0x01, // ctx tag 1 (len 4): monitoredObject AI:1
            0x29, 0x01, // ctx tag 2 (len 1): issueConfirmedNotifications=true
            0x3A, 0x01, 0x2C, // ctx tag 3 (len 2): lifetime=300
        ],
        "SubscribeCOV 请求须逐字节符合 BACnet 规范"
    );
}

/// 契约：SubscribeCOV 仅接受匹配 invoke id 的 SimpleACK，其余一律判失败
#[test]
fn parse_subscribe_cov_ack_accepts_and_rejects() {
    assert!(frames::parse_subscribe_cov_ack(&[0x20, 0x03, 0x1B], 3).is_ok());
    assert!(
        frames::parse_subscribe_cov_ack(&[0x20, 0x04, 0x1B], 3).is_err(),
        "invoke id 不匹配应拒绝"
    );
    assert!(
        frames::parse_subscribe_cov_ack(&[0x20, 0x03, 0x0C], 3).is_err(),
        "服务号不匹配应拒绝"
    );
    for b in [0x50u8, 0x60, 0x70] {
        assert!(
            frames::parse_subscribe_cov_ack(&[b, 0x03, 0x1B], 3).is_err(),
            "Error/Reject/Abort 应判失败"
        );
    }
}

/// 契约：COVNotification 解析须还原监视对象、订阅者与最新值
#[test]
fn parse_cov_notification_decodes() {
    let apdu = vec![
        0x10, 0x02, // Unconfirmed-Request, COVNotification(2)
        0x09, 0x01, // subscriberProcessIdentifier = 1
        0x1C, 0x02, 0x00, 0x00, 0x01, // initiatingDevice = Device:1
        0x2C, 0x00, 0x00, 0x00, 0x01, // monitoredObject = AI:1
        0x39, 0x1E, // timeRemaining = 30
        0x4E, // opening tag 4 (listOfValues)
        0x09, 0x55, // propertyIdentifier[0] = Present_Value(85)
        0x44, 0x3F, 0x00, 0x00, 0x00, // REAL(0.5)
        0x4F, // closing tag 4
    ];
    let n = frames::parse_cov_notification_apdu(&apdu).expect("COVNotification 应可解析");
    assert_eq!(n.subscriber_process_id, 1);
    assert_eq!(n.initiating_device_id, (OBJECT_DEVICE as u32) << 22 | 1);
    assert_eq!(n.monitored_object_id, 1);
    assert_eq!(n.time_remaining, 30);
    assert!((n.value - 0.5).abs() < 1e-6, "值应为 0.5，实际 {}", n.value);
    assert_eq!(n.quality, Quality::Good);
}

/// 契约：COV 监听 socket 的数据报须正确分派为 推送 / ACK / 拒绝 / 忽略
#[test]
fn classify_cov_datagram_dispatches() {
    let packet = frames::wrap_bvlc_npdu(&build_cov_notification_apdu(1, 7.5));
    match classify_cov_datagram(&packet) {
        CovEvent::Notify(n) => {
            assert_eq!(n.monitored_object_id, 1);
            assert!((n.value - 7.5).abs() < 1e-6, "推送值应为 7.5");
        }
        _ => panic!("COVNotification 应分派为 Notify"),
    }

    let ack = frames::wrap_bvlc_npdu(&[0x20, 0x05, SERVICE_SUBSCRIBE_COV]);
    match classify_cov_datagram(&ack) {
        CovEvent::Ack(5) => {}
        _ => panic!("匹配服务号的 SimpleACK 应分派为 Ack"),
    }

    let rejected = frames::wrap_bvlc_npdu(&[0x50, 0x06, SERVICE_SUBSCRIBE_COV, 0x09, 0x02]);
    match classify_cov_datagram(&rejected) {
        CovEvent::Rejected(6) => {}
        _ => panic!("Error PDU 应分派为 Rejected"),
    }

    // 非 SimpleACK 服务号 / BVLC-Result / 过短数据报一律忽略
    let wrong_svc = frames::wrap_bvlc_npdu(&[0x20, 0x07, SERVICE_READ_PROPERTY]);
    assert!(matches!(
        classify_cov_datagram(&wrong_svc),
        CovEvent::Ignored
    ));
    assert!(matches!(
        classify_cov_datagram(&[0x81, 0x00, 0x00, 0x04]),
        CovEvent::Ignored
    ));
    assert!(matches!(classify_cov_datagram(&[0x81]), CovEvent::Ignored));
}

/// 端到端：diag 定向 Who-Is → 收到 I-Am 应报告 ok 且含设备实例号/厂商 ID
#[tokio::test]
async fn diag_reports_device_via_who_is_i_am() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();
    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-diag".to_string(),
        Duration::from_millis(500),
    );
    collector.connect().await.unwrap();

    let server_task = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert!(n >= 8, "Who-Is 请求数据报应含 BVLC(4)+NPDU(2)+APDU(2)");
        assert_eq!(&buf[6..8], &[APDU_UNCONFIRMED_REQUEST, SERVICE_WHO_IS]);
        let reply = frames::wrap_bvlc_npdu(&build_i_am_apdu(1, 15));
        server.send_to(&reply, from).await.unwrap();
    });

    let d = collector.diag().await;
    server_task.await.unwrap();
    assert!(d.contains("ok"), "diag 应为 ok，实际: {d}");
    assert!(d.contains("device 1"), "diag 应含设备实例号，实际: {d}");
    assert!(d.contains("vendor 15"), "diag 应含厂商 ID，实际: {d}");
}

/// 端到端：subscribe_cov 收到匹配 SimpleACK 应判订阅成功
#[tokio::test]
async fn subscribe_cov_end_to_end_simple_ack() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();
    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-cov".to_string(),
        Duration::from_millis(500),
    );
    collector.connect().await.unwrap();

    let server_task = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert!(n >= 10, "请求数据报应含 BVLC(4)+NPDU(2)+APDU 头");
        assert_eq!(buf[9], SERVICE_SUBSCRIBE_COV, "服务号应为 SubscribeCOV(27)");
        let invoke = buf[8]; // APDU[2] = invoke id
        let ack = frames::wrap_bvlc_npdu(&[0x20, invoke, SERVICE_SUBSCRIBE_COV]);
        server.send_to(&ack, from).await.unwrap();
    });

    collector
        .subscribe_cov(OBJECT_ANALOG_INPUT, 1, 1, true, Some(300))
        .await
        .expect("订阅应成功");
    server_task.await.unwrap();
}

/// 端到端：COV 订阅生效后 read 直接吃推送缓存，不再发 ReadProperty。
///
/// 假设备对 SubscribeCOV 回 SimpleACK 并立即推送一次 COVNotification（7.5）。
/// 首读仍走 ReadProperty（返回 42.0）；待推送进入缓存后，后续 read 必须返回
/// 推送值，且 ReadProperty 计数不再增长——这正是"推送式采集、省轮询"的收益点。
#[tokio::test]
async fn read_serves_cached_value_after_cov_notification() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-cov-cache".to_string(),
        Duration::from_millis(300),
    );

    let read_hits = Arc::new(AtomicUsize::new(0));
    let hits = read_hits.clone();
    let stub = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (n, from) = match tokio::time::timeout(remaining, server.recv_from(&mut buf)).await
            {
                Ok(Ok(v)) => v,
                _ => break,
            };
            if n < 10 || buf[0] != 0x81 {
                continue;
            }
            let invoke = buf[8];
            match buf[9] {
                SERVICE_READ_PROPERTY => {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let ack = build_read_property_ack(invoke, 42.0);
                    server.send_to(&ack, from).await.unwrap();
                }
                SERVICE_SUBSCRIBE_COV => {
                    let ack = frames::wrap_bvlc_npdu(&[0x20, invoke, SERVICE_SUBSCRIBE_COV]);
                    server.send_to(&ack, from).await.unwrap();
                    let note = frames::wrap_bvlc_npdu(&build_cov_notification_apdu(1, 7.5));
                    server.send_to(&note, from).await.unwrap();
                }
                _ => {}
            }
        }
    });

    let point = PointConfig {
        id: "cov1".to_string(),
        address: "analog-input:1".to_string(),
        ..Default::default()
    };

    // 首读：COV 尚未建立，走主动 ReadProperty
    let first = collector.read(&point).await.expect("首读应成功");
    assert!(
        (first.value - 42.0).abs() < 1e-6,
        "首读应先取到轮询值 42.0，实际 {}",
        first.value
    );

    // 等订阅确认 + 推送落缓存（COV_TICK 为 500ms，留足余量）
    let mut pushed = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let dp = collector.read(&point).await.expect("读应成功");
        if (dp.value - 7.5).abs() < 1e-6 {
            pushed = Some(dp);
            break;
        }
    }
    let pushed = pushed.expect("2s 内应从 COV 推送缓存取到 7.5");

    // 缓存命中后：连续读不再产生 ReadProperty
    let before = read_hits.load(Ordering::SeqCst);
    let again = collector.read(&point).await.expect("缓存读应成功");
    assert!(
        (again.value - 7.5).abs() < 1e-6,
        "缓存应持续返回推送值 7.5，实际 {}",
        again.value
    );
    assert_eq!(
        read_hits.load(Ordering::SeqCst),
        before,
        "COV 缓存命中后不应再发 ReadProperty"
    );
    assert_eq!(pushed.quality, Quality::Good);

    collector.disconnect().await;
    stub.await.unwrap();
}
