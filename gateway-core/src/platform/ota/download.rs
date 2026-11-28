//! 固件下载逻辑

use std::path::Path;
use std::sync::Arc;

use super::crypto;
use super::{OtaManager, OtaState, DELTA_HEADER_SIZE, DELTA_MAGIC, MAX_FIRMWARE_SIZE};
use crate::engine::error::{GatewayError, GatewayResult};

impl OtaManager {
    /// 纯校验差分包：包头 + 签名（新固件 CRC 需要应用补丁后再算）。
    ///
    /// 返回 `(new_version, content_end)`，`content_end` 为包头 + 补丁的结束位置（不含签名后缀）。
    fn inspect_delta(&self, data: &[u8]) -> GatewayResult<(u32, usize)> {
        if data.len() < DELTA_HEADER_SIZE {
            return Err(GatewayError::Protocol("delta package too small".into()));
        }
        let magic: [u8; 4] = data[0..4]
            .try_into()
            .map_err(|_| GatewayError::Protocol("delta header too small".into()))?;
        if magic != DELTA_MAGIC {
            return Err(GatewayError::Protocol("invalid delta magic".into()));
        }
        let new_version = u32::from_le_bytes(
            data[4..8]
                .try_into()
                .map_err(|_| GatewayError::Protocol("delta version field corrupt".into()))?,
        );
        // content 含差分包头；Ed25519 的签名范围是「包头 + 补丁」
        let (content, sig) = crypto::split_firmware_signature(data);
        let patch = &content[DELTA_HEADER_SIZE..];
        self.verify_package_signature(patch, content, sig, "差分包")?;
        Ok((new_version, content.len()))
    }

    /// 供上传接口同步拒绝坏包：只校验，不改状态、不落盘。
    pub fn precheck_delta(&self, data: &[u8]) -> GatewayResult<u32> {
        self.inspect_delta(data).map(|(version, _)| version)
    }

