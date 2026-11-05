//! License 校验模块（§7）
//!
//! 纯离线 Ed25519 验签 + 机器指纹分层比对 + 时钟回拨防护。
//! 失效软降级：`Expired / Mismatch / Tampered / Invalid` 四种失效态对外行为
//! 完全一致（tier=core + 采集不停），区分只体现在 status 上便于排查。
//!
//! 无 license 文件不是错误：`present=false, tier=core`，一切照常（开源版可用）。

pub mod clock;
pub mod fingerprint;
pub mod verify;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use clock::{ClockGuard, ClockVerdict};
pub use fingerprint::LocalFingerprints;
pub use verify::{LicenseError, LicensePayload, Quota, QuotaAvailable, QuotaKind, QuotaUsage};

/// Ed25519 公钥（32 字节，编译期硬编码，随二进制分发，见 §5）。
///
/// 换钥过渡期支持（多公钥表）：`PUBLIC_KEYS` 维护 1~N 把公钥，
/// 按数组顺序逐把尝试 Ed25519 verify，任一通过即接受。
///
/// 更换流程：`license-issuer keygen` 生成新钥对 → 把新 pubkey hex
/// 填到 `PUBLIC_KEYS` 数组 → 随版本发布（v 字段自然区分新旧 payload，
/// 老 license 仍能验签通过直到客户拿到新版本）。
pub const PUBLIC_KEY_OLD: [u8; 32] = [
    0xa2, 0xc0, 0xb0, 0x2b, 0x29, 0x12, 0xe1, 0xae, 0xe6, 0xa9, 0x36, 0x31, 0x4d, 0x9b, 0x2f, 0xe3,
    0xfb, 0x12, 0xf6, 0x86, 0x47, 0x3f, 0xae, 0x49, 0xdb, 0xd8, 0x55, 0xd3, 0x3c, 0x50, 0xbd, 0xc0,
];

/// 多公钥表（按数组顺序逐把尝试，首个匹配的钥即生效）
pub const PUBLIC_KEYS: &[[u8; 32]] = &[PUBLIC_KEY_OLD];

/// B3 修复：换钥过渡期通过环境变量注入额外公钥（hex，多把以 `,` `;` 或空白分隔）。
/// 与编译期 `PUBLIC_KEY_OLD` 一起进公钥表，无需发版即可同时接受新旧 license。
pub const LICENSE_PUBKEY_HEX_ENV: &str = "GATEWAY_LICENSE_PUBKEY_HEX";

