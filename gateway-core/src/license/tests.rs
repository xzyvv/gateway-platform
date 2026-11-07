use super::*;
use test_support::*;

fn manager(dir: &Path) -> LicenseManager {
    LicenseManager::with_public_key(&dir.join("clock.db"), public_key())
}

fn write_lic(dir: &Path, text: &str) -> PathBuf {
    let p = dir.join("test.lic");
    std::fs::write(&p, text).unwrap();
    p
}

// ===== load 全链路 =====

#[test]
fn test_load_missing_file_is_ok_absent() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    assert!(m.load(&dir.path().join("nope.lic")).is_ok());
    let s = m.current();
    assert!(!s.present);
    assert_eq!(s.tier, Tier::Core);
    assert_eq!(s.status, LicenseStatus::Ok);
    // 无 license：tier=core（v2.0 调试功能不挂 gate，档位仅作展示）
    assert_eq!(s.tier, Tier::Core);
}

#[test]
fn test_load_valid_license() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let p = write_lic(dir.path(), &sign_payload(&test_payload()));
    // 用注入指纹校验（load() 走真实机器指纹，不适合单测断言）
    // now 取真实时间：时钟基准会与 license 文件 mtime 交叉校验（#18），
    // 伪造的远古时间戳会被判为回拨。
    let (s, err) = m.evaluate_at(&p, &matching_fp(), chrono::Utc::now().timestamp());
    assert!(err.is_none());
    m.state.store(Arc::new(s));
    let s = m.current();
    assert!(s.present);
    assert_eq!(s.tier, Tier::Pro);
    assert_eq!(s.status, LicenseStatus::Ok);
    assert_eq!(s.license_id.as_deref(), Some("LIC-TEST-0001"));
    assert_eq!(s.customer.as_deref(), Some("某某自动化"));
    // B3：无 quota 字段的老 payload → license 自带 quota 全空；
    // current() 在无 config 注入时回落该 tier 的代码内置默认配额（Pro=2500 点 / 协议不限）
    assert_eq!(s.quota.max_points, Some(2500));
    assert_eq!(s.quota.max_protocols, None);
    assert_eq!(s.quota_source, QuotaSource::TierMissing);
}

// ===== v2.0 Step 2 LicenseManager::current() quota 解析 =====

/// 测试辅助：注入匹配指纹评估 + 直接 store，跳过真实机器 fingerprint
fn load_with_matching_fp(m: &LicenseManager, p: &LicensePayload, dir: &Path) {
    let path = write_lic(dir, &sign_payload(p));
    let (s, _err) = m.evaluate_at(&path, &matching_fp(), chrono::Utc::now().timestamp());
    m.state.store(Arc::new(s));
}

/// quota_config 未注入时，license 自带 quota 字段原样保留
#[test]
fn test_current_quota_uses_license_payload_when_config_absent() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let mut p = test_payload();
    p.quota = Some(Quota {
        max_points: Some(5000),
        max_protocols: Some(16),
        storage_days: Some(365),
        northbound_channels: Some(8),
    });
    load_with_matching_fp(&m, &p, dir.path());
    let s = m.current();
    // license 自带 quota 非全空 → config 不覆盖
    assert_eq!(s.quota.max_points, Some(5000));
    assert_eq!(s.quota.max_protocols, Some(16));
}

/// quota_config 注入但 license 自带 quota 非空 → license 优先（不覆盖）
#[test]
fn test_current_quota_license_wins_over_config() {
    use crate::config::{LicenseConfig, TierQuotaEntry};
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let mut p = test_payload();
    p.quota = Some(Quota {
        max_points: Some(5000),
        ..Quota::default()
    });
    load_with_matching_fp(&m, &p, dir.path());
    // 注入 config tier=pro 但 max_points=999
    m.set_quota_config(LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Pro,
            quota: Quota {
                max_points: Some(999),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    });
    let s = m.current();
    // license 自带 quota 优先（max_points=5000）→ config 不生效
    assert_eq!(s.quota.max_points, Some(5000));
}

