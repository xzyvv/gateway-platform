use super::*;

#[test]
fn test_default_absent_quota_values() {
    let q = DEFAULT_ABSENT_QUOTA;
    assert_eq!(q.max_points, Some(100));
    assert_eq!(q.max_protocols, Some(1));
    assert_eq!(q.storage_days, Some(30));
    assert_eq!(q.northbound_channels, Some(1));
}

#[test]
fn test_effective_absent_quota_uses_default_when_unset() {
    let cfg = LicenseConfig::default();
    let q = cfg.effective_absent_quota();
    assert_eq!(q.max_points, Some(100));
}

#[test]
fn test_effective_absent_quota_overrides_default() {
    let cfg = LicenseConfig {
        absent_quota: Some(Quota {
            max_points: Some(50),
            ..Quota::default()
        }),
        ..LicenseConfig::default()
    };
    let q = cfg.effective_absent_quota();
    assert_eq!(q.max_points, Some(50));
    assert_eq!(q.max_protocols, None, "未指定字段保持 None");
}

#[test]
fn test_quota_for_tier_uses_configured_value() {
    let cfg = LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Pro,
            quota: Quota {
                max_points: Some(5000),
                max_protocols: Some(16),
                storage_days: Some(365),
                northbound_channels: Some(8),
            },
        }]),
        ..LicenseConfig::default()
    };
    let q = cfg.quota_for_tier(Tier::Pro);
    assert_eq!(q.max_points, Some(5000));
}

#[test]
fn test_quota_for_tier_falls_back_to_builtin_default() {
    let cfg = LicenseConfig::default();
    // Pro 未在 tier_quotas 中 → 回落代码内置默认表（Pro=2500 点 / 协议不限 / 30 天 / 通道不限）
    let q = cfg.quota_for_tier(Tier::Pro);
    assert_eq!(q.max_points, Some(2500));
    assert_eq!(q.max_protocols, None);
    assert_eq!(q.storage_days, Some(30));
    assert_eq!(q.northbound_channels, None);
}

#[test]
fn test_quota_for_tier_builtin_default_covers_all_four_tiers() {
    let cfg = LicenseConfig::default();
    assert_eq!(cfg.quota_for_tier(Tier::Core).max_points, Some(500));
    assert_eq!(cfg.quota_for_tier(Tier::Standard).max_points, Some(1500));
    assert_eq!(cfg.quota_for_tier(Tier::Pro).max_points, Some(2500));
    // Ultra 同样有 5000 点上限（v2.1 起唯一差异是 max_points）
    assert_eq!(cfg.quota_for_tier(Tier::Ultra).max_points, Some(5000));
}

/// 内置默认表数值与 config.json.example 的 tier_quotas 一致（防止两边漂移）
///
/// v2.1 口径：四档唯一差异是 `max_points`；协议数与北向通道数不限，
/// 历史保留天数统一 30 天。
#[test]
fn test_builtin_default_table_values() {
    for (tier, points) in [
        (Tier::Core, 500),
        (Tier::Standard, 1500),
        (Tier::Pro, 2500),
        (Tier::Ultra, 5000),
    ] {
        let q = default_quota_for_tier(tier);
        assert_eq!(q.max_points, Some(points), "{tier:?} 点位上限");
        assert_eq!(q.max_protocols, None, "{tier:?} 协议数不限");
        assert_eq!(q.storage_days, Some(30), "{tier:?} 历史统一 30 天");
        assert_eq!(q.northbound_channels, None, "{tier:?} 北向通道不限");
    }
}

/// 内置默认表不覆盖配置值：配置写了就以配置为准
#[test]
fn test_builtin_default_does_not_override_configured_value() {
    let cfg = LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Pro,
            quota: Quota {
                max_points: Some(123),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    };
    assert_eq!(cfg.quota_for_tier(Tier::Pro).max_points, Some(123));
}

