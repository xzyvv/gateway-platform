//! OTA 远程升级（F4）
//!
//! 工业网关固件远程升级，支持：
//! - 固件完整性校验（CRC32）
//! - A/B 双分区写入（写入非活动分区，不影响当前运行）
//! - 升级状态实时跟踪（上传中/校验中/写入中/待重启/失败）
//! - 升级失败自动回滚（由 bootloader 根据 boot_count 决定）
//!
//! 固件包格式：
//!   [magic:4][version:u32][crc32:u32][payload...]
//!   magic = 0x47 0x57 0x4F 0x54 ("GWOT")
//!
//! 差分升级包格式（bsdiff 4.x 兼容补丁）：
//!   [magic:4 "GWDT"][new_version:u32][new_crc32:u32][delta_payload...]
//!   可选签名后缀 [SIG_MAGIC:4][hmac_sha256:32]，签名覆盖 delta_payload。

mod apply;
mod bootenv;
mod crypto;
mod download;

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use self::bootenv::BootEnv;
use crate::engine::error::{GatewayError, GatewayResult};

/// OTA 状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtaState {
    Idle,
    Uploading,
    Verifying,
    Writing,
    /// 写入完成，等待重启
    PendingReboot,
    Failed,
}

/// OTA 状态信息
#[derive(Debug, Clone, Serialize)]
pub struct OtaStatusInfo {
    pub state: OtaState,
    pub progress: u8,
    pub error: Option<String>,
    pub current_version: u32,
    pub target_version: Option<u32>,
    pub active_slot: char,
    pub inactive_slot: char,
}

/// A/B 分区配置
#[derive(Debug, Clone)]
pub struct AbPartitionConfig {
    /// 活动分区设备（如 "/dev/mmcblk0p2"）
    pub slot_a: PathBuf,
    /// 备用分区设备
    pub slot_b: PathBuf,
    /// 当前活动分区
    pub active_slot: char,
}

/// OTA 活动分区设备的环境变量名。
///
/// 现场 OTA 介质不一定是 eMMC（SD 卡 / NVMe 都可能），分区节点名随之变化
/// （/dev/sda2、/dev/nvme0n1p2 …）。写死 eMMC 分区名会让 OTA 在非 eMMC
/// 介质上直接失败，因此分区路径可由环境变量覆盖。
///
/// **注意**：改了这里必须同步改 systemd unit 的 `DeviceAllow=`——
/// 白名单里没有的节点会在程序 open() 之前被 systemd 以 EPERM 拒绝。
/// `gateway-core/deploy/install.sh` 会同时改写这两处。
pub const OTA_SLOT_A_ENV: &str = "GATEWAY_OTA_SLOT_A";
/// OTA 备用分区设备的环境变量名（见 [`OTA_SLOT_A_ENV`]）。
pub const OTA_SLOT_B_ENV: &str = "GATEWAY_OTA_SLOT_B";

impl AbPartitionConfig {
    /// 解析 A/B 分区节点路径：环境变量优先，否则回退到默认值。
    ///
    /// 抽成纯函数是为了能直接单测 env 覆盖分支——直接对进程环境做
    /// `set_var` 在并行测试里会互相污染，且新版本 Rust 已不安全。
    /// 空字符串视为未设置（现场 shell 导出空变量不应让 OTA 指向空路径）。
    pub fn resolve_paths(
        slot_a: Option<&str>,
        slot_b: Option<&str>,
        default_a: &str,
        default_b: &str,
    ) -> Self {
        let pick = |env: Option<&str>, default: &str| -> PathBuf {
            match env {
                Some(v) if !v.trim().is_empty() => PathBuf::from(v),
                _ => PathBuf::from(default),
            }
        };
        Self {
            slot_a: pick(slot_a, default_a),
            slot_b: pick(slot_b, default_b),
            active_slot: 'a',
        }
    }
}

impl Default for AbPartitionConfig {
    fn default() -> Self {
        let slot_a = std::env::var(OTA_SLOT_A_ENV).ok();
        let slot_b = std::env::var(OTA_SLOT_B_ENV).ok();
        AbPartitionConfig::resolve_paths(
            slot_a.as_deref(),
            slot_b.as_deref(),
            "/dev/mmcblk0p2",
            "/dev/mmcblk0p3",
        )
    }
}

