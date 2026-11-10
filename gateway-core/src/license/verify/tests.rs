use super::*;
use crate::license::test_support;

fn payload() -> LicensePayload {
    test_support::test_payload()
}

fn signed(p: &LicensePayload) -> String {
    test_support::sign_payload(p)
}

/// 单公钥测试切片（verify_file_text 接受 &[[u8; 32]] 切片表）
fn pub_key() -> [[u8; 32]; 1] {
    [test_support::public_key()]
}

fn wrong_pub_key() -> [[u8; 32]; 1] {
    [test_support::other_public_key()]
}

// ===== 验签（§13 测试计划第 1 组）=====

#[test]
fn test_valid_file_passes() {
    let p = payload();
    let out = verify_file_text(&signed(&p), &pub_key()).unwrap();
    assert_eq!(out.license_id, p.license_id);
    assert_eq!(out.tier, p.tier);
    assert_eq!(out.machine_parts.len(), 3);
}

#[test]
fn test_tampered_payload_rejected() {
    let p = payload();
    // 对已签名文件篡改 payload 段一个字符（改字段不重签）
    let forged = test_support::flip_payload_char(&signed(&p));
    assert!(matches!(
        verify_file_text(&forged, &pub_key()),
        Err(LicenseError::BadSignature)
    ));
}

#[test]
fn test_tampered_signature_rejected() {
    let p = payload();
    let text = signed(&p);
    // 签名段第一个字符 'A' → 'B'（保持合法 base64）
    let (seg0, seg1) = text.split_once('.').unwrap();
    let mut chars: Vec<char> = seg1.chars().collect();
    chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
    let tampered = format!("{}.{}", seg0, chars.into_iter().collect::<String>());
    assert!(matches!(
        verify_file_text(&tampered, &pub_key()),
        Err(LicenseError::BadSignature)
    ));
}

#[test]
fn test_wrong_public_key_rejected() {
    let p = payload();
    assert!(matches!(
        verify_file_text(&signed(&p), &wrong_pub_key()),
        Err(LicenseError::BadSignature)
    ));
}

#[test]
fn test_empty_file_rejected() {
    assert!(matches!(
        verify_file_text("", &pub_key()),
        Err(LicenseError::Decode)
    ));
}

#[test]
fn test_malformed_formats_rejected() {
    for text in ["no-dot-here", "a.b.c", ".", "abc."] {
        assert!(
            matches!(
                verify_file_text(text, &pub_key()),
                Err(LicenseError::Decode)
            ),
            "text {:?} should be rejected as Decode",
            text
        );
    }
}

// ===== 字段校验 =====

#[test]
fn test_bad_version_rejected() {
    let mut p = payload();
    p.v = 2;
    assert!(matches!(
        verify_file_text(&signed(&p), &pub_key()),
        Err(LicenseError::BadVersion(2))
    ));
}

#[test]
fn test_bad_machine_parts_len_rejected() {
    let mut p = payload();
    p.machine_parts.pop();
    assert!(matches!(
        verify_file_text(&signed(&p), &pub_key()),
        Err(LicenseError::BadPayload(_))
    ));
}

#[test]
fn test_bad_rfc3339_rejected() {
    let mut p = payload();
    p.expires_at = Some("2027/10/11".into());
    assert!(matches!(
        verify_file_text(&signed(&p), &pub_key()),
        Err(LicenseError::BadPayload(_))
    ));
}

// ===== 有效期（§13 测试计划第 3 组）=====

#[test]
fn test_expiry_four_states() {
    // 永久
    let mut p = payload();
    p.expires_at = None;
    assert_eq!(check_expiry(&p, 1_000_000), ExpiryVerdict::Ok);

    // 未到期（100 天）
    let future = chrono::DateTime::from_timestamp(1_000_000 + 100 * 86400, 0)
        .unwrap()
        .to_rfc3339();
    p.expires_at = Some(future);
    assert_eq!(check_expiry(&p, 1_000_000), ExpiryVerdict::Ok);

    // 临期（29 天）
    let soon = chrono::DateTime::from_timestamp(1_000_000 + 29 * 86400, 0)
        .unwrap()
        .to_rfc3339();
    p.expires_at = Some(soon);
    assert_eq!(check_expiry(&p, 1_000_000), ExpiryVerdict::Expiring);

    // 过期 1 天
    let past = chrono::DateTime::from_timestamp(1_000_000 - 86400, 0)
        .unwrap()
        .to_rfc3339();
    p.expires_at = Some(past);
    assert_eq!(check_expiry(&p, 1_000_000), ExpiryVerdict::Expired);
}