/// 解析单把 64-hex 字符的 Ed25519 公钥；接受大小写 hex、忽略前导 `0x`。
pub fn parse_pubkey_hex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 {
        return None;
    }
    let bytes_in = s.as_bytes();
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = hex_digit(bytes_in[i * 2])?;
        let lo = hex_digit(bytes_in[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 从 `GATEWAY_LICENSE_PUBKEY_HEX` 解析多把额外公钥；无效项记 warn 跳过，
/// 不使 LicenseManager 启动失败（兜底是编译期 `PUBLIC_KEY_OLD`）。缺失返回空。
pub fn extra_pubkeys_from_env() -> Vec<[u8; 32]> {
    // Bug T8: 环境变量注入额外公钥等价于「运行时可信根可替换」——release 部署中
    // 任何能设置进程环境变量的本机使用者都能塞入自己的公钥并自签 license，从而
    // 完全绕过授权校验。因此仅在 debug 构建（开发 / CI 过渡期换钥）启用；release
    // 二进制明确忽略该变量并告警，可信根固定为编译期 `PUBLIC_KEY_OLD`。
    #[cfg(not(debug_assertions))]
    {
        if std::env::var(LICENSE_PUBKEY_HEX_ENV).is_ok() {
            tracing::warn!(
                "{} is set but IGNORED in release builds (runtime pubkey injection \
                 would let self-signed licenses bypass authorization)",
                LICENSE_PUBKEY_HEX_ENV
            );
        }
        return Vec::new();
    }
    #[cfg(debug_assertions)]
    {
        let Ok(raw) = std::env::var(LICENSE_PUBKEY_HEX_ENV) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for piece in raw.split(|c: char| c == ',' || c == ';' || c.is_ascii_whitespace()) {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            match parse_pubkey_hex(piece) {
                Some(k) => out.push(k),
                None => tracing::warn!(
                    "{}: skip invalid pubkey (need 64 hex chars): {}...",
                    LICENSE_PUBKEY_HEX_ENV,
                    &piece[..piece.len().min(16)]
                ),
            }
        }
        out
    }
}

/// 兼容旧调用：`PUBLIC_KEY` 作为第一把公钥的别名
#[deprecated(since = "0.2.0", note = "use PUBLIC_KEYS instead")]
pub const PUBLIC_KEY: [u8; 32] = PUBLIC_KEY_OLD;

/// 档位（与前端 `type Tier` 一致，小写序列化）
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    TS,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Tier {
    #[default]
    Core,
    Standard,
    Pro,
    Ultra,
}

/// 授权状态（§7）。失效态对前端的表现由 status 驱动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum LicenseStatus {
    /// 正常（剩余 > 30 天）
    Ok,
    /// 临期（剩余 ≤ 30 天），功能不受限
    Expiring,
    /// 过期 → 降级 core，采集不停
    Expired,
    /// 指纹不符 → 同 Expired 处理
    Mismatch,
    /// 时钟回拨 → 同 Expired 处理
    Tampered,
    /// 验签失败 / 格式错误 → 同 Expired 处理
    Invalid,
}

/// 生效配额的来源（v2.0）
///
/// 目的：把「付费档位被静默降级」变成可观测事实。配置里没写满四档时，
/// license 自带 quota 缺失的付费用户会落到 absent 兜底（100/1/7/1），
/// 现场从配额数字看不出异常；本字段让 API / 启动日志能直接指出降级原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum QuotaSource {
    /// license 文件自带 quota（最高优先级）
    License,
    /// 配置 license.tier_quotas 查表命中
    TierConfig,
    /// 无 license / 授权失效 → absent_quota 兜底（预期行为，不是配置问题）
    AbsentFallback,
    /// license 有效但 license.tier_quotas 未写该 tier → absent_quota 兜底（配置缺失，需告警）
    TierMissing,
}

/// 运行时授权状态快照（ts-rs 导出，前端类型自动同步）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LicenseState {
    /// 是否存在通过校验的 license（失效态为 false，但审计字段保留）
    pub present: bool,
    /// 当前生效档位（无 license / 失效时 = core）
    pub tier: Tier,
    /// v2.0 有效配额（license 自带 quota 优先，否则按 tier 查 tier_quotas，缺省走 absent_quota）
    /// LicenseManager 在 current() 时实时计算填入
    pub quota: Quota,
    /// 生效配额来源（LicenseManager 在 current() 时实时计算填入）
    pub quota_source: QuotaSource,
    pub status: LicenseStatus,
    /// 过期时间（unix 秒），None = 永久 / 无 license
    pub expires_at: Option<i64>,
    pub license_id: Option<String>,
    pub customer: Option<String>,
}

impl LicenseState {
    /// 无 license（开源版）默认态
    pub fn absent() -> Self {
        Self {
            present: false,
            tier: Tier::Core,
            quota: Quota::default(),
            quota_source: QuotaSource::AbsentFallback,
            status: LicenseStatus::Ok,
            expires_at: None,
            license_id: None,
            customer: None,
        }
    }

    /// 失效降级态：tier=core + quota 走 absent 兜底，但保留审计字段便于前端提示与排查
    fn degraded(status: LicenseStatus, payload: Option<&LicensePayload>) -> Self {
        Self {
            present: false,
            tier: Tier::Core,
            quota: Quota::default(),
            quota_source: QuotaSource::AbsentFallback,
            status,
            expires_at: payload
                .as_ref()
                .and_then(|p| p.expires_at.as_deref())
                .and_then(verify::parse_rfc3339),
            license_id: payload.map(|p| p.license_id.clone()),
            customer: payload.map(|p| p.customer.clone()),
        }
    }

    /// 有效态
    fn active(payload: &LicensePayload, status: LicenseStatus) -> Self {
        let quota = payload.quota.clone().unwrap_or_default();
        // payload 未带 quota（或全 null）时只是占位，真正的来源由 current() 实时判定
        let quota_source = if quota.is_all_none() {
            QuotaSource::AbsentFallback
        } else {
            QuotaSource::License
        };
        Self {
            present: true,
            tier: payload.tier,
            quota,
            quota_source,
            status,
            expires_at: payload
                .expires_at
                .as_deref()
                .and_then(verify::parse_rfc3339),
            license_id: Some(payload.license_id.clone()),
            customer: Some(payload.customer.clone()),
        }
    }