/// quota_config 注入 + license 无 quota → 按 tier 查 config
#[test]
fn test_current_quota_falls_back_to_config_when_license_empty() {
    use crate::config::{LicenseConfig, TierQuotaEntry};
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    // test_payload.quota = None → license 自带 quota 全空
    load_with_matching_fp(&m, &test_payload(), dir.path());
    m.set_quota_config(LicenseConfig {
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
    });
    let s = m.current();
    assert_eq!(s.tier, Tier::Pro);
    // license 自带 quota 全空 → config tier_quotas 兜底
    assert_eq!(s.quota.max_points, Some(5000));
    assert_eq!(s.quota.max_protocols, Some(16));
}

/// quota_config 注入但 tier_quotas 未写该 tier
/// → 回落代码内置默认表（Pro=2500 点 / 协议不限 / 30 天），不再退到 absent 兜底
#[test]
fn test_current_quota_falls_back_to_builtin_tier_default() {
    use crate::config::LicenseConfig;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    load_with_matching_fp(&m, &test_payload(), dir.path());
    // config tier_quotas 不填 → 走内置默认表
    m.set_quota_config(LicenseConfig {
        tier_quotas: None,
        ..LicenseConfig::default()
    });
    let s = m.current();
    // 内置默认表 Pro 档（v2.1：2500 点 / 协议不限 / 30 天 / 通道不限）
    assert_eq!(s.quota.max_points, Some(2500));
    assert_eq!(s.quota.max_protocols, None);
    assert_eq!(s.quota.storage_days, Some(30));
    assert_eq!(s.quota_source, QuotaSource::TierMissing);
}

/// Bug T7 回归：config.json 未配 license 节（未调用 set_quota_config）+ 授权已过期
/// 降级（present=false）时，`current()` 不得把配额静默解除为无限——必须回退到
/// DEFAULT_ABSENT_QUOTA（100/1/7/1）。
#[test]
fn test_current_quota_degraded_without_config_is_restricted() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    // 构造「签名有效但已过期」的 license：evaluate_at 应判定 Expired 并降级
    let mut p = test_payload();
    p.expires_at = Some("2020-01-01T00:00:00Z".to_string());
    let lic = write_lic(dir.path(), &sign_payload(&p));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (s, _err) = m.evaluate_at(&lic, &matching_fp(), now);
    assert!(!s.present, "过期 license 不应判定为 present");
    assert_eq!(s.status, LicenseStatus::Expired);
    m.replace_state_for_test(s);

    let s = m.current();
    assert!(!s.present);
    // Bug T7: 降级态配额必须受限（absent 兜底），而非全 None（无限）
    assert_eq!(s.quota.max_points, Some(100));
    assert_eq!(s.quota.max_protocols, Some(1));
    assert!(m.check_quota(QuotaKind::Points, 101).is_err());
    assert!(m.check_quota(QuotaKind::Points, 100).is_ok());
}

// ===== v2.0 配额来源可观测（quota_source）=====

/// license 自带 quota → 来源 = License
#[test]
fn test_quota_source_license_when_payload_carries_quota() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let mut p = test_payload();
    p.quota = Some(Quota {
        max_points: Some(5000),
        ..Quota::default()
    });
    load_with_matching_fp(&m, &p, dir.path());
    let s = m.current();
    assert_eq!(s.quota_source, QuotaSource::License);
    assert_eq!(s.quota.max_points, Some(5000));
}

/// license 有效但未自带 quota + tier_quotas 配了该档 → 来源 = TierConfig
#[test]
fn test_quota_source_tier_config_when_mapping_hit() {
    use crate::config::{LicenseConfig, TierQuotaEntry};
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    load_with_matching_fp(&m, &test_payload(), dir.path());
    m.set_quota_config(LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Pro,
            quota: Quota {
                max_points: Some(5000),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    });
    let s = m.current();
    assert_eq!(s.quota_source, QuotaSource::TierConfig);
    assert_eq!(s.quota.max_points, Some(5000));
}