/// OTA 管理器
pub struct OtaManager {
    state: Mutex<OtaState>,
    progress: Mutex<u8>,
    error: Mutex<Option<String>>,
    current_version: Mutex<u32>,
    target_version: Mutex<Option<u32>>,
    partition: AbPartitionConfig,
    /// 固件暂存目录
    staging_dir: PathBuf,
    /// HMAC-SHA256 签名密钥（旧格式兼容；None=不接受旧 HMAC 包）
    signing_key: Option<Vec<u8>>,
    /// Ed25519 验签公钥（内置 + `web.ota_public_keys` 追加）；为空时拒绝一切固件
    public_keys: Vec<[u8; 32]>,
    /// 1.2: 注入的指标实例
    metrics: Arc<crate::engine::metrics::Metrics>,
    /// bootloader 环境访问后端：写 env / 读 env / 刷盘 / 重启。
    ///
    /// 生产构建走真实 u-boot 工具（仅 Linux 目标机），测试构建走纯内存实现——
    /// 后者保证单测既不触碰宿主机 bootloader 环境，也不会真的重启测试机
    /// （在 CI runner 上执行真实 `reboot` 会直接终止整个作业）。
    boot_env: BootEnv,
}

/// 固件包头布局：magic(4) + version(u32, 4) + crc32(u32, 4) = 12 字节，
/// 实际读写按字节偏移在 apply.rs 中完成。
const FIRMWARE_MAGIC: [u8; 4] = [0x47, 0x57, 0x4F, 0x54]; // "GWOT"
const HEADER_SIZE: usize = 12;
/// 固件/补丁包大小上限：防止恶意上传超大文件撑爆内存（DoS）。
/// 全量包与差分补丁包共用（64MB 为工业网关常见固件上限）。
const MAX_FIRMWARE_SIZE: usize = 64 * 1024 * 1024;
/// 差分升级包 magic: "GWDT"（前端/后端据此分流 process_delta / process_firmware）
const DELTA_MAGIC: [u8; 4] = [0x47, 0x57, 0x44, 0x54];
/// 差分升级包头布局：magic(4) + new_version(u32, 4) + new_crc32(u32, 4) = 12 字节
const DELTA_HEADER_SIZE: usize = 12;
/// 固件签名后缀 magic（Ed25519，DD-40 / ROI-43）: "FWSG"
const ED_SIG_MAGIC: [u8; 4] = [0x46, 0x57, 0x53, 0x47];
/// Ed25519 签名长度
const ED_SIG_LEN: usize = 64;
/// Ed25519 签名后缀总长度: magic(4) + sig(64) = 68
const ED_SIG_SUFFIX_SIZE: usize = 68;
/// 验签公钥数量上限：与 license 一致，防恶意配置把 CPU 打满。
pub const MAX_OTA_PUBLIC_KEYS: usize = 8;
/// 编译内置的 OTA 验签公钥（产品签名公钥到位后填入）。
///
/// 为空表示尚未内置：此时必须通过 `web.ota_public_keys` 提供公钥，否则所有固件
/// 升级都会被拒绝（fail-closed）。产品公钥属于签名侧信任根，不由代码自行编造。
pub const BUILTIN_OTA_PUBLIC_KEYS: &[[u8; 32]] = &[];
/// 固件签名后缀 magic（旧 HMAC，仅兼容）: "GSIG"
const SIG_MAGIC: [u8; 4] = [0x47, 0x53, 0x49, 0x47];
/// 旧 HMAC 签名后缀总长度: magic(4) + hmac(32) = 36
const SIG_SUFFIX_SIZE: usize = 36;
/// r31-OTA4: PendingReboot 状态持久化标记文件名（位于 staging_dir 内）。
/// 升级写入完成后创建，rollback/启动后检测到新固件后删除。
/// 解决：升级写到一半断电 → 重启后系统不知"已写好新固件待重启"，
/// 下次正常重启启动旧固件，丢失本次升级。
const PENDING_REBOOT_MARKER: &str = "pending_reboot";
/// 写入分区超时（秒）：防止块设备 IO 卡死导致 OTA 永久阻塞
const WRITE_TIMEOUT_SECS: u64 = 120;
/// 写入失败重试次数
const WRITE_MAX_RETRIES: u32 = 3;

