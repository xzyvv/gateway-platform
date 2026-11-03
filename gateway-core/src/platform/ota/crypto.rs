//! OTA 固件加密与校验工具
//!
//! 提供 CRC32 校验、固件签名分离和 HMAC-SHA256 签名验证。

use super::{SIG_MAGIC, SIG_SUFFIX_SIZE};

/// CRC32 校验（IEEE 802.3 多项式，反转）
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// 分离固件 payload 和签名后缀
///
/// 返回 (payload, has_signature)
/// 如果固件包末尾有 SIG_MAGIC + 32 字节签名，返回 (payload, true)
/// 否则返回 (原始 payload, false)
pub(crate) fn split_firmware_signature(data: &[u8]) -> (&[u8], bool) {
    if data.len() >= SIG_SUFFIX_SIZE {
        let suffix_start = data.len() - SIG_SUFFIX_SIZE;
        let magic = &data[suffix_start..suffix_start + 4];
        if magic == SIG_MAGIC {
            return (&data[..suffix_start], true);
        }
    }
    (data, false)
}

/// HMAC-SHA256 签名验证（使用 ring）
pub(crate) fn verify_hmac_signature(payload: &[u8], key: &[u8], expected_sig: &[u8]) -> bool {
    use ring::hmac;
    // ring 0.17: Key::new 不返回 Result，直接返回 Key
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    // 恒定时间比较（ring 内部实现）
    hmac::verify(&key, payload, expected_sig).is_ok()
}
