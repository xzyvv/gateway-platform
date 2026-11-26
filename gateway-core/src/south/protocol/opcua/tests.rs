use super::*;

#[test]
fn test_parse_node_id_string() {
    let id = parse_node_id("ns=2;s=Temperature").unwrap();
    assert_eq!(id.namespace, 2);
}

#[test]
fn test_parse_node_id_numeric() {
    let id = parse_node_id("ns=2;i=1001").unwrap();
    assert_eq!(id.namespace, 2);
}

#[test]
fn test_parse_node_id_short_form() {
    let id = parse_node_id("2,1001").unwrap();
    assert_eq!(id.namespace, 2);
}

#[test]
fn test_parse_node_id_invalid() {
    assert!(parse_node_id("invalid").is_err());
}

// ============================================================
// C-11 修复：长连接半开检测契约
// ============================================================

/// 连续 read 失败阈值必须 ≥ 2（避免偶发网络抖动误触发重连）
/// 且 ≤ 5（避免长时间不触发）
// 契约守门：断言对象就是常量取值范围本身，常量断言属预期，不简化
#[allow(clippy::assertions_on_constants)]
#[test]
fn c11_read_failure_threshold_in_reasonable_range() {
    assert!(
        READ_FAILURE_THRESHOLD >= 2 && READ_FAILURE_THRESHOLD <= 5,
        "READ_FAILURE_THRESHOLD must be in [2, 5], got {}",
        READ_FAILURE_THRESHOLD
    );
}

/// READ_FAILURE_THRESHOLD 不能被 0 或 1（边界值）覆盖——单次失败就重连太激进
// 契约守门：断言对象就是常量取值范围本身，常量断言属预期，不简化
#[allow(clippy::assertions_on_constants)]
#[test]
fn c11_read_failure_threshold_not_too_aggressive() {
    assert!(
        READ_FAILURE_THRESHOLD >= 2,
        "single failure should not trigger reconnect (would cause flapping)"
    );
}

// ============================================================
// C-14 修复：会话 keepalive 契约
// ============================================================

use std::time::Duration;

/// keepalive 周期必须显著小于请求的 Session 超时（否则 server 会在本端探测之前
/// 就单方面关闭会话），响应超时必须小于探测周期，失败容忍 ≥ 2 避免抖动误触发
// 契约守门：断言对象就是常量取值范围本身，常量断言属预期，不简化
#[allow(clippy::assertions_on_constants)]
#[test]
fn c14_keepalive_timing_within_session_timeout() {
    assert!(
        KEEPALIVE_INTERVAL < Duration::from_millis(SESSION_TIMEOUT_MS as u64),
        "keepalive interval ({:?}) must be < session timeout ({}ms)",
        KEEPALIVE_INTERVAL,
        SESSION_TIMEOUT_MS
    );
    assert!(
        KEEPALIVE_RESPONSE_TIMEOUT < KEEPALIVE_INTERVAL,
        "response timeout ({:?}) must be < keepalive interval ({:?})",
        KEEPALIVE_RESPONSE_TIMEOUT,
        KEEPALIVE_INTERVAL
    );
    assert!(
        KEEPALIVE_MAX_CONSECUTIVE_FAILURES >= 2,
        "single probe failure must not tear down the session"
    );
}

/// keepalive 探测节点必须是标准 ServerStatus（ns=0;i=2256）——读它会触发一次
/// 完整的 Session + SecureChannel 往返，是最轻量的会话保活手段
#[test]
fn c14_keepalive_probes_standard_server_status_node() {
    let vid = keepalive_read_value_id();
    assert_eq!(vid.node_id.namespace, 0, "ServerStatus 位于命名空间 0");
    assert_eq!(
        vid.node_id,
        opcua::types::NodeId::new(0u16, SERVER_STATUS_NODE_NUMERIC_ID),
        "keepalive 探测点必须为 ns=0;i=2256"
    );
    assert_eq!(
        vid.attribute_id,
        opcua::types::AttributeId::Value as u32,
        "keepalive 探测读取 Value 属性"
    );
    assert_eq!(
        SERVER_STATUS_NODE_NUMERIC_ID, 2256,
        "OPC UA 标准 ServerStatus 节点 NumericIdentifier 为 2256"
    );
}

