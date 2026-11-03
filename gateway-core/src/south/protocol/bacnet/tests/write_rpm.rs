//! 决策表 #20：WriteProperty / ReadPropertyMultiple 读写测试。

use super::*;

// ============================================================
// 决策表 #20：WriteProperty（15）/ ReadPropertyMultiple（14）
// ============================================================

/// 契约：WriteProperty 请求 APDU 必须逐字节符合规范
///
/// AI:1 写 23.5 ⇒ Confirmed-Request(0x00)+MaxSeg/MaxAPDU(0x02)+invoke(7)+
/// service 15 + objectId[0] + propertyId[1]=85 + opening tag3 + REAL(23.5) +
/// closing tag3 + priority[4]=8。
#[test]
fn write_property_apdu_golden_bytes() {
    let apdu = frames::build_write_property_apdu(OBJECT_ANALOG_INPUT, 1, 7, 23.5);
    assert_eq!(
        apdu,
        vec![
            0x00, 0x02, 0x07, 0x0F, // Confirmed-Request, invoke=7, WriteProperty(15)
            0x0C, 0x00, 0x00, 0x00,
            0x01, // objectIdentifier[0] = (0<<22)|1 (ctx tag 0, len 4)
            0x19, 0x55, // propertyIdentifier[1] = Present_Value(85) (ctx tag 1, len 1)
            0x3E, // ctx tag 3 opening
            0x44, 0x41, 0xBC, 0x00, 0x00, // REAL(23.5) 大端
            0x3F, // ctx tag 3 closing
            0x49, 0x08, // priority[4] len 1 = 8
        ],
        "WriteProperty 请求必须逐字节符合 BACnet 规范"
    );
}

/// 契约：二进制对象（BI/BO/BV）写入值必须用 Enumerated(0/1) 而非 REAL
#[test]
fn write_property_binary_value_is_enumerated() {
    // 真值 ⇒ Enumerated(1)：0x91 0x01
    let apdu = frames::build_write_property_apdu(OBJECT_BINARY_OUTPUT, 5, 2, 1.0);
    assert_eq!(
        &apdu[apdu.len() - 5..],
        &[0x91, 0x01, 0x3F, 0x49, 0x08],
        "BO 写入真值应为 Enumerated(1)，实际尾段 {:02X?}",
        &apdu[apdu.len() - 5..]
    );
    // 假值 ⇒ Enumerated(0)：0x91 0x00
    let apdu0 = frames::build_write_property_apdu(OBJECT_BINARY_OUTPUT, 5, 2, 0.0);
    assert_eq!(
        &apdu0[apdu0.len() - 5..],
        &[0x91, 0x00, 0x3F, 0x49, 0x08],
        "BO 写入假值应为 Enumerated(0)，实际尾段 {:02X?}",
        &apdu0[apdu0.len() - 5..]
    );
}

/// 契约：BVLC+NPDU 包裹必须正确填充总长度字段
#[test]
fn wrap_bvlc_npdu_sets_total_length() {
    let apdu = frames::build_write_property_apdu(OBJECT_ANALOG_VALUE, 3, 1, 1.0);
    let pkt = frames::wrap_bvlc_npdu(&apdu);
    assert_eq!(pkt[0], 0x81, "BVLC type 应为 0x81");
    assert_eq!(pkt[1], 0x0a, "BVLC function 应为 Original-Unicast-NPDU");
    let total = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    assert_eq!(total, 2 + apdu.len() + 4, "BVLC 长度字段 = NPDU + APDU + 4");
    assert_eq!(&pkt[4..6], &[0x01, 0x00], "NPDU 头应为 version=1,control=0");
    assert_eq!(&pkt[6..], apdu.as_slice(), "APDU 须紧随 NPDU 头");
}

/// 契约：WriteProperty 仅接受 SimpleACK/ComplexACK + 匹配 invoke id + service 15
#[test]
fn parse_write_property_ack_accepts_simple_and_complex() {
    assert!(frames::parse_write_property_ack(&[0x20, 0x07, 0x0F], 7).is_ok());
    assert!(frames::parse_write_property_ack(&[0x30, 0x07, 0x0F], 7).is_ok());
}

