//! License 文件验签与字段校验（§4、§7 校验顺序）
//!
//! 文件格式：`base64(payload_json) + "." + base64(ed25519_signature)`（两段式），
//! 签名对象为**第一段 base64 字符串的原始字节**（JWT 同思路，避免解码歧义）。
//!
//! 校验规则：**先验签，后读字段**——签名无效直接按"文件损坏"处理，
//! 不解析 payload；字段容忍策略只作用于验签通过之后的比对。
//!
//! B3 修复：新增 `verify_with_fingerprint` 统一入口，把"验签 + 指纹比对"打包
//! 一次完成（之前需由调用方在 verify 之后再单独调 fingerprint::compare，
//! 漏调即绕过——典型 B3 第 1 子问题）。`LicenseManager::evaluate_at` /
//! `LicenseManager::dry_parse` 已切到本入口。

use ring::signature;
use serde::{Deserialize, Serialize};

use crate::engine::util::encoding::{b64_decode, b64_encode};

use super::fingerprint::{compare as compare_fingerprint, LocalFingerprints};
use super::Tier;

/// License 校验错误（§7：状态映射 — Decode/BadSignature/BadVersion/BadPayload →
/// Invalid；Fingerprint → Mismatch；Expired → Expired；ClockTampered → Tampered）
#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("license file not found")]
    NotFound,
    #[error("read license file failed: {0}")]
    Read(String),
    #[error("license format invalid (two-part decode)")]
    Decode,
    #[error("license signature invalid")]
    BadSignature,
    #[error("license version unsupported: {0}")]
    BadVersion(u32),
    #[error("license payload invalid: {0}")]
    BadPayload(String),
    #[error("machine fingerprint mismatch")]
    Fingerprint,
    #[error("license expired")]
    Expired,
    #[error("clock rollback detected")]
    ClockTampered,
    #[error("license clock store error: {0}")]
    ClockStore(String),
    #[error("license io error: {0}")]
    Io(String),
}

/// 临期阈值：剩余 ≤ 30 天 → Expiring
pub const EXPIRING_WINDOW_SECS: i64 = 30 * 86400;

/// r34-LV1: license payload/sig base64 解码后最大字节数。
/// 防止恶意 license 文件（GB 级 base64 串）触发 OOM。典型 license payload
/// ≤ 4KB（v1/v2 字段约 20 个），sig 64 字节（Ed25519）。
const MAX_LICENSE_PAYLOAD_BYTES: usize = 64 * 1024; // 64 KiB
const MAX_LICENSE_SIG_BYTES: usize = 1024; // 1 KiB（容纳 base64 padding）

/// r34-LV2: license 公钥数量上限。换钥过渡期典型 2-3 把（old + new），10 把已
/// 足够冗余。超过 10 把视为配置错误（可能是配置回滚 / 误用）。
const MAX_LICENSE_PUBLIC_KEYS: usize = 10;

/// RFC3339 → unix 秒
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// 配额上限（v2.0 license 模型，替换原 v1.2 的 features 细粒度开关）
///
/// 各字段语义：
/// - `None` = 无限配额（serialized 为 JSON `null`）
/// - `Some(n)` = 上限为 n
///
/// 字段缺失 = 无限，向后兼容老 payload：旧 license 没 quota 字段时，
/// `LicensePayload.quota` 解析为 `None`，网关按"该配额无上限"处理。
///
/// 序列化说明：`Option<T>` 字段保留 None（输出 JSON null），不省略——前端类型
/// 与后端字段一一对应，且 `null` 是 ts-rs 生成的 TS 类型 `T | null` 的合法值。
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[ts(export, export_to = "Quota.ts")]
pub struct Quota {
    /// 最大点位总数（None = 无限）
    pub max_points: Option<u32>,
    /// 最大并发协议数（None = 无限）
    pub max_protocols: Option<u32>,
    /// 历史数据保留天数（None = 无限；超限按 FIFO 清理）
    pub storage_days: Option<u32>,
    /// 北向上行并发通道数（None = 无限）
    pub northbound_channels: Option<u32>,
}

impl Quota {
    /// 所有 4 个字段都是 None（license 自带 quota 全空时的判定）
    /// LicenseManager 用此判断"license 没配 quota"→ 走配置兜底
    pub fn is_all_none(&self) -> bool {
        self.max_points.is_none()
            && self.max_protocols.is_none()
            && self.storage_days.is_none()
            && self.northbound_channels.is_none()
    }
}

/// 配额类型（LicenseManager::check_quota 第一个参数）
///
/// 与 Quota 字段一一对应；增加字段时同步更新 match 分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaKind {
    Points,
    Protocols,
    StorageDays,
    NorthboundChannels,
}