/// license 有效但未自带 quota + tier_quotas 缺该档 → 来源 = TierMissing（降级可观测）
#[test]
fn test_quota_source_tier_missing_when_mapping_absent() {
    use crate::config::LicenseConfig;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    load_with_matching_fp(&m, &test_payload(), dir.path());
    // 只配了 core，Pro 未配 → Pro 回落代码内置默认表
    m.set_quota_config(LicenseConfig {
        tier_quotas: Some(vec![crate::config::TierQuotaEntry {
            tier: Tier::Core,
            quota: Quota {
                max_points: Some(500),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    });
    let s = m.current();
    assert_eq!(s.tier, Tier::Pro);
    assert_eq!(s.quota_source, QuotaSource::TierMissing);
    // Pro 档内置默认（2500 点 / 协议不限），而不是 absent 兜底（100/1）
    assert_eq!(s.quota.max_points, Some(2500));
    assert_eq!(s.quota.max_protocols, None);
}

/// 无 license（present=false）→ 来源 = AbsentFallback（预期行为，不是配置问题）
#[test]
fn test_quota_source_absent_fallback_without_license() {
    use crate::config::LicenseConfig;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    m.set_quota_config(LicenseConfig {
        tier_quotas: Some(vec![crate::config::TierQuotaEntry {
            tier: Tier::Core,
            quota: Quota {
                max_points: Some(500),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    });
    let s = m.current();
    assert!(!s.present);
    assert_eq!(s.quota_source, QuotaSource::AbsentFallback);
}

/// 语义修正（§6/§9）：无 license / 失效时**不查 tier_quotas**。
///
/// config.json 写全四档后 core=500，但无 license 的设备必须拿 100/1/7/1
/// （无 license 默认配额），不能因为 tier=core 就拿到 core 档配额。
#[test]
fn test_absent_license_does_not_use_tier_quotas() {
    use crate::config::{LicenseConfig, TierQuotaEntry};
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    // 四档写全（core=500/3/30/1）
    let q = |max_points: u32, max_protocols: u32| Quota {
        max_points: Some(max_points),
        max_protocols: Some(max_protocols),
        storage_days: Some(30),
        northbound_channels: Some(1),
    };
    m.set_quota_config(LicenseConfig {
        tier_quotas: Some(vec![
            TierQuotaEntry {
                tier: Tier::Core,
                quota: q(500, 3),
            },
            TierQuotaEntry {
                tier: Tier::Standard,
                quota: q(1500, 8),
            },
            TierQuotaEntry {
                tier: Tier::Pro,
                quota: q(5000, 16),
            },
            TierQuotaEntry {
                tier: Tier::Ultra,
                quota: Quota::default(),
            },
        ]),
        ..LicenseConfig::default()
    });
    // 无 license 文件 → absent
    assert!(m.load(&dir.path().join("nope.lic")).is_ok());
    let s = m.current();
    assert!(!s.present);
    assert_eq!(s.tier, Tier::Core);
    // 关键：走 absent 兜底（100/1），而非 core 档（500/3）
    assert_eq!(s.quota.max_points, Some(100));
    assert_eq!(s.quota.max_protocols, Some(1));
    assert_eq!(s.quota_source, QuotaSource::AbsentFallback);
}

/// 语义修正：授权失效降级（present=false）时同样不查 tier_quotas。
#[test]
fn test_degraded_license_does_not_use_tier_quotas() {
    use crate::config::{LicenseConfig, TierQuotaEntry};
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    m.set_quota_config(LicenseConfig {
        tier_quotas: Some(vec![TierQuotaEntry {
            tier: Tier::Core,
            quota: Quota {
                max_points: Some(500),
                max_protocols: Some(3),
                ..Quota::default()
            },
        }]),
        ..LicenseConfig::default()
    });
    let mut p = test_payload();
    p.expires_at = Some("2020-01-01T00:00:00Z".to_string());
    let lic = write_lic(dir.path(), &sign_payload(&p));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (s, _err) = m.evaluate_at(&lic, &matching_fp(), now);
    assert!(!s.present);
    m.replace_state_for_test(s);
    let s = m.current();
    assert_eq!(s.status, LicenseStatus::Expired);
    assert_eq!(s.quota.max_points, Some(100));
    assert_eq!(s.quota.max_protocols, Some(1));
    assert_eq!(s.quota_source, QuotaSource::AbsentFallback);
}

/// API 视图必须带 quota_source 键（前端据此提示降级原因）
#[test]
fn test_to_json_value_includes_quota_source() {
    let s = LicenseState::absent();
    let v = s.to_json_value();
    assert_eq!(v["quota_source"], serde_json::json!("absent_fallback"));
    assert!(v.get("quota_source").is_some());
}

#[test]
fn test_load_rejects_and_degrades() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    // 篡改 payload
    let p = write_lic(
        dir.path(),
        &flip_payload_char(&sign_payload(&test_payload())),
    );
    assert!(matches!(m.load(&p), Err(LicenseError::BadSignature)));
    assert!(!m.current().present);
    assert_eq!(m.current().status, LicenseStatus::Invalid);

    // 错误钥对
    let p2 = write_lic(dir.path(), &sign_with_other_key(&test_payload()));
    assert!(matches!(m.load(&p2), Err(LicenseError::BadSignature)));

    // 版本不支持
    let mut bad = test_payload();
    bad.v = 2;
    let p3 = write_lic(dir.path(), &sign_payload(&bad));
    assert!(matches!(m.load(&p3), Err(LicenseError::BadVersion(2))));
}

// ===== 校验顺序：指纹 → 有效期 → 时钟 =====

#[test]
fn test_evaluate_fingerprint_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let lic = write_lic(dir.path(), &sign_payload(&test_payload()));
    let (s, err) = m.evaluate_at(&lic, &mismatching_fp(), 1_000_000);
    assert_eq!(s.status, LicenseStatus::Mismatch);
    assert!(!s.present);
    assert_eq!(s.tier, Tier::Core);
    // 审计字段保留
    assert_eq!(s.license_id.as_deref(), Some("LIC-TEST-0001"));
    assert!(matches!(err, Some(LicenseError::Fingerprint)));

    // 匹配指纹 → 通过
    let (s2, err2) = m.evaluate_at(&lic, &matching_fp(), chrono::Utc::now().timestamp());
    assert_eq!(s2.status, LicenseStatus::Ok);
    assert!(s2.present);
    assert!(err2.is_none());
}

#[test]
fn test_evaluate_expired() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let mut p = test_payload();
    p.expires_at = Some("2026-01-02T00:00:00Z".into());
    let lic = write_lic(dir.path(), &sign_payload(&p));
    // now = 2026-03-01 → 已过期
    let (s, err) = m.evaluate_at(&lic, &matching_fp(), 1_772_300_000);
    assert_eq!(s.status, LicenseStatus::Expired);
    assert!(!s.present);
    assert_eq!(s.tier, Tier::Core);
    assert!(matches!(err, Some(LicenseError::Expired)));
}

#[test]
fn test_evaluate_clock_tampered_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let lic = write_lic(dir.path(), &sign_payload(&test_payload()));
    // 首次建立基准（用真实时间，避免被 #18 的 mtime 交叉校验判为回拨）
    let now = chrono::Utc::now().timestamp();
    let (s1, _) = m.evaluate_at(&lic, &matching_fp(), now);
    assert!(s1.present);
    // 回拨 49h → Tampered
    let (s2, err) = m.evaluate_at(&lic, &matching_fp(), now - 49 * 3600);
    assert_eq!(s2.status, LicenseStatus::Tampered);
    assert!(!s2.present);
    assert!(matches!(err, Some(LicenseError::ClockTampered)));
}

// ===== 时钟 / license 反回拨行为验证（决策清单 P1-6）=====
//
// 现场最常见的「授权突然失效」不是签名问题，而是工控板无电池 RTC 断电后
// 时间漂回过去。这里锁定三件事：
// 1. < 48h 的抖动属于常态，不得误判（合法设备不能被锁）；
// 2. 真回拨必须判 Tampered，且原因明确写「clock rollback」，不能混成
//    「signature invalid」（后者会把人引向完全错误的排查方向）；
// 3. 对外状态是 `tampered` 而不是 `invalid`，前端据此显示「时钟异常」。

#[test]
fn test_clock_drift_within_48h_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let lic = write_lic(dir.path(), &sign_payload(&test_payload()));
    let now = chrono::Utc::now().timestamp();
    // 建立基准
    let (s1, err1) = m.evaluate_at(&lic, &matching_fp(), now);
    assert_eq!(s1.status, LicenseStatus::Ok);
    assert!(err1.is_none());

    // 回拨 47h：在 48h 容差内，应继续正常授权
    let (s2, err2) = m.evaluate_at(&lic, &matching_fp(), now - 47 * 3600);
    assert_eq!(
        s2.status,
        LicenseStatus::Ok,
        "47h 抖动属于无电池 RTC 常态，不得判 Tampered"
    );
    assert!(s2.present, "容差内应保持授权有效");
    assert!(err2.is_none(), "容差内不应产生失效原因");
}

#[test]
fn test_clock_rollback_reason_says_rollback_not_signature() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let lic = write_lic(dir.path(), &sign_payload(&test_payload()));
    let now = chrono::Utc::now().timestamp();
    m.evaluate_at(&lic, &matching_fp(), now);

    // 回拨 49h（超阈值）→ Tampered，原因必须是「回拨」而非「签名无效」
    let (state, err) = m.evaluate_at(&lic, &matching_fp(), now - 49 * 3600);
    assert_eq!(state.status, LicenseStatus::Tampered);
    let err = err.expect("超阈值回拨必须给出失效原因");
    let reason = err.to_string();
    assert!(
        reason.contains("clock rollback"),
        "原因应明确写 clock rollback，实际: {reason}"
    );
    assert!(
        !reason.contains("signature"),
        "回拨原因不能混入 signature（会误导排查方向），实际: {reason}"
    );

    // 对外 JSON 状态是 tampered（前端显示「时钟异常」），不是 invalid
    let json = state.to_json_value();
    assert_eq!(json["status"], "tampered");
    assert_ne!(json["status"], "invalid");
}

/// Bug C9: clock 存储不可用必须 fail-closed（判 Tampered），不得放行。
#[test]
fn test_clock_store_error_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    // clock.db 的父路径用文件占位，使 SQLite 无法创建/打开数据库（模拟存储被破坏/只读）
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let db = blocker.join("clock.db");
    let m = LicenseManager::with_public_key(&db, public_key());
    let lic = write_lic(dir.path(), &sign_payload(&test_payload()));
    let (s, err) = m.evaluate_at(&lic, &matching_fp(), 1_000_000_000);
    assert_eq!(s.status, LicenseStatus::Tampered);
    assert!(!s.present);
    assert!(matches!(err, Some(LicenseError::ClockTampered)));
}

// ===== 降级语义一致性（§13 第 5 组）=====

#[test]
fn test_degradation_semantics_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());

    // Expired
    let mut p = test_payload();
    p.expires_at = Some("2026-01-02T00:00:00Z".into());
    let lic_expired = write_lic(dir.path(), &sign_payload(&p));
    let (s_exp, _) = m.evaluate_at(&lic_expired, &matching_fp(), 1_772_300_000);

    // Mismatch
    let lic_fp = write_lic(dir.path(), &sign_payload(&test_payload()));
    let (s_mis, _) = m.evaluate_at(&lic_fp, &mismatching_fp(), 1_772_300_000);

    // Invalid
    let lic_bad = write_lic(
        dir.path(),
        &flip_payload_char(&sign_payload(&test_payload())),
    );
    let (s_inv, _) = m.evaluate_at(&lic_bad, &matching_fp(), 1_772_300_000);

    // Tampered：先建基准再回拨
    let dir2 = tempfile::tempdir().unwrap();
    let m2 = manager(dir2.path());
    let lic2 = write_lic(dir2.path(), &sign_payload(&test_payload()));
    let now2 = chrono::Utc::now().timestamp();
    m2.evaluate_at(&lic2, &matching_fp(), now2);
    let (s_tam, _) = m2.evaluate_at(&lic2, &matching_fp(), now2 - 49 * 3600);

    for s in [&s_exp, &s_mis, &s_inv, &s_tam] {
        assert!(s.is_degraded(), "{:?} should be degraded", s.status);
        assert!(!s.present, "{:?} present must be false", s.status);
        assert_eq!(s.tier, Tier::Core, "{:?} tier must be core", s.status);
    }
}