/// B3：诊断必须真建链 + 读 ServerStatus，而不是直接返回「正常」
#[tokio::test]
async fn b3_diag_reports_connect_failure_instead_of_faking_ok() {
    let c = OpcUaCollector::new("not-a-valid-endpoint".into(), "opcua-bus".into());
    assert!(c.diag_supported(), "opcua 应声明支持诊断");
    let msg = c.diag().await;
    assert!(msg.contains("建链失败"), "无数据必须如实报告失败：{msg}");
    assert!(!msg.contains("读取正常"), "不得伪造成功：{msg}");
}

/// 未建链时 disconnect 必须安全（keepalive_handle 为 None，不 panic）
#[tokio::test]
async fn c14_disconnect_without_connection_is_safe() {
    let c = OpcUaCollector::new("opc.tcp://127.0.0.1:4840".into(), "opcua-bus".into());
    assert!(!c.is_connected(), "新建采集器应为未连接状态");
    c.disconnect().await;
    assert!(!c.is_connected(), "disconnect 后仍应保持未连接状态");
}

// ============================================================
// 修复(审查#B4) 运行时回归：drop 必须 abort 两个后台任务并清 connected
// ============================================================
//
// 修复前：热重载整体替换 collectors 时不调用 disconnect()，keepalive 与
//         event_loop 两个后台任务继续运行（每次热重载堆积一份，与已废弃的
//         session 争用同一个 SecureChannel）。
// 修复后：Drop 中 abort 两个句柄、置 connected=false，并尽力断开 session。
//
// 已知局限：`opcua::Session` 无公开构造入口，测试无法伪造真实会话，故此处用
// 「假 JoinHandle + 假 connected」直接驱动真实 Drop，覆盖两个句柄的 abort 与
// connected 复位；Drop 中 session.disconnect() 分支需真实建链才会走到，不在
// 本用例覆盖范围内。

/// abort 触发 future drop 时置位——证明任务句柄确实被 abort
struct AbortFlag(Arc<AtomicBool>);

impl Drop for AbortFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn drop_aborts_both_tasks_and_clears_connected() {
    let event_aborted = Arc::new(AtomicBool::new(false));
    let keepalive_aborted = Arc::new(AtomicBool::new(false));

    let ef = Arc::clone(&event_aborted);
    let event_handle: JoinHandle<opcua::types::StatusCode> = tokio::spawn(async move {
        let _g = AbortFlag(ef);
        std::future::pending::<opcua::types::StatusCode>().await
    });
    let kf = Arc::clone(&keepalive_aborted);
    let keepalive_handle: JoinHandle<()> = tokio::spawn(async move {
        let _g = AbortFlag(kf);
        std::future::pending::<()>().await;
    });

    let collector = OpcUaCollector {
        endpoint: "opc.tcp://127.0.0.1:4840".to_string(),
        auth: None,
        session: Arc::new(Mutex::new(None)),
        event_loop_handle: Mutex::new(Some(event_handle)),
        keepalive_handle: Mutex::new(Some(keepalive_handle)),
        bus_name: "opcua-drop-bus".to_string(),
        connected: Arc::new(AtomicBool::new(true)),
        consecutive_read_failures: AtomicU32::new(0),
    };

    // 等价热重载整体替换采集器：直接 drop（不调用 disconnect）
    //
    // 注意：AbortFlag guard 是 async 块内的**首条语句**，只有任务被轮询过一次才会
    // 构造。若 spawn 后立刻 abort，future 从未 poll → guard 从未创建 → abort 时
    // 无 guard 可 drop。故先让出调度让两个任务各被轮询一次（进入 pending），
    // 再执行 drop。
    tokio::task::yield_now().await;
    let connected = Arc::clone(&collector.connected);
    drop(collector);