impl QuotaKind {
    /// 当前 kind 在 Quota 中的字段值
    pub fn value_in(&self, q: &Quota) -> Option<u32> {
        match self {
            QuotaKind::Points => q.max_points,
            QuotaKind::Protocols => q.max_protocols,
            QuotaKind::StorageDays => q.storage_days,
            QuotaKind::NorthboundChannels => q.northbound_channels,
        }
    }

    /// 客户可读的中文标签（用于 4021 错误 message）
    pub fn label_zh(&self) -> &'static str {
        match self {
            QuotaKind::Points => "采集点数",
            QuotaKind::Protocols => "并发协议数",
            QuotaKind::StorageDays => "数据保留天数",
            QuotaKind::NorthboundChannels => "北向通道数",
        }
    }
}

/// payload 字段（§4 字段表）。签发工具与网关共用本结构定义，保证字段一致。
///
/// v2.0：移除 v1.2 的 `features: Vec<String>`（细粒度功能开关已废弃：调试/分析
/// 功能全部免费，无需按 feature 授权）。业务校验由 `quota` 字段（v2.0 配额）
/// + `tier_quotas` 配置（运维可调档位默认配额）共同决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicensePayload {
    /// 格式版本，当前 1，版本不识别即拒绝
    pub v: u32,
    /// 唯一编号（如 LIC-2026-0001），用于审计与吊销列表
    pub license_id: String,
    /// 客户标识（签发时写入，便于排查）
    pub customer: String,
    /// 档位（v2.0 仅作展示用，业务校验改由 quota 决定）
    pub tier: Tier,
    /// v2.0 配额上限：老 license（v1.2）该字段缺失 → None = 各配额无上限
    /// 实际生效需配合 tier_quotas 配置；详见 design license-v2-quota §3
    #[serde(default)]
    pub quota: Option<Quota>,
    /// 主键指纹哈希：machine-id / MachineGuid 的 sha256 hex
    pub machine_key: String,
    /// 整体指纹哈希（审计用，v1 比对逻辑不使用，见 §6 决策记录）
    pub machine: String,
    /// 回退项单项哈希：[板载 MAC, 存储序列号, 设备序列号]，缺失项为空串
    pub machine_parts: Vec<String>,
    /// 签发时间（RFC3339）
    pub issued_at: String,
    /// 过期时间（RFC3339），null = 永久授权
    pub expires_at: Option<String>,
}

/// 签发入口（签发工具与单测共用，防止两侧格式漂移）：
/// 对第一段 base64 字节的原文做 Ed25519 签名，输出完整 `.lic` 文件内容
pub fn sign_license(payload_json: &str, keypair: &ring::signature::Ed25519KeyPair) -> String {
    let seg0 = b64_encode(payload_json.as_bytes());
    let sig = keypair.sign(seg0.as_bytes());
    format!("{}.{}", seg0, b64_encode(sig.as_ref()))
}

/// 拆分两段式：返回 (第一段原文, payload 字节, 签名字节)
fn split_two_part(text: &str) -> Result<(&str, Vec<u8>, Vec<u8>), LicenseError> {
    let t = text.trim();
    let (seg0, seg1) = t.split_once('.').ok_or(LicenseError::Decode)?;
    if seg0.is_empty() || seg1.is_empty() || seg1.contains('.') {
        return Err(LicenseError::Decode);
    }
    let payload = b64_decode(seg0).ok_or(LicenseError::Decode)?;
    let sig = b64_decode(seg1).ok_or(LicenseError::Decode)?;
    if payload.is_empty() || sig.is_empty() {
        return Err(LicenseError::Decode);
    }
    // r34-LV1: 长度上限——恶意 license（GB 级 base64）触发 OOM 防护
    if payload.len() > MAX_LICENSE_PAYLOAD_BYTES {
        return Err(LicenseError::Decode);
    }
    if sig.len() > MAX_LICENSE_SIG_BYTES {
        return Err(LicenseError::Decode);
    }
    Ok((seg0, payload, sig))
}

