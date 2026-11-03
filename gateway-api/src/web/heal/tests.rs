use super::*;

fn snap(stale: &[&str], cellular: Option<CellularHealth>) -> HealthSnapshot {
    HealthSnapshot {
        ts: 0,
        devices: Vec::new(),
        mqtt_connected: true,
        mqtt_circuit_open: false,
        queue_len: 0,
        cellular,
        stale_loops: stale.iter().map(|s| s.to_string()).collect(),
    }
}

fn enabled_policy() -> HealPolicy {
    HealPolicy::new(SelfHealSettings {
        enabled: true,
        ..Default::default()
    })
}

#[test]
fn disabled_policy_never_acts() {
    // 默认配置 enabled=false：无论异常持续多久都不动作
    let mut p = HealPolicy::new(SelfHealSettings::default());
    let s = snap(&["collect"], None);
    for t in 0..10 {
        assert!(p.evaluate(&s, t).is_none());
    }
}

#[test]
fn debounce_requires_k_consecutive_bad() {
    // debounce_k 默认 3：前两次不动手，第三次触发
    let mut p = enabled_policy();
    let s = snap(&["collect"], None);
    assert!(p.evaluate(&s, 0).is_none());
    assert!(p.evaluate(&s, 1).is_none());
    let d = p.evaluate(&s, 2).expect("第三次连续异常应触发");
    assert_eq!(d.action, HealAction::RestartService);
    assert_eq!(d.target, "loop:collect");
}

#[test]
fn transient_blip_does_not_trigger() {
    // 坏→好→坏→坏：计数被中间恢复清零，不应触发
    let mut p = enabled_policy();
    let bad = snap(&["collect"], None);
    let good = snap(&[], None);
    assert!(p.evaluate(&bad, 0).is_none());
    assert!(p.evaluate(&good, 1).is_none());
    assert!(p.evaluate(&bad, 2).is_none());
    assert!(p.evaluate(&bad, 3).is_none());
}

#[test]
fn cooldown_suppresses_repeat_action() {
    let mut p = enabled_policy(); // debounce_k=3, cooldown=300s
    let s = snap(&["collect"], None);
    assert!(p.evaluate(&s, 0).is_none(), "第 1 次异常：未达防抖阈值");
    assert!(p.evaluate(&s, 1).is_none(), "第 2 次异常：未达防抖阈值");
    assert!(p.evaluate(&s, 2).is_some(), "第 3 次异常触发首次动作");
    assert!(p.evaluate(&s, 3).is_none(), "冷却期内不重复动作");
    assert!(p.evaluate(&s, 302).is_some(), "冷却期结束后可再次动作");
}

#[test]
fn window_rate_limit_caps_actions() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        cooldown_secs: 0,
        max_actions_per_window: 2,
        ..Default::default()
    });
    let s = snap(&["collect"], None);
    assert!(p.evaluate(&s, 0).is_some());
    assert!(p.evaluate(&s, 1).is_some());
    assert!(p.evaluate(&s, 2).is_none(), "窗口内已达上限");
}

#[test]
fn fuse_after_repeated_failures() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        cooldown_secs: 0,
        max_actions_per_window: 100,
        fail_streak_to_fuse: 3,
        ..Default::default()
    });
    let s = snap(&["collect"], None);
    let target = "loop:collect";
    for t in 0..3 {
        assert!(p.evaluate(&s, t).is_some());
        p.record_outcome(target, false);
    }
    assert!(p.is_fused(target), "连续 3 次恢复失败应熔断");
    assert!(p.evaluate(&s, 10).is_none(), "熔断后不再动作");
}

#[test]
fn recovery_clears_fuse() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        cooldown_secs: 0,
        max_actions_per_window: 100,
        fail_streak_to_fuse: 1,
        ..Default::default()
    });
    let bad = snap(&["collect"], None);
    let good = snap(&[], None);
    assert!(p.evaluate(&bad, 0).is_some());
    p.record_outcome("loop:collect", false);
    assert!(p.is_fused("loop:collect"));
    // 恢复正常 → 解除熔断
    assert!(p.evaluate(&good, 1).is_none());
    assert!(!p.is_fused("loop:collect"));
    // 再次异常可重新触发
    assert!(p.evaluate(&bad, 2).is_some());
}

