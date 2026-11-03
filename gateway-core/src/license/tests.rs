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
    // current() 在无 config 注入时兜底为 DEFAULT_ABSENT_QUOTA（不再等于"无限"）
    assert_eq!(s.quota.max_points, Some(100));
    assert_eq!(s.quota.max_protocols, Some(1));
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

/// quota_config 注入 + license 无 quota + config tier_quotas 无此 tier 映射
/// → 走 absent_quota（effective_absent_quota）
#[test]
fn test_current_quota_falls_back_to_absent_quota() {
    use crate::config::LicenseConfig;
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path());
    load_with_matching_fp(&m, &test_payload(), dir.path());
    // config tier_quotas 不填 → 走 absent
    m.set_quota_config(LicenseConfig {
        tier_quotas: None,
        ..LicenseConfig::default()
    });
    let s = m.current();
    // absent_quota = DEFAULT_ABSENT_QUOTA（100/1/7/1）
    assert_eq!(s.quota.max_points, Some(100));
    assert_eq!(s.quota.max_protocols, Some(1));
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
    // 兜底为 DEFAULT_ABSENT_QUOTA（100 点 / 1 协议 / 7 天 / 1 通道）
    assert_eq!(s.quota.max_points, Some(100));
    assert_eq!(s.quota.max_protocols, Some(1));
    assert_eq!(s.quota.storage_days, Some(7));
    assert_eq!(s.quota.northbound_channels, Some(1));
    // 配额校验确实生效：超限拒、等于上限放行
    assert!(m.check_quota(QuotaKind::Points, 101).is_err());
    assert!(m.check_quota(QuotaKind::Points, 100).is_ok());
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
