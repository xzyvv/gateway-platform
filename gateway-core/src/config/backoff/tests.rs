use super::*;
use std::time::Duration;

/// MQTT 默认表：默认序列必须精确复刻现状 2,4,8,16,32,60,60……
#[test]
fn mqtt_default_sequence_matches_legacy() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![2, 4, 8, 16, 32, 60],
        jitter_ratio: 0.4,
    };
    let got: Vec<f64> = (1..=8).map(|a| cfg.base_delay_secs(a)).collect();
    assert_eq!(got, vec![2.0, 4.0, 8.0, 16.0, 32.0, 60.0, 60.0, 60.0]);
}

/// Modbus TCP 默认表：1,2,5,10,30，用尽后保持 30
#[test]
fn bus_default_sequence_matches_legacy() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![1, 2, 5, 10, 30],
        jitter_ratio: 0.0,
    };
    let got: Vec<f64> = (1..=7).map(|a| cfg.base_delay_secs(a)).collect();
    assert_eq!(got, vec![1.0, 2.0, 5.0, 10.0, 30.0, 30.0, 30.0]);
}

/// attempt 从 1 开始：第 1 次失败取第 1 档，而非第 0 档以外的偏移
#[test]
fn base_delay_attempt_is_one_based() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![2, 4, 8],
        jitter_ratio: 0.0,
    };
    assert_eq!(cfg.base_delay_secs(1), 2.0);
    assert_eq!(cfg.base_delay_secs(2), 4.0);
    assert_eq!(cfg.base_delay_secs(3), 8.0);
}

/// 空表兜底返回 1.0
#[test]
fn base_delay_empty_table_falls_back_to_one() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![],
        jitter_ratio: 0.4,
    };
    assert_eq!(cfg.base_delay_secs(1), 1.0);
    assert_eq!(cfg.base_delay_secs(100), 1.0);
}

/// rand01=0.5 时抖动项为 0，结果应精确等于 base（保证默认序列可复现）
#[test]
fn delay_secs_no_jitter_at_midpoint() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![2, 4, 8, 16, 32, 60],
        jitter_ratio: 0.4,
    };
    for (attempt, base) in (1..=6).zip([2.0, 4.0, 8.0, 16.0, 32.0, 60.0]) {
        assert_eq!(cfg.delay_secs(attempt, 0.5), base);
    }
}

/// jitter_ratio=0.0 时任意随机数结果都等于 base
#[test]
fn delay_secs_zero_jitter_is_deterministic() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![1, 2, 5, 10, 30],
        jitter_ratio: 0.0,
    };
    assert_eq!(cfg.delay_secs(3, 0.0), 5.0);
    assert_eq!(cfg.delay_secs(3, 1.0), 5.0);
    assert_eq!(cfg.delay_secs(3, 0.37), 5.0);
}

/// 抖动范围符合 ±jitter_ratio/2：40% 抖动在 rand01∈[0,1) 时落在 base±20%
#[test]
fn delay_secs_jitter_bounds() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![10],
        jitter_ratio: 0.4,
    };
    assert!((cfg.delay_secs(1, 0.0) - 8.0).abs() < 1e-9);
    assert!((cfg.delay_secs(1, 1.0) - 12.0).abs() < 1e-9);
    assert!((cfg.delay_secs(1, 0.5) - 10.0).abs() < 1e-9);
}

/// 下界 1.0：抖动把结果压到 1 秒以下时须被夹紧
#[test]
fn delay_secs_clamped_to_one_second() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![1],
        jitter_ratio: 1.0,
    };
    // base=1, rand01=0 -> 1 + 1*1*(-0.5) = 0.5 -> 夹到 1.0
    assert_eq!(cfg.delay_secs(1, 0.0), 1.0);
}

/// time_rand01 落在 [0,1)
#[test]
fn time_rand01_in_unit_range() {
    for _ in 0..100 {
        let r = ReconnectBackoffConfig::time_rand01();
        assert!((0.0..1.0).contains(&r), "rand01 越界: {r}");
    }
}

