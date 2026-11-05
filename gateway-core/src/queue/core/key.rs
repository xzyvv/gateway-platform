//! SQLCipher 数据库加密密钥管理
//!
//! 首次启动时生成 32 字节随机密钥，存储在 `{db_path}.key` 文件中（权限 0600）。
//! 后续启动读取该文件，用 hex 编码后作为 `PRAGMA key` 传入 SQLCipher。
//!
//! 密钥丢失 = 数据不可恢复（对网关可接受，重新采集即可）。
//!
//! r15-KEY1（修复）：
//! - 旧实现先 `fs::write`（Unix 上默认 0644，同机其它用户可读）再 chmod 0600，
//!   两者之间存在**竞态窗口**；且 chmod 失败被 `let _ =` 吞掉，文件永久保持宽权限。
//!   现在改为**创建时即指定 0600**（`OpenOptions::mode`），并对收紧失败显式告警。
//! - 旧实现不校验已有 keyfile 长度：磁盘写满等场景会产生 0 字节 keyfile，
//!   此时 `PRAGMA key = ''` → 数据库既解不开也不报错，表现为「数据丢失」。
//!   现在长度不符直接报错并给出可执行的处置提示。
//!
//! 本模块的核心逻辑（路径推导 / 长度校验 / 安全落盘 / 权限收紧）**不依赖
//! SQLCipher**，因此无条件编译：这样密钥管理逻辑可以在任何平台被单测覆盖，
//! 而 `encryption` feature 只决定「是否真的把密钥喂给 `PRAGMA key`」。

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use crate::engine::error::GatewayError;
use crate::engine::error::GatewayResult;

/// 密钥长度（字节）：SQLCipher 期望 32 字节密钥。
pub(crate) const KEY_LEN: usize = 32;

/// 返回 db_path 对应的 keyfile 路径
pub(crate) fn keyfile_path(db_path: &Path) -> PathBuf {
    let mut p = db_path.as_os_str().to_owned();
    p.push(".key");
    PathBuf::from(p)
}

/// 生成或读取 SQLCipher 加密密钥（hex 编码字符串）。
///
/// - keyfile 不存在 -> 生成 32 字节随机密钥，以 0600 权限原子创建 keyfile，返回 hex
/// - keyfile 存在 -> 读取并返回 hex（长度必须为 32 字节，否则视为损坏并报错）
///
/// 在 Windows 上权限位由 ACL 管理，创建时不做 POSIX 权限设置。
#[cfg(feature = "encryption")]
pub(crate) fn ensure_key(db_path: &Path) -> GatewayResult<String> {
    Ok(to_hex(&ensure_key_bytes(db_path)?))
}

/// 加密未启用：不生成 keyfile，返回空字符串（`PRAGMA key` 被跳过）。
#[cfg(not(feature = "encryption"))]
pub(crate) fn ensure_key(_db_path: &Path) -> GatewayResult<String> {
    Ok(String::new())
}

/// 读取或生成密钥原始字节（32 字节）。
///
/// 与 feature 解耦：即使不启用 SQLCipher 加密，也可对「落盘 + 权限 + 校验」
/// 这段逻辑做跨平台单测。
pub(crate) fn ensure_key_bytes(db_path: &Path) -> GatewayResult<Vec<u8>> {
    let keyfile = keyfile_path(db_path);

    if keyfile.exists() {
        let key_bytes = std::fs::read(&keyfile)
            .map_err(|e| GatewayError::Queue(format!("read db keyfile failed: {}", e)))?;
        validate_key_len(&keyfile, key_bytes.len())?;
        tracing::info!("loaded existing SQLCipher key from {}", keyfile.display());
        Ok(key_bytes)
    } else {
        let key_bytes = generate_random_bytes(KEY_LEN)?;
        write_keyfile_secure(&keyfile, &key_bytes)?;
        tracing::info!("generated new SQLCipher key at {}", keyfile.display());
        Ok(key_bytes)
    }
}

/// 校验已有 keyfile 的长度是否合法（纯函数，任意平台可测）。
///
/// 长度不符 = keyfile 损坏（磁盘写满 / 掉电 / 被人为改动）。
/// 此时若继续使用，会得到 `PRAGMA key = ''` 或错误密钥 →
/// 数据库打不开且现象像「数据丢了」。必须显式报错。
pub(crate) fn validate_key_len(path: &Path, len: usize) -> GatewayResult<()> {
    if len != KEY_LEN {
        return Err(GatewayError::Queue(format!(
            "db keyfile {} is corrupted: expected {} bytes, got {} — \
             refusing to open the database with an invalid SQLCipher key \
             (delete the keyfile to regenerate, but note the old database \
             will then be unreadable)",
            path.display(),
            KEY_LEN,
            len
        )));
    }
    Ok(())
}

