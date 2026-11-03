//! 配置敏感字段加密（机器绑定密钥）
//!
//! 用于保护 config.json 中的 api_token 等敏感字段，避免明文落盘。
//! 加密后的字段以 `enc:` 前缀标识，启动时由 `decrypt_config_secrets` 解密。
//!
//! 设计要点：
//! - 机器绑定：密钥由本机指纹（machine-id + CPU 信息）派生，密文离开本机即失效
//! - 无外部依赖：手写 SHA256 + base64 + XOR，避免引入新 crate
//! - 对称加密：XOR 流式加密，加解密同一函数
//! - 密钥派生：SHA256 迭代 1000 次（PBKDF1 风格），增加暴力推算成本
//!
//! 安全性说明：
//! 本方案面向"防止明文配置被随意翻看"的场景，不属于强加密。
//! 机器指纹可被本机 root 读取，因此本机被攻破后密文仍可被解出。
//! 如需更强保护，应使用 HSM 或 TPM 密封。
//!
//! 故障策略（C-2 / C-3 修复）：
//! 机器指纹获取失败 / 单字段解密失败时**直接返回 Err**，由 bootstrap 阻断启动。
//! 旧实现 `decrypt_field` 失败仅 `warn` 后保留原文 `enc:xxx` 字面串，
//! 下游模块（如 MQTT 连接）拿到 `enc:xxx` 直接发 broker，导致"启动看似正常但链路失败"。
//! 改为 fail-fast：现场必须先修复 config 加密与本机指纹一致性。

use ring::rand::{SecureRandom, SystemRandom};

use super::sha256::sha256;
use crate::config::GatewayConfig;
use crate::engine::encoding::{b64_decode, b64_encode};
use zeroize::{Zeroize, Zeroizing};

/// 加密字段前缀
const ENC_PREFIX: &str = "enc:";
/// SHA256 迭代次数（密钥派生）
const KDF_ITERATIONS: usize = 1000;
/// Bug 174: 每条密文携带的随机 nonce 长度（字节）。
const NONCE_LEN: usize = 16;
/// Bug 174: 新格式密文的魔数（base64 解码后的前缀），用于区分「带 nonce 的新格式」
/// 与历史「裸 XOR 固定密钥流」旧格式（旧密文仍需可解）。
const NONCE_MAGIC: &[u8] = b"ENC2";

/// 判断字段是否已加密
pub fn is_encrypted(s: &str) -> bool {
    s.starts_with(ENC_PREFIX)
}

/// 加密明文字符串，返回 `enc:<base64>` 形式；机器指纹不可用时返回 `Err`。
///
/// 与 [`encrypt`] 的区别：不 panic。供落盘路径（[`encrypt_config_secrets`]）使用——
/// 落盘失败应作为可恢复错误返回给调用方，而不是 panic 掉整个进程。
///
/// C-1：使用 `zeroize::Zeroizing` 包装 cipher / key，离开作用域时自动
/// 用 volatile write + fence 真正清零（手写 `for *b = 0` 会被 LLVM release
/// profile 优化掉，导致 release 构建零化失效）。
pub fn try_encrypt(plaintext: &str) -> Result<String, String> {
    let mut key = Zeroizing::new(derive_machine_key()?);
    // Bug 174: 每条密文生成随机 nonce，使相同明文每次得到不同密文。
    // 原实现使用固定密钥流（key 重复）→ 相同明文恒等密文，且两密文异或 = 两明文异或，
    // 泄漏明文间关系。随机源不可用时 fail-closed 返回 Err（与 hash_password 的 C-4 策略一致）。
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|e| format!("entropy source unavailable: cannot generate nonce: {e}"))?;
    let mut cipher = Zeroizing::new(xor_stream_nonce(
        plaintext.as_bytes(),
        key.as_slice(),
        &nonce,
    ));
    // 新格式 = MAGIC + nonce + ciphertext；旧格式（无 MAGIC）启动时仍可解。
    let mut payload = Vec::with_capacity(NONCE_MAGIC.len() + NONCE_LEN + cipher.len());
    payload.extend_from_slice(NONCE_MAGIC);
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(cipher.as_slice());
    let result = format!("{}{}", ENC_PREFIX, b64_encode(&payload));
    // Zeroizing Drop 会自动清零；这里保留显式 zeroize 主要是为了在错误路径上也覆盖
    cipher.as_mut_slice().zeroize();
    key.as_mut_slice().zeroize();
    Ok(result)
}