/// 默认反序列化：缺 seconds 时用兜底表，缺 jitter_ratio 时为 0.0
#[test]
fn deserialize_defaults() {
    let cfg: ReconnectBackoffConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.seconds, vec![1, 2, 5, 10, 30]);
    assert_eq!(cfg.jitter_ratio, 0.0);
}

/// 校验：合法配置通过
#[test]
fn validate_ok() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![2, 4, 8, 16, 32, 60],
        jitter_ratio: 0.4,
    };
    assert!(cfg.validate().is_ok());
}

/// 校验：空表被拒
#[test]
fn validate_rejects_empty() {
    use validator::Validate;
    let cfg = ReconnectBackoffConfig {
        seconds: vec![],
        jitter_ratio: 0.4,
    };
    assert!(cfg.validate().is_err());
}

/// 校验：存在 <1 的档位被拒
#[test]
fn validate_rejects_zero_element() {
    use validator::Validate;
    let cfg = ReconnectBackoffConfig {
        seconds: vec![0, 2, 4],
        jitter_ratio: 0.0,
    };
    assert!(cfg.validate().is_err());
}

/// 校验：非递减序列被拒（[5,2]）
#[test]
fn validate_rejects_unsorted() {
    use validator::Validate;
    let cfg = ReconnectBackoffConfig {
        seconds: vec![5, 2],
        jitter_ratio: 0.0,
    };
    assert!(cfg.validate().is_err());
}

/// 校验：相等档位允许（非递减，而非严格递增）
#[test]
fn validate_allows_equal_neighbors() {
    use validator::Validate;
    let cfg = ReconnectBackoffConfig {
        seconds: vec![1, 2, 2, 4],
        jitter_ratio: 0.0,
    };
    assert!(cfg.validate().is_ok());
}

/// 串口后台重连纯函数：默认 20% 抖动下 rand01=0/0.5/1 落在 base±10% 边界与中值。
#[test]
fn next_reconnect_backoff_default_jitter_bounds() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![1, 2, 5, 10, 30],
        jitter_ratio: 0.2,
    };
    for (attempt, base) in (1..=5).zip([1.0, 2.0, 5.0, 10.0, 30.0]) {
        let lo = next_reconnect_backoff(&cfg, attempt, 0.0);
        let mid = next_reconnect_backoff(&cfg, attempt, 0.5);
        let hi = next_reconnect_backoff(&cfg, attempt, 1.0);
        // delay_secs 有 .max(1.0) 下界：base=1 的负抖动被钳到 1.0 而非 0.9。
        assert!((lo.as_secs_f64() - (base * 0.9f64).max(1.0)).abs() < 1e-9);
        assert!((mid.as_secs_f64() - base).abs() < 1e-9);
        assert!((hi.as_secs_f64() - base * 1.1).abs() < 1e-9);
    }
}

/// 同一 rand01 下延迟随失败次数单调不减；用尽档位后保持在上限档位。
#[test]
fn next_reconnect_backoff_monotonic_and_capped() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![1, 2, 5, 10, 30],
        jitter_ratio: 0.2,
    };
    let mut prev = Duration::ZERO;
    for attempt in 1..=20 {
        let cur = next_reconnect_backoff(&cfg, attempt, 0.5);
        assert!(cur >= prev, "延迟必须单调不减: {:?} -> {:?}", prev, cur);
        assert!(cur <= Duration::from_secs_f64(33.0), "不得超过上限档位+抖动");
        prev = cur;
    }
    assert_eq!(
        next_reconnect_backoff(&cfg, 100, 0.5),
        Duration::from_secs(30)
    );
}

/// jitter=0 时纯函数结果与档位精确相等（TCP 侧统一走 delay_secs 后行为不变）。
#[test]
fn next_reconnect_backoff_zero_jitter_matches_base() {
    let cfg = ReconnectBackoffConfig {
        seconds: vec![1, 2, 5, 10, 30],
        jitter_ratio: 0.0,
    };
    for attempt in 1..=8 {
        let d = next_reconnect_backoff(&cfg, attempt, 0.13);
        assert_eq!(d, Duration::from_secs_f64(cfg.base_delay_secs(attempt)));
    }
}