    /// 降级语义一致性（§7）：四种失效态对外的行为字段完全一致
    pub fn is_degraded(&self) -> bool {
        matches!(
            self.status,
            LicenseStatus::Expired
                | LicenseStatus::Mismatch
                | LicenseStatus::Tampered
                | LicenseStatus::Invalid
        )
    }

    /// API 层序列化视图（Arc<LicenseState> 不便 JSON 序列化，故提供 to_json_value）
    /// v2.0：输出 quota 字段（前端授权管理对话框展示配额信息）
    /// v2.0：输出 quota_source 字段（前端据此提示"配置缺该档位，配额已降级"）
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "present": self.present,
            "tier": self.tier,
            "quota": &self.quota,
            "quota_source": self.quota_source,
            "status": self.status,
            "expires_at": self.expires_at,
            "license_id": self.license_id,
            "customer": self.customer,
        })
    }
}

/// API 快照视图：授权状态 + 本机设备码（sha256 指纹哈希，无隐私风险）+ 配额用量。
/// GET /api/license 返回此视图；客户在授权管理界面复制设备码 → 供应商凭码签发。
///
/// `usage` / `available` 依赖运行时配置与队列数据（不属于 license 本身状态），
/// 故放在快照层而非 `LicenseState`，避免授权状态被运行态数据污染。
#[derive(Debug, TS)]
#[ts(export, export_to = "LicenseSnapshot.ts")]
pub struct LicenseSnapshot {
    #[ts(flatten)]
    pub state: LicenseState,
    pub machine: LocalFingerprints,
    /// 配额已用量（当前生效配置 / 数据的实际占用）
    pub usage: QuotaUsage,
    /// 配额剩余量（None = 该项无上限）
    pub available: QuotaAvailable,
}

impl LicenseSnapshot {
    /// 手写 JSON（沿用 to_json_value 模式，字段与 TS 导出严格一致）
    pub fn to_json_value(
        state: &LicenseState,
        machine: &LocalFingerprints,
        usage: &QuotaUsage,
        available: &QuotaAvailable,
    ) -> serde_json::Value {
        let mut v = state.to_json_value();
        v["machine"] = serde_json::json!({
            "machine_key": machine.machine_key,
            "onboard_mac": machine.onboard_mac,
            "storage_serial": machine.storage_serial,
            "device_serial": machine.device_serial,
        });
        v["usage"] = serde_json::to_value(usage).unwrap_or_else(|_| serde_json::json!({}));
        v["available"] = serde_json::to_value(available).unwrap_or_else(|_| serde_json::json!({}));
        v
    }
}

/// 测试签发辅助（doc-hidden，仅测试目标使用）：
/// 测试进程内生成一次性随机 Ed25519 钥对，`public_key()` 与 `sign_payload_json`
/// 在同一进程内自洽——集成测试用它构造 manager 并签发合法 license。
///
/// W2-#18：本模块内的 `unwrap` 是 license 路径上唯一被 clippy 豁免、
/// 且会被编进生产构建的 panic 点。改为 `test-support` feature gate 后，
/// 发行二进制（gatewayd / gateway-desktop）不再包含该代码；单元测试走 `cfg(test)`，
/// 集成测试由 gateway-api 的 dev-dependencies 自动启用 feature。
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod issuer_support {
    // 本模块为测试签发辅助：集成测试需从 crate 外部访问，故由 `test-support`
    // feature 开启（单元测试走 cfg(test)）。unwrap 集中在一次性随机钥对生成 /
    // PKCS8 解析 / 定长公钥转换，在测试进程内不可能失败（失败即测试环境异常），
    // 对这些点定向豁免；发行二进制不编译本模块，因此不进入生产 panic 面。
    #![allow(clippy::unwrap_used)]

    use super::verify;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::sync::LazyLock;

    pub(crate) static KEY_PKCS8: LazyLock<Vec<u8>> = LazyLock::new(|| {
        Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .unwrap()
            .as_ref()
            .to_vec()
    });

    fn keypair() -> Ed25519KeyPair {
        Ed25519KeyPair::from_pkcs8(&KEY_PKCS8).unwrap()
    }

    /// 测试钥对公钥（配合 `LicenseManager::with_public_key` 使用）
    pub fn public_key() -> [u8; 32] {
        keypair().public_key().as_ref().try_into().unwrap()
    }

    /// 用测试钥对签发 .lic 文件内容（sign_license 共用入口，防格式漂移）
    pub fn sign_payload_json(payload_json: &str) -> String {
        verify::sign_license(payload_json, &keypair())
    }
}

