//! SQLCipher 数据库加密密钥管理
//!
//! 首次启动时生成 32 字节随机密钥，存储在 `{db_path}.key` 文件中（权限 0600）。
//! 后续启动读取该文件，用 hex 编码后作为 `PRAGMA key` 传入 SQLCipher。
//!
//! 密钥丢失 = 数据不可恢复（对网关可接受，重新采集即可）。

use std::path::Path;
#[cfg(feature = "encryption")]
use std::path::PathBuf;

#[cfg(feature = "encryption")]
use crate::engine::error::GatewayError;
use crate::engine::error::GatewayResult;

/// 返回 db_path 对应的 keyfile 路径
#[cfg(feature = "encryption")]
fn keyfile_path(db_path: &Path) -> PathBuf {
    let mut p = db_path.as_os_str().to_owned();
    p.push(".key");
    PathBuf::from(p)
}

/// 生成或读取 SQLCipher 加密密钥（hex 编码字符串）
///
/// - keyfile 不存在 -> 生成 32 字节随机密钥，写入 keyfile（权限 0600），返回 hex
/// - keyfile 存在 -> 读取并返回 hex
///
/// 在 Windows 上 set_permissions 是 no-op（文件权限由 ACL 管理）。
#[cfg(feature = "encryption")]
pub fn ensure_key(db_path: &Path) -> GatewayResult<String> {
    let keyfile = keyfile_path(db_path);

    if keyfile.exists() {
        let key_bytes = std::fs::read(&keyfile)
            .map_err(|e| GatewayError::Queue(format!("read db keyfile failed: {}", e)))?;
        let key_hex = to_hex(&key_bytes);
        tracing::info!("loaded existing SQLCipher key from {}", keyfile.display());
        Ok(key_hex)
    } else {
        let key_bytes = generate_random_bytes(32)?;
        std::fs::write(&keyfile, &key_bytes)
            .map_err(|e| GatewayError::Queue(format!("write db keyfile failed: {}", e)))?;
        set_file_permissions(&keyfile);
        let key_hex = to_hex(&key_bytes);
        tracing::info!("generated new SQLCipher key at {}", keyfile.display());
        Ok(key_hex)
    }
}

/// 生成随机字节（使用 ring 的 SystemRandom）
#[cfg(feature = "encryption")]
fn generate_random_bytes(len: usize) -> GatewayResult<Vec<u8>> {
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    let mut buf = vec![0u8; len];
    rng.fill(&mut buf)
        .map_err(|e| GatewayError::Queue(format!("generate random key failed: {e}")))?;
    Ok(buf)
}

/// 将字节编码为十六进制字符串
#[cfg(feature = "encryption")]
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 设置文件权限为 0600（仅 Unix）
#[cfg(feature = "encryption")]
#[cfg(unix)]
fn set_file_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(mut perms) = std::fs::metadata(path).map(|m| m.permissions()) {
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

/// Windows 上文件权限由 ACL 管理，此处 no-op
#[cfg(feature = "encryption")]
#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) {}

#[cfg(not(feature = "encryption"))]
pub fn ensure_key(_db_path: &Path) -> GatewayResult<String> {
    // 加密功能未启用，返回空字符串（PRAGMA key 被跳过）
    Ok(String::new())
}