/// 加密明文字符串，返回 `enc:<base64>` 形式。
///
/// 机器指纹不可用时 panic（fail-fast）：取不到机器指纹就无法生成「本机可解」的密文，
/// 宁可 panic 也不写出用弱密钥加密的字段。需要不 panic 的版本请用 [`try_encrypt`]。
pub fn encrypt(plaintext: &str) -> String {
    // 不用 `.expect`（crate 级 `deny(clippy::expect_used)`），改用显式 match + panic!，
    // 保留原 fail-fast 语义。
    match try_encrypt(plaintext) {
        Ok(s) => s,
        Err(e) => panic!(
            "machine fingerprint unavailable; cannot encrypt without a stable machine key: {}",
            e
        ),
    }
}

/// 解密 `enc:<base64>` 形式的字符串。返回 None 表示格式非法或解密失败。
///
/// C-1 修复：key / cipher / plain 三块缓冲区全部 `Zeroizing` 包装。
///
/// Bug P0-CR1 (fix) 兼容性：旧密文用「无 purpose」key 派生（P0-CR1 之前），新密文用
/// KDF_PURPOSE_CONFIG 派生。decrypt 先用新 key 试，失败回退到 legacy 入口（无 purpose）。
/// legacy 入口与 CR-1 前完全等价 → 旧密文可解，跨域攻击阻断效果不变（新 key 不再
/// 与 license HMAC 共享）。
pub fn decrypt(input: &str) -> Option<String> {
    if !is_encrypted(input) {
        return None;
    }
    let b64 = &input[ENC_PREFIX.len()..];
    let raw = Zeroizing::new(b64_decode(b64)?);
    // Bug 174: 新格式（MAGIC + nonce + ciphertext）用随机 nonce 派生密钥流；
    // 历史格式（无 MAGIC）沿用裸 XOR 固定密钥流，保持对已落盘旧密文的兼容。
    let try_decrypt = |cipher_text: &[u8], nonce: Option<&[u8]>| -> Option<String> {
        // 第一轮：用新 key（KDF_PURPOSE_CONFIG）
        if let Some(plain_bytes) = try_decrypt_with_purpose(cipher_text, nonce, KDF_PURPOSE_CONFIG)
        {
            return Some(plain_bytes);
        }
        // 第二轮：fallback 到 legacy key（无 purpose，与 CR-1 前完全等价）—— 兼容
        // 升级部署中已落盘的旧密文。KDF 派生 1ms 内完成，重试几乎无开销。
        try_decrypt_with_purpose(cipher_text, nonce, b"")
    };
    if raw.len() >= NONCE_MAGIC.len() + NONCE_LEN && raw.starts_with(NONCE_MAGIC) {
        let nonce = &raw[NONCE_MAGIC.len()..NONCE_MAGIC.len() + NONCE_LEN];
        let cipher_text = &raw[NONCE_MAGIC.len() + NONCE_LEN..];
        let result = try_decrypt(cipher_text, Some(nonce))?;
        Some(result)
    } else {
        let result = try_decrypt(raw.as_slice(), None)?;
        Some(result)
    }
}