    // abort 异步生效：让出调度，等被取消的 future 完成 drop
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        event_aborted.load(Ordering::SeqCst),
        "event_loop 任务必须被 abort（否则会话事件循环泄漏）"
    );
    assert!(
        keepalive_aborted.load(Ordering::SeqCst),
        "keepalive 任务必须被 abort（否则探测任务泄漏）"
    );
    assert!(
        !connected.load(Ordering::Relaxed),
        "drop 后 connected 必须置 false"
    );
}

// =========================================================
// 修复(审查#B4-b) 运行时回归：Drop 的 session.disconnect() 必须真正把
// CloseSession 送达服务端（真实建链，补上仅假句柄用例覆盖不到的分支）
// =========================================================
//
// 判据依据：async-opcua 服务端 SessionManager 只在两种情况下把会话移出
// sessions 表：
//   1) 收到显式 CloseSession 请求（session/manager.rs 的 close_session）；
//   2) 会话超时到期（check_session_expiry → expire_session）。
// TCP 断开 / CloseSecureChannel / event loop abort 都**不会**移除会话。
// 客户端请求的会话超时为 SESSION_TIMEOUT_MS=10s，而观察窗口仅 3s，故
// 「Drop 后 3s 内服务端会话数归零」只能由 CloseSession 送达解释。
//
// 计数取法：直接读服务端 ServerHandle 的 diagnostics（需先
// diagnostics_enabled(true)）中的 CurrentSessionCount——不去复用「第二个会话
// 能否建链」这种判据，因为客户端在 BadTooManySessions 下会按指数退避重试，
// 连接调用要十几秒才返回，无法在秒级窗口内稳定观测。
#[cfg(feature = "opcua-server")]
use opcua::server::{ServerBuilder, ServerHandle};
#[cfg(feature = "opcua-server")]
use opcua::types::{VariableId, Variant};
#[cfg(feature = "opcua-server")]
use tokio::net::TcpListener;

/// 读取服务端当前会话数（须先 `diagnostics_enabled(true)`）
#[cfg(feature = "opcua-server")]
fn server_session_count(handle: &ServerHandle) -> u32 {
    let dv = handle
        .info()
        .diagnostics
        .get(VariableId::Server_ServerDiagnostics_ServerDiagnosticsSummary_CurrentSessionCount)
        .expect("diagnostics_enabled(true) 后 CurrentSessionCount 必须可读");
    match dv.value {
        Some(Variant::UInt32(n)) => n,
        other => panic!("CurrentSessionCount 应为 Variant::UInt32，实际 {other:?}"),
    }
}