#[test]
fn cellular_signal_maps_to_reset_network() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        ..Default::default()
    });
    let s = snap(
        &[],
        Some(CellularHealth {
            dial_failed: true,
            link_down: true,
        }),
    );
    let d = p.evaluate(&s, 0).expect("蜂窝掉线应触发");
    assert_eq!(d.action, HealAction::ResetNetwork);
    assert_eq!(d.target, "cellular");
}

#[test]
fn priority_prefers_loop_over_cellular() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        ..Default::default()
    });
    let s = snap(
        &["collect"],
        Some(CellularHealth {
            dial_failed: true,
            link_down: true,
        }),
    );
    let d = p.evaluate(&s, 0).expect("应触发");
    assert_eq!(d.action, HealAction::RestartService);
    assert_eq!(d.target, "loop:collect");
}

// ===== P3: ResetSerial =====

fn dev(id: &str, failures: u32, bus: &str, serial: bool) -> DeviceHealth {
    DeviceHealth {
        id: id.to_string(),
        consecutive_failures: failures,
        quality: "stale".to_string(),
        bus: bus.to_string(),
        serial,
    }
}

#[test]
fn serial_bus_failures_map_to_reset_serial() {
    // 串口总线上设备进入退避（连续失败 >= 3）→ ResetSerial，target 为 bus:{name}
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        ..Default::default()
    });
    let mut s = snap(&[], None);
    s.devices = vec![
        dev("p1", 3, "/dev/ttyS1", true),
        dev("p2", 5, "/dev/ttyS1", true),
    ];
    let d = p.evaluate(&s, 0).expect("串口总线持续失败应触发");
    assert_eq!(d.action, HealAction::ResetSerial);
    assert_eq!(d.target, "bus:/dev/ttyS1", "同总线多设备只产出一条决策");
}

#[test]
fn tcp_bus_failures_do_not_trigger_reset_serial() {
    // TCP 总线设备失败不触发 ResetSerial（重构串口对 TCP 无意义）
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        ..Default::default()
    });
    let mut s = snap(&[], None);
    s.devices = vec![dev("p1", 10, "tcp://10.0.0.2:502", false)];
    assert!(p.evaluate(&s, 0).is_none());
}

#[test]
fn serial_failures_below_threshold_do_not_trigger() {
    // 未进入退避（< 3 次连续失败）不触发，避免单次抖动误动串口
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        ..Default::default()
    });
    let mut s = snap(&[], None);
    s.devices = vec![dev("p1", 2, "/dev/ttyS1", true)];
    assert!(p.evaluate(&s, 0).is_none());
}

#[test]
fn priority_prefers_cellular_over_serial() {
    // 蜂窝掉线与串口总线异常并存时，本周期先处理蜂窝（保持既有优先级序）
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        ..Default::default()
    });
    let mut s = snap(
        &[],
        Some(CellularHealth {
            dial_failed: true,
            link_down: true,
        }),
    );
    s.devices = vec![dev("p1", 5, "/dev/ttyS1", true)];
    let d = p.evaluate(&s, 0).expect("应触发");
    assert_eq!(d.action, HealAction::ResetNetwork);
}

// ===== r40: 熔断有效性 / 事件 id 唯一性 =====

/// r40-HEAL1 回归：动作执行返回成功 ≠ 异常已消除。
///
/// 对配置重载无法真正恢复的 target（如心跳僵死的任务循环），异常在下一轮快照依旧存在时，
/// 必须继续累计「恢复失败」次数，达 `fail_streak_to_fuse` 后熔断停止自动动作，
/// 避免每过冷却期就重复一次全量配置重载（周期性重载风暴）。
#[test]
fn persistent_anomaly_after_successful_action_fuses() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        cooldown_secs: 0,
        max_actions_per_window: 100,
        fail_streak_to_fuse: 3,
        ..Default::default()
    });
    let s = snap(&["collect"], None);
    let target = "loop:collect";
    // 首次动作：此前无动作历史，不计恢复失败
    assert!(p.evaluate(&s, 0).is_some());
    p.record_outcome(target, true); // 命令成功，但异常仍在
                                    // 每次动作前累计一次恢复失败：action#2 记 1、action#3 记 2，第 4 轮达 3 → 熔断
    assert!(p.evaluate(&s, 1).is_some());
    p.record_outcome(target, true);
    assert!(p.evaluate(&s, 2).is_some());
    p.record_outcome(target, true);
    assert!(
        p.evaluate(&s, 3).is_none(),
        "异常持续时达阈值应熔断，不再动作"
    );
    assert!(p.is_fused(target), "持续未恢复的 target 应被熔断");
}