// ===== v2.0 quota 字段 + 老 license 兼容 =====

/// v1.2 老 license 无 quota 字段、且带已废弃的 features 字段
/// → quota 解析为 None；features 作为未知字段被 serde 忽略（向后兼容，不报错）
#[test]
fn test_v1_license_without_quota_parses_as_none() {
    // 手工构造 v1.2 风格 JSON（不写 quota 字段，含已废弃 features，模拟老 license 文件）
    let v1_json = r#"{
        "v": 1,
        "license_id": "LIC-V1-0001",
        "customer": "某某自动化",
        "tier": "pro",
        "features": ["perf-monitor"],
        "machine_key": "0000000000000000000000000000000000000000000000000000000000000000",
        "machine": "1111111111111111111111111111111111111111111111111111111111111111",
        "machine_parts": ["", "", ""],
        "issued_at": "2026-01-01T00:00:00Z",
        "expires_at": null
    }"#;
    // 直接走 LicensePayload 解析（不验签，验证字段容错）
    let parsed: LicensePayload = serde_json::from_str(v1_json).unwrap();
    assert!(parsed.quota.is_none(), "老 license 无 quota → None");
    assert_eq!(parsed.license_id, "LIC-V1-0001");
    assert_eq!(parsed.tier, Tier::Pro);
}

/// v2.0 license 含 quota 字段 → 解析为 Some，4 个字段按需填充
#[test]
fn test_v2_license_with_quota_parses_correctly() {
    let mut p = payload();
    p.quota = Some(Quota {
        max_points: Some(5000),
        max_protocols: Some(16),
        storage_days: Some(365),
        northbound_channels: Some(8),
    });
    let text = signed(&p);
    let parsed = verify_file_text(&text, &pub_key()).unwrap();
    let q = parsed.quota.expect("quota parsed");
    assert_eq!(q.max_points, Some(5000));
    assert_eq!(q.max_protocols, Some(16));
    assert_eq!(q.storage_days, Some(365));
    assert_eq!(q.northbound_channels, Some(8));
}

/// v2.0 部分 quota 字段为 None → 表示该配额无上限
#[test]
fn test_v2_quota_with_none_fields_means_unlimited() {
    let mut p = payload();
    p.quota = Some(Quota {
        max_points: Some(5000),
        max_protocols: None, // 协议不限
        storage_days: Some(365),
        northbound_channels: None, // 通道不限
    });
    let parsed = verify_file_text(&signed(&p), &pub_key()).unwrap();
    let q = parsed.quota.unwrap();
    assert_eq!(q.max_points, Some(5000));
    assert_eq!(q.max_protocols, None);
    assert!(q.max_protocols.is_none(), "None = 无限配额");
}

/// 序列化产物：None 字段输出为 JSON null，前端 ts-rs 类型能正确解析
#[test]
fn test_quota_serialize_uses_null_for_none() {
    let q = Quota {
        max_points: Some(5000),
        max_protocols: None,
        storage_days: Some(365),
        northbound_channels: None,
    };
    let s = serde_json::to_string(&q).unwrap();
    assert!(s.contains("\"max_points\":5000"));
    assert!(s.contains("\"max_protocols\":null"));
    assert!(s.contains("\"northbound_channels\":null"));
}

// ===== 多公钥表（换钥过渡期，2026-Q1 新增）=====

/// 2.1 换钥过渡：表头公钥签发的 license 应被 2 钥表接受
#[test]
fn test_multi_key_table_accepts_match_with_any_key() {
    let p = payload();
    let text = signed(&p);
    // 旧钥 + 测试钥（顺序：测试钥在前，命中即返）
    let keys = [test_support::public_key(), test_support::other_public_key()];
    let parsed = verify_file_text(&text, &keys).expect("test key should match");
    assert_eq!(parsed.license_id, p.license_id);
}

/// 2.1 换钥过渡：旧钥在表但 payload 是新钥签的 → 仍能通过（只要新钥在表中）
#[test]
fn test_multi_key_table_first_match_wins() {
    let p = payload();
    let text = signed(&p); // 测试钥签的
                           // 顺序：wrong + test（test 在后应能命中）
    let keys = [test_support::other_public_key(), test_support::public_key()];
    let parsed = verify_file_text(&text, &keys).expect("test key in tail should still match");
    assert_eq!(parsed.license_id, p.license_id);
}