#[cfg(feature = "opcua-server")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_delivers_close_session_to_real_server() {
    // 进程内真实 OPC UA 服务端，开启 diagnostics 以便直接观测会话数
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let port = listener.local_addr().expect("取本地端口").port();
    let endpoint = format!("opc.tcp://127.0.0.1:{port}/");

    let (server, handle) = ServerBuilder::new_anonymous("gw-opcua-drop-test")
        .application_uri("urn:gateway:opcua-test-server")
        .create_sample_keypair(true)
        .trust_client_certs(true)
        .discovery_urls(vec![endpoint.clone()])
        .diagnostics_enabled(true)
        .build()
        .expect("构建测试服务端");
    tokio::spawn(server.run_with(listener));

    // A：真实建链，服务端登记一个会话
    let a = OpcUaCollector::new(endpoint.clone(), "bus_opcua_real".into());
    tokio::time::timeout(Duration::from_secs(10), a.connect())
        .await
        .expect("A connect 超时")
        .expect("A 真实建链必须成功");
    assert!(
        a.connected.load(Ordering::Relaxed),
        "A 建链后 connected 应为 true"
    );

    // 服务端必须已登记 A 的会话（证明真实建链成功、且计数可用）
    let mut counted = false;
    for _ in 0..100 {
        if server_session_count(&handle) == 1 {
            counted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        counted,
        "A 建链后服务端会话数应为 1，实际 {}",
        server_session_count(&handle)
    );

    // 触发 Drop：应把 CloseSession 送达服务端、移除会话
    drop(a);

    // ≤3s（远小于 10s 会话超时）内服务端会话数必须归零
    let mut released = false;
    for _ in 0..150 {
        if server_session_count(&handle) == 0 {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        released,
        "Drop 后 3s 内服务端会话必须被 CloseSession 移除（否则 disconnect 未送达），实际 {}",
        server_session_count(&handle)
    );
}

// =========================================================
// UA-E2（ROI-21）：单点 Bad / 取不到值 ≠ 链路失败
// =========================================================

/// 单点 Bad / Uncertain 必须带可辨识的 `<场景>:<稳定错误码>` 原因，
/// 且后缀能被 `CollectErrorCode::from_code` 还原（北向 / 前端按码分类依赖此契约）。
#[test]
fn ua_e2_bad_and_uncertain_carry_identifiable_reason() {
    let (q, reason) = quality_from_status(Some(&StatusCode::BadUserAccessDenied));
    assert_eq!(q, Quality::Bad);
    assert_eq!(reason, "opcua_status_BadUserAccessDenied:OTHER");
    let code = reason.rsplit_once(':').map(|(_, c)| c).unwrap();
    assert_eq!(
        CollectErrorCode::from_code(code),
        Some(CollectErrorCode::Other),
        "原因后缀必须能被 from_code 还原：{reason}"
    );

    let (q, reason) = quality_from_status(Some(&StatusCode::UncertainLastUsableValue));
    assert_eq!(q, Quality::Uncertain, "Uncertain 不得被降级成 Bad");
    assert_eq!(reason, "opcua_status_UncertainLastUsableValue:OTHER");

    // 不同坏因必须可区分：权限不足 vs 节点不存在不能压成同一句
    let (_, denied) = quality_from_status(Some(&StatusCode::BadUserAccessDenied));
    let (_, unknown) = quality_from_status(Some(&StatusCode::BadNodeIdUnknown));
    assert_ne!(denied, unknown);
}

/// Good 不带原因；无状态码按 Bad 处理且原因非空（不伪造 Good）。
#[test]
fn ua_e2_good_has_no_reason_and_missing_status_is_bad() {
    let (q, reason) = quality_from_status(Some(&StatusCode::Good));
    assert_eq!(q, Quality::Good);
    assert!(reason.is_empty(), "Good 不应带失败原因：{reason}");

    let (q, reason) = quality_from_status(None);
    assert_eq!(q, Quality::Bad);
    assert_eq!(reason, "opcua_status_Bad:OTHER");
}

/// ROI-25：Bad 状态且无值是服务端明确拒绝，不得报成 InvalidFrame。
#[test]
fn ua_roi25_bad_status_without_value_is_device_rejected() {
    let dv = DataValue {
        value: None,
        status: Some(StatusCode::BadNodeIdUnknown),
        ..Default::default()
    };
    assert!(extract_value(&dv).is_err(), "前提：确实取不到值");
    match rejected_status(&dv) {
        Some(CollectorError::DeviceRejected(msg)) => {
            assert!(msg.contains("BadNodeIdUnknown"), "原因需可辨识: {msg}");
        }
        other => panic!("期望 DeviceRejected，实际 {other:?}"),
    }

    // Good 状态取不到值仍属解码问题，不冒充拒绝
    let dv = DataValue {
        value: None,
        status: Some(StatusCode::Good),
        ..Default::default()
    };
    assert!(rejected_status(&dv).is_none());
}

/// 原因前缀不得命中北向 OPC UA 服务端的细分关键字（小写子串匹配）。
#[test]
fn ua_e2_reason_does_not_trip_north_status_classifier() {
    for code in [
        StatusCode::Bad,
        StatusCode::BadCommunicationError,
        StatusCode::BadTimeout,
        StatusCode::BadUserAccessDenied,
        StatusCode::BadNodeIdUnknown,
    ] {
        let (_, reason) = quality_from_status(Some(&code));
        for needle in ["timeout", "communication", "disconnect", "connection"] {
            assert!(
                !reason.contains(needle),
                "原因 {:?} 误命中北向细分关键字 {:?}",
                reason,
                needle
            );
        }
    }
}

/// 回归：对「取不到值」的点位连续读取超过拆会话阈值次，
/// 链路失败计数必须保持 0、会话不得被拆（旧实现第 3 次就会拆会话）。
#[cfg(feature = "opcua-server")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ua_e2_bad_point_does_not_trip_session_teardown() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let port = listener.local_addr().expect("取本地端口").port();
    let endpoint = format!("opc.tcp://127.0.0.1:{port}/");
    let (server, _handle) = ServerBuilder::new_anonymous("gw-opcua-ua-e2-test")
        .application_uri("urn:gateway:opcua-ua-e2-test-server")
        .create_sample_keypair(true)
        .trust_client_certs(true)
        .discovery_urls(vec![endpoint.clone()])
        .diagnostics_enabled(true)
        .build()
        .expect("构建测试服务端");
    tokio::spawn(server.run_with(listener));

    let collector = OpcUaCollector::new(endpoint, "bus_opcua_ua_e2".into());
    tokio::time::timeout(Duration::from_secs(10), collector.connect())
        .await
        .expect("connect 超时")
        .expect("真实建链必须成功");

    let point: PointConfig = serde_json::from_value(serde_json::json!({
        "id": "ua_e2_bad",
        "bus": "bus_opcua_ua_e2",
        "protocol": "opcua",
        "address": "ns=2;i=999999",
        "interval": 1
    }))
    .expect("构造点位配置");

    // 该节点在示例服务端不存在 → 每轮都拿不到值（点级失败，非链路失败）
    for _ in 0..(READ_FAILURE_THRESHOLD + 2) {
        let res = tokio::time::timeout(Duration::from_secs(5), collector.read(&point))
            .await
            .expect("read 超时");
        assert!(res.is_err(), "不存在的节点不应返回成功数据点");
        assert_eq!(
            collector.consecutive_read_failures.load(Ordering::Relaxed),
            0,
            "单点取不到值不得计入拆会话阈值"
        );
        assert!(
            collector.connected.load(Ordering::Relaxed),
            "单点问题不得把整条会话置为断连"
        );
    }
}

