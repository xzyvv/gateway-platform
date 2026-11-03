//! 固件下载逻辑

use std::path::Path;
use std::sync::Arc;

use super::crypto;
use super::{OtaManager, OtaState, DELTA_HEADER_SIZE, DELTA_MAGIC, MAX_FIRMWARE_SIZE, SIG_LEN};
use crate::engine::error::{GatewayError, GatewayResult};

impl OtaManager {
    /// 差分升级（bsdiff 4.x 兼容补丁）
    ///
    /// 差分包格式：[delta_magic:4 "GWDT"][new_version:u32][new_crc32:u32][delta_payload...]
    /// 可选签名后缀：`delta_payload` 之后追加 [SIG_MAGIC:4][hmac_sha256:32]，
    /// 验签策略与全量包一致（HMAC 覆盖 delta_payload）。
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

        // 1. 解析包头
        if data.len() < DELTA_HEADER_SIZE {
            return self.fail(GatewayError::Protocol("delta package too small".into()));
        }
        let magic: [u8; 4] = data[0..4]
            .try_into()
            .map_err(|_| GatewayError::Protocol("delta header too small".into()))?;
        if magic != DELTA_MAGIC {
            return self.fail(GatewayError::Protocol("invalid delta magic".into()));
        }
        let new_version = u32::from_le_bytes(
            data[4..8]
                .try_into()
                .map_err(|_| GatewayError::Protocol("delta version field corrupt".into()))?,
        );
        let expected_new_crc = u32::from_le_bytes(
            data[8..12]
                .try_into()
                .map_err(|_| GatewayError::Protocol("delta crc field corrupt".into()))?,
        );

        // 分离可选签名后缀（与全量包同一 [SIG_MAGIC][sig] 格式），得到纯补丁数据。
        let (delta_payload, has_sig) = crypto::split_firmware_signature(&data[DELTA_HEADER_SIZE..]);

        *self.progress.lock() = 30;
        *self.target_version.lock() = Some(new_version);

        // 2. HMAC-SHA256 验签（策略与全量包一致：签名覆盖 delta_payload）
        if has_sig {
            match &self.signing_key {
                Some(key) => {
                    let sig_start = DELTA_HEADER_SIZE + delta_payload.len() + 4;
                    if !crypto::verify_hmac_signature(
                        delta_payload,
                        key,
                        &data[sig_start..sig_start + SIG_LEN],
                    ) {
                        return self.fail(GatewayError::Protocol(
                            "HMAC-SHA256 signature verification FAILED — possible delta tampering"
                                .into(),
                        ));
                    }
                    tracing::info!("OTA: delta HMAC-SHA256 signature verified OK");
                }
                None => {
                    return self.fail(GatewayError::Config(
                        "delta has signature but no ota_signing_key configured".into(),
                    ));
                }
            }
        } else if self.signing_key.is_some() {
            return self.fail(GatewayError::Config(
                "ota_signing_key configured but delta has no signature — rejected".into(),
            ));
        }

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