    /// 差分升级（bsdiff 4.x 兼容补丁）
    ///
    /// 差分包格式：[delta_magic:4 "GWDT"][new_version:u32][new_crc32:u32][delta_payload...]
    /// 签名后缀：新格式 [FWSG][ed25519:64]（覆盖包头 + 补丁）；旧格式 [GSIG][hmac:32] 仅兼容。
    /// 无签名一律拒绝（fail-closed）。
    ///
    /// 流程：解析包头 → 校验包体大小 → 读取当前活动分区固件作为 diff 源 →
    ///       bsdiff 应用差分得到新固件 → 校验新固件 CRC32 → 写入非活动分区。
    /// 收益：升级包体积减少 60-90%，适合弱网/4G 场景。
    pub async fn process_delta(self: Arc<Self>, data: Vec<u8>) -> GatewayResult<()> {
        // 大小上限：与全量包共用 64MB 上限。检查在置位 Uploading 之前，超限时状态保持 Idle。
        if data.len() > MAX_FIRMWARE_SIZE {
            return Err(GatewayError::Protocol(format!(
                "delta package too large: {} bytes (max {})",
                data.len(),
                MAX_FIRMWARE_SIZE
            )));
        }

        // 并发保护：与非 Idle 状态互斥，防止并发 OTA 损坏非活动分区（与全量升级一致）。
        // f4 修复：允许从 Failed 重新开始（同 process_firmware），否则一次失败后
        // 无法再重试差分升级。
        {
            let mut state = self.state.lock();
            if !matches!(*state, OtaState::Idle | OtaState::Failed) {
                return Err(GatewayError::Config(format!(
                    "OTA already in progress (state: {:?}), reject concurrent delta write",
                    *state
                )));
            }
            *state = OtaState::Uploading;
            self.metrics.set_ota_progress(10);
        }

        // 1~2. 包头 + 签名校验（DD-40：无签名/验签失败一律拒绝；新固件 CRC 待打补丁后再验）
        let (new_version, content_end) = match self.inspect_delta(&data) {
            Ok(v) => v,
            Err(e) => return self.fail(e),
        };
        let delta_payload = &data[DELTA_HEADER_SIZE..content_end];
        let expected_new_crc = u32::from_le_bytes(
            data[8..12]
                .try_into()
                .map_err(|_| GatewayError::Protocol("delta crc field corrupt".into()))?,
        );

        *self.progress.lock() = 30;
        *self.target_version.lock() = Some(new_version);

        // 3. 读取当前活动分区固件作为 bsdiff 的 source
        *self.state.lock() = OtaState::Verifying;
        self.metrics.set_ota_progress(50);
        *self.progress.lock() = 50;
        let active = if self.partition.active_slot == 'a' {
            &self.partition.slot_a
        } else {
            &self.partition.slot_b
        };
        let old_fw = match self.read_partition(active).await {
            Ok(v) => v,
            Err(e) => {
                return self.fail(GatewayError::Io(format!(
                    "read active partition for delta failed: {}",
                    e
                )))
            }
        };

        // 4. 应用 bsdiff 补丁（CPU 密集，放入 spawn_blocking 避免阻塞 async 运行时）
        // r34-DL1: 加 timeout——恶意/损坏的大 patch 可能让 bzip2 解码卡死
        // spawn_blocking 线程。120 秒足够 64MB bsdiff 应用，异常 patch
        // 超时即失败，遵循"fail 永远比挂起好"。
        let patch = delta_payload.to_vec();
        let apply_fut = tokio::task::spawn_blocking(move || apply_delta(&old_fw, &patch));
        let apply_result = tokio::time::timeout(
            std::time::Duration::from_secs(super::WRITE_TIMEOUT_SECS * 2),
            apply_fut,
        )
        .await;
        let new_fw = match apply_result {
            Ok(Ok(Ok(v))) => v,
            Ok(Ok(Err(e))) => return self.fail(e),
            Ok(Err(e)) => {
                return self.fail(GatewayError::Io(format!(
                    "delta apply task panicked: {}",
                    e
                )))
            }
            Err(_elapsed) => {
                return self.fail(GatewayError::Io(format!(
                    "delta apply timed out after {}s",
                    super::WRITE_TIMEOUT_SECS * 2
                )));
            }
        };

        // 5. 校验打补丁后的新固件 CRC32 与包头声明一致（防补丁错误 / 源固件不匹配）
        let actual_crc = crypto::crc32(&new_fw);
        if actual_crc != expected_new_crc {
            return self.fail(GatewayError::Protocol(format!(
                "delta result CRC32 mismatch: expected {:#010x}, got {:#010x}",
                expected_new_crc, actual_crc
            )));
        }

        // 6. 暂存新固件（best-effort，便于断电/重启后重放或事后审计）
        let staged = self.staging_dir.join("delta.bin");
        if let Err(e) = tokio::fs::write(&staged, &new_fw).await {
            tracing::warn!(
                "OTA: stage delta result to {:?} failed (ignored): {}",
                staged,
                e
            );
        }

        // 7. 写入非活动分区并置为待重启（与全量升级共用尾部流程）
        self.write_verified_to_inactive(&new_fw, new_version).await
    }