// ============================================================
// ROI-27：UA-R3 / UA-R8 批量回填不变量
// ============================================================

fn batch_point(id: &str) -> PointConfig {
    PointConfig {
        id: id.into(),
        device_id: Some("dev".into()),
        ..PointConfig::default()
    }
}

fn double_value(v: f64) -> DataValue {
    DataValue {
        value: Some(opcua::types::Variant::Double(v)),
        ..Default::default()
    }
}

/// UA-R3：服务端少返回结果时，只有缺失的下标报 `InvalidFrame`，
/// 已返回的点位仍拿到自己的值，不得整体失败或错位。
#[test]
fn ua_r3_short_batch_marks_only_missing_index() {
    let points = vec![batch_point("p1"), batch_point("p2"), batch_point("p3")];
    let mut out: Vec<Option<CollectorResult<DataPoint>>> = points.iter().map(|_| None).collect();
    fill_batch_results(
        &points,
        &[true, true, true],
        &[],
        &[double_value(1.5), double_value(2.5)],
        100,
        |i, r| out[i] = Some(r),
    );

    let p1 = out[0].take().expect("p1 必须有结果").expect("p1 应成功");
    assert_eq!(p1.point_id, "p1");
    assert!((p1.value - 1.5).abs() < 1e-9);
    let p2 = out[1].take().expect("p2 必须有结果").expect("p2 应成功");
    assert!((p2.value - 2.5).abs() < 1e-9);

    match out[2].take().expect("p3 必须有结果") {
        Err(CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("fewer results"), "原因需可辨识: {msg}");
        }
        other => panic!("缺失下标必须报 InvalidFrame，实际 {other:?}"),
    }
}