/// Bug C8: 从配置字符串（base64 编码，见 `WebConfig::ota_signing_key`）解析 HMAC-SHA256 密钥。
///
/// 解码失败返回 Err（fail-closed），由调用方阻断启动——避免"配了签名密钥却因解码失败
/// 静默退化为不验签"的降级。未配置密钥时调用方直接传 `None`，保持 CRC32-only 向后兼容路径。
pub fn parse_signing_key(encoded: &str) -> GatewayResult<Vec<u8>> {
    crate::engine::codec::encoding::b64_decode(encoded)
        .ok_or_else(|| GatewayError::Config("web.ota_signing_key is not valid base64".into()))
}

/// 解析 `web.ota_public_keys`（64-hex，同 license 格式），并合并编译内置公钥。
///
/// 任一项非法即返回 Err（fail-closed）：配置了验签公钥却因格式错误静默退化，
/// 等于把「拒绝一切固件」误当成「没配公钥」；数量超上限同样拒绝。
pub fn parse_ota_public_keys(encoded: &[String]) -> GatewayResult<Vec<[u8; 32]>> {
    if encoded.len() > MAX_OTA_PUBLIC_KEYS {
        return Err(GatewayError::Config(format!(
            "web.ota_public_keys 最多 {MAX_OTA_PUBLIC_KEYS} 把，当前 {}",
            encoded.len()
        )));
    }
    let mut keys = BUILTIN_OTA_PUBLIC_KEYS.to_vec();
    for item in encoded {
        let key = crate::license::parse_pubkey_hex(item).ok_or_else(|| {
            GatewayError::Config(format!(
                "web.ota_public_keys 含非法公钥（需 64 位 hex）：{}",
                item
            ))
        })?;
        keys.push(key);
    }
    Ok(keys)
}

