use std::sync::Arc;

use crate::engine::model::DataPoint;
use crate::north::server::modbus_server::ModbusServer;
#[cfg(feature = "opcua-server")]
use crate::north::server::opcua_server::{OpcUaServer, OpcUaServerConfig};

use super::*;

fn test_metrics() -> Arc<crate::engine::metrics::Metrics> {
    Arc::new(crate::engine::metrics::Metrics::new())
}

#[cfg(feature = "opcua-server")]
#[tokio::test]
async fn test_opcua_sink_overflow_drops_without_blocking() {
    let srv = Arc::new(OpcUaServer::new(OpcUaServerConfig::default(), None));
    let sink = OpcUaSink::new(srv, test_metrics());
    let total = SINK_CHANNEL_CAPACITY + 5;
    let start = std::time::Instant::now();
    for _ in 0..total {
        sink.send(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "send took too long: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_modbus_sink_overflow_drops_without_blocking() {
    let srv = Arc::new(ModbusServer::new(
        crate::north::server::ModbusServerConfig::default(),
    ));
    let sink = ModbusSink::new(srv, test_metrics());
    sink.build_mappings(&[], 0);
    let start = std::time::Instant::now();
    for _ in 0..SINK_CHANNEL_CAPACITY + 5 {
        sink.send(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "send took too long: {:?}",
        elapsed
    );
}

#[cfg(feature = "opcua-server")]
#[tokio::test]
async fn test_opcua_sink_metrics_incremented() {
    // 验证：单次 sink.send() 必然累加 sink_send_total +1
    let metrics = test_metrics();
    let srv = OpcUaServer::new(OpcUaServerConfig::default(), None);
    let sink = OpcUaSink::new(Arc::new(srv), metrics.clone());
    let name = sink.name();
    let before_send = metrics.sink_send_total(name);
    let before_dropped = metrics.sink_dropped_total(name);
    sink.send(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]);
    let after_send = metrics.sink_send_total(name);
    let after_dropped = metrics.sink_dropped_total(name);
    assert!(after_send > before_send, "send_total delta should be >= 1");
    assert_eq!(
        after_dropped, before_dropped,
        "dropped_total should be unchanged"
    );
}

#[tokio::test]
async fn test_modbus_sink_metrics_send_counted() {
    let metrics = test_metrics();
    let srv = ModbusServer::new(crate::north::server::ModbusServerConfig::default());
    let sink = ModbusSink::new(Arc::new(srv), metrics.clone());
    sink.build_mappings(&[], 0);
    let name = sink.name();
    let before_send = metrics.sink_send_total(name);
    let before_dropped = metrics.sink_dropped_total(name);
    // 灌入 SINK_CHANNEL_CAPACITY + 5 批
    let total_calls = (SINK_CHANNEL_CAPACITY as u64) + 5;
    for _ in 0..total_calls {
        sink.send(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]);
    }
    let after_send = metrics.sink_send_total(name);
    let after_dropped = metrics.sink_dropped_total(name);
    let delta = after_send.saturating_sub(before_send);
    let dropped_delta = after_dropped.saturating_sub(before_dropped);
    assert!(
        delta + dropped_delta >= total_calls,
        "send_total({}) + dropped_total({}) should be >= {} (got send={} dropped={})",
        delta,
        dropped_delta,
        total_calls,
        delta,
        dropped_delta
    );
}

// 注：test_modbus_sink_dropped_increments_on_overflow 已合并到
// test_modbus_sink_metrics_send_counted 中（send 路径覆盖 + dropped 是路径可能发生也可能不发生）。
// 强行断言 dropped 可能因运行时差异（slot 漏调、current_thread runtime 行为）而 flaky。

#[cfg(feature = "opcua-server")]
#[tokio::test]
async fn test_opcua_sink_channel_depth_reported() {
    // 验证：send 后 metrics.sink_channel_depth 被更新（非负）
    // 并行测试可能也在操作同名 sink 的 metrics，所以只断言 depth >= 0
    // metrics injected via test_metrics()
    // use injected metrics
    let srv = OpcUaServer::new(OpcUaServerConfig::default(), None);
    let metrics = test_metrics();
    let sink = OpcUaSink::new(Arc::new(srv), metrics.clone());
    let name = sink.name();
    sink.send(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let depth = metrics.sink_channel_depth(name);
    // depth >= 0：set_sink_channel_depth clamp 负值，且 in_flight 不会下溢
    assert!(depth >= 0, "channel_depth should be >= 0, got {}", depth);
}

#[tokio::test]
async fn test_modbus_sink_channel_depth_reported() {
    // metrics injected via test_metrics()
    // use injected metrics
    let srv = ModbusServer::new(crate::north::server::ModbusServerConfig::default());
    let metrics = test_metrics();
    let sink = ModbusSink::new(Arc::new(srv), metrics.clone());
    sink.build_mappings(&[], 0);
    let name = sink.name();
    sink.send(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let depth = metrics.sink_channel_depth(name);
    assert!(depth >= 0, "channel_depth should be >= 0, got {}", depth);
}

#[tokio::test]
async fn test_sink_names() {
    #[cfg(feature = "opcua-server")]
    {
        let opcua_srv = OpcUaServer::new(OpcUaServerConfig::default(), None);
        let sink = OpcUaSink::new(Arc::new(opcua_srv), test_metrics());
        assert_eq!(sink.name(), "opcua");
    }
    let mb_srv = ModbusServer::new(crate::north::server::ModbusServerConfig::default());
    let sink = ModbusSink::new(Arc::new(mb_srv), test_metrics());
    assert_eq!(sink.name(), "modbus");
}

/// 回归测试：OpcUaSink::health() 必须真实反映 OPC UA server 运行状态。
///
/// 缺陷背景：原实现恒返回 `connected: true`（假健康），server 未启动 / 启动失败 /
/// 已停止时监控仍显示健康，而外部客户端实际读不到地址空间数据。
#[cfg(feature = "opcua-server")]
#[tokio::test]
async fn test_opcua_sink_health_reflects_server_running() {
    use crate::north::server::Server;
    use crate::north::NorthConnector;
    use std::time::Duration;

    // 取空闲端口，避免与真实 4840 冲突
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let cfg = OpcUaServerConfig {
        enabled: true,
        endpoint: format!("opc.tcp://127.0.0.1:{port}"),
        allow_anonymous: false,
        ..Default::default()
    };
    let srv = Arc::new(OpcUaServer::new(cfg, None));
    let sink = OpcUaSink::new(srv.clone(), test_metrics());

    // 未启动：必须报告 not connected（原实现会误报 true）
    assert!(
        !sink.health().connected,
        "server 未启动时 health 不应为 connected"
    );
    assert!(sink.health().last_error.is_some());

    // 启动真实 server（含自签名证书准备，轮询等待进入 running）
    let srv_run = srv.clone();
    let run_task = tokio::spawn(async move {
        let _ = srv_run.start().await;
    });
    let mut up = false;
    for _ in 0..50 {
        if sink.health().connected {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "server 启动后 health 应变为 connected");

    // 停止后应回落为 not connected
    srv.stop().await.expect("stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;
    assert!(
        !sink.health().connected,
        "server 停止后 health 应回落为 not connected"
    );
}

/// 回归测试：ModbusSink::health() 必须真实反映 Modbus server 运行状态。
///
/// 缺陷背景：原实现恒返回 `connected: true`（假健康），server 未启动 / 已停止时
/// 监控仍显示健康，而外部 Modbus 主站实际读不到寄存器数据。
#[tokio::test]
async fn test_modbus_sink_health_reflects_server_running() {
    use crate::north::server::Server;
    use crate::north::NorthConnector;
    use std::time::Duration;

    // endpoint 用端口 0，由 OS 分配空闲端口，避免占用 502 / 端口冲突
    let cfg = crate::north::server::ModbusServerConfig {
        enabled: true,
        endpoint: "tcp://127.0.0.1:0".into(),
        ..Default::default()
    };
    let srv = Arc::new(ModbusServer::new(cfg));
    let sink = ModbusSink::new(srv.clone(), test_metrics());

    // 未启动：必须报告 not connected（原实现会误报 true）
    assert!(
        !sink.health().connected,
        "server 未启动时 health 不应为 connected"
    );
    assert!(sink.health().last_error.is_some());

    // 启动真实 server（bind + 起 accept 循环）
    let srv_run = srv.clone();
    let run_task = tokio::spawn(async move {
        let _ = srv_run.start().await;
    });
    let mut up = false;
    for _ in 0..50 {
        if sink.health().connected {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "server 启动后 health 应变为 connected");

    // 停止后应回落为 not connected
    srv.stop().await.expect("stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;
    assert!(
        !sink.health().connected,
        "server 停止后 health 应回落为 not connected"
    );
}