/// UA-R8：返回顺序必须与请求下标一一对应，不得因某点解析失败而整体前移。
#[test]
fn ua_r8_batch_values_keep_request_order() {
    let points = vec![batch_point("p1"), batch_point("p2"), batch_point("p3")];
    let mut out: Vec<Option<CollectorResult<DataPoint>>> = points.iter().map(|_| None).collect();
    // 第 2 个点地址解析失败（parse_ok=false），但服务端仍按原下标返回 3 个值
    fill_batch_results(
        &points,
        &[true, false, true],
        &["bad node id".into()],
        &[double_value(10.0), double_value(20.0), double_value(30.0)],
        200,
        |i, r| out[i] = Some(r),
    );

    let p1 = out[0].take().unwrap().expect("p1 成功");
    assert!((p1.value - 10.0).abs() < 1e-9, "p1 不得拿到后面的值");
    assert!(matches!(
        out[1].take().unwrap(),
        Err(CollectorError::InvalidFrame(msg)) if msg.contains("bad node id")
    ));
    let p3 = out[2].take().unwrap().expect("p3 成功");
    assert!((p3.value - 30.0).abs() < 1e-9, "p3 必须保持第三个下标的值");
}

/// UA-R2：生产路径不显式 connect，首次 read 必须先惰性建链再发请求。
#[cfg(feature = "opcua-server")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ua_r2_read_connects_lazily_before_first_read() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let port = listener.local_addr().expect("取本地端口").port();
    let endpoint = format!("opc.tcp://127.0.0.1:{port}/");
    let (server, _handle) = ServerBuilder::new_anonymous("gw-opcua-ua-r2-test")
        .application_uri("urn:gateway:opcua-ua-r2-test-server")
        .create_sample_keypair(true)
        .trust_client_certs(true)
        .discovery_urls(vec![endpoint.clone()])
        .diagnostics_enabled(true)
        .build()
        .expect("构建测试服务端");
    tokio::spawn(server.run_with(listener));

    let collector = OpcUaCollector::new(endpoint, "bus_opcua_ua_r2".into());
    assert!(
        !collector.connected.load(Ordering::Relaxed),
        "前提：构造后未建链"
    );

    let point: PointConfig = serde_json::from_value(serde_json::json!({
        "id": "ua_r2",
        "bus": "bus_opcua_ua_r2",
        "protocol": "opcua",
        "address": "ns=2;i=999999",
        "interval": 1
    }))
    .expect("构造点位配置");

    // 节点不存在，但 read 入口必须先完成建链；不能因未连接就跳过建链
    let _ = tokio::time::timeout(Duration::from_secs(10), collector.read(&point))
        .await
        .expect("read 超时");
    assert!(
        collector.connected.load(Ordering::Relaxed),
        "read 入口必须先惰性建链"
    );
}

// ============================================================
// 审计批 G-T2a：UA-R2 / UA-R5 / UA-R6 边界与关系断言
// ============================================================

