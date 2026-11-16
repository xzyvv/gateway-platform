use super::*;
use crate::config::BusConfig;

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