/// 扫描配置中的敏感字段，若已加密则就地解密为明文。
///
/// C-3 修复：解密失败时返回 `Err(field_name, reason)`，由调用方（`config/loader.rs`）
/// 决定阻断启动。旧实现仅 `warn` 后保留 `enc:xxx` 字面串，
/// 下游模块拿到后会用字面串当凭据使用，导致"启动看似正常但链路失败"且无任何告警。
///
/// 覆盖字段（须与 [`encrypt_config_secrets`] 完全一致，保证加解密往返）：
/// - web.api_token / web.password_hash / web.ota_signing_key
/// - mqtt.username / mqtt.password
/// - cellular.username / cellular.password / cellular.pin
/// - buses[].opcua.username / buses[].opcua.password（P0-3，按数组下标逐项处理）
/// - opcua.users[].password（OPC UA Server 用户名认证，按数组下标逐项处理）
pub fn decrypt_config_secrets(config: &mut GatewayConfig) -> Result<(), String> {
    if let Some(web) = config.web.as_mut() {
        decrypt_field(&mut web.api_token, "web.api_token")?;
        decrypt_field(&mut web.password_hash, "web.password_hash")?;
        if let Some(key) = web.ota_signing_key.as_mut() {
            decrypt_field(key, "web.ota_signing_key")?;
        }
    }
    if let Some(u) = config.mqtt.username.as_mut() {
        decrypt_field(u, "mqtt.username")?;
    }
    if let Some(p) = config.mqtt.password.as_mut() {
        decrypt_field(p, "mqtt.password")?;
    }
    if let Some(cell) = config.cellular.as_mut() {
        decrypt_field(&mut cell.username, "cellular.username")?;
        decrypt_field(&mut cell.password, "cellular.password")?;
        if let Some(pin) = cell.pin.as_mut() {
            decrypt_field(pin, "cellular.pin")?;
        }
    }
    // P0-3: OPC UA Client 凭据。数组下标无法用静态 JSON Pointer 表达，
    // 与其他动态字段一样在此逐项处理（API 侧脱敏/还原按 port_name 匹配）。
    for (i, bus) in config.buses.iter_mut().enumerate() {
        if let Some(opcua) = bus.opcua.as_mut() {
            let prefix = format!("buses[{}].opcua", i);
            if let Some(u) = opcua.username.as_mut() {
                decrypt_field(u, &format!("{}.username", prefix))?;
            }
            if let Some(p) = opcua.password.as_mut() {
                decrypt_field(p, &format!("{}.password", prefix))?;
            }
        }
    }
    // OPC UA Server 用户名/密码。用户名可读，密码属于凭据必须加密；
    // 数组下标无法表达为静态 JSON Pointer，在此逐项处理（API 侧按 username 匹配还原）。
    if let Some(opcua) = config.opcua.as_mut() {
        for (i, u) in opcua.users.iter_mut().enumerate() {
            decrypt_field(&mut u.password, &format!("opcua.users[{}].password", i))?;
        }
    }
    Ok(())
}

/// 扫描配置中的敏感字段，落盘前就地加密为 `enc:<base64>`。
///
/// Bug C5: `ConfigLoader::save` / `ConfigVersionStore::save_version` 直接序列化内存中的
/// 配置，而内存中的配置已被 `load()` / `reload()` 解密为**明文**。于是任何一次在线编辑
/// （web 改配置、版本快照）都会把 api_token / password 以明文写回磁盘，
/// `enc:` 保护在第一次保存后即失效。落盘前统一重新加密。
///
/// 幂等：已是 `enc:` 的字段跳过，空串跳过，可安全重复调用。
/// 机器指纹不可用时返回 `Err`（不 panic），由调用方 fail-closed 阻断落盘。
pub fn encrypt_config_secrets(config: &mut GatewayConfig) -> Result<(), String> {
    if let Some(web) = config.web.as_mut() {
        encrypt_field(&mut web.api_token, "web.api_token")?;
        encrypt_field(&mut web.password_hash, "web.password_hash")?;
        if let Some(key) = web.ota_signing_key.as_mut() {
            encrypt_field(key, "web.ota_signing_key")?;
        }
    }
    if let Some(u) = config.mqtt.username.as_mut() {
        encrypt_field(u, "mqtt.username")?;
    }
    if let Some(p) = config.mqtt.password.as_mut() {
        encrypt_field(p, "mqtt.password")?;
    }
    if let Some(cell) = config.cellular.as_mut() {
        encrypt_field(&mut cell.username, "cellular.username")?;
        encrypt_field(&mut cell.password, "cellular.password")?;
        if let Some(pin) = cell.pin.as_mut() {
            encrypt_field(pin, "cellular.pin")?;
        }
    }
    // P0-3: OPC UA Client 凭据（与 decrypt_config_secrets 覆盖范围严格一致）
    for (i, bus) in config.buses.iter_mut().enumerate() {
        if let Some(opcua) = bus.opcua.as_mut() {
            let prefix = format!("buses[{}].opcua", i);
            if let Some(u) = opcua.username.as_mut() {
                encrypt_field(u, &format!("{}.username", prefix))?;
            }
            if let Some(p) = opcua.password.as_mut() {
                encrypt_field(p, &format!("{}.password", prefix))?;
            }
        }
    }
    // OPC UA Server 用户名/密码（与 decrypt_config_secrets 覆盖范围严格一致）
    if let Some(opcua) = config.opcua.as_mut() {
        for (i, u) in opcua.users.iter_mut().enumerate() {
            encrypt_field(&mut u.password, &format!("opcua.users[{}].password", i))?;
        }
    }
    Ok(())
}