    /// 读取分区内容（用于写入后 CRC 复核 / 差分升级时获取当前固件）
    ///
    /// 直接读取 `device` 路径：真实环境为块设备节点，测试 / 非 Linux 环境为普通文件，
    /// 二者语义一致——「写入哪个设备就读回哪个设备」，保证 `verify_partition` 校验有效。
    pub(super) async fn read_partition(&self, device: &Path) -> std::io::Result<Vec<u8>> {
        let device = device.to_path_buf();
        let read_fut = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            use std::io::Read;
            let mut f = std::fs::File::open(&device)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            Ok(buf)
        });
        match tokio::time::timeout(
            std::time::Duration::from_secs(super::WRITE_TIMEOUT_SECS / 2),
            read_fut,
        )
        .await
        {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(e)) => Err(std::io::Error::other(format!(
                "read_partition task panicked: {}",
                e
            ))),
            Err(_elapsed) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "read_partition timed out after {}s",
                    super::WRITE_TIMEOUT_SECS / 2
                ),
            )),
        }
    }

    /// Bug 156: 限量读取分区前 `len` 字节（用于写入后 CRC 复核）。
    ///
    /// `verify_partition` 要求读回长度严格等于 payload_len，但块设备分区通常
    /// 大于固件本身，`read_partition` 整分区读回会比 payload 长 → 除镜像恰好
    /// 填满分区外，OTA 永远卡在 80% 长度校验失败。固件自分区头部写入，故只读
    /// 前 payload_len 字节比对即可。
    pub(super) async fn read_partition_prefix(
        &self,
        device: &Path,
        len: usize,
    ) -> std::io::Result<Vec<u8>> {
        let device = device.to_path_buf();
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let f = std::fs::File::open(&device)?;
            let mut buf = Vec::with_capacity(len);
            f.take(len as u64).read_to_end(&mut buf)?;
            Ok(buf)
        })
        .await
        .map_err(std::io::Error::other)?
    }
}

/// 应用 bsdiff 4.x 补丁，返回打补丁后的新固件字节。
///
/// r34-DL3: `Bspatch::new` 内部会 bzip2 解码 patch header。恶意大 patch 在
/// `with_capacity` 之前应先校验 target_size 上限——避免校验失败时仍占用
/// `target_size` 字节的 Vec with_capacity 预分配（虽然不会 write，但 capacity
/// 已被 mmap/分配）。
///
/// 实测：`hint_target_size()` 在 `Bspatch::new` 完成后立即可用（不需 `apply`），
/// 校验前置可在分配阶段就拒绝。
fn apply_delta(old: &[u8], patch: &[u8]) -> GatewayResult<Vec<u8>> {
    use qbsdiff::Bspatch;

    let patcher = Bspatch::new(patch)
        .map_err(|e| GatewayError::Protocol(format!("invalid bsdiff patch: {}", e)))?;
    let target_size = patcher.hint_target_size();
    if target_size > MAX_FIRMWARE_SIZE as u64 {
        return Err(GatewayError::Protocol(format!(
            "delta target size {} exceeds firmware limit {}",
            target_size, MAX_FIRMWARE_SIZE
        )));
    }
    // r34-DL3: 0 大小立即拒（恶意空 patch + capacity 0 仍可能走 apply 路径）
    if target_size == 0 {
        return Err(GatewayError::Protocol("delta target size is 0".into()));
    }
    let mut out = Vec::with_capacity(target_size as usize);
    patcher
        .apply(old, std::io::Cursor::new(&mut out))
        .map_err(|e| GatewayError::Protocol(format!("apply bsdiff patch failed: {}", e)))?;
    Ok(out)
}

/// 缺陷#1：块设备定长、无法 `ftruncate`，用写零覆盖尾部残留（emulated truncate）。
///
/// 从 `keep_len`（新固件结尾）写零直到 `scrub_end`，清除「新头 + 旧尾」混合体。
/// 设备长度须由调用方用 `seek(SeekFrom::End(0))` 取得——块设备 fstat st_size=0，
/// 不能用 `metadata().len()`；`scrub_end` 上界由调用方限到 MAX_FIRMWARE_SIZE。
/// 返回实际擦除（写零）的字节数。
pub(super) fn scrub_stale_tail(
    f: &mut std::fs::File,
    keep_len: u64,
    scrub_end: u64,
) -> std::io::Result<u64> {
    use std::io::{Seek, SeekFrom, Write};
    if scrub_end <= keep_len {
        return Ok(0);
    }
    f.seek(SeekFrom::Start(keep_len))?;
    let zeros = [0u8; 64 * 1024];
    let total = scrub_end - keep_len;
    let mut remaining = total;
    while remaining > 0 {
        let chunk = remaining.min(zeros.len() as u64) as usize;
        f.write_all(&zeros[..chunk])?;
        remaining -= chunk as u64;
    }
    f.sync_all()?;
    Ok(total)
}
