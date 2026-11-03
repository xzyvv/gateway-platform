//! 回归：BACnet 惰性建链与设备发现去重。

use super::*;

// ============================================================
// 回归：BACnet 惰性建链（生产路径从不调用 connect()）
// ============================================================

/// 回归缺陷维度：调度器生产路径只调用 `read`/`read_batch`，从不显式 `connect()`。
/// 修复前 `read_batch` 直接取 `self.socket`（恒为 None），整批 100% 失败；
/// 既有测试都在调用前显式 `connect()`，掩盖了该缺陷。
///
/// 本测试构造 collector 后**不调用 `.connect()`**，直接 `read_batch`，
/// 断言惰性建链生效：返回 Ok、长度与入参一致、每个点位按对象 ID 正确回填。
#[tokio::test]
async fn read_batch_lazily_connects_without_explicit_connect() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();

    let collector = BacnetCollector::new(
        server_addr.to_string(),
        "bacnet-lazy".to_string(),
        Duration::from_millis(500),
    );
    // 关键：全程不调用 collector.connect()

    let server_task = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (_, from) = server.recv_from(&mut buf).await.unwrap();
        let invoke = buf[8]; // APDU[2] = invoke id
        let ack = build_rpm_ack(
            invoke,
            &[
                (OBJECT_ANALOG_INPUT, 1, 3.0),
                (OBJECT_ANALOG_VALUE, 100, 7.0),
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

    // 先断言（回退修复时此处立即失败，避免 mock 任务永久阻塞）
    assert_eq!(results.len(), 2, "返回值必须与入参等长");
    assert!(
        results[0].is_ok(),
        "未显式 connect() 时首个点位也应惰性建链成功: {:?}",
        results[0]
    );
    assert!(
        results[1].is_ok(),
        "未显式 connect() 时次个点位也应惰性建链成功: {:?}",
        results[1]
    );
    let a = results[0].as_ref().unwrap();
    let b = results[1].as_ref().unwrap();
    assert!(
        (a.value - 3.0).abs() < 1e-6,
        "a 应为 AI:1=3.0，实际 {}",
        a.value
    );
    assert!(
        (b.value - 7.0).abs() < 1e-6,
        "b 应为 AV:100=7.0，实际 {}",
        b.value
    );

    server_task.await.unwrap();
}

// ===== 设备发现去重（决策表 #21 / X4）=====
//
// X4：`discover_devices` 在 X4 之前是 `#[allow(dead_code)]`——进生产二进制
// 且零测试覆盖。它绑定 0.0.0.0:0 并广播到 255.255.255.255:47808，CI 里无法
// 可靠跑端到端；故把其中唯一的纯逻辑（按 device_instance 去重）抽成
// `frames::dedup_i_am`，在此直接验证。
//
// 反证闭环：删掉下面任一测试 → dedup_i_am / discover_devices 报 never used。

/// 去重：同 device_instance 多次应答只保留首次出现的那条
#[test]
fn dedup_i_am_keeps_first_occurrence_in_order() {
    let got = frames::dedup_i_am(vec![
        iam(1, 15),
        iam(2, 42),
        // 设备 1 的第二次应答（不同网卡/网段），vendor 字段可能不同
        iam(1, 99),
        iam(3, 7),
    ]);
    assert_eq!(
        got.iter().map(|x| x.device_instance).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "应按首次出现顺序保留 1/2/3，设备 1 的重复应答被丢弃"
    );
    // 保留的是首次出现的那条（vendor 15），不是后来的 99
    assert_eq!(got[0].vendor_id, 15);
}

/// 去重：空输入 → 空输出（窗口期内一台设备都没应答）
#[test]
fn dedup_i_am_empty_input_yields_empty_output() {
    assert!(frames::dedup_i_am(Vec::new()).is_empty());
}

/// 去重：无重复时原样返回，顺序不变
#[test]
fn dedup_i_am_passthrough_when_unique() {
    let got = frames::dedup_i_am(vec![iam(5, 1), iam(6, 2), iam(7, 3)]);
    assert_eq!(got.len(), 3);
    assert_eq!(
        got.iter().map(|x| x.device_instance).collect::<Vec<_>>(),
        vec![5, 6, 7]
    );
}

/// discover_devices 无设备应答时应在窗口到期后返回空列表而非挂死
///
/// 只验证「广播发出去 + 窗口到期正常收敛」这条路径，不依赖外部设备：
/// bind 到 `0.0.0.0:0` 是本地操作，广播目标在受限网络里不可达也不影响
/// 返回（收不到包 → 超时 break）。窗口取 150ms，容忍 CI 抖动。
#[tokio::test]
async fn discover_devices_returns_empty_when_no_device_responds() {
    let collector = BacnetCollector::new(
        "127.0.0.1:47808".to_string(),
        "bacnet-disc".to_string(),
        Duration::from_millis(500),
    );
    let found = collector
        .discover_devices(None, None, Duration::from_millis(150))
        .await
        .expect("无设备应答应正常返回而非报错");
    assert!(found.is_empty(), "无设备应答时发现列表应为空");
}