/// 单字段就地加密：空串或已是 `enc:` 密文则跳过（幂等），否则加密。
fn encrypt_field(field: &mut String, name: &str) -> Result<(), String> {
    if field.is_empty() || is_encrypted(field) {
        return Ok(());
    }
    match try_encrypt(field) {
        Ok(enc) => {
            *field = enc;
            Ok(())
        }
        Err(e) => Err(format!(
            "encrypt {} failed: {}; refusing to persist plaintext secrets to disk",
            name, e
        )),
    }
}

/// 单字段就地解密：若以 `enc:` 开头则尝试解密，失败时返回 `Err(field_name, reason)`。
/// 抽出来避免每个字段重复 match 模板。
fn decrypt_field(field: &mut String, name: &str) -> Result<(), String> {
    if !is_encrypted(field) {
        return Ok(());
    }
    match decrypt(field) {
        Some(plain) => {
            *field = plain;
            Ok(())
        }
        None => Err(format!(
            "decrypt {} failed: ciphertext cannot be decrypted on this host. \
             this usually means the machine fingerprint changed (e.g. OS reinstall, \
             /etc/machine-id rotated, registry MachineGuid replaced) or the \
             ciphertext was generated on a different machine. Re-issue the secret \
             on the current host before starting the gateway.",
            name
        )),
    }
}

/// 取本机指纹，跨平台：
/// - Linux: /etc/machine-id + /proc/cpuinfo Serial/Hardware
/// - Windows: 注册表 MachineGuid（HKLM\SOFTWARE\Microsoft\Cryptography）
/// - macOS: IOPlatformUUID（ioreg 命令）
///
/// C-2 修复：所有来源都失败时返回 `Err`，不再静默回退到 hostname / computername。
/// 旧实现在 Windows 上 `/etc/machine-id` 不存在、`read_windows_machine_guid` 失败
/// 时落到 `COMPUTERNAME`（短字符串如 "DESKTOP-XXXX"），攻击者拿到镜像后
/// 数小时内可批量破解 config 加密字段。改为 fail-fast：现场必须
/// 修复指纹来源（如 `reg` 命令路径异常、注册表权限丢失）才能启动。
fn machine_fingerprint() -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut sources_tried: Vec<&str> = Vec::new();

    // 1. machine-id（Linux）
    #[cfg(target_os = "linux")]
    {
        sources_tried.push("/etc/machine-id");
        match std::fs::read_to_string("/etc/machine-id") {
            Ok(id) => {
                let trimmed = id.trim();
                if !trimmed.is_empty() {
                    buf.extend_from_slice(trimmed.as_bytes());
                }
            }
            Err(e) => {
                tracing::warn!("machine fingerprint: /etc/machine-id read failed: {}", e);
            }
        }
    }
    // 1. Windows MachineGuid（注册表）
    #[cfg(target_os = "windows")]
    {
        sources_tried.push("HKLM\\SOFTWARE\\Microsoft\\Cryptography\\MachineGuid");
        match read_windows_machine_guid() {
            Some(guid) if !guid.is_empty() => buf.extend_from_slice(guid.as_bytes()),
            Some(_) => tracing::warn!("machine fingerprint: Windows MachineGuid is empty"),
            None => tracing::warn!("machine fingerprint: read_windows_machine_guid returned None"),
        }
    }
    // 1. macOS IOPlatformUUID
    #[cfg(target_os = "macos")]
    {
        sources_tried.push("ioreg IOPlatformUUID");
        match read_macos_platform_uuid() {
            Some(uuid) if !uuid.is_empty() => buf.extend_from_slice(uuid.as_bytes()),
            Some(_) => tracing::warn!("machine fingerprint: macOS IOPlatformUUID is empty"),
            None => tracing::warn!("machine fingerprint: read_macos_platform_uuid returned None"),
        }
    }

    // 2. CPU 信息（Linux：取 Serial/Hardware 行）
    #[cfg(target_os = "linux")]
    {
        sources_tried.push("/proc/cpuinfo Serial/Hardware");
        if let Ok(cpu) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in cpu.lines() {
                if line.starts_with("Serial") || line.starts_with("Hardware") {
                    buf.extend_from_slice(line.as_bytes());
                    break;
                }
            }
        }
    }

    if buf.is_empty() {
        return Err(format!(
            "machine fingerprint unavailable: none of the sources yielded a value ({:?}). \
             refusing to derive an encryption key from a weak fallback (hostname) — \
             the previous behavior made config secrets recoverable by anyone with a disk image. \
             Fix the platform-specific source (e.g. restore /etc/machine-id, \
             ensure `reg query` works on Windows) and restart.",
            sources_tried
        ));
    }
    Ok(buf)
}