/// r40-HEAL1 回归（反向）：动作成功后异常真正消除时，恢复分支须清零失败计数、
/// 解除「待确认」标记，避免后续新异常带着历史失败计数被过早熔断。
#[test]
fn recovery_after_action_clears_pending_failures() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        cooldown_secs: 0,
        max_actions_per_window: 100,
        fail_streak_to_fuse: 2,
        ..Default::default()
    });
    let bad = snap(&["collect"], None);
    let good = snap(&[], None);
    let target = "loop:collect";
    assert!(p.evaluate(&bad, 0).is_some());
    p.record_outcome(target, true);
    // 真正恢复正常：清空 bad_streak / fail_streak / fused / awaiting
    assert!(p.evaluate(&good, 1).is_none());
    assert!(!p.is_fused(target));
    // 再次异常：计数从 0 起算，首次动作不应立即熔断
    assert!(p.evaluate(&bad, 2).is_some());
    assert!(!p.is_fused(target), "恢复后重新异常不应携带历史失败计数");
}

/// r40-HEAL2 回归：同一秒内相同 action+target 的事件 id 必须唯一，
/// 否则前端以 id 作列表 key 会冲突（渲染错位 / 复用）。
#[test]
fn heal_event_id_is_unique_within_same_second() {
    let a = next_heal_event_id(1000, &HealAction::RestartService, "loop:collect");
    let b = next_heal_event_id(1000, &HealAction::RestartService, "loop:collect");
    assert_ne!(a, b, "同秒同 action+target 的 id 必须唯一");
    assert!(a.starts_with("1000-"), "id 应保留秒级时间前缀：{}", a);
}

// ===== r44: 动作超时不得取消在飞动作 =====

/// r44-HEAL1 回归：`run_action_with_timeout` 超时时只把本轮判定为失败，
/// **不得取消**在飞动作——否则 RestartService 的 A5 原子序列（reload→store→save）
/// 会被撕裂成「运行时已换、内存/磁盘未换」的半更新。
///
/// 断言机制：用一个「耗时 > 超时、且带副作用」的 future 模拟原子动作，
/// 旧实现（直接 `timeout(fut)`）会 drop 掉该 future 使副作用永不发生；
/// 新实现（spawn 后计时）应让副作用在超时后仍然完成。
#[tokio::test]
async fn action_timeout_does_not_cancel_in_flight_future() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let done = std::sync::Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let fut = async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        flag.store(true, Ordering::SeqCst);
        Ok(())
    };

    let mut in_flight = InFlightAction::default();
    let r = run_action_with_timeout(
        Duration::from_millis(20),
        "test action",
        fut,
        &mut in_flight,
    )
    .await;
    assert!(r.is_err(), "耗时超过超时应返回 Err");
    assert!(
        in_flight.is_busy(),
        "超时后动作仍在后台执行，必须被执行器跟踪"
    );

    // 给后台任务留出跑完的时间：副作用必须发生，证明动作未被取消。
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        done.load(Ordering::SeqCst),
        "超时后动作必须继续执行到完成（避免半更新），不得被取消"
    );
    assert!(!in_flight.is_busy(), "动作结束后应回收句柄，恢复下一轮决策");
}