/// 2.1 换钥过渡：表内无任何匹配钥 → BadSignature（不泄露是哪个公钥匹配失败）
#[test]
fn test_multi_key_table_no_match_rejects() {
    let p = payload();
    let text = signed(&p);
    // 测试钥签的，但表里只放 other → 应拒
    let keys = [test_support::other_public_key()];
    assert!(matches!(
        verify_file_text(&text, &keys),
        Err(LicenseError::BadSignature)
    ));
}

/// 2.1 换钥过渡：空公钥表 = 配置错误（不静默接受）
#[test]
fn test_multi_key_table_empty_rejects() {
    let p = payload();
    let text = signed(&p);
    let empty: [[u8; 32]; 0] = [];
    assert!(matches!(
        verify_file_text(&text, &empty),
        Err(LicenseError::BadSignature)
    ));
}

// ===== B3：验签 + 指纹统一入口（避免漏调）=====

/// 正常路径：签名 + 字段 + 指纹全通过 → Ok
#[test]
fn test_verify_with_fingerprint_passes() {
    let p = payload();
    let text = signed(&p);
    let local = test_support::matching_fp();
    let out = verify_with_fingerprint(&text, &pub_key(), &local).expect("ok");
    assert_eq!(out.license_id, p.license_id);
}

// ===== r34-LV1/LV2：DoS 防护上限 =====

/// r34-LV1 (回归)：payload 段超 64KB 时直接拒（防 OOM）。
#[test]
fn test_oversized_payload_rejected() {
    use crate::engine::codec::encoding::b64_encode;
    // 构造 70KB 随机数据作为 payload 段，附 64B 有效 sig（64B base64 编码 ~88 字符）
    let big = vec![0xABu8; 70 * 1024];
    let seg0 = b64_encode(&big);
    let sig = b64_encode(&[0u8; 64]);
    let text = format!("{}.{}", seg0, sig);
    assert!(matches!(
        verify_file_text(&text, &pub_key()),
        Err(LicenseError::Decode)
    ));
}

/// r34-LV1 (回归)：sig 段超 1KB 时也拒。
#[test]
fn test_oversized_signature_rejected() {
    use crate::engine::codec::encoding::b64_encode;
    // 合法小 payload + 2KB sig（base64 ~2700 字符）
    let p = payload();
    let seg0 = b64_encode(&serde_json::to_vec(&p).expect("serialize"));
    let big_sig = vec![0u8; 2 * 1024];
    let seg1 = b64_encode(&big_sig);
    let text = format!("{}.{}", seg0, seg1);
    assert!(matches!(
        verify_file_text(&text, &pub_key()),
        Err(LicenseError::Decode)
    ));
}

/// r34-LV2 (回归)：公钥数量超过 MAX_LICENSE_PUBLIC_KEYS 时直接拒。
#[test]
fn test_too_many_public_keys_rejected() {
    let p = payload();
    let text = signed(&p);
    // 构造 11 把公钥（全用同一把，多余 10 把）
    let many_keys: Vec<[u8; 32]> = std::iter::repeat_n(test_support::public_key(), 11).collect();
    assert!(matches!(
        verify_file_text(&text, &many_keys),
        Err(LicenseError::BadSignature)
    ));
}

/// 验签失败：篡改 payload → BadSignature（与 verify_file_text 一致）
#[test]
fn test_verify_with_fingerprint_bad_signature() {
    let p = payload();
    let forged = test_support::flip_payload_char(&signed(&p));
    let local = test_support::matching_fp();
    assert!(matches!(
        verify_with_fingerprint(&forged, &pub_key(), &local),
        Err(LicenseError::BadSignature)
    ));
}

/// 验签通过但指纹不符 → Fingerprint（统一入口最关键的语义）
#[test]
fn test_verify_with_fingerprint_mismatch() {
    let p = payload();
    let text = signed(&p);
    let wrong = test_support::mismatching_fp();
    assert!(matches!(
        verify_with_fingerprint(&text, &pub_key(), &wrong),
        Err(LicenseError::Fingerprint)
    ));
}

/// 单公钥便捷入口 + 指纹：与多公钥版语义一致
#[test]
fn test_verify_with_fingerprint_single() {
    let p = payload();
    let text = signed(&p);
    let local = test_support::matching_fp();
    let out =
        verify_with_fingerprint_single(&text, &test_support::public_key(), &local).expect("ok");
    assert_eq!(out.license_id, p.license_id);
}