// ===== NTP 重置 =====

#[test]
fn test_ntp_resync_clears_tampered() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    let lic = write_lic(dir.path(), &sign_payload(&test_payload()));
    let now = chrono::Utc::now().timestamp();
    let (s1, _) = m.evaluate_at(&lic, &matching_fp(), now);
    assert!(s1.present);
    let (s, _) = m.evaluate_at(&lic, &matching_fp(), now - 49 * 3600);
    assert_eq!(s.status, LicenseStatus::Tampered);

    // NTP 校时成功 → 重置基准 → 同一时刻重校验恢复 Ok
    m.apply_ntp_resync(now);
    let (s2, err2) = m.evaluate_at(&lic, &matching_fp(), now);
    assert_eq!(s2.status, LicenseStatus::Ok);
    assert!(s2.present);
    assert!(err2.is_none());
}

// ===== ts-rs 类型导出 =====

#[test]
fn export_ts_types() {
    let out = std::path::Path::new("../gateway-web/src/api/generated");
    std::fs::create_dir_all(out).expect("create generated dir");
    LicenseState::export_all_to(out).expect("LicenseState");
}

// ===== B3：无 config 注入时 quota 全空的安全兜底 =====

/// license 自带 quota 全空 + 未注入 quota_config（config.json 无 license 节）
/// → current() 兜底为 DEFAULT_ABSENT_QUOTA，不再落回全 None（=无限）
#[test]
fn test_current_quota_all_none_falls_back_to_safe_default_without_config() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    load_with_matching_fp(&m, &test_payload(), dir.path());
    let s = m.current();
    assert_eq!(s.tier, Tier::Pro);
    // 回落 Pro 档代码内置默认配额（v2.1：2500 点 / 协议不限 / 30 天 / 通道不限）
    assert_eq!(s.quota.max_points, Some(2500));
    assert_eq!(s.quota.max_protocols, None);
    assert_eq!(s.quota.storage_days, Some(30));
    assert_eq!(s.quota.northbound_channels, None);
    // 配额校验确实生效：超限拒、等于上限放行
    assert!(m.check_quota(QuotaKind::Points, 2501).is_err());
    assert!(m.check_quota(QuotaKind::Points, 2500).is_ok());
}

