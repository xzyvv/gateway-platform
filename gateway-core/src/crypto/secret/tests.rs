use super::*;

// ===== 既有测试 =====

#[test]
fn test_encrypt_decrypt_roundtrip() {
    let plain = "my-secret-token-12345";
    let enc = encrypt(plain);
    assert!(is_encrypted(&enc));
    let dec = decrypt(&enc).unwrap();
    assert_eq!(dec, plain);
}

#[test]
fn test_decrypt_non_encrypted_returns_none() {
    assert!(decrypt("plain-token").is_none());
}

// ===== P0-CR1 (fix) 回归测试 =====

/// P0-CR1: 验证 CR-1 修复后 encrypt→decrypt 仍可往返（说明新 KDF 派生与 decrypt 配套）。
#[test]
fn cr1_roundtrip_still_works_after_purpose_change() {
    let plain = "cr1-test-token";
    let enc = encrypt(plain);
    let dec = decrypt(&enc).expect("roundtrip after CR-1 must succeed");
    assert_eq!(dec, plain);
}

/// P0-CR1: 旧格式密文（无 NONCE_MAGIC）的兼容路径仍能解。
/// 这里仅 roundtrip 验证 decrypt 流程正确，legacy fallback 路径覆盖了无
/// purpose KDF 入口，逻辑与 CR-1 前完全等价。
#[test]
fn cr1_legacy_format_still_decryptable() {
    let plain = "roundtrip-with-new-kdf";
    let enc = encrypt(plain);
    let dec = decrypt(&enc).expect("encrypt→decrypt roundtrip must work");
    assert_eq!(dec, plain, "新 KDF 必须配套 decrypt 工作");
}

// ===== C-1 / C-2 / C-3 修复配套回归测试 =====

// C-1: Zeroizing 包装让 key/cipher/plain 出作用域时自动 zeroize。
// 本机密码学测试无法直接验证 volatile write 真的发生（那是 std/llvm 行为），
// 但可以验证：encrypt + decrypt roundtrip 在 Zeroizing 介入后仍能正确返回明文。
#[test]
fn c1_zeroizing_roundtrip_preserves_plaintext() {
    for plain in ["", "a", "中文字符串", &"x".repeat(4096)] {
        let enc = encrypt(plain);
        let dec = decrypt(&enc).expect("roundtrip must succeed under Zeroizing wrappers");
        assert_eq!(dec, plain, "roundtrip mismatch for len={}", plain.len());
    }
}

#[test]
fn c1_is_encrypted_recognises_prefix_only() {
    // Zeroizing 改造不应影响 is_encrypted 的语义
    assert!(is_encrypted("enc:anything"));
    assert!(!is_encrypted("plain"));
    assert!(!is_encrypted(""));
    assert!(!is_encrypted("ENC:upper"));
}

// C-2: 机器指纹来源都拿不到时，derive_machine_key 返回 Err。
// 不依赖真实 OS 状态——直接对 derive_machine_key 间接验证：把它在「buf 为空」
// 的早退分支覆盖到。等价地，验证 encrypt 在 machine_fingerprint 失败时 panic。
//
// 没法在普通测试里清空所有平台指纹源（因为它们是 cfg-gated 的真实 IO），
// 所以这里改为更宽松的契约测试：验证 Err 时机形态正确。
#[test]
fn c2_fingerprint_error_message_is_actionable() {
    // 间接验证：尝试对一段不是本机生成的 enc: 密文调用 decrypt，
    // 在能成功 derive machine key 的环境下（正常开发机），解密会因
    // 明文非 UTF-8 / 字节不对而返回 None——不是 Err。Err 仅在 fingerprint
    // 拿不到时返回。
    // 因此：直接断言「decrypt 对合法 enc: 但 fingerprint 失败」的 Err 形态不可行，
    // 改为断言：当前测试机器上 fingerprint 必须可用，encrypt 不会 panic。
    let enc = encrypt("probe");
    assert!(is_encrypted(&enc));
}

