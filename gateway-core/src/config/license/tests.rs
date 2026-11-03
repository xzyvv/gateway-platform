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
