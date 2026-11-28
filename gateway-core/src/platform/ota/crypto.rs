//! OTA 固件加密与校验工具
//!
//! 提供 CRC32、签名后缀分离，以及 Ed25519 / 旧 HMAC-SHA256 两种验签。
//!
//! 签名后缀（接在包头 + 载荷之后）：
//! - 新格式：`[payload][FWSG][ed25519_sig:64]`，签名覆盖**包头 + 载荷**
//!   （因此 magic / version / crc 字段也被签名绑定，改版本号同样会导致验签失败）；
//! - 旧格式：`[payload][GSIG][hmac_sha256:32]`，签名只覆盖载荷，仅为兼容已签发旧包保留。

use super::{ED_SIG_LEN, ED_SIG_MAGIC, ED_SIG_SUFFIX_SIZE, SIG_MAGIC, SIG_SUFFIX_SIZE};

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

/// 包尾签名的算法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignatureKind {
    /// 旧格式：共享密钥 HMAC-SHA256（32 字节）
    Hmac,
    /// 新格式：Ed25519 非对称签名（64 字节）
    Ed25519,
}

/// 分离出的签名
#[derive(Debug, Clone, Copy)]
pub(crate) struct FirmwareSignature<'a> {
    pub kind: SignatureKind,
    pub bytes: &'a [u8],
}

/// 分离固件包尾的签名后缀。
///
/// 返回 `(不含签名的内容, 签名)`；`None` 表示包尾没有可识别的签名后缀。
/// 内容包含包头（全量包 12 字节 / 差分包 12 字节），调用方按自己的包头长度切载荷。
/// 先识别 64 字节 Ed25519 后缀，再回退 32 字节旧 HMAC 后缀。
pub(crate) fn split_firmware_signature(data: &[u8]) -> (&[u8], Option<FirmwareSignature<'_>>) {
    if data.len() >= ED_SIG_SUFFIX_SIZE {
        let start = data.len() - ED_SIG_SUFFIX_SIZE;
        if data[start..start + 4] == ED_SIG_MAGIC {
            return (
                &data[..start],
                Some(FirmwareSignature {
                    kind: SignatureKind::Ed25519,
                    bytes: &data[start + 4..start + 4 + ED_SIG_LEN],
                }),
            );
        }
    }
    if data.len() >= SIG_SUFFIX_SIZE {
        let start = data.len() - SIG_SUFFIX_SIZE;
        if data[start..start + 4] == SIG_MAGIC {
            return (
                &data[..start],
                Some(FirmwareSignature {
                    kind: SignatureKind::Hmac,
                    bytes: &data[start + 4..],
                }),
            );
        }
    }
    (data, None)
}

/// 旧格式 HMAC-SHA256 验签（共享密钥，仅为兼容保留）
pub(crate) fn verify_hmac_signature(payload: &[u8], key: &[u8], expected_sig: &[u8]) -> bool {
    use ring::hmac;
    // ring 0.17: Key::new 不返回 Result，直接返回 Key
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    // 恒定时间比较（ring 内部实现）
    hmac::verify(&key, payload, expected_sig).is_ok()
}

/// Ed25519 验签：多公钥逐把尝试，任一通过即接受（换钥过渡期可同时挂新旧公钥）。
///
/// `public_keys` 为空时直接返回 false，由调用方按「未配置公钥」给出明确错误。
pub(crate) fn verify_ed25519_signature(
    message: &[u8],
    public_keys: &[[u8; 32]],
    sig: &[u8],
) -> bool {
    use ring::signature;
    if sig.len() != ED_SIG_LEN || public_keys.is_empty() {
        return false;
    }
    public_keys.iter().any(|key| {
        signature::UnparsedPublicKey::new(&signature::ED25519, key.as_ref())
            .verify(message, sig)
            .is_ok()
    })
}
