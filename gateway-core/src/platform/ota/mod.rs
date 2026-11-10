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
    /// HMAC-SHA256 签名密钥（None=不验签，向后兼容 CRC32-only）
    signing_key: Option<Vec<u8>>,
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
/// 固件签名后缀 magic: "GSIG"
const SIG_MAGIC: [u8; 4] = [0x47, 0x53, 0x49, 0x47];
/// HMAC-SHA256 签名长度
const SIG_LEN: usize = 32;
/// 签名后缀总长度: magic(4) + sig(32) = 36
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

impl OtaManager {
    /// 创建 OTA 管理器
    pub fn new(
        partition: AbPartitionConfig,
        staging_dir: &str,
        signing_key: Option<Vec<u8>>,
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

        // 1. 检查包头
        if data.len() < HEADER_SIZE {
            return self.fail(GatewayError::Protocol("firmware too small".into()));
        }
        let magic: [u8; 4] = data[0..4]
            .try_into()
            .map_err(|_| GatewayError::Protocol("firmware header too small".into()))?;
        if magic != FIRMWARE_MAGIC {
            return self.fail(GatewayError::Protocol("invalid firmware magic".into()));
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
        // Bug 27 修复：先分离签名后缀，再计算 CRC32
        // 原实现先计算 CRC32（含签名后缀），再 split，导致带签名固件包 CRC 校验失败
        // 正确顺序：split → CRC32(payload_without_sig) → HMAC 验签
        let (payload, has_sig) = crypto::split_firmware_signature(&data[HEADER_SIZE..]);

        *self.state.lock() = OtaState::Uploading;
        *self.progress.lock() = 30;
        *self.target_version.lock() = Some(version);

        // 2. CRC32 校验（仅对 payload，不含签名后缀）
        *self.state.lock() = OtaState::Verifying;
        self.metrics.set_ota_progress(50);
        *self.progress.lock() = 50;
        let actual_crc = crypto::crc32(payload);
        if actual_crc != expected_crc {
            return self.fail(GatewayError::Protocol(format!(
                "CRC32 mismatch: expected {:#010x}, got {:#010x}",
                expected_crc, actual_crc
            )));
        }

        // 2.5 HMAC-SHA256 数字签名验证
        // 固件包可选签名后缀: [原固件包][SIG_MAGIC:4][hmac_sig:32]
        // 如果固件包带签名后缀，且配置了 ota_signing_key，则验签
        // 如果固件包不带签名但配置了 key，拒绝（强制签名）
        // 如果固件包不带签名且未配置 key，退化为 CRC32-only（向后兼容）
        if has_sig {
            match &self.signing_key {
                Some(key) => {
                    if !crypto::verify_hmac_signature(
                        payload,
                        key,
                        &data[HEADER_SIZE + payload.len() + 4
                            ..HEADER_SIZE + payload.len() + 4 + SIG_LEN],
                    ) {
                        return self.fail(GatewayError::Protocol("HMAC-SHA256 signature verification FAILED — possible firmware tampering".into()));
                    }
                    tracing::info!("OTA: HMAC-SHA256 signature verified OK");
                }
                None => {
                    return self.fail(GatewayError::Config(
                        "firmware has signature but no ota_signing_key configured".into(),
                    ));
                }
            }
        } else if self.signing_key.is_some() {
            return self.fail(GatewayError::Config(
                "ota_signing_key configured but firmware has no signature — rejected".into(),
            ));
        }

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