#[test]
fn test_quota_for_tier_partial_field_none_means_unlimited() {
    let cfg = LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Standard,
            quota: Quota {
                max_points: Some(1500),
                max_protocols: None, // 协议不限
                storage_days: Some(90),
                northbound_channels: None, // 通道不限
            },
        }]),
        ..LicenseConfig::default()
    };
    let q = cfg.quota_for_tier(Tier::Standard);
    assert_eq!(q.max_points, Some(1500));
    assert_eq!(q.max_protocols, None);
    assert!(q.max_protocols.is_none(), "None = 无限");
}

/// 配置 JSON 往返：模拟运维在 config.json 写 tier_quotas 节点，反序列化还原
#[test]
fn test_tier_quotas_json_round_trip() {
    let json = r#"{
        "tier_quotas": [
            { "tier": "pro", "quota": { "max_points": 5000, "max_protocols": 16, "storage_days": 365, "northbound_channels": 8 } },
            { "tier": "core", "quota": { "max_points": 500, "max_protocols": 3, "storage_days": 30, "northbound_channels": 1 } }
        ],
        "absent_quota": { "max_points": 100, "max_protocols": 1, "storage_days": 30, "northbound_channels": 1 }
    }"#;
    let cfg: LicenseConfig = serde_json::from_str(json).unwrap();
    let pro = cfg.quota_for_tier(Tier::Pro);
    assert_eq!(pro.max_points, Some(5000));
    let core = cfg.quota_for_tier(Tier::Core);
    assert_eq!(core.max_points, Some(500));
    let absent = cfg.effective_absent_quota();
    assert_eq!(absent.max_points, Some(100));
}

// ===== tier 显式配置判定（启动告警 / 前端提示的依据）=====
//
// 注意：这里判定的是「运维有没有在 config.json 里显式写这一档」，
// 与「生效配额」解耦——未显式写的档位生效配额来自代码内置默认表，
// 不会退到 absent 兜底。

/// tier_configured：只有显式配了该 tier 才算已配置
#[test]
fn test_tier_configured_only_for_explicit_entries() {
    let cfg = LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Pro,
            quota: Quota {
                max_points: Some(5000),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    };
    assert!(cfg.tier_configured(Tier::Pro));
    assert!(!cfg.tier_configured(Tier::Ultra));
    assert!(!cfg.tier_configured(Tier::Core));
    assert!(!cfg.tier_configured(Tier::Standard));
}

/// 整个 tier_quotas 未配置 → 四档全缺
#[test]
fn test_missing_tiers_all_when_unconfigured() {
    let cfg = LicenseConfig::default();
    let missing = cfg.missing_tiers();
    assert_eq!(missing.len(), 4);
    assert!(missing.contains(&Tier::Core));
    assert!(missing.contains(&Tier::Standard));
    assert!(missing.contains(&Tier::Pro));
    assert!(missing.contains(&Tier::Ultra));
}

/// 只配了一档 → 其余三档列为缺失
#[test]
fn test_missing_tiers_reports_only_unconfigured_ones() {
    let cfg = LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Standard,
            quota: Quota {
                max_points: Some(1500),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    };
    let missing = cfg.missing_tiers();
    assert_eq!(missing.len(), 3);
    assert!(!missing.contains(&Tier::Standard));
    assert!(missing.contains(&Tier::Core));
    assert!(missing.contains(&Tier::Pro));
    assert!(missing.contains(&Tier::Ultra));
}

/// 四档写全 → 无缺失
#[test]
fn test_missing_tiers_empty_when_all_configured() {
    let q = Quota {
        max_points: Some(500),
        ..Quota::default()
    };
    let cfg = LicenseConfig {
        tier_quotas: Some(vec![
            TierQuotaEntry {
                tier: Tier::Core,
                quota: q.clone(),
            },
            TierQuotaEntry {
                tier: Tier::Standard,
                quota: q.clone(),
            },
            TierQuotaEntry {
                tier: Tier::Pro,
                quota: q.clone(),
            },
            TierQuotaEntry {
                tier: Tier::Ultra,
                quota: q,
            },
        ]),
        ..LicenseConfig::default()
    };
    assert!(cfg.missing_tiers().is_empty());
}