/// r45-HEAL2 回归：超时遗留的在飞动作未结束时，执行器必须**跳过后续轮次**，
/// 不得在同一执行器上叠加第二个恢复动作。
///
/// 旧实现超时后只返回 Err，执行器 15s 后照常采样并可能再次动作；若前一动作仍在
/// `reload_lock`/串口重开中，新动作会排在其后，最坏形成重连风暴。
#[tokio::test]
async fn timed_out_action_blocks_next_cycle_until_finished() {
    let mut in_flight = InFlightAction::default();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let fut = async move {
        let _ = wait.await;
        Ok(())
    };
    let r = run_action_with_timeout(
        Duration::from_millis(10),
        "slow reload",
        fut,
        &mut in_flight,
    )
    .await;
    assert!(r.is_err(), "超时轮应记为失败");
    assert!(
        in_flight.is_busy(),
        "超时后动作仍在跑，下一轮必须被阻塞（run_once 据此跳过）"
    );

    release.send(()).unwrap();
    for _ in 0..100 {
        if !in_flight.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!in_flight.is_busy(), "动作完成后应允许下一轮动作");
}

/// r45-HEAL2 回归：停机时必须等待超时遗留的动作收尾，避免半更新。
#[tokio::test]
async fn shutdown_drain_waits_for_timeout_leftover() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let flag = finished.clone();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let fut = async move {
        let _ = wait.await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        flag.store(true, Ordering::SeqCst);
        Ok(())
    };
    let mut in_flight = InFlightAction::default();
    let r = run_action_with_timeout(
        Duration::from_millis(10),
        "slow reload",
        fut,
        &mut in_flight,
    )
    .await;
    assert!(r.is_err());

    // 松开动作，随后停机 drain 必须阻塞到动作真正完成。
    release.send(()).unwrap();
    in_flight.drain(Duration::from_secs(2)).await;
    assert!(
        finished.load(Ordering::SeqCst),
        "drain 返回时在飞动作必须已经完成（避免半写）"
    );
    assert!(!in_flight.is_busy(), "drain 后句柄应被释放");
}

/// r44-HEAL1 补充：未超时的动作须正常返回其结果（成功/失败）。
#[tokio::test]
async fn action_within_timeout_returns_result() {
    let mut slot = InFlightAction::default();
    let ok = run_action_with_timeout(
        Duration::from_millis(500),
        "fast ok",
        async { Ok::<(), String>(()) },
        &mut slot,
    )
    .await;
    assert!(ok.is_ok(), "未超时应透传成功结果");
    assert!(!slot.is_busy(), "未超时的动作不应残留句柄");

    let err = run_action_with_timeout(
        Duration::from_millis(500),
        "fast err",
        async { Err("boom".to_string()) },
        &mut slot,
    )
    .await;
    assert_eq!(err, Err("boom".to_string()), "未超时应透传失败结果");
    assert!(!slot.is_busy());
}

/// r45-HEAL1 回归：`ResetSerial` 只能定向重建目标串口总线。
///
/// 旧实现完全忽略 `target`，一条串口总线异常会把**所有**串口总线全部重建：
/// 单一健康快照里的一个异常信号被放大成全局动作，正常总线上的设备在重开期间
/// 全部拿到 `BusClosed`。这里锁定目标解析/校验口径（前缀剥离 + 必须是串口）。
#[test]
fn reset_serial_target_must_be_configured_serial_bus() {
    use gateway_core::config::BusConfig;

    let buses = vec![
        BusConfig {
            port_name: "/dev/ttyS1".to_string(),
            protocol: "modbus".to_string(),
            tcp_addr: None,
            ..BusConfig::default()
        },
        BusConfig {
            port_name: "plc-a".to_string(),
            protocol: "s7".to_string(),
            tcp_addr: Some("10.0.0.2:102".to_string()),
            ..BusConfig::default()
        },
    ];

    assert_eq!(
        serial_bus_target(&buses, "bus:/dev/ttyS1"),
        Ok("/dev/ttyS1".to_string()),
        "应剥离 bus: 前缀并定位到串口总线"
    );
    assert_eq!(
        serial_bus_target(&buses, "/dev/ttyS1"),
        Ok("/dev/ttyS1".to_string())
    );
    assert!(
        serial_bus_target(&buses, "bus:plc-a").is_err(),
        "TCP 总线不得触发串口重建"
    );
    assert!(
        serial_bus_target(&buses, "bus:").is_err(),
        "空目标必须报错，不能退化为全局重建"
    );
    assert!(serial_bus_target(&buses, "bus:/dev/ttyS9").is_err());
}