/// A7: 读取 Windows 注册表 MachineGuid，无需引入 winreg 依赖。
///
/// 调 `reg query HKLM\SOFTWARE\Microsoft\Cryptography /v MachineGuid`，
/// 输出形如：
///   HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Cryptography
///       MachineGuid    REG_SZ    <guid>
#[cfg(target_os = "windows")]
fn read_windows_machine_guid() -> Option<String> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| line.contains("MachineGuid"))
        .and_then(|line| line.split_whitespace().last())
        .map(|s| s.to_string())
}

/// A7: 读取 macOS IOPlatformUUID（ioreg 命令）。
#[cfg(target_os = "macos")]
fn read_macos_platform_uuid() -> Option<String> {
    let output = std::process::Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| line.contains("IOPlatformUUID"))
        .and_then(|line| {
            line.split('=')
                .last()
                .map(|s| s.trim().trim_matches('"').to_string())
        })
}

/// KDF 派生时混入的用途标签（Bug P0-CR1 fix：阻断跨域攻击）。
///
/// 旧实现 `derive_machine_key` 与 `derive_machine_hmac_key` 输出**完全相同**的 32 字节，
/// 攻击者在 root 失守场景下拿到 config.json + license 副本 HMAC 后可离线做 KDF
/// 暴力（1000 次 SHA256，GPU 数小时可破），恢复机器指纹后**所有 config 字段**
/// （api_token / mqtt.password / cellular.pin）可批量还原。
///
/// 修复：每个用途用不同标签混入 KDF 输入，派生独立密钥。同一指纹 + 不同
/// purpose → 不同密钥，跨域攻击失效。
///
/// 注意：**这是首次部署引入**——已有密文是用旧 `derive_machine_key`（无 purpose）
/// 派生的，仍可解密（保留旧入口作为 fallback）。新写入的密文用新入口。
const KDF_PURPOSE_CONFIG: &[u8] = b"config-encryption-v1";
const KDF_PURPOSE_LICENSE_HMAC: &[u8] = b"license-clock-hmac-v1";

