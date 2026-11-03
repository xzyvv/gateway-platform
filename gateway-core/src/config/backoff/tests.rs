use super::*;

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
