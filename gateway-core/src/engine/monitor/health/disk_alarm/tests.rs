//! 磁盘水位分级告警的单测。
//!
//! 重点覆盖三类"现场最容易踩"的语义：
//! 1. 阈值边界（25% / 15% / 10% 的含开闭关系）——写反一位就会误报或漏报；
//! 2. 边沿判定（档位不变不重复发、回落算 recover）——避免每 60s 刷屏；
//! 3. 与阻断语义的一致性（10% 恰好不阻断，与 `disk_ok_from_free_pct` 同口径）。

use super::*;

/// 档位判定：正常区。
#[test]
fn level_is_ok_above_warn_threshold() {
    assert_eq!(disk_level_from_free_pct(100.0), DiskLevel::Ok);
    assert_eq!(disk_level_from_free_pct(50.0), DiskLevel::Ok);
    // 恰好等于预警线仍是 ok（与"≥ 25% 即正常"一致）
    assert_eq!(
        disk_level_from_free_pct(DISK_WARN_THRESHOLD_PCT),
        DiskLevel::Ok
    );
}

/// 档位判定：预警区（15% ~ 25%）。
#[test]
fn level_is_warn_between_warn_and_critical() {
    assert_eq!(
        disk_level_from_free_pct(DISK_WARN_THRESHOLD_PCT - 0.01),
        DiskLevel::Warn
    );
    // 恰好等于严重线仍是 warn
    assert_eq!(
        disk_level_from_free_pct(DISK_CRITICAL_THRESHOLD_PCT),
        DiskLevel::Warn
    );
}

/// 档位判定：严重区（10% ~ 15%）。
#[test]
fn level_is_critical_between_critical_and_blocking() {
    assert_eq!(
        disk_level_from_free_pct(DISK_CRITICAL_THRESHOLD_PCT - 0.01),
        DiskLevel::Critical
    );
    // 恰好等于阻断线仍是 critical——阻断判定用 `>= 10.0` 放行
    assert_eq!(
        disk_level_from_free_pct(DISK_BLOCKING_THRESHOLD_PCT),
        DiskLevel::Critical
    );
}

/// 档位判定：阻断区（< 10%）。
#[test]
fn level_is_blocking_below_blocking_threshold() {
    assert_eq!(
        disk_level_from_free_pct(DISK_BLOCKING_THRESHOLD_PCT - 0.01),
        DiskLevel::Blocking
    );
    assert_eq!(disk_level_from_free_pct(0.0), DiskLevel::Blocking);
}

/// NaN（采样异常）按最保守档处理：宁可误报阻断，也不能在丢数据时报"正常"。
#[test]
fn level_of_nan_is_blocking() {
    assert_eq!(disk_level_from_free_pct(f64::NAN), DiskLevel::Blocking);
}

/// 阻断档与入队放行判定必须同口径：档位说 blocking 时入队确实被阻断。
#[test]
fn blocking_level_matches_enqueue_blocking() {
    for free_pct in [0.0, 1.0, 5.0, 9.99] {
        assert_eq!(disk_level_from_free_pct(free_pct), DiskLevel::Blocking);
        assert!(
            !super::super::disk_ok_from_free_pct(free_pct),
            "free_pct={} 应阻断入队",
            free_pct
        );
    }
    for free_pct in [10.0, 15.0, 25.0] {
        assert_ne!(disk_level_from_free_pct(free_pct), DiskLevel::Blocking);
        assert!(
            super::super::disk_ok_from_free_pct(free_pct),
            "free_pct={} 应放行入队",
            free_pct
        );
    }
}

/// 档位编码往返：数值越大越严重。
#[test]
fn level_code_roundtrip_and_ordering() {
    assert_eq!(DiskLevel::from_u8(DiskLevel::Ok as u8), DiskLevel::Ok);
    assert_eq!(DiskLevel::from_u8(DiskLevel::Warn as u8), DiskLevel::Warn);
    assert_eq!(
        DiskLevel::from_u8(DiskLevel::Critical as u8),
        DiskLevel::Critical
    );
    assert_eq!(
        DiskLevel::from_u8(DiskLevel::Blocking as u8),
        DiskLevel::Blocking
    );
    // 越界编码按最严重档处理（保守优先）
    assert_eq!(DiskLevel::from_u8(200), DiskLevel::Blocking);
    assert!(DiskLevel::Ok < DiskLevel::Warn);
    assert!(DiskLevel::Warn < DiskLevel::Critical);
    assert!(DiskLevel::Critical < DiskLevel::Blocking);
}