/// 以 0600 权限原子创建 keyfile（Unix）；Windows 走默认 ACL。
///
/// 写失败时删除半截 keyfile，避免下次启动读到损坏密钥。
pub(crate) fn write_keyfile_secure(path: &Path, bytes: &[u8]) -> GatewayResult<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 创建即带 0600：不存在「先宽后紧」的窗口期。
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| GatewayError::Queue(format!("create db keyfile failed: {}", e)))?;
    if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
        let _ = std::fs::remove_file(path);
        return Err(GatewayError::Queue(format!(
            "write db keyfile failed: {} (incomplete keyfile removed)",
            e
        )));
    }
    // 兜底收紧权限：若 umask 或已有文件导致放宽则修正。
    set_file_permissions(path);
    Ok(())
}

/// 生成随机字节（使用 ring 的 SystemRandom）
fn generate_random_bytes(len: usize) -> GatewayResult<Vec<u8>> {
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    let mut buf = vec![0u8; len];
    rng.fill(&mut buf)
        .map_err(|e| GatewayError::Queue(format!("generate random key failed: {e}")))?;
    Ok(buf)
}

/// 将字节编码为十六进制字符串
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 设置文件权限为 0600（仅 Unix）
///
/// r15-KEY1：失败不再静默——权限收紧失败意味着密钥文件对同机其它用户可读，
/// 必须留下明确告警日志（否则现场无从发现）。
#[cfg(unix)]
fn set_file_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            if let Err(e) = std::fs::set_permissions(path, perms) {
                tracing::error!(
                    "db keyfile {}: chmod 0600 failed: {} — the key file may be readable by other users",
                    path.display(),
                    e
                );
            }
        }
        Err(e) => {
            tracing::error!(
                "db keyfile {}: stat failed, cannot enforce 0600: {}",
                path.display(),
                e
            );
        }
    }
}

/// 非 Unix：权限由 ACL 管理，无需处理。
#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gw-key-test-{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// 首次生成：keyfile 存在、长度 32 字节。
    #[test]
    fn generates_keyfile_with_expected_length() {
        let dir = tmp_dir("gen");
        let db = dir.join("data.db");
        let bytes = ensure_key_bytes(&db).expect("ensure_key_bytes");
        assert_eq!(bytes.len(), KEY_LEN);
        let keyfile = keyfile_path(&db);
        assert!(keyfile.exists(), "keyfile must be created");
        assert_eq!(
            std::fs::metadata(&keyfile).expect("stat").len(),
            KEY_LEN as u64
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 二次调用返回同一密钥（幂等，不能每次启动都换密钥）。
    #[test]
    fn returns_same_key_on_second_call() {
        let dir = tmp_dir("idem");
        let db = dir.join("data.db");
        let first = ensure_key_bytes(&db).expect("first");
        let second = ensure_key_bytes(&db).expect("second");
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 损坏的 keyfile（长度不符）必须报错，而不是拿它去开库。
    #[test]
    fn rejects_corrupted_keyfile() {
        let dir = tmp_dir("corrupt");
        let db = dir.join("data.db");
        std::fs::write(keyfile_path(&db), b"short").expect("write corrupt keyfile");
        let err = ensure_key_bytes(&db).expect_err("corrupted keyfile must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("corrupted"),
            "error must say corrupted: {}",
            msg
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空 keyfile（磁盘写满等场景）必须报错，
    /// 否则 `PRAGMA key = ''` 会让数据库「打不开也不报错」。
    #[test]
    fn rejects_empty_keyfile() {
        let dir = tmp_dir("empty");
        let db = dir.join("data.db");
        std::fs::write(keyfile_path(&db), b"").expect("write empty keyfile");
        assert!(ensure_key_bytes(&db).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 长度校验是纯函数：边界值（31/32/33）行为明确。
    #[test]
    fn key_len_validation_boundaries() {
        let p = Path::new("/tmp/x.key");
        assert!(validate_key_len(p, KEY_LEN).is_ok());
        assert!(validate_key_len(p, 0).is_err());
        assert!(validate_key_len(p, KEY_LEN - 1).is_err());
        assert!(validate_key_len(p, KEY_LEN + 1).is_err());
    }

    /// Unix 上 keyfile 必须是 0600（创建即收紧，不存在宽权限窗口）。
    #[cfg(unix)]
    #[test]
    fn keyfile_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("perm");
        let db = dir.join("data.db");
        ensure_key_bytes(&db).expect("ensure_key_bytes");
        let mode = std::fs::metadata(keyfile_path(&db))
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "keyfile mode must be 0600, got {:o}",
            mode & 0o777
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
