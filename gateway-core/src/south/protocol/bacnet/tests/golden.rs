//! Golden 报文回归与 ReadProperty 取值解析测试。

use super::*;

// =========================================================================
// Golden 报文回归（决策表 #10）
//
// APDU 逐字节与 `tests/fixtures/golden/bacnet/*.hex` 比对；ACK 样本做解码断言。
// =========================================================================

/// WriteProperty 请求 APDU（AI:1 ← 23.5，invoke=7）
#[test]
fn golden_bacnet_write_property_ai1() {
    let apdu = frames::build_write_property_apdu(OBJECT_ANALOG_INPUT, 1, 7, 23.5);
    crate::south::protocol::golden::assert_golden("bacnet/write_property_ai1.hex", &apdu);
}

/// RPM 请求 APDU（AI:1 + AV:100，invoke=5）
#[test]
fn golden_bacnet_rpm_request_ai1_av100() {
    let apdu =
        frames::build_rpm_request_apdu(&[(OBJECT_ANALOG_INPUT, 1), (OBJECT_ANALOG_VALUE, 100)], 5);
    crate::south::protocol::golden::assert_golden("bacnet/rpm_request_ai1_av100.hex", &apdu);
}

/// RPM-ACK 解码：样本必须还原两个对象的值
#[test]
fn golden_bacnet_rpm_ack_decode() {
    let packet = build_rpm_ack(
        5,
        &[
            (OBJECT_ANALOG_INPUT, 1, 12.5),
            (OBJECT_ANALOG_VALUE, 100, -3.25),
        ],
    );
    let apdu = BacnetCollector::skip_npdu(&packet[4..]).expect("剥离 BVLC/NPDU 失败");
    crate::south::protocol::golden::assert_golden("bacnet/rpm_ack_two_objects.hex", apdu);

    let loaded = crate::south::protocol::golden::load("bacnet/rpm_ack_two_objects.hex");
    let list = frames::parse_rpm_ack(&loaded, 5).expect("golden RPM-ACK 样本必须可解析");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].0, (OBJECT_ANALOG_INPUT as u32) << 22 | 1);
    assert!((list[0].1 - 12.5).abs() < 1e-6);
    assert_eq!(list[1].0, (OBJECT_ANALOG_VALUE as u32) << 22 | 100);
    assert!((list[1].1 + 3.25).abs() < 1e-6);
}

/// WriteProperty SimpleACK 解码：必须判写入成功
#[test]
fn golden_bacnet_write_property_ack_decode() {
    let apdu = vec![0x20, 0x07, 0x0F]; // SimpleACK + invoke 7 + WriteProperty(15)
    crate::south::protocol::golden::assert_golden("bacnet/write_property_simple_ack.hex", &apdu);
    let loaded = crate::south::protocol::golden::load("bacnet/write_property_simple_ack.hex");
    frames::parse_write_property_ack(&loaded, 7).expect("golden SimpleACK 必须判成功");
}

#[test]
fn test_parse_object_address() {
    let (t, i) = BacnetCollector::parse_object_address("analog-input:1").unwrap();
    assert_eq!(t, OBJECT_ANALOG_INPUT);
    assert_eq!(i, 1);
}

#[test]
fn test_parse_object_address_numeric() {
    let (t, i) = BacnetCollector::parse_object_address("2:100").unwrap();
    assert_eq!(t, OBJECT_ANALOG_VALUE);
    assert_eq!(i, 100);
}

#[test]
fn test_parse_real() {
    // Standard: (4 << 4) | TAG_REAL(4) = 0x44, then 0.5 f32 big-endian
    let bytes = vec![(4 << 4) | TAG_REAL, 0x3F, 0x00, 0x00, 0x00];
    let (v, _) = frames::parse_app_value(&bytes).unwrap();
    assert!((v - 0.5).abs() < 1e-6);
}

#[test]
fn test_parse_unsigned() {
    // Standard: (TAG_UNSIGNED(2) << 4) | 1 = 0x21, then 100
    let bytes = vec![(TAG_UNSIGNED << 4) | 1, 0x64];
    let (v, _) = frames::parse_app_value(&bytes).unwrap();
    assert_eq!(v, 100.0);
}

#[test]
fn test_parse_value_from_apdu_opening_tag() {
    // 标准 BACnet ReadProperty-ACK 格式，使用 opening/closing tag 对
    // ComplexACK(0x30) + invoke ID(1) + service choice(12) +
    // context tag 0 (Object ID, len=4) + context tag 1 (Property ID, len=1) +
    // context tag 3 opening + application tag (Real, 4 bytes) + context tag 3 closing
    let apdu = vec![
        0x30, 0x01, 0x0C, // ComplexACK, invoke=1, service=12
        0x0C, 0x00, 0x00, 0x00, 0x01, // ctx tag 0: Object ID (AI:1, len 4)
        0x19, 0x55, // ctx tag 1: Property ID=85 (len 1)
        0x3E, // ctx tag 3 opening
        0x44, 0x3F, 0x00, 0x00, 0x00, // Real 0.5
        0x3F, // ctx tag 3 closing
    ];
    let (v, q) = frames::parse_value_from_apdu(&apdu, 1).unwrap();
    assert!((v - 0.5).abs() < 1e-6);
    assert_eq!(q, Quality::Good);
}