/// 边沿判定：档位不变不产生事件（否则每 60s 刷一次屏）。
#[test]
fn event_is_none_when_level_unchanged() {
    assert!(disk_alarm_event(DiskLevel::Warn, DiskLevel::Warn, 20.0).is_none());
    assert!(disk_alarm_event(DiskLevel::Ok, DiskLevel::Ok, 80.0).is_none());
}

/// 边沿判定：恶化 = trigger，回落 = recover。
#[test]
fn event_direction_follows_level_change() {
    let up = disk_alarm_event(DiskLevel::Ok, DiskLevel::Warn, 20.0).expect("应产生事件");
    assert_eq!(up.event, "trigger");
    assert_eq!(up.prev_level, "ok");
    assert_eq!(up.level, "warn");

    let down = disk_alarm_event(DiskLevel::Warn, DiskLevel::Ok, 60.0).expect("应产生事件");
    assert_eq!(down.event, "recover");
    assert_eq!(down.prev_level, "warn");
    assert_eq!(down.level, "ok");
}

/// 事件载荷字段：阈值取变化后档位对应的线，数值原样带上。
#[test]
fn event_payload_carries_threshold_and_free_pct() {
    let ev = disk_alarm_event(DiskLevel::Ok, DiskLevel::Critical, 12.5).expect("应产生事件");
    assert_eq!(ev.level, "critical");
    assert_eq!(ev.prev_level, "ok");
    assert_eq!(ev.free_pct, 12.5);
    assert_eq!(ev.threshold_pct, DISK_CRITICAL_THRESHOLD_PCT);
    assert!(!ev.message.is_empty());
    assert!(ev.timestamp > 0);
}

/// 外呼 topic：系统级告警走独立后缀，不与点级报警混流。
#[test]
fn notifier_topic_has_system_alarm_suffix() {
    let notifier = DiskAlarmNotifier::new(None, "gateway/data");
    assert_eq!(notifier.topic(), "gateway/data/system/alarm");
}

/// 无 publisher 时外呼不应 panic（未配置 MQTT 的部署必须能正常跑）。
#[test]
fn publish_without_publisher_is_noop() {
    let notifier = DiskAlarmNotifier::new(None, "t");
    let ev = disk_alarm_event(DiskLevel::Ok, DiskLevel::Warn, 20.0).expect("应产生事件");
    notifier.publish(&ev);
}

/// tick：把档位写进 DiskStatus（唯一真源），并暴露给指标。
#[test]
fn tick_records_level_into_disk_status() {
    let status = super::super::DiskStatus::new();
    let metrics = crate::engine::metrics::Metrics::new();
    let tick = tick(None, &status, 20.0, &metrics);
    assert_eq!(tick.level, DiskLevel::Warn);
    assert!(tick.ok);
    assert_eq!(status.level(), DiskLevel::Warn);
    assert_eq!(
        metrics
            .disk_level
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

/// tick：档位不变时不产生告警（计数不增）。
#[test]
fn tick_does_not_count_alarm_when_level_unchanged() {
    let status = super::super::DiskStatus::new();
    let metrics = crate::engine::metrics::Metrics::new();
    tick(None, &status, 20.0, &metrics);
    let before = metrics
        .alarm
        .disk_triggers_total
        .load(std::sync::atomic::Ordering::Relaxed);
    tick(None, &status, 21.0, &metrics);
    let after = metrics
        .alarm
        .disk_triggers_total
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(before, after, "档位未变不应重复计告警");
}

/// tick：档位恶化时计一次告警；回落不计入（与点级告警口径一致）。
#[test]
fn tick_counts_alarm_only_on_worsening() {
    let status = super::super::DiskStatus::new();
    let metrics = crate::engine::metrics::Metrics::new();
    tick(None, &status, 80.0, &metrics);
    tick(None, &status, 20.0, &metrics); // ok -> warn
    assert_eq!(
        metrics
            .alarm
            .disk_triggers_total
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    tick(None, &status, 12.0, &metrics); // warn -> critical
    assert_eq!(
        metrics
            .alarm
            .disk_triggers_total
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    tick(None, &status, 60.0, &metrics); // critical -> ok（recover，不计数）
    assert_eq!(
        metrics
            .alarm
            .disk_triggers_total
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

/// tick：进入阻断档时，入队放行判定同步变为阻断（阻断语义未被分级改动）。
#[test]
fn tick_blocking_state_matches_enqueue_gate() {
    let status = super::super::DiskStatus::new();
    let metrics = crate::engine::metrics::Metrics::new();
    let tick = tick(None, &status, 5.0, &metrics);
    assert!(!tick.ok);
    assert!(tick.pressure_changed);
    assert!(status.is_pressure());
    assert_eq!(status.level(), DiskLevel::Blocking);
}
