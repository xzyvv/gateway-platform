//! 蜂窝模块状态机与生命周期测试（C2）
use super::*;
use std::time::Duration;

/// heartbeat_secs 设为 1 小时，避免后台 loop 在测试中真正发起网络/AT 调用。
fn mgr(enabled: bool) -> Arc<CellularManager> {
    CellularManager::new(CellularConfig {
        enabled,
        heartbeat_secs: 3600,
        ..CellularConfig::default()
    })
}

#[test]
fn default_status_is_init_with_unknown_signal() {
    let m = mgr(false);
    let st = m.status();
    assert_eq!(st.state, CellularState::Init);
    assert_eq!(st.signal_strength, 99);
    assert!(st.ip_address.is_none());
    assert_eq!(m.link_state(), LinkState::Down);
}

#[test]
fn set_state_updates_status_and_broadcasts() {
    let m = mgr(false);
    let mut rx = m.subscribe_state();
    m.set_state(CellularState::Registered);
    assert_eq!(m.status().state, CellularState::Registered);
    assert_eq!(rx.try_recv().unwrap(), CellularState::Registered);
}

#[test]
fn start_when_disabled_spawns_no_tasks() {
    let m = mgr(false);
    m.clone().start();
    assert!(m.task_handles.lock().is_empty());
}

#[tokio::test]
async fn stop_without_start_marks_not_ready() {
    let m = mgr(false);
    m.stop().await;
    assert_eq!(m.status().state, CellularState::NotReady);
}

#[tokio::test]
async fn stop_cancels_background_tasks_promptly() {
    // heartbeat 为 1 小时：若 stop() 未真正取消 task，本测试会超时失败。
    let m = mgr(true);
    m.clone().start();
    assert!(
        !m.task_handles.lock().is_empty(),
        "monitor task should have been spawned"
    );
    tokio::time::timeout(Duration::from_secs(3), m.stop())
        .await
        .expect("stop() should cancel background tasks promptly");
    assert_eq!(m.status().state, CellularState::NotReady);
}