#[test]
fn settings_from_config_maps_fields() {
    let c = gateway_core::config::SelfHealConfig {
        enabled: true,
        check_interval_secs: 20,
        debounce_k: 4,
        cooldown_secs: 60,
        window_secs: 1800,
        max_actions_per_window: 5,
        fail_streak_to_fuse: 2,
    };
    let s: SelfHealSettings = c.into();
    assert!(s.enabled);
    assert_eq!(s.check_interval_secs, 20);
    assert_eq!(s.debounce_k, 4);
    assert_eq!(s.cooldown_secs, 60);
    assert_eq!(s.window_secs, 1800);
    assert_eq!(s.max_actions_per_window, 5);
    assert_eq!(s.fail_streak_to_fuse, 2);
}

// ===== r45-HEAL1：ResetSerial 定向重建（决策层） =====

/// r45-HEAL1 回归：`ResetSerial` 决策的 target 必须精确到**总线名**，
/// 且同一条总线上的多台设备只产出一条决策（不重复触发同一条总线的重建）。
///
/// 旧实现里 `reset_serial` 忽略 target，一条串口总线异常会把**所有**串口总线
/// 全部重建：单一健康快照里的一个异常信号被放大成全局动作。
#[test]
fn reset_serial_decision_targets_one_bus_only() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        ..Default::default()
    });
    let mut s = snap(&[], None);
    // 同一条串口总线上的 3 台设备全部进入退避
    s.devices = vec![
        dev("p1", 3, "/dev/ttyS1", true),
        dev("p2", 4, "/dev/ttyS1", true),
        dev("p3", 5, "/dev/ttyS1", true),
    ];
    let d = p.evaluate(&s, 0).expect("串口总线持续失败应触发");
    assert_eq!(d.action, HealAction::ResetSerial);
    assert_eq!(
        d.target, "bus:/dev/ttyS1",
        "同总线多设备只产出一条决策，且 target 精确到总线"
    );
}

/// 两条串口总线同时异常时，每个周期**只产出一条**决策，且目标精确到某一条总线
/// ——不得出现「一次动作作用于全部总线」的全局决策。
#[test]
fn reset_serial_decision_targets_one_bus_each_cycle() {
    let mut p = HealPolicy::new(SelfHealSettings {
        enabled: true,
        debounce_k: 1,
        cooldown_secs: 0,
        ..Default::default()
    });
    let mut s = snap(&[], None);
    s.devices = vec![
        dev("p1", 3, "/dev/ttyS1", true),
        dev("p2", 3, "/dev/ttyS2", true),
    ];
    let d = p.evaluate(&s, 0).expect("应触发一次动作");
    assert_eq!(d.action, HealAction::ResetSerial);
    assert!(
        d.target == "bus:/dev/ttyS1" || d.target == "bus:/dev/ttyS2",
        "target 必须精确到某一条总线，实际 {}",
        d.target
    );
    // 候选集合覆盖两条异常总线（每条各一条决策），但每周期只执行其中一条
    let candidates = p.candidates(&s);
    assert_eq!(candidates.len(), 2, "两条异常总线应各有候选决策");
    let targets: Vec<&str> = candidates.iter().map(|c| c.target.as_str()).collect();
    assert!(targets.contains(&"bus:/dev/ttyS1"), "{targets:?}");
    assert!(targets.contains(&"bus:/dev/ttyS2"), "{targets:?}");
}

/// 挂在 TCP 总线上的设备不进入 `bad_serial_buses`（不触发串口重建）。
#[test]
fn tcp_bus_devices_are_not_bad_serial_buses() {
    let mut s = snap(&[], None);
    s.devices = vec![dev("p1", 9, "tcp://10.0.0.2:502", false)];
    assert!(
        bad_serial_buses(&s).is_empty(),
        "TCP 总线设备不得进入串口异常集合"
    );
}

/// 未进入退避阈值的设备不算异常总线。
#[test]
fn devices_below_backoff_threshold_are_not_bad_serial_buses() {
    let mut s = snap(&[], None);
    s.devices = vec![dev("p1", SERIAL_FAIL_THRESHOLD - 1, "/dev/ttyS1", true)];
    assert!(bad_serial_buses(&s).is_empty());
}

// ===== r45-HEAL2：InFlightAction 生命周期 =====

