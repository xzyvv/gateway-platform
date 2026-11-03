//! 固件写入/应用/回滚

use std::path::Path;
use std::time::Duration;

use super::crypto;
use super::{OtaManager, OtaState, WRITE_MAX_RETRIES, WRITE_TIMEOUT_SECS};
use crate::engine::error::{GatewayError, GatewayResult};

impl OtaManager {
    /// 将已校验的固件 payload 写入非活动分区并置为待重启（全量 / 差分升级共用尾部流程）。
    ///
    /// 流程：置 Writing(70) → 后台 kick watchdog 期间写入 → 重读比对 CRC32(80)
    ///       → 设置 bootloader 启动分区(90) → PendingReboot(100)。
    /// CRC 期望值由 `payload` 现算，保证与写入内容一致。`version` 仅用于日志。
    pub(super) async fn write_verified_to_inactive(
        &self,
        payload: &[u8],
        version: u32,
    ) -> GatewayResult<()> {
        *self.state.lock() = OtaState::Writing;
        self.metrics.set_ota_progress(70);
        *self.progress.lock() = 70;

        let inactive = if self.partition.active_slot == 'a' {
            &self.partition.slot_b
        } else {
            &self.partition.slot_a
        };

        // M2-5: OTA 写入期间后台 kick watchdog，防止 watchdog 15s 超时被踢
        // Bug 2.5: 原实现仅 Linux 下 write /dev/watchdog，Windows / macOS 无硬件 watchdog，
        //         但写入卡死后 write_partition 的 spawn_blocking 依然会挂在系统线程，
        //         fail() 重置 progress 为 0 后，进一步提供进程级守护：
        //         超过 (WRITE_TIMEOUT_SECS * 3 + 30) 未完成，强制 fail 走为失败状态，
        //         避免调用方经赖超时机制。
        let write_result = {
            let watchdog_kick =
                crate::engine::supervision::spawn_catch_unwind("ota_watchdog_kick", {
                    // 每隔 10s kick 一次 watchdog（watchdog 超时 15s）
                    async move {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                            kick_platform_watchdog();
                            tracing::debug!("OTA: watchdog tick during write");
                        }
                    }
                });
            // Bug 2.5: 进程级守护——超过 3 倍 WRITE_TIMEOUT_SECS 未完成
            // 且 spawn_blocking 未退出，则判定为卡死，走 fail 路径。
            let guard_deadline = std::time::Duration::from_secs(super::WRITE_TIMEOUT_SECS * 3 + 30);
            let write_fut = self.write_partition(inactive, payload);
            let timed_result = tokio::time::timeout(guard_deadline, write_fut).await;
            watchdog_kick.abort();
            match timed_result {
                Ok(r) => r,
                Err(_elapsed) => {
                    tracing::error!(
                        "OTA write guard timeout after {:?} — partition IO appears stuck,                          failing OTA to avoid permanent UI hang",
                        guard_deadline
                    );
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "OTA write guard timeout after {}s",
                            guard_deadline.as_secs()
                        ),
                    ))
                }
            }
        };

        if let Err(e) = write_result {
            return self.fail(GatewayError::Io(format!("write partition failed: {}", e)));
        }

        *self.progress.lock() = 80;

        // M1-6: 写入后 CRC 重读校验
        let expected_crc = crypto::crc32(payload);
        if let Err(e) = self
            .verify_partition(inactive, expected_crc, payload.len())
            .await
        {
            return self.fail(GatewayError::Protocol(format!(
                "partition verify failed: {}",
                e
            )));
        }

        *self.progress.lock() = 90;

        // 设置 boot 标志到非活动分区（Linux: 通过 bootloader 环境变量）
        if let Err(e) = self.set_boot_partition(inactive) {
            return self.fail(GatewayError::Io(format!("set boot flag failed: {}", e)));
        }

        *self.state.lock() = OtaState::PendingReboot;
        self.metrics.set_ota_progress(100);
        *self.progress.lock() = 100;
        // r31-OTA4: 持久化 PendingReboot 标记。断电场景下重启后启动自检
        // 会读到本标记，触发"新固件已写入但未生效"告警/自动 set_boot_partition。
        // 失败仅 warn——主流程已成功，标记缺失最坏后果是断电后丢本次升级。
        let marker = self.pending_reboot_marker_path();
        // #2: 标记须同时记录「目标槽位」，供启动自检判断升级是否已生效。
        let target_slot = if self.partition.active_slot == 'a' {
            'b'
        } else {
            'a'
        };
        if let Err(e) = std::fs::write(&marker, Self::encode_pending_reboot(version, target_slot)) {
            tracing::warn!(
                "OTA: failed to write pending_reboot marker at {}: {} \
                 (power loss before reboot may lose this upgrade)",
                marker.display(),
                e
            );
        }
        tracing::warn!(
            "OTA: firmware v{} written + verified to {:?}, reboot required",
            version,
            inactive
        );
        Ok(())
    }

    /// 写入分区（Linux 下直接写块设备，带超时 + 重试）
    pub(super) async fn write_partition(
        &self,
        device: &Path,
        payload: &[u8],
    ) -> std::io::Result<()> {
        let device = device.to_path_buf();
        let payload = payload.to_vec();

        for attempt in 0..WRITE_MAX_RETRIES {
            let dev = device.clone();
            let data = payload.clone();
            let result = tokio::time::timeout(
                Duration::from_secs(WRITE_TIMEOUT_SECS),
                tokio::task::spawn_blocking(move || {
                    use std::io::Write;
                    let mut f = std::fs::OpenOptions::new().write(true).open(&dev)?;
                    f.write_all(&data)?;
                    f.sync_all()?;
                    Ok::<(), std::io::Error>(())
                }),
            )
            .await;

            match result {
                Ok(Ok(Ok(()))) => return Ok(()),
                Ok(Ok(Err(e))) => {
                    tracing::warn!("write partition attempt {} failed: {}", attempt + 1, e);
                    if attempt + 1 == WRITE_MAX_RETRIES {
                        return Err(e);
                    }
                }
                Ok(Err(e)) => {
                    tracing::error!("write partition task panicked: {}", e);
                    if attempt + 1 == WRITE_MAX_RETRIES {
                        return Err(std::io::Error::other(e));
                    }
                }
                Err(_elapsed) => {
                    tracing::error!(
                        "write partition attempt {} timed out after {}s",
                        attempt + 1,
                        WRITE_TIMEOUT_SECS
                    );
                    if attempt + 1 == WRITE_MAX_RETRIES {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("write timeout after {}s", WRITE_TIMEOUT_SECS),
                        ));
                    }
                }
            }
            // r31-OTA2: 重试间固定 1s 等待改为 attempt 线性退避 + ±25% 抖动，
            // 避免多设备同时升级遇阻时同步重试（thundering herd）。
            // 抖动来源：tokio Instant::now() 的纳秒位（无外部 rand 依赖）。
            let base_ms = (attempt as u64 + 1) * 1000; // 1s, 2s, 3s
            let now_nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(0);
            let jitter_ms = (now_nanos % 500).saturating_sub(0); // 0..500ms
            let total_ms = base_ms + jitter_ms;
            tokio::time::sleep(Duration::from_millis(total_ms)).await;
        }
        Err(std::io::Error::other("exhausted all retries"))
    }

    /// 校验已写入分区：重读并比对 CRC32（M1-6）
    ///
    /// 写入后立即重读，防止块设备写入静默损坏（如 NAND 坏块、断电）。
    pub(super) async fn verify_partition(
        &self,
        device: &Path,
        expected_crc: u32,
        payload_len: usize,
    ) -> GatewayResult<()> {
        let device = device.to_path_buf();
        let read_result = tokio::time::timeout(
            Duration::from_secs(WRITE_TIMEOUT_SECS),
            // Bug 156: 限量读取 payload_len 字节——分区通常大于固件，整分区读回
            // 长度 != payload_len 会让下方长度校验恒失败（OTA 永远卡在 80%）。
            self.read_partition_prefix(&device, payload_len),
        )
        .await
        .map_err(|_| {
            GatewayError::Io(format!(
                "verify read timed out after {}s",
                WRITE_TIMEOUT_SECS
            ))
        })?;

        let read_data =
            read_result.map_err(|e| GatewayError::Io(format!("verify read failed: {}", e)))?;
        if read_data.len() != payload_len {
            return Err(GatewayError::Protocol(format!(
                "verify length mismatch: expected {}, got {}",
                payload_len,
                read_data.len()
            )));
        }
        let actual_crc = crypto::crc32(&read_data);
        if actual_crc != expected_crc {
            return Err(GatewayError::Protocol(format!(
                "verify CRC mismatch: expected {:#010x}, got {:#010x}",
                expected_crc, actual_crc
            )));
        }
        tracing::info!(
            "partition verify OK: CRC={:#010x}, len={}",
            actual_crc,
            payload_len
        );
        Ok(())
    }

    /// 设置 bootloader 启动分区为指定槽位（写 u-boot 环境变量）
    ///
    /// 修复(审查#5)：原 `set_boot_partition` 忽略传入的 device 参数、恒写「活动分区的反面」，
    /// 于是 `rollback()` 虽然传入了活动分区，实际仍把 boot_partition 设成新固件所在槽位
    /// → 取消升级后重启仍旧加载新固件（静默失败）。现改为按槽位显式设置，由调用方决定目标槽。
    ///
    /// 同时写 `bootcount=0`（bootloader 根据 bootcount 决定是否回滚）。
    /// 底层命令的 io 错误与退出码检查由 `BootEnv` 承担——生产路径一旦 `fw_setenv`
    /// 失败（env 分区写坏 / fw_env.config 缺失）必须报错，否则会得到
    /// 「代码以为切换成功、设备重启后仍启动旧分区」的结果。
    pub(super) fn set_boot_partition_slot(&self, slot: char) -> GatewayResult<()> {
        self.boot_env.setenv("boot_partition", &slot.to_string())?;
        self.boot_env.setenv("bootcount", "0")?;
        Ok(())
    }

    /// 按分区设备路径设置启动分区（升级写入非活动分区时使用）
    pub(super) fn set_boot_partition(&self, device: &Path) -> GatewayResult<()> {
        let slot = if device == self.partition.slot_a.as_path() {
            'a'
        } else if device == self.partition.slot_b.as_path() {
            'b'
        } else {
            return Err(GatewayError::Config(format!(
                "set_boot_partition: {} is neither slot_a nor slot_b",
                device.display()
            )));
        };
        self.set_boot_partition_slot(slot)
    }

    /// #2: pending_reboot 标记编码——version(u32 LE) + 目标槽位(1 字节 ASCII 'a'/'b')。
    ///
    /// 记录「目标槽位」是关键：启动自检据此判断升级是否已生效。旧实现仅存 version，
    /// 自检只能按「active_slot 的反面」猜目标槽，于是升级生效后（active_slot 已是
    /// 新槽）反而把 boot_partition 切回旧槽 → 每次重启都回退。
    fn encode_pending_reboot(version: u32, target_slot: char) -> [u8; 5] {
        let mut buf = [0u8; 5];
        buf[..4].copy_from_slice(&version.to_le_bytes());
        buf[4] = target_slot as u8;
        buf
    }

    /// #2: 解析 pending_reboot 标记 → (target_version, target_slot)。
    /// 长度不足 / 槽位非法（含旧版 4 字节格式）返回 None，调用方按损坏处理。
    fn decode_pending_reboot(bytes: &[u8]) -> Option<(u32, char)> {
        if bytes.len() < 5 {
            return None;
        }
        let version = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let slot = match bytes[4] {
            b'a' => 'a',
            b'b' => 'b',
            _ => return None,
        };
        Some((version, slot))
    }

    /// #2: 清除 pending_reboot 标记（幂等，文件不存在视为已清除）。
    fn clear_pending_reboot_marker(&self) {
        let marker = self.pending_reboot_marker_path();
        if let Err(e) = std::fs::remove_file(&marker) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "OTA: failed to remove pending_reboot marker at {}: {}",
                    marker.display(),
                    e
                );
            }
        }
    }

    /// F2: 回滚到当前活动分区（取消待生效的升级）
    ///
    /// 将 bootloader 启动分区设回当前活动分区，并重置 bootcount。
    /// 仅在 PendingReboot 状态下有效（升级写入后尚未重启时）。
    pub fn rollback(&self) -> GatewayResult<()> {
        let state = *self.state.lock();
        if state != OtaState::PendingReboot {
            return Err(GatewayError::Config(format!(
                "cannot rollback in state {:?} (only PendingReboot)",
                state
            )));
        }
        // 将启动分区设回当前活动分区（修复(审查#5)：原先复用「写非活动槽」的函数，等于没改回）
        self.set_boot_partition_slot(self.partition.active_slot)?;
        *self.state.lock() = OtaState::Idle;
        self.metrics.set_ota_progress(0);
        *self.target_version.lock() = None;
        *self.progress.lock() = 0;
        // r31-OTA4: 取消升级后清理 PendingReboot 标记——否则下次启动自检会误判
        // "上次升级还没生效"（#2 后影响更大：残留标记会把启动分区重新指向被取消的
        // 目标槽，导致反复回退）。文件不存在视为已清理（幂等）。
        self.clear_pending_reboot_marker();
        tracing::warn!("OTA rollback: booting from current active partition");
        Ok(())
    }

    /// Bug 155: 启动自检——若 bootloader bootcount >= 3（新分区连续启动失败），
    /// 执行 A/B 回滚（切回上一分区并重启）。需在 OtaManager 构造后、启动早期调用；
    /// 非 Linux / 无 fw_printenv 时 `check_bootcount_rollback` 返回 false，为无副作用 no-op。
    pub fn boot_rollback_check(&self) {
        // r31-OTA4: 启动自检 PendingReboot 标记。标记记录「目标槽位」，据此判断
        // 上次升级是否已生效：
        // - active_slot == 目标槽位：本次已从新固件启动 → 升级成功，清除标记；
        // - 否则：bootloader 尚未切到目标槽（如断电 / env 写丢失）→ 重新确认
        //   boot_partition 指向目标槽，保留标记待下次启动复核。
        //
        // 修复(#2)：旧实现无条件把 boot_partition 设为「active_slot 的反面」——
        // 升级生效后（active_slot 已是新槽）反而把启动分区切回旧槽，导致每次重启
        // 都回退、新固件永不生效。注意：本检查不修改 active_slot。
        let marker = self.pending_reboot_marker_path();
        if let Ok(bytes) = std::fs::read(&marker) {
            match Self::decode_pending_reboot(&bytes) {
                Some((target_version, target_slot)) => {
                    if self.partition.active_slot == target_slot {
                        tracing::warn!(
                            "OTA: pending upgrade v{} is now effective (active slot '{}'); \
                             clearing pending_reboot marker",
                            target_version,
                            target_slot
                        );
                        self.clear_pending_reboot_marker();
                    } else {
                        tracing::warn!(
                            "OTA: pending_reboot marker at {} — active slot '{}' != target slot '{}', re-affirming boot_partition",
                            marker.display(),
                            self.partition.active_slot,
                            target_slot
                        );
                        if let Err(e) = self.set_boot_partition_slot(target_slot) {
                            tracing::error!(
                                "ota: failed to re-affirm boot_partition={} after detected pending reboot: {}",
                                target_slot,
                                e
                            );
                        }
                    }
                }
                None => {
                    // 标记损坏 / 旧格式（无槽位信息）：无法判定目标槽，若据此反向
                    // 切换启动分区会造成「反复回退」。安全起见清除标记，让设备以
                    // 当前分区稳定运行（操作员可重新发起升级）。
                    tracing::warn!(
                        "OTA: pending_reboot marker at {} is unreadable/legacy; clearing it to avoid boot-partition flapping",
                        marker.display()
                    );
                    self.clear_pending_reboot_marker();
                }
            }
        }

        if self.check_bootcount_rollback() {
            if let Err(e) = self.perform_rollback() {
                tracing::error!("ota: auto rollback failed: {}", e);
            }
        }
    }

    /// M1-13: 检查 bootcount，判断是否需要回滚
    ///
    /// bootloader 维护 bootcount 计数器：每次启动 +1，正常启动到用户态后清零。
    /// 若 bootcount >= 3，说明新分区连续启动失败，应回滚到上一个分区。
    ///
    /// 返回值：true 表示需要回滚，false 表示正常启动。
    ///
    /// r31-OTA8 同步：按 `bootcount=N` 严格行解析，避免其他变量干扰。
    pub fn check_bootcount_rollback(&self) -> bool {
        match self.boot_env.bootcount() {
            Some(count) if count >= 3 => {
                tracing::error!(
                    "bootcount={} >= 3, rollback required (new partition failed to boot)",
                    count
                );
                true
            }
            _ => false,
        }
    }

    /// M2-6: 执行自动回滚：切换 boot_partition 到上一个分区并重启
    ///
    /// 在 bootstrap 启动时检测到 bootcount >= 3 后调用。
    pub fn perform_rollback(&self) -> GatewayResult<()> {
        // #2: 回滚已处置本次升级——清除 pending_reboot 标记，否则下次启动自检会
        // 依据残留标记把 boot_partition 重新指向「已知启动失败」的新槽，形成回滚震荡。
        self.clear_pending_reboot_marker();
        // 切换到另一个分区
        let rollback_slot = if self.partition.active_slot == 'a' {
            "b"
        } else {
            "a"
        };
        tracing::error!(
            "performing A/B rollback: switching to slot {}",
            rollback_slot
        );

        // #12: `fw_setenv` 若因 env 分区只读/损坏而退出码非零，必须报错并中止
        //   （不 reboot）——否则回滚以为成功，reboot 后仍启动故障分区 → 设备变砖。
        self.boot_env.setenv("boot_partition", rollback_slot)?;
        // r31-OTA3: 每个 env 写成功后立即 sync——失败返 Err 不需 sync
        // (无 env 写要持久化)，成功路径必须确保 env 写已刷盘再 reboot。
        // 旧实现只在最后 reboot 前 sync 一次，但若中间 bootcount 失败返 Err，
        // boot_partition 写可能因 page cache 丢失而实际未生效。
        self.boot_env.sync()?;

        self.boot_env.setenv("bootcount", "0")?;
        self.boot_env.sync()?;

        // 同步文件系统后重启
        self.boot_env.sync()?;
        tracing::warn!("rebooting to apply rollback...");
        self.boot_env.reboot()?;
        Ok(())
    }
}

/// Bug 2.5: 跨平台 watchdog kick
///
/// - Linux: 写 /dev/watchdog（依赖系统服务/board support，不存在则 no-op）
/// - Windows / macOS: 无硬件 watchdog，kick 为 no-op；上层进程级守护（
///   tokio::time::timeout 包裵 write_partition）是主要保障。
fn kick_platform_watchdog() {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        // Bug 157: 旧实现 `std::fs::write("/dev/watchdog", b"")` 写入 0 字节——
        // 内核 watchdog 仅在有"非空写入"时才刷新计时器，空写等于没喂狗，OTA 写入
        // 期间仍可能被复位 → 变砖；且 fs::write 带 truncate 语义，对设备节点不合适。
        // 改为只写模式 open（不 truncate）并写入 1 字节。
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/watchdog")
        {
            let _ = f.write_all(b"\0");
            let _ = f.flush();
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // No-op: 跨平台 stub。设备部署上如需 watchdog，可改为 Windows
        // service control / OpenHardwareMonitor / 工业主板 BMC 协议。
    }
}