// ===== v2.0 配额用量：Quota::available 计算 =====

/// 上限有值时：剩余 = 上限 - 已用
#[test]
fn test_quota_available_subtracts_used_from_limit() {
    let q = Quota {
        max_points: Some(5000),
        max_protocols: Some(16),
        storage_days: Some(365),
        northbound_channels: Some(8),
    };
    let usage = QuotaUsage {
        points: 1820,
        protocols: 8,
        storage_days: 365,
        northbound_channels: 3,
    };
    let a = q.available(&usage);
    assert_eq!(a.points, Some(3180));
    assert_eq!(a.protocols, Some(8));
    assert_eq!(a.storage_days, Some(0));
    assert_eq!(a.northbound_channels, Some(5));
}

/// 上限为 None（不限）时：剩余也是 None，不能退化成 0
#[test]
fn test_quota_available_none_limit_stays_none() {
    let q = Quota::default();
    let usage = QuotaUsage {
        points: 100,
        protocols: 4,
        storage_days: 10,
        northbound_channels: 2,
    };
    let a = q.available(&usage);
    assert_eq!(a.points, None);
    assert_eq!(a.protocols, None);
    assert_eq!(a.storage_days, None);
    assert_eq!(a.northbound_channels, None);
}

/// 已用超过上限时：饱和到 0，不出现负数（u32 不能为负）
#[test]
fn test_quota_available_saturates_at_zero_when_over_limit() {
    let q = Quota {
        max_points: Some(100),
        max_protocols: None,
        storage_days: None,
        northbound_channels: None,
    };
    let usage = QuotaUsage {
        points: 250,
        protocols: 0,
        storage_days: 0,
        northbound_channels: 0,
    };
    let a = q.available(&usage);
    assert_eq!(a.points, Some(0));
}