/// r45-HEAL2 回归：空闲状态 `is_busy` 恒为 false（首轮不应被误阻塞）。
#[tokio::test]
async fn in_flight_is_idle_when_no_action_tracked() {
    let mut slot = InFlightAction::default();
    assert!(!slot.is_busy(), "无在飞动作时不应阻塞执行器");
    // drain 空句柄是安全的空操作
    slot.drain(Duration::from_millis(10)).await;
    assert!(!slot.is_busy());
}

/// r45-HEAL2 回归：动作结束后 `is_busy` 必须回收句柄，
/// 否则执行器会永久卡在「跳过本轮」状态（自愈彻底失效）。
#[tokio::test]
async fn in_flight_reclaims_finished_handle() {
    let mut slot = InFlightAction::default();
    let r = run_action_with_timeout(
        Duration::from_millis(500),
        "fast",
        async { Ok::<(), String>(()) },
        &mut slot,
    )
    .await;
    assert!(r.is_ok());
    assert!(!slot.is_busy(), "已完成的动作不应残留句柄");
}

/// r45-HEAL2 回归：停机 drain 超时（动作赖着不结束）不得 panic，
/// 且必须释放句柄——否则停机会一直挂在这个动作上。
#[tokio::test]
async fn drain_gives_up_after_grace_and_releases_handle() {
    let mut slot = InFlightAction::default();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let fut = async move {
        let _ = wait.await;
        Ok(())
    };
    let r = run_action_with_timeout(Duration::from_millis(10), "stuck", fut, &mut slot).await;
    assert!(r.is_err());
    assert!(slot.is_busy());

    // 宽限期很短，动作来不及结束 → drain 超时返回
    let start = std::time::Instant::now();
    slot.drain(Duration::from_millis(50)).await;
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "drain 必须在宽限期后返回，不得无限等待"
    );
    assert!(!slot.is_busy(), "drain 后句柄应被释放");
    // 释放后台任务，避免测试结束时残留
    release.send(()).ok();
}

/// r45-HEAL2 回归：drain 等待期间动作完成 → 返回后动作确实已完成。
#[tokio::test]
async fn drain_waits_until_action_finishes() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let flag = finished.clone();
    let mut slot = InFlightAction::default();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let fut = async move {
        let _ = wait.await;
        flag.store(true, Ordering::SeqCst);
        Ok(())
    };
    let r = run_action_with_timeout(Duration::from_millis(10), "slow", fut, &mut slot).await;
    assert!(r.is_err());

    release.send(()).unwrap();
    slot.drain(Duration::from_secs(2)).await;
    assert!(
        finished.load(Ordering::SeqCst),
        "drain 返回时在飞动作必须已完成"
    );
    assert!(!slot.is_busy());
}

/// r45-HEAL2 回归：超时动作在后台完成前，执行器必须整轮跳过——
/// 不采样、不决策、不执行第二个恢复动作（避免动作叠加成重连风暴）。
///
/// 反证闭环：把 `run_once` 里的 `in_flight.is_busy()` 判断去掉，本测试必然失败
/// （会产出第二条 heal 事件）。
#[tokio::test]
async fn busy_in_flight_blocks_whole_cycle_not_just_execution() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let action_done = std::sync::Arc::new(AtomicBool::new(false));
    let flag = action_done.clone();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let mut in_flight = InFlightAction::default();

    // 模拟一次超时动作：句柄被跟踪，动作仍在后台跑
    let r = run_action_with_timeout(
        Duration::from_millis(10),
        "slow reload",
        async move {
            let _ = wait.await;
            flag.store(true, Ordering::SeqCst);
            Ok(())
        },
        &mut in_flight,
    )
    .await;
    assert!(r.is_err(), "超时轮应记为失败");

    // 动作仍在后台：执行器必须整轮跳过
    assert!(in_flight.is_busy(), "超时后动作仍在跑，下一轮必须被阻塞");

    // 动作结束后恢复决策能力
    release.send(()).unwrap();
    for _ in 0..100 {
        if !in_flight.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!in_flight.is_busy(), "动作完成后应允许下一轮动作");
    assert!(action_done.load(Ordering::SeqCst));
}