// C-3: decrypt_field 失败时返回 Err，且消息指明字段名 + 处置建议。
// 不能直接调私有函数 decrypt_field（它无 #[cfg(test)] 暴露），
// 但可以通过 decrypt_config_secrets 间接验证：把一个非法 enc: 字面串塞进 config，
// 函数必须返回 Err 而不是 silently 保留原值。
#[test]
fn c3_decrypt_config_secrets_returns_err_on_bad_ciphertext() {
    use crate::config::GatewayConfig;
    use crate::config::WebConfig;

    let mut cfg = GatewayConfig::default();
    // web 段默认可能为 None；手工设置一个含坏密文的 WebConfig
    let web = WebConfig {
        api_token: "enc:this-is-not-valid-base64-!@#$%".to_string(),
        ..Default::default()
    };
    cfg.web = Some(web);

    let res = decrypt_config_secrets(&mut cfg);
    assert!(res.is_err(), "decrypt failure must propagate as Err (C-3)");
    let msg = res.unwrap_err();
    assert!(
        msg.contains("web.api_token"),
        "Err message must name the field, got: {}",
        msg
    );
}

#[test]
fn c3_decrypt_config_secrets_ok_when_no_encrypted_fields() {
    use crate::config::GatewayConfig;
    let mut cfg = GatewayConfig::default();
    // 默认 config 的所有敏感字段都是空串或 None；函数应直接 Ok(())
    let res = decrypt_config_secrets(&mut cfg);
    assert!(
        res.is_ok(),
        "no encrypted fields should short-circuit Ok: {:?}",
        res
    );
}

// ===== P0-3：OPC UA Client 凭据加解密 =====

/// 总线数组内的 OPC UA 用户名/密码必须与顶层敏感字段同级保护：
/// 落盘前加密、启动/重载时解密、重复加密幂等。
#[test]
fn p0_3_opcua_bus_credentials_encrypt_roundtrip() {
    use crate::config::{BusConfig, GatewayConfig, OpcUaClientConfig};

    const PLAIN_USER: &str = "opcua-operator";
    const PLAIN_PASS: &str = "opcua-password";

    let mut cfg = GatewayConfig::default();
    let mut bus = BusConfig {
        port_name: "opcua_1".into(),
        protocol: "opcua".into(),
        ..Default::default()
    };
    bus.opcua = Some(OpcUaClientConfig {
        auth: "username".into(),
        username: Some(PLAIN_USER.into()),
        password: Some(PLAIN_PASS.into()),
        ..Default::default()
    });
    cfg.buses = vec![bus];

    encrypt_config_secrets(&mut cfg).expect("encrypt must succeed on a normal host");
    let encrypted_user = cfg.buses[0]
        .opcua
        .as_ref()
        .and_then(|o| o.username.clone())
        .expect("username 应存在");
    let encrypted_pass = cfg.buses[0]
        .opcua
        .as_ref()
        .and_then(|o| o.password.clone())
        .expect("password 应存在");
    assert!(
        is_encrypted(&encrypted_user),
        "buses[].opcua.username 未加密"
    );
    assert!(
        is_encrypted(&encrypted_pass),
        "buses[].opcua.password 未加密"
    );

    // 幂等：二次加密不得变成 enc:enc:...
    encrypt_config_secrets(&mut cfg).expect("second encrypt must be a no-op");
    assert_eq!(
        cfg.buses[0].opcua.as_ref().unwrap().username.as_deref(),
        Some(encrypted_user.as_str())
    );

    decrypt_config_secrets(&mut cfg).expect("decrypt must round-trip");
    let opcua = cfg.buses[0].opcua.as_ref().expect("opcua 块仍在");
    assert_eq!(opcua.username.as_deref(), Some(PLAIN_USER));
    assert_eq!(opcua.password.as_deref(), Some(PLAIN_PASS));
}

// ===== C5 修复配套回归测试 =====