/// 契约：Error/Reject/Abort 一律判写入失败，绝不当作成功
#[test]
fn parse_write_property_ack_rejects_error_reject_abort() {
    for (b, kind) in [(0x50u8, "Error"), (0x60, "Reject"), (0x70, "Abort")] {
        let apdu = vec![b, 0x07, 0x0F];
        let err =
            frames::parse_write_property_ack(&apdu, 7).expect_err("Error/Reject/Abort 必须判失败");
        assert!(
            format!("{err}").contains(kind),
            "错误信息应包含 {}，实际: {}",
            kind,
            err
        );
    }
}

/// 契约：WriteProperty ACK 的 invoke id / 服务号不匹配必须拒绝
#[test]
fn parse_write_property_ack_rejects_mismatch() {
    let err = frames::parse_write_property_ack(&[0x20, 0x09, 0x0F], 7)
        .expect_err("invoke id 不匹配应拒绝");
    assert!(
        format!("{err}").contains("invoke id mismatch"),
        "实际: {err}"
    );
    let err2 =
        frames::parse_write_property_ack(&[0x20, 0x07, 0x0C], 7).expect_err("服务号不匹配应拒绝");
    assert!(format!("{err2}").contains("service choice"), "实际: {err2}");
}

/// 契约：RPM 请求必须为每个对象生成一条 ReadAccessSpecification
#[test]
fn rpm_request_apdu_golden_bytes() {
    let apdu =
        frames::build_rpm_request_apdu(&[(OBJECT_ANALOG_INPUT, 1), (OBJECT_ANALOG_VALUE, 100)], 5);
    assert_eq!(
        apdu,
        vec![
            0x00, 0x02, 0x05, 0x0E, // Confirmed-Request, invoke=5, ReadPropertyMultiple(14)
            0x0C, 0x00, 0x00, 0x00, 0x01, // obj[0] = AI:1 (ctx tag 0, len 4)
            0x1E, 0x09, 0x55, 0x1F, // listOfPropertyReferences: opening 1, prop=85, closing 1
            0x0C, 0x00, 0x80, 0x00, 0x64, // obj[1] = AV:100
            0x1E, 0x09, 0x55, 0x1F,
        ],
        "RPM 请求须逐对象生成 ReadAccessSpecification"
    );
}

/// 契约：RPM-ACK 必须解析出每个对象的 (对象 ID, 值)，且 ID 计算符合 (type<<22)|instance
#[test]
fn parse_rpm_ack_roundtrip() {
    let packet = build_rpm_ack(
        5,
        &[
            (OBJECT_ANALOG_INPUT, 1, 12.5),
            (OBJECT_ANALOG_VALUE, 100, -3.25),
        ],
    );
    let apdu = BacnetCollector::skip_npdu(&packet[4..]).unwrap();
    let list = frames::parse_rpm_ack(apdu, 5).unwrap();
    assert_eq!(list.len(), 2, "应解析出两个对象结果");
    assert_eq!(list[0].0, (OBJECT_ANALOG_INPUT as u32) << 22 | 1);
    assert!(
        (list[0].1 - 12.5).abs() < 1e-6,
        "值应为 12.5，实际 {}",
        list[0].1
    );
    assert_eq!(list[1].0, (OBJECT_ANALOG_VALUE as u32) << 22 | 100);
    assert!(
        (list[1].1 - (-3.25)).abs() < 1e-6,
        "值应为 -3.25，实际 {}",
        list[1].1
    );
}

/// 契约：RPM-ACK 的 invoke id / 服务号不匹配必须拒绝
#[test]
fn parse_rpm_ack_rejects_mismatch() {
    let packet = build_rpm_ack(5, &[(OBJECT_ANALOG_INPUT, 1, 1.0)]);
    let apdu = BacnetCollector::skip_npdu(&packet[4..]).unwrap();
    assert!(
        frames::parse_rpm_ack(apdu, 9).is_err(),
        "invoke id 不匹配应拒绝"
    );
    let mut wrong = apdu.to_vec();
    wrong[2] = SERVICE_READ_PROPERTY; // 服务号被改坏
    assert!(
        frames::parse_rpm_ack(&wrong, 5).is_err(),
        "服务号不匹配应拒绝"
    );
}