/// License 管理器：无锁读状态 + 每日重校验 + 时钟回拨防护
#[derive(Debug)]
pub struct LicenseManager {
    state: ArcSwap<LicenseState>,
    clock: ClockGuard,
    public_keys: Arc<[[u8; 32]]>,
    /// 已加载的 license 文件路径（refresh_loop 重读用）
    path: Mutex<Option<PathBuf>>,
    /// 本机指纹快照（构造时采集一次；指纹为 sha256 hex 形态，可安全对外展示）
    machine: LocalFingerprints,
    /// v2.0 tier_quotas 配置（bootstrap 初始化后注入；None 时 LicenseState.quota 走 license 自带值或 default）
    quota_config: Mutex<Option<crate::config::LicenseConfig>>,
}

impl LicenseManager {
    /// 测试辅助：直接覆写内部状态（集成测试用，不参与生产路径）。
    /// doc-hidden：生产路径不走它；集成测试 crate 通过 `gateway_core::license::LicenseManager::replace_state_for_test` 访问。
    #[doc(hidden)]
    pub fn replace_state_for_test(&self, new_state: LicenseState) {
        self.state.store(Arc::new(new_state));
    }

    /// v2.0 注入 tier_quotas 配置；调用后 next current() 会按配置刷新 quota 字段
    pub fn set_quota_config(&self, cfg: crate::config::LicenseConfig) {
        *self.quota_config.lock() = Some(cfg);
    }

    /// 生产入口：编译期公钥 + `GATEWAY_LICENSE_PUBKEY_HEX` 注入的额外公钥。
    ///
    /// B3 修复：换钥过渡期无需发版即可同时接受新旧 license。额外公钥排在
    /// 编译期公钥之后；未注入时行为与之前完全一致（仅 1 把编译期公钥）。
    pub fn new(clock_db: &Path) -> Self {
        let extra = extra_pubkeys_from_env();
        let mut all = Vec::with_capacity(1 + extra.len());
        all.push(PUBLIC_KEY_OLD);
        all.extend(extra);
        tracing::info!(
            "license: {} pubkey(s) loaded (1 compiled-in + {} runtime from {})",
            all.len(),
            all.len() - 1,
            LICENSE_PUBKEY_HEX_ENV
        );
        Self::with_public_keys(clock_db, Arc::from(all.into_boxed_slice()))
    }

    /// 单公钥便捷入口（保留旧 API 兼容；行为等价于 with_public_keys(..., &[key])）
    pub fn with_public_key(clock_db: &Path, public_key: [u8; 32]) -> Self {
        Self::with_public_keys(clock_db, Arc::new([public_key]))
    }

    /// 多公钥表入口（换钥过渡期：传入 N 把公钥，逐把尝试 verify）
    pub fn with_public_keys(clock_db: &Path, public_keys: Arc<[[u8; 32]]>) -> Self {
        Self {
            state: ArcSwap::from_pointee(LicenseState::absent()),
            clock: ClockGuard::new(clock_db),
            public_keys,
            path: Mutex::new(None),
            machine: fingerprint::collect(),
            quota_config: Mutex::new(None),
        }
    }

    /// 启动时加载（§7 校验顺序）。
    ///
    /// 返回值语义：`Ok` = 无 license 文件（一切照常）或校验通过；
    /// `Err` = 文件存在但被拒（状态已写入，`current()` 可读），由调用方记 warn。
    pub fn load(&self, path: &Path) -> Result<(), LicenseError> {
        *self.path.lock() = Some(path.to_path_buf());
        let fp = fingerprint::collect();
        let (state, err) = self.evaluate_at(path, &fp, chrono::Utc::now().timestamp());
        self.state.store(Arc::new(state));
        err.map_or(Ok(()), Err)
    }