/// 完整验签：拆分 → 逐公钥尝试验签（按顺序返回第一个匹配）→ JSON 解析 → 字段校验
///
/// 换钥过渡期支持：生产入口可传入多把公钥（旧钥 + 新钥），逐把尝试 Ed25519 verify。
/// 任一通过即接受；全部失败 → `BadSignature`。v 字段约束在验签通过后由
/// `validate_payload` 进一步检查（不在此函数提前读 payload，避免把未验签数据
/// 当作可信任源）。
pub fn verify_file_text(
    text: &str,
    public_keys: &[[u8; 32]],
) -> Result<LicensePayload, LicenseError> {
    let (seg0, payload_bytes, sig) = split_two_part(text)?;
    if public_keys.is_empty() {
        // 无公钥 = 配置错误；不静默接受任何 license
        return Err(LicenseError::BadSignature);
    }
    // r34-LV2: 公钥数量上限——恶意配置（上千把公钥）触发 CPU 尖峰（Ed25519 verify
    // 逐把执行）。换钥过渡期典型 2-3 把，超出视为配置异常。
    if public_keys.len() > MAX_LICENSE_PUBLIC_KEYS {
        return Err(LicenseError::BadSignature);
    }
    for key in public_keys {
        let pk = signature::UnparsedPublicKey::new(&signature::ED25519, key.as_ref());
        if pk.verify(seg0.as_bytes(), &sig).is_ok() {
            let payload: LicensePayload = serde_json::from_slice(&payload_bytes)
                .map_err(|e| LicenseError::BadPayload(e.to_string()))?;
            validate_payload(&payload)?;
            return Ok(payload);
        }
    }
    Err(LicenseError::BadSignature)
}

/// B3 修复：验签 + 指纹比对 统一入口
///
/// 之前 `verify_file_text` 只验签，调用方需要再单独调 `fingerprint::compare`。
/// 这导致两类风险：
/// 1. 未来新加调用方忘了调 fingerprint 比对，license 落到任意设备上都被接受；
/// 2. 现有 `LicenseManager::evaluate_at` 已正确调用，但 `dry_parse` 之外的
///    未来 API（如 license 预览 / 调试面板）易漏调。
///
/// 本入口把"验签 + 字段校验 + 指纹比对"打包为单一 API：
/// - 验签失败 → `BadSignature` / `BadVersion` / `BadPayload`（与原 verify 一致）
/// - 验签通过但指纹不符 → `Fingerprint`
/// - 全部通过 → 返回 payload
///
/// 行为不变：原 `verify_file_text` 仍保留以供单测 / 内部使用；外部 API
///（LicenseManager 公共方法、未来的预览端点）必须走本入口。
pub fn verify_with_fingerprint(
    text: &str,
    public_keys: &[[u8; 32]],
    local_fp: &LocalFingerprints,
) -> Result<LicensePayload, LicenseError> {
    let payload = verify_file_text(text, public_keys)?;
    if !compare_fingerprint(local_fp, &payload.machine_key, &payload.machine_parts) {
        return Err(LicenseError::Fingerprint);
    }
    Ok(payload)
}

/// 单公钥便捷入口 + 指纹
pub fn verify_with_fingerprint_single(
    text: &str,
    public_key: &[u8; 32],
    local_fp: &LocalFingerprints,
) -> Result<LicensePayload, LicenseError> {
    verify_with_fingerprint(text, std::slice::from_ref(public_key), local_fp)
}

/// 字段校验（§4）：v、必填非空、machine_parts 恰 3 项、时间格式
fn validate_payload(p: &LicensePayload) -> Result<(), LicenseError> {
    if p.v != 1 {
        return Err(LicenseError::BadVersion(p.v));
    }
    if p.license_id.trim().is_empty() || p.customer.trim().is_empty() {
        return Err(LicenseError::BadPayload(
            "license_id / customer must not be empty".into(),
        ));
    }
    if p.machine_parts.len() != 3 {
        return Err(LicenseError::BadPayload(
            "machine_parts must be exactly 3 entries (missing item = empty string)".into(),
        ));
    }
    if parse_rfc3339(&p.issued_at).is_none() {
        return Err(LicenseError::BadPayload("issued_at is not RFC3339".into()));
    }
    if let Some(e) = &p.expires_at {
        if parse_rfc3339(e).is_none() {
            return Err(LicenseError::BadPayload("expires_at is not RFC3339".into()));
        }
    }
    Ok(())
}

/// 有效期判定结论
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryVerdict {
    /// 未到期 / 永久
    Ok,
    /// 临期（剩余 ≤ 30 天），功能不受限
    Expiring,
    /// 已过期 → 降级 core
    Expired,
}

/// 有效期判定（§7 步骤 4）
pub fn check_expiry(p: &LicensePayload, now: i64) -> ExpiryVerdict {
    match p.expires_at.as_deref().and_then(parse_rfc3339) {
        None => ExpiryVerdict::Ok, // 永久授权
        Some(exp) if now > exp => ExpiryVerdict::Expired,
        Some(exp) if exp - now <= EXPIRING_WINDOW_SECS => ExpiryVerdict::Expiring,
        Some(_) => ExpiryVerdict::Ok,
    }
}

#[cfg(test)]
mod tests;
