//! 自愈执行器集成测试（F4 第二层 / P2）
//!
//! 端到端验证执行器闭环：采样坏状态 → 决策 → 执行动作 → `HealEvent` 落库。
//! 用真实的 `CellularManager`（enabled=false，避免真实拨号）注入
//! `platform.cellular`：其初始 `link_state = Down` 使健康快照恒为「蜂窝链路不可用」，
//! 触发 `ResetNetwork` 动作；动作走 `CellularManager::restart` 真实路径并返回成功。
//!
//! 独立成单独的测试二进制（独立进程），避免与其它测试共享全局心跳看门狗状态。

mod common;

use std::time::Duration;

use gateway_api::web::heal::{HealAction, SelfHealSettings};
use gateway_core::config::{BusConfig, CellularConfig};
use gateway_core::platform::cellular::CellularManager;
use gateway_core::south::scheduler::DeviceStatus;

/// 坏状态 + enabled=true → 执行器应执行 ResetNetwork 并落库一条 success 事件。
#[tokio::test]
async fn executor_acts_on_cellular_link_down_and_records_event() {
    // enabled=false：实例化但不真正拨号（CI/Windows 无 pppd）。初始 link_state=Down。
    let cellular = CellularManager::new(CellularConfig {
        enabled: false,
        heartbeat_secs: 3600,
        ..CellularConfig::default()
    });
    let state = common::mock_state::mock_app_state_with_cellular(Some(cellular));

    let (cancel, handle) = gateway_api::web::heal::spawn_executor(
        state.clone(),
        SelfHealSettings {
            enabled: true,
            check_interval_secs: 1,
            debounce_k: 1,
            cooldown_secs: 0,
            ..Default::default()
        },
    );

    // 首个 tick 立即触发；轮询等待事件落库（最多 5s）
    let mut found = None;
    for _ in 0..50 {
        if let Some(ev) = state
            .platform
            .heal_events
            .recent(10)
            .into_iter()
            .find(|e| e.action == HealAction::ResetNetwork)
        {
            found = Some(ev);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

    let ev = found.expect("蜂窝链路 Down 时执行器应产出 ResetNetwork 事件");
    assert_eq!(ev.action, HealAction::ResetNetwork);
    assert_eq!(ev.target, "cellular");
    assert_eq!(ev.result, "success", "CellularManager::restart 应成功");
}

/// 默认配置（enabled=false）→ 执行器即使运行也不产出任何自愈动作（存量零影响）。
#[tokio::test]
async fn disabled_executor_is_inert() {
    let state = common::mock_state::mock_app_state();
    let (cancel, handle) =
        gateway_api::web::heal::spawn_executor(state.clone(), SelfHealSettings::default());

    tokio::time::sleep(Duration::from_millis(300)).await;

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

    assert!(
        state.platform.heal_events.recent(10).is_empty(),
        "enabled=false 时不应产出任何自愈事件"
    );
}

/// 构造一条处于退避状态（连续失败 ≥ 阈值）的设备运行态。
fn failing_device(point_id: &str, bus: &str, failures: u32) -> DeviceStatus {
    DeviceStatus {
        point_id: point_id.to_string(),
        bus: bus.to_string(),
        device_id: point_id.to_string(),
        consecutive_failures: failures,
        quality: "stale".to_string(),
        ..Default::default()
    }
}

/// r45-HEAL1 回归（核心）：串口总线异常时，执行器只**定向重建目标总线**。
///
/// 旧实现忽略动作 target，一条串口总线异常会把**所有**串口总线全部重建：
/// 单一健康快照里的一个异常信号被放大成全局动作，正常总线上的设备在重开期间
/// 全部拿到 `BusClosed`（一次误判 = 全站串口中断开）。
#[tokio::test]
async fn reset_serial_rebuilds_only_the_failing_bus() {
    let buses = vec![
        BusConfig {
            port_name: "/dev/ttyS1".to_string(),
            protocol: "modbus".to_string(),
            tcp_addr: None,
            ..BusConfig::default()
        },
        BusConfig {
            port_name: "/dev/ttyS2".to_string(),
            protocol: "modbus".to_string(),
            tcp_addr: None,
            ..BusConfig::default()
        },
    ];
    // 只有 ttyS1 上的设备持续失败
    let devices = vec![failing_device("p1", "/dev/ttyS1", 5)];
    let (state, gateway) = common::mock_state::mock_app_state_for_self_heal(buses, devices);

    let (cancel, handle) = gateway_api::web::heal::spawn_executor(
        state.clone(),
        SelfHealSettings {
            enabled: true,
            check_interval_secs: 1,
            debounce_k: 1,
            cooldown_secs: 0,
            ..Default::default()
        },
    );

    // 等待执行器产出 ResetSerial 动作（最多 5s）
    let mut acted = false;
    for _ in 0..50 {
        if state
            .platform
            .heal_events
            .recent(10)
            .into_iter()
            .any(|e| e.action == HealAction::ResetSerial)
        {
            acted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

    assert!(acted, "串口总线持续失败应触发 ResetSerial");

    let calls = gateway.reset_serial_calls();
    assert_eq!(calls.len(), 1, "应恰好调用一次总线重建: {calls:?}");
    assert_eq!(
        calls[0].only.as_deref(),
        Some("/dev/ttyS1"),
        "必须定向到异常总线，不得退化为全局重建: {calls:?}"
    );
}

/// r45-HEAL1 回归（反向）：TCP 总线设备持续失败**不得**触发串口重建。
#[tokio::test]
async fn tcp_bus_failure_does_not_trigger_reset_serial() {
    let buses = vec![BusConfig {
        port_name: "plc-a".to_string(),
        protocol: "modbus".to_string(),
        tcp_addr: Some("10.0.0.2:502".to_string()),
        ..BusConfig::default()
    }];
    let devices = vec![failing_device("p1", "plc-a", 9)];
    let (state, gateway) = common::mock_state::mock_app_state_for_self_heal(buses, devices);

    let (cancel, handle) = gateway_api::web::heal::spawn_executor(
        state.clone(),
        SelfHealSettings {
            enabled: true,
            check_interval_secs: 1,
            debounce_k: 1,
            cooldown_secs: 0,
            ..Default::default()
        },
    );

    tokio::time::sleep(Duration::from_millis(500)).await;

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

    assert!(
        gateway.reset_serial_calls().is_empty(),
        "TCP 总线异常不得触发串口重建"
    );
}
