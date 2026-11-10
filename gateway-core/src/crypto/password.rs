//! 密码哈希（PBKDF2-HMAC-SHA256，复用 ring crate）
//!
//! 用于 Web 登录密码的哈希存储与校验。PHC 风格字符串 `pbkdf2$<iter>$<salt_b64>$<hash_b64>`。

use crate::engine::codec::encoding::{b64_decode, b64_encode};

const PBKDF2_ITERATIONS: u32 = 100_000;
// D5: 编译期常量对应的 NonZeroU32。const 上下文下 NonZeroU32::new 不可用,
// 用 match 模式匹配 + const panic 触发编译期错误。
const PBKDF2_ITERATIONS_NZ: std::num::NonZeroU32 =
    match std::num::NonZeroU32::new(PBKDF2_ITERATIONS) {
        Some(v) => v,
        None => panic!("PBKDF2_ITERATIONS must be non-zero"),
    };
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
/// 从存储串解析迭代次数时的硬上限。存储串若被篡改为超大迭代值（如 u32::MAX），
/// 每次登录将耗时分钟级 → 登录接口 DoS。上限取当前值（100k）的 50 倍，
/// 既给未来提高强度留足余量，又把单次校验耗时限制在可接受范围。
const MAX_PBKDF2_ITERATIONS: u32 = 5_000_000;
/// Bug r17-PWD1 (fix): salt 长度硬上限。
///
/// 防止存储串被篡改为超大 salt（如 10MB）触发 PBKDF2 长时间迭代导致登录 DoS。
/// 默认 SALT_LEN=16，但攻击者可篡改存储串 salt 字段 → 攻击者控制 salt 长度。
/// 硬上限取 4KB（远大于默认 16 字节，又远小于能触发分钟级耗时的 10MB），
/// 即使最坏情况下 `pbkdf2::verify` 4KB salt 也只需 ms 级。
const MAX_SALT_LEN: usize = 4096;

/// 对明文密码进行 PBKDF2 哈希，返回 PHC 风格字符串：`pbkdf2$<iter>$<salt_b64>$<hash_b64>`
///
/// C-4 修复：SystemRandom 失败时**直接 panic** 阻断启动，不再降级到 xorshift + PID + 时间
/// 拼凑伪随机盐。旧实现在 Linux 熵池极早期耗尽 / Windows CSP 不可用场景下，会让同毫秒
/// 同 PID 的两次启动生成**完全相同的 salt**，salt 退化为常数 → 彩虹表直接复用。
/// 改为 fail-fast：熵源不可用 = 拒绝继续启动 = 运维必须先修复熵源。
pub fn hash_password(plain: &str) -> String {
    use ring::pbkdf2;
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    if let Err(e) = rng.fill(&mut salt) {
        // Fail-fast: 拒绝使用任何弱随机替代。ring 的 SystemRandom 失败说明
        // OS 熵源不可用（Linux 熵池耗尽 / Windows CSP 不可达 / 沙箱权限丢失），
        // 此时生成的"密码" salt 会被攻击者精确预测，hash 不再安全。
        // 选择 panic 而不是降级：运维日志会立刻看到 stack trace + 明确根因，
        // 比"用户能登录但密码几乎可破解"更可被现场处理。
        panic!(
            "SystemRandom.fill failed ({}): entropy source unavailable, refusing to derive \
             a password hash with a weak fallback. Fix the OS entropy source \
             (e.g. wait for Linux kernel entropy to refill, or check Windows CSP service) \
             and restart. See previous warn() entries for context.",
            e
        );
    }

    let mut hash = [0u8; HASH_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        PBKDF2_ITERATIONS_NZ,
        &salt,
        plain.as_bytes(),
        &mut hash,
    );

    format!(
        "pbkdf2${}${}${}",
        PBKDF2_ITERATIONS,
        b64_encode(&salt),
        b64_encode(&hash),
    )
}