/// 由本机指纹 + 用途标签派生 256 位密钥：SHA256 迭代 1000 次
///
/// C-2 修复：指纹获取失败返回 `Err`，让上层（`encrypt` / `decrypt`）决定
/// 是 panic（encrypt 路径，确保写出的密文仍可在本机解）还是返回 None（decrypt 路径）。
///
/// P0-CR1 (fix)：原实现忽略 purpose，license HMAC 与 config 加密用同一密钥，
/// 攻击者可跨域攻击。新实现混入 purpose 标签阻断跨域。
fn derive_machine_key_with_purpose(purpose: &[u8]) -> Result<[u8; 32], String> {
    let fp = machine_fingerprint()?;
    // 首次 sha256 输入 = fingerprint || purpose，确保不同 purpose 派生不同初始 state
    let mut input = Vec::with_capacity(fp.len() + purpose.len());
    input.extend_from_slice(&fp);
    input.extend_from_slice(purpose);
    let mut state = sha256(&input);
    for _ in 0..KDF_ITERATIONS {
        state = sha256(&state);
    }
    Ok(state)
}

/// Bug P0-CR1 (fix) 配套：用指定 purpose 派生的 key 尝试解密，返回 Some(plain) 表示
/// 解出 UTF-8 有效字符串，None 表示失败（key 不匹配或 UTF-8 无效）。try_encrypt / decrypt
/// 共用此函数，encrypt 时只传目的 purpose，decrypt 时按"新 purpose → 旧 purpose"回退。
fn try_decrypt_with_purpose(
    cipher_text: &[u8],
    nonce: Option<&[u8]>,
    purpose: &[u8],
) -> Option<String> {
    let mut key = Zeroizing::new(derive_machine_key_with_purpose(purpose).ok()?);
    let plain_vec = match nonce {
        Some(n) => xor_stream_nonce(cipher_text, key.as_slice(), n),
        None => xor_stream(cipher_text, key.as_slice()),
    };
    let mut plain = Zeroizing::new(plain_vec);
    let result = String::from_utf8(plain.as_slice().to_vec()).ok();
    key.as_mut_slice().zeroize();
    plain.as_mut_slice().zeroize();
    result
}

/// 配置加密用的机器绑定密钥（config.json 中的 api_token / mqtt.password 等）。
/// 用途标签 = KDF_PURPOSE_CONFIG，与 license HMAC 完全隔离。
fn derive_machine_key() -> Result<[u8; 32], String> {
    derive_machine_key_with_purpose(KDF_PURPOSE_CONFIG)
}

/// 暴露机器绑定密钥派生函数（仅 crate 内）——供 license/clock 副本 HMAC 复用同源密钥。
///
/// 与 `derive_machine_key` 共享 machine_fingerprint，但用不同 purpose 标签，
/// **确保两个用途派生出完全独立的 32 字节**（P0-CR1 fix）。
pub(crate) fn derive_machine_hmac_key() -> Result<[u8; 32], String> {
    derive_machine_key_with_purpose(KDF_PURPOSE_LICENSE_HMAC)
}

/// XOR 流式加解密（同一函数双向可用）
///
/// 入参切片长度任意；零拷贝返回 `Vec<u8>`，由调用方用 `Zeroizing` 包装。
fn xor_stream(data: &[u8], key: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, &b)| b ^ key[i % key.len()])
        .collect()
}

/// Bug 174: 由 机器密钥 + nonce 派生的密钥流（SHA256 计数器模式扩展）。
///
/// 同一明文配不同 nonce 得到不同密钥流，从而不同密文，修复固定 keystream 下
/// 「相同明文 = 相同密文」「两密文异或 = 两明文异或」的信息泄漏。
fn derive_keystream(key: &[u8], nonce: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut counter: u64 = 0;
    while out.len() < len {
        let mut buf = Vec::with_capacity(key.len() + nonce.len() + 8);
        buf.extend_from_slice(key);
        buf.extend_from_slice(nonce);
        buf.extend_from_slice(&counter.to_le_bytes());
        out.extend_from_slice(&sha256(&buf));
        counter = counter.wrapping_add(1);
    }
    out.truncate(len);
    out
}

/// Bug 174: 带 nonce 的 XOR 流式加解密（加解密同一函数，双向可用）。
fn xor_stream_nonce(data: &[u8], key: &[u8], nonce: &[u8]) -> Vec<u8> {
    let keystream = derive_keystream(key, nonce, data.len());
    data.iter()
        .zip(keystream.iter())
        .map(|(a, b)| a ^ b)
        .collect()
}

#[cfg(test)]
mod tests;
