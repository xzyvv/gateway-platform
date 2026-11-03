use super::*;

#[test]
fn test_backoff_multiplier() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    assert_eq!(s.backoff_multiplier(), 1);
    s.record_failure();
    s.record_failure();
    assert_eq!(s.backoff_multiplier(), 1);
    s.record_failure();
    assert_eq!(s.backoff_multiplier(), 5);
    s.record_failure();
    assert_eq!(s.backoff_multiplier(), 5);
    s.record_failure();
    assert_eq!(s.backoff_multiplier(), 15);
}

#[test]
fn test_quality_stale_after_3_failures() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    // Bug C2: 健康点位（0 次失败）应为 Good
    assert_eq!(s.current_quality(), Quality::Good);
    s.record_failure();
    s.record_failure();
    assert_eq!(s.current_quality(), Quality::Bad);
    s.record_failure();
    assert_eq!(s.current_quality(), Quality::Stale);
}

#[test]
fn test_success_resets_backoff() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    for _ in 0..5 {
        s.record_failure();
    }
    assert!(s.consecutive_failures >= 3);
    assert!(s.stale_count >= 5);
    s.record_success(42.0);
    assert_eq!(s.consecutive_failures, 0);
    assert_eq!(s.last_good_value, Some(42.0));
    assert_eq!(s.current_interval, Duration::from_secs(1));
    // Q13: 新字段也需重置
    assert_eq!(s.stale_count, 0);
    assert!(s.last_good_ts > 0);
}

#[test]
fn test_stale_count_increments() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    assert_eq!(s.stale_count, 0);
    s.record_failure();
    assert_eq!(s.stale_count, 1);
    s.record_failure();
    assert_eq!(s.stale_count, 2);
    s.record_failure();
    assert_eq!(s.stale_count, 3);
    // 成功后归零
    s.record_success(1.0);
    assert_eq!(s.stale_count, 0);
}

#[test]
fn test_current_reason() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    assert_eq!(s.current_reason(), "device_timeout");
    s.record_failure();
    s.record_failure();
    assert_eq!(s.current_reason(), "device_timeout");
    s.record_failure();
    assert_eq!(s.current_reason(), "device_backoff");
}

/// C-6 回归：base_interval 极大时退避乘法不得 panic（旧实现 `Duration * u32` 溢出即崩）。
#[test]
fn test_record_failure_saturates_on_interval_overflow() {
    // Duration::MAX 的倍数必然溢出——旧实现在这里 panic
    let mut s = DeviceState::new(Duration::MAX);
    for _ in 0..10 {
        s.record_failure();
    }
    // 溢出收敛到 Duration::MAX，而不是崩溃
    assert_eq!(s.current_interval, Duration::MAX);
    assert_eq!(s.consecutive_failures, 10);

    // 正常量级下退避倍率语义不变（1s × 5 = 5s）
    let mut n = DeviceState::new(Duration::from_secs(1));
    n.record_failure();
    n.record_failure();
    n.record_failure();
    assert_eq!(n.current_interval, Duration::from_secs(5));
}

#[test]
fn test_offline_episodes_counts_recovery_cycles() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    assert_eq!(s.offline_episodes, 0);
    // 连续失败只算一次离线事件
    s.record_failure();
    s.record_failure();
    s.record_failure();
    assert_eq!(s.offline_episodes, 1);
    // 恢复后再失败，算第二次
    s.record_success(1.0);
    assert_eq!(s.offline_episodes, 1, "恢复不应清零累计掉线次数");
    s.record_failure();
    assert_eq!(s.offline_episodes, 2);
}

#[test]
fn test_downtime_accumulates_across_offline_segments() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    assert_eq!(s.offline_since_ts, 0);
    assert_eq!(s.downtime_ms_at(chrono::Utc::now().timestamp_millis()), 0);

    // 第一段离线：失败开始计时，进行中时长由 downtime_ms_at 现算
    s.record_failure();
    let since1 = s.offline_since_ts;
    assert!(since1 > 0);
    let during = s.downtime_ms_at(since1 + 5000);
    assert_eq!(during, 5000, "进行中的离线段应现算 5s");

    // 恢复：本段时长落入累计，offline_since 归零
    s.record_success(1.0);
    assert_eq!(s.offline_since_ts, 0);
    let acc1 = s.total_downtime_ms;
    assert!(acc1 >= 0);

    // 第二段离线：累计值不清零，两段叠加
    s.record_failure();
    assert!(s.offline_since_ts > 0);
    let total = s.downtime_ms_at(s.offline_since_ts + 3000);
    assert!(total >= acc1 + 3000, "两段离线时长应叠加");
}

#[test]
fn test_tracking_since_set_on_creation() {
    let before = chrono::Utc::now().timestamp_millis();
    let s = DeviceState::new(Duration::from_secs(1));
    let after = chrono::Utc::now().timestamp_millis();
    assert!(s.tracking_since_ts >= before && s.tracking_since_ts <= after);
}

#[test]
fn test_stale_count_saturating() {
    let mut s = DeviceState::new(Duration::from_secs(1));
    for _ in 0..1000 {
        s.record_failure();
    }
    // u32 边界保护，不应溢出
    assert!(s.stale_count >= 1000);
}