/// 生成一个新的 Web API Token（32 字节 CSPRNG 熵 → 64 位十六进制字符串）。
///
/// 用于「改密后轮换 api_token」：旧 token 立即失效，避免长期静态凭证一旦泄漏
/// （日志、截图、共享浏览器）在改密后仍可继续使用，使"改密"这个止损动作失效。
///
/// 采用十六进制而非 base64：base64 的 `+` / `/` 在 URL query 中会被二次解释，
/// 而 token 会通过 `?token=` 传递（WebSocket 兼容路径）。
pub fn generate_api_token() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut buf = [0u8; 32];
    if let Err(e) = rng.fill(&mut buf) {
        panic!(
            "SystemRandom.fill failed ({}): entropy source unavailable, refusing to generate \
             an API token with a weak fallback. Fix the OS entropy source and restart.",
            e
        );
    }
    let mut out = String::with_capacity(buf.len() * 2);
    // 查找表替代 char::from_digit().unwrap()：b>>4 与 b&0x0F 恒在 0..=15，
    // 索引永不越界，行为与 from_digit(x, 16) 完全一致（小写 hex），
    // 且满足 crate 级 deny(clippy::unwrap_used)。
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for b in buf {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

/// 校验明文密码与 PHC 格式哈希是否匹配
pub fn verify_password(plain: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2" {
        return false;
    }
    let iter: u32 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    // 硬上限：拒绝被篡改的超大迭代值，避免单次校验耗时分钟级（登录 DoS）
    if iter == 0 || iter > MAX_PBKDF2_ITERATIONS {
        return false;
    }
    let salt = match b64_decode(parts[2]) {
        Some(s) => s,
        None => return false,
    };
    // Bug r17-PWD1 (fix): 硬上限防 salt 长度 DoS。
    // 攻击者篡改存储串 salt 字段为超大值时，PBKDF2 按 salt 长度迭代，
    // 10MB salt 可导致单次 verify 耗时分钟级 → 登录接口拒绝服务。
    // 4KB 上限远大于默认 16 字节的合法值，又远小于 DoS 触发值。
    if salt.len() > MAX_SALT_LEN {
        return false;
    }
    let stored_hash = match b64_decode(parts[3]) {
        Some(s) => s,
        None => return false,
    };
    if stored_hash.len() != HASH_LEN {
        return false;
    }

    use ring::pbkdf2;
    pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        match std::num::NonZeroU32::new(iter) {
            Some(n) => n,
            // iter=0 是无效配置(可能哈希被篡改),直接判失败避免 panic
            None => return false,
        },
        &salt,
        plain.as_bytes(),
        &stored_hash,
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verify_roundtrip() {
        let h = hash_password("s3cret-pw");
        assert!(h.starts_with("pbkdf2$"));
        assert!(verify_password("s3cret-pw", &h));
        assert!(!verify_password("wrong-pw", &h));
    }

    #[test]
    fn malformed_hash_rejected() {
        assert!(!verify_password("pw", ""));
        assert!(!verify_password("pw", "pbkdf2$100000$onlythree"));
        assert!(!verify_password("pw", "bcrypt$10$aaaa$bbbb"));
    }

    /// 存储串被篡改为超大迭代值时必须快速拒绝（不能变成登录接口 DoS）
    #[test]
    fn oversized_iteration_count_rejected_fast() {
        let h = hash_password("pw");
        let parts: Vec<&str> = h.split('$').collect();
        let tampered = format!("pbkdf2${}${}${}", u32::MAX, parts[2], parts[3]);
        let start = std::time::Instant::now();
        assert!(!verify_password("pw", &tampered));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "超上限迭代值必须快速拒绝，实际耗时 {:?}",
            start.elapsed()
        );
    }

    /// Bug r17-PWD1 (fix): 存储串被篡改为超大 salt 时必须快速拒绝。
    ///
    /// 攻击场景：把 salt 字段替换为 100KB 随机数据 → PBKDF2 按 100KB 迭代
    /// → 单次 verify 耗时秒级 → 登录接口 DoS。修复后 salt 长度硬上限 4KB，
    /// 超限直接返回 false，不进入 PBKDF2 计算。
    #[test]
    fn oversized_salt_rejected_fast() {
        // 构造超大 salt（10KB，超过 4KB 上限）
        use crate::engine::codec::encoding::b64_encode;
        let big_salt = vec![0u8; 10 * 1024];
        let tampered = format!(
            "pbkdf2$100000${}${}",
            b64_encode(&big_salt),
            b64_encode(&[0u8; 32]),
        );
        let start = std::time::Instant::now();
        assert!(!verify_password("pw", &tampered));
        assert!(
            start.elapsed() < std::time::Duration::from_millis(50),
            "超限 salt 必须快速拒绝（< 50ms），实际耗时 {:?}",
            start.elapsed()
        );
    }
}