    /// 单次完整校验（load / refresh 共用，§7 校验顺序 1-6）。
    /// 返回 (状态快照, 失效原因)。`now` 参数化便于单测注入。
    pub(crate) fn evaluate_at(
        &self,
        path: &Path,
        fp: &LocalFingerprints,
        now: i64,
    ) -> (LicenseState, Option<LicenseError>) {
        // 1. 读文件 + 验签 + 字段校验
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (LicenseState::absent(), None); // 无 license 不是错误
            }
            Err(e) => {
                tracing::warn!("license file unreadable: {}", e);
                return (LicenseState::degraded(LicenseStatus::Invalid, None), None);
            }
        };
        // 2. 验签 + 字段 + 指纹：走统一入口，避免日后新路径漏调指纹比对
        let payload = match verify::verify_with_fingerprint(&text, &self.public_keys, fp) {
            Ok(p) => p,
            Err(LicenseError::Fingerprint) => {
                // 验签通过、字段合法，但指纹不符：重新解析以保留 audit 字段（license_id/customer）
                let audit = verify::verify_file_text(&text, &self.public_keys).ok();
                tracing::warn!(
                    "license fingerprint mismatch: {}",
                    audit.as_ref().map(|p| p.license_id.as_str()).unwrap_or("?")
                );
                return (
                    LicenseState::degraded(LicenseStatus::Mismatch, audit.as_ref()),
                    Some(LicenseError::Fingerprint),
                );
            }
            Err(e) => {
                tracing::warn!("license verify failed: {}", e);
                return (
                    LicenseState::degraded(LicenseStatus::Invalid, None),
                    Some(e),
                );
            }
        };
        // 4. 有效期
        let expiry = verify::check_expiry(&payload, now);
        // Bug C9: clock store failure is fail-closed (was fail-open) -> degrade license to Tampered
        let clock_ok = match self.clock.check_and_update(now, Some(path)) {
            Ok(ClockVerdict::Tampered) => {
                tracing::warn!("license clock rollback detected: {}", payload.license_id);
                false
            }
            Ok(_) => true,
            Err(e) => {
                tracing::error!("license clock store unavailable, fail-closed: {}", e);
                false
            }
        };
        // 6. 终态（先到期的失效先报：Expired 优先于 Tampered）
        match (expiry, clock_ok) {
            (verify::ExpiryVerdict::Expired, _) => (
                LicenseState::degraded(LicenseStatus::Expired, Some(&payload)),
                Some(LicenseError::Expired),
            ),
            (_, false) => (
                LicenseState::degraded(LicenseStatus::Tampered, Some(&payload)),
                Some(LicenseError::ClockTampered),
            ),
            (verify::ExpiryVerdict::Expiring, true) => (
                LicenseState::active(&payload, LicenseStatus::Expiring),
                None,
            ),
            (verify::ExpiryVerdict::Ok, true) => {
                (LicenseState::active(&payload, LicenseStatus::Ok), None)
            }
        }
    }

    /// 每日定时重校验（重读文件 + 重算有效期），由后台任务驱动
    pub async fn refresh_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
            if let Some(path) = self.path.lock().clone() {
                if let Err(e) = self.load(&path) {
                    tracing::warn!("license daily refresh rejected: {}", e);
                }
            }
        }
    }

    /// API / 中间件读取入口
    ///
    /// v2.0 增量：返回前按 quota_config 优先级刷新 quota 字段：
    /// - license 自带 quota（LicenseState.quota 已被 evaluate_at 填入）→ 优先使用
    /// - 注入的 tier_quotas 配置（tier 查表）→ 兜底
    /// - 配置 absent_quota → 最后兜底
    ///
    /// 当前实现为"读时计算"（无锁），适用于 API 调用频次 ≤ 中间件调用频次场景；
    /// 若未来发现配额查询成为热点，再迁移到 evaluate_at 内的同步计算。
    pub fn current(&self) -> Arc<LicenseState> {
        let state = self.state.load_full();
        let cfg = self.quota_config.lock();
        let mut updated = (*state).clone();

        // ① 无 license / 授权失效降级（present=false）→ 一律 absent 兜底，
        //    **不查 tier_quotas**。docs §6/§9：失效时配额回退到「无 license 默认配额」
        //    （100/1/7/1），而不是 core 档配额（500/3/30/1）。
        if !state.present {
            updated.quota = cfg
                .as_ref()
                .map(|c| c.effective_absent_quota())
                .unwrap_or_else(|| {
                    crate::config::LicenseConfig::default().effective_absent_quota()
                });
            updated.quota_source = QuotaSource::AbsentFallback;
            return Arc::new(updated);
        }

        // ② license 自带 quota 非空 → 原样保留（license 自带优先，配置不覆盖）
        if !state.quota.is_all_none() {
            updated.quota_source = QuotaSource::License;
            return Arc::new(updated);
        }

        // ③ license 有效但未自带 quota（B3：签发遗漏 / 老 payload）→ 按 tier 查配置。
        //    查不到该 tier → 降级到 absent 兜底，并标记 TierMissing（现场可观测，
        //    避免付费用户被静默降到开源配额而无人察觉）。
        match cfg.as_ref() {
            Some(cfg) if cfg.tier_configured(state.tier) => {
                updated.quota = cfg.quota_for_tier(state.tier);
                updated.quota_source = QuotaSource::TierConfig;
            }
            Some(cfg) => {
                updated.quota = cfg.effective_absent_quota();
                updated.quota_source = QuotaSource::TierMissing;
            }
            None => {
                // Bug T7: 原实现在 config.json 未配 license 节、且状态非 present 时直接
                // 原样返回——而 absent（无 license）与 Expired/Mismatch/Tampered/Invalid
                // 降级态的 quota 都是 `Quota::default()`（全 None = 无限），导致授权
                // 过期后配额被静默解除为无限。统一走「license 自带 > tier 查表 > absent
                // 兜底」语义：两类状态都回退到 effective_absent_quota（100/1/7/1）。
                updated.quota = crate::config::LicenseConfig::default().effective_absent_quota();
                updated.quota_source = QuotaSource::TierMissing;
            }
        }
        Arc::new(updated)
    }

    /// 本机设备码（指纹快照，构造时采集）。API 展示与测试签发共用。
    pub fn machine(&self) -> &LocalFingerprints {
        &self.machine
    }

    /// 导入前 dry-run：验签 + 指纹比对（不落盘、不改状态、不查有效期）。
    /// 供 POST /api/license/import 在写入磁盘前做确定性校验，
    /// 避免把无效文件写进授权路径导致每日重校验反复拒绝。
    pub fn dry_parse(&self, text: &str) -> Result<LicensePayload, LicenseError> {
        // B3：走统一入口（验签 + 指纹），避免漏调指纹比对
        verify::verify_with_fingerprint(text, &self.public_keys, &self.machine)
    }

    /// 导入：dry-run 通过后落盘到当前授权路径并全流程重载。
    /// 语义约定：签名/指纹不通过 → Err 且不落盘；有效期/时钟问题 → 正常落盘
    /// （文件本身是供应商签发的合法产物，导入后状态降级由 current() 呈现，
    /// 让客户看到"已导入但已过期/待续期"的准确反馈）。
    pub fn import(&self, text: &str) -> Result<(), LicenseError> {
        self.dry_parse(text)?;
        let path = self
            .path
            .lock()
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("license.lic"));
        std::fs::write(&path, text).map_err(|e| {
            tracing::warn!("license import write failed: {}", e);
            LicenseError::Io(e.to_string())
        })?;
        if let Err(e) = self.load(&path) {
            tracing::info!("license imported (degraded state): {}", e);
        }
        Ok(())
    }

    /// NTP 校时成功后重置时钟基准并立即重校验（断电恢复自然洗白，§9）
    pub fn apply_ntp_resync(&self, ntp_now: i64) {
        if let Err(e) = self.clock.resync(ntp_now) {
            tracing::warn!("license clock resync failed: {}", e);
            return;
        }
        if let Some(path) = self.path.lock().clone() {
            if let Err(e) = self.load(&path) {
                tracing::warn!("license reload after ntp resync rejected: {}", e);
            }
        }
    }

    /// v2.0 配额校验：检查新增后是否超限
    ///
    /// 语义：
    /// - `next_value` 为本次操作后的值（已含本次新增）
    /// - 配额字段为 `None` = 无限，永远通过
    /// - 配额字段为 `Some(上限)` 且 next_value > 上限 → 拒绝
    ///
    /// 调用场景：保存点位/协议/北向前计算"加完后总点数"传过来。
    /// 与 current() 共用同一套 quota 解析逻辑（license 自带 > config tier_quotas > absent）。
    pub fn check_quota(&self, kind: QuotaKind, next_value: u32) -> Result<(), QuotaError> {
        let quota = self.current().quota.clone();
        match kind.value_in(&quota) {
            None => Ok(()), // 无限配额
            Some(limit) if next_value <= limit => Ok(()),
            Some(limit) => Err(QuotaError {
                kind,
                limit,
                attempted: next_value,
            }),
        }
    }
}

