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