impl OtaManager {
    /// 创建 OTA 管理器
    pub fn new(
        partition: AbPartitionConfig,
        staging_dir: &str,
        signing_key: Option<Vec<u8>>,
        public_keys: Vec<[u8; 32]>,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Arc<Self> {
        // P2-d: 原实现 `let _ = create_dir_all(staging_dir)` 静默吞掉创建失败，
        // 后续写入固件包时才报错，掩盖了真正的根因（权限/磁盘）。改为留痕。
        if let Err(e) = std::fs::create_dir_all(staging_dir) {
            tracing::warn!("ota: create staging dir {} failed: {}", staging_dir, e);
        }
        // Bug 155: 启动时以 bootloader 环境变量 boot_partition 校正活动分区。
        // gatewayd 传入的 active_slot 硬编码为 'a'，若实际已从 b 分区启动，
        // 会把新固件写进正在运行的分区（第二次 OTA 覆盖活动分区 → 变砖）。
        let mut partition = partition;
        let boot_env = BootEnv::for_current_build();
        if let Some(slot) = boot_env.boot_partition_slot() {
            if slot != partition.active_slot {
                tracing::warn!(
                    "ota: active slot corrected from '{}' to '{}' per bootloader env",
                    partition.active_slot,
                    slot
                );
            }
            partition.active_slot = slot;
        }
        Arc::new(Self {
            state: Mutex::new(OtaState::Idle),
            progress: Mutex::new(0),
            error: Mutex::new(None),
            current_version: Mutex::new(0),
            target_version: Mutex::new(None),
            partition,
            staging_dir: PathBuf::from(staging_dir),
            signing_key,
            public_keys,
            metrics,
            boot_env,
        })
    }

    /// r31-OTA4: PendingReboot 标记文件完整路径（位于 staging_dir 内）。
    /// 由 apply.rs 在写入完成时创建、rollback 时删除、启动自检时检查。
    pub(super) fn pending_reboot_marker_path(&self) -> PathBuf {
        self.staging_dir.join(PENDING_REBOOT_MARKER)
    }

    /// 获取当前状态
    pub fn status(&self) -> OtaStatusInfo {
        let active = self.partition.active_slot;
        OtaStatusInfo {
            state: *self.state.lock(),
            progress: *self.progress.lock(),
            error: self.error.lock().clone(),
            current_version: *self.current_version.lock(),
            target_version: *self.target_version.lock(),
            active_slot: active,
            inactive_slot: if active == 'a' { 'b' } else { 'a' },
        }
    }

    /// 处理固件上传并执行升级
    ///
    /// `data` 为完整固件包（含包头）。
    /// 纯校验全量固件：包头 + CRC32 + 签名。不改状态、不落盘、不写分区。
    ///
    /// 返回 `(version, content_end)`，`content_end` 是包头 + 载荷的结束位置（不含签名后缀）。
    fn inspect_firmware(&self, data: &[u8]) -> GatewayResult<(u32, usize)> {
        if data.len() < HEADER_SIZE {
            return Err(GatewayError::Protocol("firmware too small".into()));
        }
        let magic: [u8; 4] = data[0..4]
            .try_into()
            .map_err(|_| GatewayError::Protocol("firmware header too small".into()))?;
        if magic != FIRMWARE_MAGIC {
            return Err(GatewayError::Protocol("invalid firmware magic".into()));
        }
        let version = u32::from_le_bytes(
            data[4..8]
                .try_into()
                .map_err(|_| GatewayError::Protocol("firmware version field corrupt".into()))?,
        );
        let expected_crc = u32::from_le_bytes(
            data[8..12]
                .try_into()
                .map_err(|_| GatewayError::Protocol("firmware crc field corrupt".into()))?,
        );
        // Bug 27：先分离签名后缀再算 CRC，否则带签名包的 CRC 会把后缀算进去。
        // 返回值 content 含包头；Ed25519 的签名范围正是「包头 + 载荷」。
        let (content, sig) = crypto::split_firmware_signature(data);
        let payload = &content[HEADER_SIZE..];
        let actual_crc = crypto::crc32(payload);
        if actual_crc != expected_crc {
            return Err(GatewayError::Protocol(format!(
                "CRC32 mismatch: expected {:#010x}, got {:#010x}",
                expected_crc, actual_crc
            )));
        }
        self.verify_package_signature(payload, content, sig, "固件")?;
        Ok((version, content.len()))
    }

    /// 签名策略（DD-40 / ROI-43）：
    ///
    /// - **无签名 → 拒绝**（fail-closed，不再保留 CRC32-only 放行）；
    /// - Ed25519 → 用「内置 + 配置」公钥逐把验证，签名覆盖包头 + 载荷；
    /// - 旧 HMAC-SHA256 → 仅当配置了 `ota_signing_key` 时接受，只覆盖载荷，保留兼容。
    fn verify_package_signature(
        &self,
        payload: &[u8],
        content: &[u8],
        sig: Option<crypto::FirmwareSignature<'_>>,
        label: &str,
    ) -> GatewayResult<()> {
        let Some(sig) = sig else {
            return Err(GatewayError::Config(format!(
                "{label}未签名，已拒绝安装（fail-closed；请使用 Ed25519 签名包）"
            )));
        };
        match sig.kind {
            crypto::SignatureKind::Ed25519 => {
                if self.public_keys.is_empty() {
                    return Err(GatewayError::Config(format!(
                        "{label}为 Ed25519 签名，但未配置验签公钥（web.ota_public_keys）"
                    )));
                }
                if !crypto::verify_ed25519_signature(content, &self.public_keys, sig.bytes) {
                    return Err(GatewayError::Protocol(format!(
                        "{label} Ed25519 验签失败——疑似被篡改或公钥不匹配"
                    )));
                }
                tracing::info!("OTA: {label} Ed25519 signature verified OK");
            }
            crypto::SignatureKind::Hmac => match &self.signing_key {
                Some(key) if crypto::verify_hmac_signature(payload, key, sig.bytes) => {
                    tracing::warn!(
                        "OTA: {label}使用旧 HMAC-SHA256 签名（仅兼容），建议改用 Ed25519"
                    );
                }
                Some(_) => {
                    return Err(GatewayError::Protocol(format!(
                        "{label} HMAC-SHA256 验签失败——疑似被篡改"
                    )));
                }
                None => {
                    return Err(GatewayError::Config(format!(
                        "{label}使用旧 HMAC 签名，但未配置 web.ota_signing_key"
                    )));
                }
            },
        }
        Ok(())
    }

    /// 供上传接口同步拒绝坏包：只校验，不改状态、不落盘。
    pub fn precheck_firmware(&self, data: &[u8]) -> GatewayResult<u32> {
        self.inspect_firmware(data).map(|(version, _)| version)
    }

    pub async fn process_firmware(self: Arc<Self>, data: Vec<u8>) -> GatewayResult<()> {
        // Bug 136: 固件大小上限——防止恶意上传超大文件撑爆内存（DoS）。
        // 超出则直接拒绝（不进锁，状态保持 Idle）。
        if data.len() > MAX_FIRMWARE_SIZE {
            return Err(GatewayError::Protocol(format!(
                "firmware too large: {} bytes (max {})",
                data.len(),
                MAX_FIRMWARE_SIZE
            )));
        }

        // Bug 79: 防止并发 OTA 写入同一非活动分区导致固件损坏。
        // 持锁原子性地检查 state 并置为 Uploading，拒绝非 Idle 状态下的重复调用。
        {
            let mut state = self.state.lock();
            // f4 修复：失败后状态停在 Failed，而 reset() 无任何调用者，
            // 旧实现只允许 Idle 进入，导致一次升级失败后永远无法重试（只能重启进程）。
            // Failed 不代表有升级在进行，允许其重新开始。
            if !matches!(*state, OtaState::Idle | OtaState::Failed) {
                return Err(GatewayError::Config(format!(
                    "OTA already in progress (state: {:?}), reject concurrent firmware write",
                    *state
                )));
            }
            *state = OtaState::Uploading;
            self.metrics.set_ota_progress(10);
        }

        *self.state.lock() = OtaState::Uploading;
        *self.progress.lock() = 30;
        *self.state.lock() = OtaState::Verifying;
        self.metrics.set_ota_progress(50);
        *self.progress.lock() = 50;

        // 1~2.5 包头 / CRC32 / 签名校验（DD-40：无签名或验签失败一律拒绝）
        let (version, content_end) = match self.inspect_firmware(&data) {
            Ok(v) => v,
            Err(e) => return self.fail(e),
        };
        *self.target_version.lock() = Some(version);
        let payload = &data[HEADER_SIZE..content_end];

        // 2.9 固件暂存：将通过校验的固件包落盘，便于断电/重启后重放或事后审计。
        // 暂存失败不影响本次升级（best-effort），仅记录告警。
        let staged = self.staging_dir.join("firmware.bin");
        if let Err(e) = tokio::fs::write(&staged, &data).await {
            tracing::warn!(
                "OTA: stage firmware to {:?} failed (ignored): {}",
                staged,
                e
            );
        }

        // 3. 写入非活动分区并置为待重启（与差分升级共用尾部流程，见 apply.rs）
        self.write_verified_to_inactive(payload, version).await
    }

    /// 标记升级失败
    fn fail(&self, err: GatewayError) -> GatewayResult<()> {
        *self.state.lock() = OtaState::Failed;
        // Bug 2.5: 失败时重置 progress 为 0，避免 UI 永远卡在 70% (Writing)
        *self.progress.lock() = 0;
        self.metrics.set_ota_progress(0);
        *self.error.lock() = Some(err.message());
        // 保留 target_version 供质量追踪（实际上下次 OTA 会覆盖）
        tracing::error!("OTA failed: {}", err);
        Err(err)
    }

    /// 是否正在升级中（供 watchdog flush 回调判断是否需保存状态）
    pub fn is_in_progress(&self) -> bool {
        let state = *self.state.lock();
        matches!(
            state,
            OtaState::Uploading | OtaState::Verifying | OtaState::Writing
        )
    }

    /// 重置状态（升级完成或放弃后）
    pub fn reset(&self) {
        *self.state.lock() = OtaState::Idle;
        self.metrics.set_ota_progress(0);
        *self.progress.lock() = 0;
        *self.error.lock() = None;
        *self.target_version.lock() = None;
    }
}

#[cfg(test)]
mod tests;