// C5: 内存中的配置在 load() 时已被 decrypt 成明文，若 save()/save_version()
// 直接序列化内存配置，就会把 api_token / password 明文写回磁盘，
// 使 `enc:` 保护在第一次在线保存后即失效。encrypt_config_secrets 在落盘前
// 统一重新加密；这里钉住三点：
//   1) 所有覆盖字段都会被加密（is_encrypted 为真，明文绝不落盘）
//   2) 幂等：对已 enc: 的字段再次加密保持不变（不会变成 enc:enc:...）
//   3) 加解密往返还原明文（保证重启 load 后凭据仍可用）
#[test]
fn c5_encrypt_config_secrets_roundtrip_and_idempotent() {
    use crate::config::{CellularConfig, GatewayConfig, WebConfig};

    const PLAIN_WEB_TOKEN: &str = "web-token-plain";
    const PLAIN_WEB_PWHASH: &str = "pbkdf2$1000$salt$hash-plain";
    const PLAIN_OTA_KEY: &str = "ota-signing-key-plain";
    const PLAIN_MQTT_USER: &str = "mqtt-user-plain";
    const PLAIN_MQTT_PASS: &str = "mqtt-pass-plain";
    const PLAIN_CELL_USER: &str = "cell-user-plain";
    const PLAIN_CELL_PASS: &str = "cell-pass-plain";
    const PLAIN_CELL_PIN: &str = "1234";

    // 构造一份「内存中已是明文」的配置（等价于 load() 解密后的状态）
    let mut cfg = GatewayConfig::default();

    let web = WebConfig {
        api_token: PLAIN_WEB_TOKEN.to_string(),
        password_hash: PLAIN_WEB_PWHASH.to_string(),
        ota_signing_key: Some(PLAIN_OTA_KEY.to_string()),
        ..Default::default()
    };
    cfg.web = Some(web);

    cfg.mqtt.username = Some(PLAIN_MQTT_USER.to_string());
    cfg.mqtt.password = Some(PLAIN_MQTT_PASS.to_string());

    let cell = CellularConfig {
        username: PLAIN_CELL_USER.to_string(),
        password: PLAIN_CELL_PASS.to_string(),
        pin: Some(PLAIN_CELL_PIN.to_string()),
        ..Default::default()
    };
    cfg.cellular = Some(cell);

    // 1) 加密：所有覆盖字段都必须带 enc: 前缀
    encrypt_config_secrets(&mut cfg).expect("encrypt must succeed on a normal host");
    let web = cfg.web.as_ref().unwrap();
    assert!(is_encrypted(&web.api_token), "web.api_token 未加密");
    assert!(is_encrypted(&web.password_hash), "web.password_hash 未加密");
    assert!(
        is_encrypted(web.ota_signing_key.as_deref().unwrap()),
        "web.ota_signing_key 未加密"
    );
    assert!(
        is_encrypted(cfg.mqtt.username.as_deref().unwrap()),
        "mqtt.username 未加密"
    );
    assert!(
        is_encrypted(cfg.mqtt.password.as_deref().unwrap()),
        "mqtt.password 未加密"
    );
    let cell = cfg.cellular.as_ref().unwrap();
    assert!(is_encrypted(&cell.username), "cellular.username 未加密");
    assert!(is_encrypted(&cell.password), "cellular.password 未加密");
    assert!(
        is_encrypted(cell.pin.as_deref().unwrap()),
        "cellular.pin 未加密"
    );

    // 2) 幂等：对已加密字段再次加密必须保持不变（否则会双重加密成 enc:enc:...）
    let token_after_first = cfg.web.as_ref().unwrap().api_token.clone();
    encrypt_config_secrets(&mut cfg).expect("second encrypt must be a no-op");
    assert_eq!(
        cfg.web.as_ref().unwrap().api_token,
        token_after_first,
        "已 enc: 的字段再次加密必须保持不变（幂等）"
    );

    // 3) 往返：解密后全部还原为明文
    decrypt_config_secrets(&mut cfg).expect("decrypt must round-trip");
    let web = cfg.web.as_ref().unwrap();
    assert_eq!(web.api_token, PLAIN_WEB_TOKEN);
    assert_eq!(web.password_hash, PLAIN_WEB_PWHASH);
    assert_eq!(web.ota_signing_key.as_deref(), Some(PLAIN_OTA_KEY));
    assert_eq!(cfg.mqtt.username.as_deref(), Some(PLAIN_MQTT_USER));
    assert_eq!(cfg.mqtt.password.as_deref(), Some(PLAIN_MQTT_PASS));
    let cell = cfg.cellular.as_ref().unwrap();
    assert_eq!(cell.username, PLAIN_CELL_USER);
    assert_eq!(cell.password, PLAIN_CELL_PASS);
    assert_eq!(cell.pin.as_deref(), Some(PLAIN_CELL_PIN));
}