/// UA-R2：重复 connect 不得重建会话。
///
/// 实现 `mod.rs:619-620` 在 session 槽位已有会话时直接返回 `Ok(())`。本用例用两个
/// 可观测点锁定该幂等行为：
/// 1) `Arc::as_ptr` 身份不变——槽位里仍是同一个 `Arc<Session>` 实例；
/// 2) 服务端 `CurrentSessionCount` 仍为 1——没有在服务端登记第二个会话。
/// 若实现改成「先拆再建」，两项断言都会失败。
#[cfg(feature = "opcua-server")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ua_r2_repeated_connect_does_not_rebuild_session() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let port = listener.local_addr().expect("取本地端口").port();
    let endpoint = format!("opc.tcp://127.0.0.1:{port}/");
    let (server, handle) = ServerBuilder::new_anonymous("gw-opcua-ua-r2-repeat")
        .application_uri("urn:gateway:opcua-ua-r2-repeat-server")
        .create_sample_keypair(true)
        .trust_client_certs(true)
        .discovery_urls(vec![endpoint.clone()])
        .diagnostics_enabled(true)
        .build()
        .expect("构建测试服务端");
    tokio::spawn(server.run_with(listener));

    let collector = OpcUaCollector::new(endpoint, "bus_opcua_ua_r2_repeat".into());
    tokio::time::timeout(Duration::from_secs(10), collector.connect())
        .await
        .expect("首次 connect 超时")
        .expect("首次真实建链必须成功");
    assert!(
        collector.connected.load(Ordering::Relaxed),
        "首次建链成功后 connected 必须为 true（connect 内部已等到 wait_for_connection 成功）"
    );

    // 服务端登记 1 个会话：证明首次建链真实生效，也确认该计数可作后续判据
    let mut counted = false;
    for _ in 0..100 {
        if server_session_count(&handle) == 1 {
            counted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        counted,
        "首次建链后服务端会话数应为 1，实际 {}",
        server_session_count(&handle)
    );

    let ptr_before = {
        let guard = collector.session.lock().await;
        let session = guard.as_ref().expect("首次建链后 session 必须存在");
        Arc::as_ptr(session)
    };

    // 重复 connect：实现应在槽位已有会话时直接返回 Ok，不得重建
    tokio::time::timeout(Duration::from_secs(10), collector.connect())
        .await
        .expect("重复 connect 超时")
        .expect("重复 connect 必须幂等成功");

    let ptr_after = {
        let guard = collector.session.lock().await;
        let session = guard.as_ref().expect("重复 connect 后 session 不得丢失");
        Arc::as_ptr(session)
    };
    assert_eq!(
        ptr_before, ptr_after,
        "重复 connect 不得替换会话对象（必须仍是同一个 Arc<Session> 实例）"
    );
    assert_eq!(
        server_session_count(&handle),
        1,
        "重复 connect 不得在服务端新建第二个会话"
    );
    assert!(
        collector.connected.load(Ordering::Relaxed),
        "重复 connect 后仍应保持已连接状态"
    );
}

/// UA-R5：读失败阈值必须在会话超时之前到达。
///
/// 阈值是「连续读失败次数」，会话超时是毫秒；把阈值按采集最快 1s/次 换算成
/// 最长判定耗时，必须严格小于会话超时——否则会出现「会话先掉、计数还没到」，
/// 整站离线但页面仍显示连接中。
// 契约守门：断言对象就是常量之间的关系本身，常量断言属预期，不简化
#[allow(clippy::assertions_on_constants)]
#[test]
fn ua_r5_threshold_is_below_session_timeout() {
    // 采集周期最快按 1s 计（现场最小配置粒度），阈值次失败的总耗时必须早于会话超时。
    let worst_case_ms = u64::from(READ_FAILURE_THRESHOLD) * 1_000;
    assert!(
        worst_case_ms < u64::from(SESSION_TIMEOUT_MS),
        "读失败阈值 {} 次 × 1s = {}ms 必须小于会话超时 {}ms，否则会话先掉、计数还没到",
        READ_FAILURE_THRESHOLD,
        worst_case_ms,
        SESSION_TIMEOUT_MS
    );
}

