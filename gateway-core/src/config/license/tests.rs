use super::*;

#[test]
fn test_default_absent_quota_values() {
    let q = DEFAULT_ABSENT_QUOTA;
    assert_eq!(q.max_points, Some(100));
    assert_eq!(q.max_protocols, Some(1));
    assert_eq!(q.storage_days, Some(7));
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
fn test_quota_for_tier_falls_back_to_absent() {
    let cfg = LicenseConfig::default();
    let q = cfg.quota_for_tier(Tier::Ultra);
    // Ultra 未在 tier_quotas 中 → 降级到 absent_quota 默认
    assert_eq!(q.max_points, Some(100));
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
        "absent_quota": { "max_points": 100, "max_protocols": 1, "storage_days": 7, "northbound_channels": 1 }
    }"#;
    let cfg: LicenseConfig = serde_json::from_str(json).unwrap();
    let pro = cfg.quota_for_tier(Tier::Pro);
    assert_eq!(pro.max_points, Some(5000));
    let core = cfg.quota_for_tier(Tier::Core);
    assert_eq!(core.max_points, Some(500));
    let absent = cfg.effective_absent_quota();
    assert_eq!(absent.max_points, Some(100));
}

// ===== tier 缺失判定（启动告警 / 前端提示的依据）=====

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