/// 已用量超过上限的配额项应被识别
///
/// v2.1 口径：`exceeded_kinds` 只含**硬拒绝**项（当前只有点数）；
/// 协议种类数与北向通道数归入 `warned_kinds`（只告警、不阻断保存）。
#[test]
fn test_quota_exceeded_kinds_detects_over_limit() {
    let q = Quota {
        max_points: Some(1000),
        max_protocols: Some(16),
        storage_days: Some(365),
        northbound_channels: Some(8),
    };
    let usage = QuotaUsage {
        points: 5000,           // 超
        protocols: 8,           // 未超
        storage_days: 400,      // 超，但不在告警范围（运行时 FIFO 截断，不拒绝保存）
        northbound_channels: 9, // 超
    };
    let kinds = q.exceeded_kinds(&usage);
    assert_eq!(kinds.len(), 1);
    assert!(kinds.contains(&QuotaKind::Points));
    assert!(!kinds.contains(&QuotaKind::NorthboundChannels));
    assert!(!kinds.contains(&QuotaKind::Protocols));
}

/// v2.1：协议种类数 / 北向通道数超限归入**告警**项，不阻断保存
#[test]
fn test_quota_warned_kinds_detects_protocols_and_channels() {
    let q = Quota {
        max_points: Some(1000),
        max_protocols: Some(16),
        storage_days: Some(365),
        northbound_channels: Some(8),
    };
    let usage = QuotaUsage {
        points: 100,            // 未超
        protocols: 20,          // 超
        storage_days: 400,      // 超，但不在告警范围
        northbound_channels: 9, // 超
    };
    let warned = q.warned_kinds(&usage);
    assert_eq!(warned.len(), 2);
    assert!(warned.contains(&QuotaKind::Protocols));
    assert!(warned.contains(&QuotaKind::NorthboundChannels));
    // 硬拒绝项不受影响：点数未超 → 不拒绝
    assert!(q.exceeded_kinds(&usage).is_empty());
}