/// v2.0 配额超限错误
///
/// 不归入 LicenseError（LicenseError 是文件级错误：签名/指纹/格式/IO/效期），
/// 配额是业务级错误（增量超限），由调用方返回 4021 + 中文 message。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaError {
    pub kind: QuotaKind,
    pub limit: u32,
    pub attempted: u32,
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "当前档位支持最多 {} {}，当前已 {}",
            self.limit,
            self.kind.label_zh(),
            self.attempted
        )
    }
}

impl std::error::Error for QuotaError {}

// ===== 单测支撑：代码内生成测试钥对（§5 私钥红线：真实私钥永不进仓库）=====
#[cfg(test)]
pub(crate) mod test_support {
    use once_cell::sync::Lazy;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::fingerprint::{compute_overall, hash_id_field, hash_mac_field};
    use super::verify::{sign_license, LicensePayload};
    use super::Tier;

    static KEY_PKCS8: Lazy<Vec<u8>> = Lazy::new(|| {
        ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .unwrap()
            .as_ref()
            .to_vec()
    });
    static OTHER_PKCS8: Lazy<Vec<u8>> = Lazy::new(|| {
        ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .unwrap()
            .as_ref()
            .to_vec()
    });

    fn keypair(bytes: &[u8]) -> Ed25519KeyPair {
        Ed25519KeyPair::from_pkcs8(bytes).unwrap()
    }