/// 端到端：WriteProperty 收到匹配 SimpleACK 应判定写入成功
#[tokio::test]
async fn write_property_end_to_end_simple_ack() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-wp".to_string(),
        Duration::from_millis(500),
    );
    collector.connect().await.unwrap();

    let server_task = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert!(n >= 10, "请求数据报应至少包含 BVLC(4)+NPDU(2)+APDU 头");
        assert_eq!(
            buf[9], SERVICE_WRITE_PROPERTY,
            "服务号应为 WriteProperty(15)"
        );
        let invoke = buf[8]; // APDU[2] = invoke id
        let ack = frames::wrap_bvlc_npdu(&[0x20, invoke, SERVICE_WRITE_PROPERTY]);
        server.send_to(&ack, from).await.unwrap();
    });

    let point = PointConfig {
        id: "wp1".to_string(),
        address: "analog-output:5".to_string(),
        ..Default::default()
    };
    collector.write(&point, 42.0).await.expect("应写入成功");
    server_task.await.unwrap();
}

/// 端到端：WriteProperty 收到 Error 响应必须判写入失败（不得静默成功）
#[tokio::test]
async fn write_property_end_to_end_rejects_error() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-wp-err".to_string(),
        Duration::from_millis(500),
    );
    collector.connect().await.unwrap();

    let server_task = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (_, from) = server.recv_from(&mut buf).await.unwrap();
        let invoke = buf[8];
        // Error APDU: 0x50 + invoke + service + errorClass(ctx tag0,len1=0x09,0x02) + errorCode(ctx tag1,len1=0x19,0x20)
        let ack =
            frames::wrap_bvlc_npdu(&[0x50, invoke, SERVICE_WRITE_PROPERTY, 0x09, 0x02, 0x19, 0x20]);
        server.send_to(&ack, from).await.unwrap();
    });

    let point = PointConfig {
        id: "wp2".to_string(),
        address: "analog-output:5".to_string(),
        ..Default::default()
    };
    let err = collector
        .write(&point, 1.0)
        .await
        .expect_err("Error 响应须判失败");
    assert!(format!("{err}").contains("Error"), "实际: {err}");
    server_task.await.unwrap();
}

/// 端到端：read_batch 用单个 RPM 事务读多点，且结果顺序与入参顺序逐位对应
#[tokio::test]
async fn read_batch_rpm_maps_results_to_input_order() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-rpm".to_string(),
        Duration::from_millis(500),
    );
    collector.connect().await.unwrap();

    // 服务器故意把响应顺序倒过来，验证按对象 ID 映射回入参顺序
    let server_task = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (_, from) = server.recv_from(&mut buf).await.unwrap();
        let invoke = buf[8];
        let ack = build_rpm_ack(
            invoke,
            &[
                (OBJECT_ANALOG_VALUE, 100, 7.0),
                (OBJECT_ANALOG_INPUT, 1, 3.0),
            ],
        );
        server.send_to(&ack, from).await.unwrap();
    });

    let points = vec![
        PointConfig {
            id: "a".to_string(),
            address: "analog-input:1".to_string(),
            ..Default::default()
        },
        PointConfig {
            id: "b".to_string(),
            address: "analog-value:100".to_string(),
            ..Default::default()
        },
    ];
    let results = collector.read_batch(&points).await;
    server_task.await.unwrap();
    assert_eq!(results.len(), 2);
    let a = results[0].as_ref().expect("a 应成功");
    let b = results[1].as_ref().expect("b 应成功");
    assert!(
        (a.value - 3.0).abs() < 1e-6,
        "a 应映射到 AI:1=3.0，实际 {}",
        a.value
    );
    assert!(
        (b.value - 7.0).abs() < 1e-6,
        "b 应映射到 AV:100=7.0，实际 {}",
        b.value
    );
}

/// 契约：read_batch 中地址非法的点位仅报单项错误，不影响其余点位
#[tokio::test]
async fn read_batch_isolates_invalid_address() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-rpm-bad".to_string(),
        Duration::from_millis(500),
    );
    collector.connect().await.unwrap();

    let server_task = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (_, from) = server.recv_from(&mut buf).await.unwrap();
        let invoke = buf[8];
        let ack = build_rpm_ack(invoke, &[(OBJECT_ANALOG_INPUT, 1, 9.0)]);
        server.send_to(&ack, from).await.unwrap();
    });

    let points = vec![
        PointConfig {
            id: "bad".to_string(),
            address: "not-an-address".to_string(),
            ..Default::default()
        },
        PointConfig {
            id: "good".to_string(),
            address: "analog-input:1".to_string(),
            ..Default::default()
        },
    ];
    let results = collector.read_batch(&points).await;
    server_task.await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].is_err(), "非法地址点位应报单项错误");
    let good = results[1].as_ref().expect("合法点位应成功");
    assert!((good.value - 9.0).abs() < 1e-6, "实际 {}", good.value);
}