/// 全部未超限 → 无告警项
#[test]
fn test_quota_exceeded_kinds_empty_when_within_limit() {
    let q = Quota {
        max_points: Some(5000),
        max_protocols: Some(16),
        storage_days: Some(365),
        northbound_channels: Some(8),
    };
    let usage = QuotaUsage {
        points: 1820,
        protocols: 8,
        storage_days: 30,
        northbound_channels: 3,
    };
    assert!(q.exceeded_kinds(&usage).is_empty());
}

/// 无限额（None）不产生告警项
#[test]
fn test_quota_exceeded_kinds_none_limit_never_exceeded() {
    let q = Quota::default();
    let usage = QuotaUsage {
        points: 999_999,
        protocols: 999,
        storage_days: 9999,
        northbound_channels: 99,
    };
    assert!(q.exceeded_kinds(&usage).is_empty());
}

// ===== B3：换钥过渡期环境变量公钥解析 =====

#[test]
fn test_parse_pubkey_hex_accepts_valid_and_rejects_invalid() {
    // 编译期公钥的 hex 形态应能解析回同一字节数组（大小写 / 0x 前缀均接受）
    let hex: String = PUBLIC_KEY_OLD
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    assert_eq!(parse_pubkey_hex(&hex), Some(PUBLIC_KEY_OLD));
    assert_eq!(
        parse_pubkey_hex(&format!("0x{}", hex.to_uppercase())),
        Some(PUBLIC_KEY_OLD)
    );
    // 长度 / 字符合法性校验：非法输入返回 None（不会让 LicenseManager 启动失败）
    assert!(parse_pubkey_hex("").is_none());
    assert!(parse_pubkey_hex("deadbeef").is_none());
    assert!(parse_pubkey_hex(&"zz".repeat(32)).is_none());
}