    pub fn public_key() -> [u8; 32] {
        keypair(&KEY_PKCS8)
            .public_key()
            .as_ref()
            .try_into()
            .unwrap()
    }

    /// 另一把无关钥对的公钥（验签应拒绝）
    pub fn other_public_key() -> [u8; 32] {
        keypair(&OTHER_PKCS8)
            .public_key()
            .as_ref()
            .try_into()
            .unwrap()
    }

    /// 标准测试 payload：主键 sha256("k1")，回退仅板载 MAC sha256("m1")
    pub fn test_payload() -> LicensePayload {
        let key = hash_id_field("k1").unwrap();
        let mac = hash_mac_field("aa:bb:cc:dd:ee:01").unwrap();
        let parts = [mac.clone(), String::new(), String::new()];
        let part_refs = [mac.as_str(), "", ""];
        LicensePayload {
            v: 1,
            license_id: "LIC-TEST-0001".into(),
            customer: "某某自动化".into(),
            tier: Tier::Pro,
            quota: None, // 无 quota 字段的老 payload 兼容测试
            machine_key: key.clone(),
            machine: compute_overall(&key, &part_refs),
            machine_parts: parts.to_vec(),
            issued_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        }
    }

    /// 用测试钥对签发（与生产签发工具同一实现，防两侧漂移）
    pub fn sign_payload(p: &LicensePayload) -> String {
        let json = serde_json::to_string(p).unwrap();
        sign_license(&json, &keypair(&KEY_PKCS8))
    }

    /// 篡改 payload 段一个字符（保持合法 base64），模拟"改字段不重签"
    pub fn flip_payload_char(text: &str) -> String {
        let (seg0, seg1) = text.split_once('.').unwrap();
        let mut chars: Vec<char> = seg0.chars().collect();
        chars[4] = if chars[4] == 'A' { 'B' } else { 'A' };
        format!("{}.{}", chars.into_iter().collect::<String>(), seg1)
    }

    /// 用错误钥对签发（验签应拒绝）
    pub fn sign_with_other_key(p: &LicensePayload) -> String {
        let json = serde_json::to_string(p).unwrap();
        sign_license(&json, &keypair(&OTHER_PKCS8))
    }

    /// 与 test_payload 匹配的本机指纹
    pub fn matching_fp() -> super::LocalFingerprints {
        super::LocalFingerprints {
            machine_key: hash_id_field("k1"),
            onboard_mac: hash_mac_field("aa:bb:cc:dd:ee:01"),
            ..Default::default()
        }
    }

    /// 与 test_payload 不匹配的本机指纹
    pub fn mismatching_fp() -> super::LocalFingerprints {
        super::LocalFingerprints {
            machine_key: hash_id_field("other"),
            onboard_mac: hash_mac_field("11:22:33:44:55:66"),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;
