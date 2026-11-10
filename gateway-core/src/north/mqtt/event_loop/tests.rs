use super::*;

/// r45-MQTT1 回归：退避必须叠加配置的抖动比例（MQTT 默认 0.4）。
///
/// 旧实现（ConnAck 被拒分支用无抖动的 `base_delay_secs`）会忽略这里的
/// `rand01`，使 lo/hi 恒等于基准；本测试的 `lo < base < hi` 断言即会失败。
#[test]
fn reconnect_delay_applies_configured_jitter() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![60],
        jitter_ratio: 0.4,
    };
    let base = cfg.base_delay_secs(1);
    assert_eq!(base, 60.0);

    let lo = reconnect_delay_secs(&cfg, 1, 0.0); // 下界 = base*(1 - 0.4/2)
    let hi = reconnect_delay_secs(&cfg, 1, 1.0); // 上界 = base*(1 + 0.4/2)

    assert!(lo < base, "rand01=0 应低于基准，抖动未生效: {}", lo);
    assert!(hi > base, "rand01=1 应高于基准，抖动未生效: {}", hi);
    assert!(
        (lo - base * 0.8).abs() < 1e-9 && (hi - base * 1.2).abs() < 1e-6,
        "抖动应落在基准 ±20% 内（lo={} hi={}）",
        lo,
        hi
    );
}

/// `jitter_ratio=0.0`（如 Modbus TCP 默认）时退避仍等于基准，保证不改变既有节奏。
#[test]
fn reconnect_delay_is_base_without_jitter() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![2, 4, 8],
        jitter_ratio: 0.0,
    };
    for attempt in 1..=5 {
        assert_eq!(
            reconnect_delay_secs(&cfg, attempt, 0.77),
            cfg.base_delay_secs(attempt),
            "attempt={} 无抖动时应等于基准",
            attempt
        );
    }
}