#[test]
fn test_parse_value_from_apdu_unsigned_opening_tag() {
    // 测试 opening tag + Unsigned 值
    let apdu = vec![
        0x30, 0x02, 0x0C, // ComplexACK, invoke=2, service=12
        0x0C, 0x00, 0x00, 0x00, 0x01, // ctx tag 0: Object ID (AI:1, len 4)
        0x19, 0x55, // ctx tag 1: Property ID=85 (len 1)
        0x3E, // ctx tag 3 opening
        0x21, 0x64, // Unsigned(2), length=1, value=100
        0x3F, // ctx tag 3 closing
    ];
    let (v, _) = frames::parse_value_from_apdu(&apdu, 2).unwrap();
    assert_eq!(v, 100.0);
}

#[test]
fn test_build_read_property_apdu() {
    let apdu = BacnetCollector::build_read_property_apdu(OBJECT_ANALOG_INPUT, 1, 5);
    // PDU type 0x00, max APDU 0x02, invoke 5, service 12
    assert_eq!(apdu[0], 0x00);
    assert_eq!(apdu[1], 0x02);
    assert_eq!(apdu[2], 5);
    assert_eq!(apdu[3], SERVICE_READ_PROPERTY);
}

/// 契约：apdu_invoke_id 必须从第 2 字节提取 invoke id
#[test]
fn test_apdu_invoke_id_extracts() {
    assert_eq!(frames::apdu_invoke_id(&[0x30, 0x07, 0x0C]), Some(7));
    assert_eq!(frames::apdu_invoke_id(&[0x30]), None);
    assert_eq!(frames::apdu_invoke_id(&[]), None);
}

/// 契约：invoke id 不匹配的响应必须被拒绝（防止迟到/残留回包错配）
#[test]
fn test_parse_value_from_apdu_rejects_invoke_id_mismatch() {
    let apdu = vec![
        0x30, 0x09, 0x0C, // ComplexACK, invoke=9
        0x3E, 0x44, 0x3F, 0x00, 0x00, 0x00, 0x3F,
    ];
    let err = frames::parse_value_from_apdu(&apdu, 1).expect_err("invoke id 不匹配应报错");
    assert!(
        format!("{err}").contains("invoke id mismatch"),
        "错误信息应指明 invoke id 不匹配，实际: {err}"
    );
}

/// 契约：非 ReadProperty 服务的响应必须被拒绝
#[test]
fn test_parse_value_from_apdu_rejects_wrong_service() {
    let apdu = vec![
        0x30, 0x01, 0x0E, // ComplexACK, invoke=1, service=14 (非 ReadProperty)
        0x3E, 0x44, 0x3F, 0x00, 0x00, 0x00, 0x3F,
    ];
    let err = frames::parse_value_from_apdu(&apdu, 1).expect_err("服务选择不符应报错");
    assert!(
        format!("{err}").contains("service choice"),
        "错误信息应指明服务选择异常，实际: {err}"
    );
}

/// 契约：接收路径须按 invoke id 归位——invoke id 错配的残留/迟到响应
/// 不得被当作当前请求的结果返回（须跳过并取到匹配响应）。
#[tokio::test]
async fn read_ignores_stale_response_with_mismatched_invoke_id() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-test".to_string(),
        Duration::from_millis(500),
    );
    collector.connect().await.unwrap();
    let local = {
        let guard = collector.socket.lock().await;
        guard.as_ref().unwrap().local_addr().unwrap()
    };

    // 服务器响应任务：先发一个 invoke id 错配的迟到回包（值 111.0），
    // 等 collector 发出请求后再回正确响应（invoke id=1，值 42.0）
    let server_task = tokio::spawn(async move {
        server
            .send_to(&build_read_property_ack(99, 111.0), local)
            .await
            .unwrap();
        let mut buf = [0u8; 1500];
        let _ = server.recv_from(&mut buf).await.unwrap();
        server
            .send_to(&build_read_property_ack(1, 42.0), local)
            .await
            .unwrap();
    });

    let point = PointConfig {
        id: "p1".to_string(),
        address: "analog-input:1".to_string(),
        ..Default::default()
    };
    let dp = collector.read(&point).await.unwrap();
    server_task.await.unwrap();
    assert!(
        (dp.value - 42.0).abs() < 1e-6,
        "应返回 invoke id 匹配的正确值 42.0，而非陈旧值 {}",
        dp.value
    );
}