/// UA-R6：keepalive 失败达阈值必须拆会话；`try_lock` 抢不到必须跳过、不得阻塞采集。
///
/// 一个用例覆盖两个可观测行为：
/// 1) 锁竞争跳过：测试持有 `session` 锁跨越一个完整 `KEEPALIVE_INTERVAL`。supervisor
///    醒来时 `try_lock` 立即返回 `Err`（`mod.rs:347-355` 的跳过分支），烧掉一个周期
///    而不拆会话；同时 `try_lock` 本身不阻塞（耗时 < 100ms），说明探测不会卡住读路径。
/// 2) 达阈值拆会话：让当前 session 失效（直接 `disconnect`，模拟半开会话），
///    supervisor 连续 `KEEPALIVE_MAX_CONSECUTIVE_FAILURES` 次探测失败后必须清空
///    session 槽位并置 `connected=false`。
#[cfg(feature = "opcua-server")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ua_r6_keepalive_threshold_tears_down_and_skips_on_lock_contention() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let port = listener.local_addr().expect("取本地端口").port();
    let endpoint = format!("opc.tcp://127.0.0.1:{port}/");
    let (server, _handle) = ServerBuilder::new_anonymous("gw-opcua-ua-r6-test")
        .application_uri("urn:gateway:opcua-ua-r6-test-server")
        .create_sample_keypair(true)
        .trust_client_certs(true)
        .discovery_urls(vec![endpoint.clone()])
        .diagnostics_enabled(true)
        .build()
        .expect("构建测试服务端");
    tokio::spawn(server.run_with(listener));

    let collector = OpcUaCollector::new(endpoint, "bus_opcua_ua_r6".into());
    tokio::time::timeout(Duration::from_secs(10), collector.connect())
        .await
        .expect("connect 超时")
        .expect("真实建链必须成功");
    assert!(
        collector.connected.load(Ordering::Relaxed),
        "建链后 connected 应为 true"
    );

    // ---- 阶段 1：session 锁被读路径占用 → 本周期 try_lock 跳过，不阻塞 ----
    // 持锁等价于 read 正在使用 session；supervisor 此时必须跳过而不是排队等待。
    let guard = collector.session.lock().await;

    let t0 = std::time::Instant::now();
    let busy = collector.session.try_lock().is_err();
    assert!(
        busy,
        "锁被占用时 try_lock 必须返回 Err（supervisor 据此跳过本周期）"
    );
    assert!(
        t0.elapsed() < Duration::from_millis(100),
        "try_lock 抢不到必须立即返回，不得阻塞采集（实测耗时 {:?}）",
        t0.elapsed()
    );

    // 跨过一个完整探测周期：supervisor 应跳过，会话不得被拆除
    tokio::time::sleep(KEEPALIVE_INTERVAL + Duration::from_secs(1)).await;
    assert!(
        collector.connected.load(Ordering::Relaxed),
        "锁竞争导致跳过时，supervisor 不得把连接置为断连"
    );
    assert!(guard.is_some(), "锁竞争导致跳过时，session 槽位不得被清空");

    // ---- 阶段 2：探测连续失败达阈值 → 拆会话 ----
    // 直接断开当前 session（模拟半开会话），让后续 keepalive 探测必然失败；
    // 槽位仍由 collector 持有，由 supervisor 在达到阈值后负责拆除。
    let dead_session = guard.as_ref().expect("session 仍应存在").clone();
    let _ = dead_session.disconnect().await;
    drop(guard);

    // 等 supervisor 累积到阈值次数后拆会话：留出 ≥2 个探测周期 + 单次响应超时的余量
    let mut torn_down = false;
    for _ in 0..300 {
        if !collector.connected.load(Ordering::Relaxed) {
            torn_down = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        torn_down,
        "keepalive 连续失败达阈值 {} 次后必须置 connected=false",
        KEEPALIVE_MAX_CONSECUTIVE_FAILURES
    );
    assert!(
        collector.session.lock().await.is_none(),
        "达阈值拆会话后 session 槽位必须被清空（否则下一次 read 会复用死会话）"
    );
}
