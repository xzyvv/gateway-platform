use super::*;
use crate::config::{BusConfig, GatewayConfig, PointConfig};
use crate::engine::error::GatewayError;

/// r36-APP2 回归：配置热重载（ResetSerial 语义）中串口打开失败时，必须保留
/// 该总线**已有的** manager，而不是静默丢弃。
///
/// 旧实现失败即 `continue`，使该总线既不进 `new_managers` 也不进
/// `all_collectors`；随后 `bus_managers` 被整体替换并 drop 旧 manager，
/// 该总线采集静默停止。修复后旧实例应被回退保留，且不计入"已重建"。
#[tokio::test]
async fn reload_buses_keeps_old_manager_when_open_fails() {
    let gw = gateway_runtime_without_buses_for_test();

    // 预置一个"旧"串口总线 manager（serial 句柄 None，仅作占位旧实例）
    let cfg = BusConfig {
        port_name: "COM_R36_NONEXISTENT".to_string(),
        protocol: "modbus".to_string(),
        tcp_addr: None,
        ..BusConfig::default()
    };
    let old = Arc::new(BusManager::spawn(&cfg, None, gw.metrics.clone()));
    gw.bus_managers
        .lock()
        .insert(cfg.port_name.clone(), old.clone());
    assert!(gw.bus_names().contains(&cfg.port_name));

    // 强制重建：打开一个不存在的串口必定失败
    let rebuilt = gw
        .reset_serial_buses(std::slice::from_ref(&cfg), &[], None)
        .await
        .expect("reload should succeed when no points need rebuilding");

    assert_eq!(rebuilt, 0, "打开失败的总线不应计入已重建数量");
    assert!(
        gw.bus_names().contains(&cfg.port_name),
        "打开失败时旧 manager 必须被保留，不能从 bus_managers 中消失"
    );
    let now = gw.bus_manager(&cfg.port_name).expect("bus still present");
    assert!(Arc::ptr_eq(&old, &now), "应复用同一个旧 manager 实例");
}

/// ROI-12：prepare 阶段拒绝未知协议时必须**零副作用**——不新增总线、不改点位。
///
/// 旧实现会先落盘再热重载，失败后只能回滚磁盘；现在 prepare 前置于落盘，
/// 因此拒绝路径天然保持磁盘与运行态都为旧配置。
#[tokio::test]
async fn prepare_reload_rejects_unknown_protocol_without_side_effects() {
    let gw = gateway_runtime_without_buses_for_test();
    let old = GatewayConfig::default();
    let new = GatewayConfig {
        buses: vec![BusConfig {
            port_name: "COM_ROI12_UNKNOWN".to_string(),
            protocol: "no-such-protocol".to_string(),
            tcp_addr: None,
            ..BusConfig::default()
        }],
        points: vec![PointConfig {
            id: "p_roi12".to_string(),
            bus: "COM_ROI12_UNKNOWN".to_string(),
            protocol: "no-such-protocol".to_string(),
            address: "1".to_string(),
            interval: 1,
            ..PointConfig::default()
        }],
        ..GatewayConfig::default()
    };

    let err = match gw.prepare_reload(&old, &new, new.points.clone()).await {
        Ok(_) => panic!("未知协议必须在 prepare 阶段被拒绝"),
        Err(e) => e,
    };
    assert!(
        matches!(err, GatewayError::Config(_)),
        "应返回配置错误，实际: {err:?}"
    );

    assert!(gw.bus_names().is_empty(), "prepare 失败不得上岗任何总线");
    assert!(
        gw.scheduler.points().is_empty(),
        "prepare 失败不得改动点位表"
    );
}

/// ROI-12：点位引用不存在的总线时，prepare 直接拒绝。
///
/// 旧实现到 spawn 才 `warn!("no collector for bus")` 后 return，
/// 表现为「配置已生效、该组静默消失」；现在整次保存被拒绝。
#[tokio::test]
async fn prepare_reload_rejects_point_on_missing_bus() {
    let gw = gateway_runtime_without_buses_for_test();
    let old = GatewayConfig::default();
    let new = GatewayConfig {
        points: vec![PointConfig {
            id: "p_ghost".to_string(),
            bus: "ghost-bus".to_string(),
            protocol: "modbus".to_string(),
            address: "1".to_string(),
            interval: 1,
            ..PointConfig::default()
        }],
        ..GatewayConfig::default()
    };

    let err = match gw.prepare_reload(&old, &new, new.points.clone()).await {
        Ok(_) => panic!("缺 collector 的点位组必须在 prepare 阶段拒绝"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("collector"),
        "错误信息应说明缺少 collector，实际: {err}"
    );
    assert!(gw.scheduler.points().is_empty());
}
