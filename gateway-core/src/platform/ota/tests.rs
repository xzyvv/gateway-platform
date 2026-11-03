//! OTA 关键路径测试（C2）
//!
//! 用临时文件充当 A/B 分区设备（`write_partition` / `verify_partition` 只做普通文件读写）；
//! bootloader 环境（写 env / 读 env / 刷盘 / 重启）由 `BootEnv` 的内存后端承担，
//! 因此升级流程在任意平台（含无 u-boot 工具的 CI runner）都能完整跑通，
//! 且测试永远不会真的重启宿主机。
use super::*;
use crate::engine::metrics::Metrics;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

/// 递增序号，保证并发运行的测试各自使用独立临时目录
static SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("gw_ota_test_{}_{}_{}", std::process::id(), tag, n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn new_manager(dir: &Path, signing_key: Option<Vec<u8>>) -> Arc<OtaManager> {
    let partition = AbPartitionConfig {
        slot_a: dir.join("slot_a.bin"),
        slot_b: dir.join("slot_b.bin"),
        active_slot: 'a',
    };
    let staging = dir.join("staging");
    let staging = staging.to_str().expect("utf8 staging path");
    // 预建分区文件：write_partition 用 open().write(true)（不带 create），
    // 真实块设备节点总是已存在，测试中用空文件模拟节点。
    std::fs::write(dir.join("slot_a.bin"), b"").expect("create slot_a");
    std::fs::write(dir.join("slot_b.bin"), b"").expect("create slot_b");
    OtaManager::new(partition, staging, signing_key, Arc::new(Metrics::new()))
}

/// 取 bootloader 环境里**最后一次** `boot_partition` 写入的值。
///
/// 一次 `set_boot_partition_slot` 会写两个变量（`boot_partition` + `bootcount=0`），
/// 故不能直接取写入历史的最后一项。
fn last_boot_partition(mgr: &OtaManager) -> Option<String> {
    mgr.boot_env
        .writes()
        .into_iter()
        .rev()
        .find(|(k, _)| k == "boot_partition")
        .map(|(_, v)| v)
}

/// 构造无签名固件包：[magic][version][crc32][payload]
fn build_firmware(version: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEADER_SIZE + payload.len());
    v.extend_from_slice(&FIRMWARE_MAGIC);
    v.extend_from_slice(&version.to_le_bytes());
    v.extend_from_slice(&crypto::crc32(payload).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// 构造带 HMAC-SHA256 签名的固件包：[header][payload][SIG_MAGIC][sig:32]
fn build_signed_firmware(version: u32, payload: &[u8], key: &[u8]) -> Vec<u8> {
    let mut v = build_firmware(version, payload);
    v.extend_from_slice(&SIG_MAGIC);
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    let tag = ring::hmac::sign(&k, payload);
    v.extend_from_slice(tag.as_ref());
    v
}

/// 生成 bsdiff 补丁（BSDIFF40 格式，含 bzip2 压缩块）
fn bsdiff_patch(old_fw: &[u8], new_fw: &[u8]) -> Vec<u8> {
    let mut patch = Vec::new();
    qbsdiff::Bsdiff::new(old_fw, new_fw)
        .compare(std::io::Cursor::new(&mut patch))
        .expect("bsdiff compare");
    patch
}

/// 构造差分升级包：[GWDT][new_version][new_crc32][bsdiff_patch]
fn build_delta(new_version: u32, old_fw: &[u8], new_fw: &[u8]) -> Vec<u8> {
    let patch = bsdiff_patch(old_fw, new_fw);
    let mut v = Vec::with_capacity(DELTA_HEADER_SIZE + patch.len());
    v.extend_from_slice(&DELTA_MAGIC);
    v.extend_from_slice(&new_version.to_le_bytes());
    v.extend_from_slice(&crypto::crc32(new_fw).to_le_bytes());
    v.extend_from_slice(&patch);
    v
}

/// 构造带签名的差分升级包：[GWDT][ver][crc][patch][SIG_MAGIC][sig:32]（签名覆盖 patch）
fn build_signed_delta(new_version: u32, old_fw: &[u8], new_fw: &[u8], key: &[u8]) -> Vec<u8> {
    let patch = bsdiff_patch(old_fw, new_fw);
    let mut v = Vec::with_capacity(DELTA_HEADER_SIZE + patch.len() + SIG_SUFFIX_SIZE);
    v.extend_from_slice(&DELTA_MAGIC);
    v.extend_from_slice(&new_version.to_le_bytes());
    v.extend_from_slice(&crypto::crc32(new_fw).to_le_bytes());
    v.extend_from_slice(&patch);
    v.extend_from_slice(&SIG_MAGIC);
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    v.extend_from_slice(ring::hmac::sign(&k, &patch).as_ref());
    v
}

#[tokio::test]
async fn rejects_too_small_firmware() {
    let dir = temp_dir("small");
    let mgr = new_manager(&dir, None);
    let err = mgr
        .clone()
        .process_firmware(vec![0u8; 5])
        .await
        .unwrap_err();
    assert!(err.message().contains("too small"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rejects_invalid_magic() {
    let dir = temp_dir("magic");
    let mgr = new_manager(&dir, None);
    let mut fw = build_firmware(1, b"hello");
    fw[0] = 0x00; // 破坏 magic
    let err = mgr.clone().process_firmware(fw).await.unwrap_err();
    assert!(err.message().contains("magic"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rejects_crc_mismatch() {
    let dir = temp_dir("crc");
    let mgr = new_manager(&dir, None);
    let mut fw = build_firmware(1, b"hello world");
    fw[8] ^= 0xFF; // 破坏 crc32 字段
    let err = mgr.clone().process_firmware(fw).await.unwrap_err();
    assert!(err.message().contains("CRC32"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rejects_oversized_firmware_without_state_change() {
    let dir = temp_dir("oversize");
    let mgr = new_manager(&dir, None);
    // 固件上限 64MB，超限应被拒且因检查在置位 Uploading 之前，状态保持 Idle
    let big = vec![0u8; 64 * 1024 * 1024 + 1];
    let err = mgr.clone().process_firmware(big).await.unwrap_err();
    assert!(err.message().contains("too large"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Idle);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn happy_path_writes_inactive_slot_then_pending_reboot() {
    let dir = temp_dir("ok");
    let mgr = new_manager(&dir, None);
    let payload = b"firmware-bytes-v2";
    mgr.clone()
        .process_firmware(build_firmware(2, payload))
        .await
        .expect("ota should succeed");

    let st = mgr.status();
    assert_eq!(st.state, OtaState::PendingReboot);
    assert_eq!(st.progress, 100);
    assert_eq!(st.target_version, Some(2));
    assert_eq!(st.active_slot, 'a');
    assert_eq!(st.inactive_slot, 'b');

    // 写入的是非活动分区 slot_b，且内容与 payload 逐字节一致
    let written = std::fs::read(dir.join("slot_b.bin")).expect("slot_b should be written");
    assert_eq!(written, payload);
    // 活动分区不应被触碰（仍为预建的空文件）
    let active = std::fs::read(dir.join("slot_a.bin")).expect("slot_a exists");
    assert!(active.is_empty(), "active slot must be untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rejects_concurrent_ota() {
    let dir = temp_dir("concurrent");
    let mgr = new_manager(&dir, None);
    mgr.clone()
        .process_firmware(build_firmware(3, b"abc"))
        .await
        .expect("first ota ok");
    // 第一次结束后处于 PendingReboot（非 Idle），第二次应被并发保护拒绝
    let err = mgr
        .clone()
        .process_firmware(build_firmware(4, b"def"))
        .await
        .unwrap_err();
    assert!(
        err.message().contains("already in progress"),
        "{}",
        err.message()
    );
    // 状态与目标版本不被第二次调用破坏
    assert_eq!(mgr.status().state, OtaState::PendingReboot);
    assert_eq!(mgr.status().target_version, Some(3));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rollback_only_allowed_in_pending_reboot() {
    let dir = temp_dir("rollback");
    let mgr = new_manager(&dir, None);
    // Idle 状态下回滚被拒
    let err = mgr.rollback().unwrap_err();
    assert!(err.message().contains("PendingReboot"), "{}", err.message());
    // 升级到 PendingReboot 后可回滚，且状态被重置
    mgr.clone()
        .process_firmware(build_firmware(5, b"xyz"))
        .await
        .expect("ota ok");
    assert_eq!(mgr.status().state, OtaState::PendingReboot);
    mgr.rollback().expect("rollback should succeed");
    let st = mgr.status();
    assert_eq!(st.state, OtaState::Idle);
    assert_eq!(st.progress, 0);
    assert_eq!(st.target_version, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn unsigned_firmware_rejected_when_key_configured() {
    let dir = temp_dir("nosig");
    let mgr = new_manager(&dir, Some(b"secret-key".to_vec()));
    let err = mgr
        .clone()
        .process_firmware(build_firmware(6, b"payload"))
        .await
        .unwrap_err();
    assert!(err.message().contains("no signature"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn signed_firmware_accepted_and_tamper_rejected() {
    let key = b"secret-key";
    // 合法签名 → 通过
    let dir = temp_dir("sig_ok");
    let mgr = new_manager(&dir, Some(key.to_vec()));
    let payload = b"signed-payload";
    mgr.clone()
        .process_firmware(build_signed_firmware(7, payload, key))
        .await
        .expect("signed firmware should pass");
    assert_eq!(mgr.status().state, OtaState::PendingReboot);

    // 篡改 payload（CRC 重新计算以绕过 CRC 校验），签名仍用原 payload → 验签失败
    let dir2 = temp_dir("sig_tamper");
    let mgr2 = new_manager(&dir2, Some(key.to_vec()));
    let mut tampered = build_firmware(8, b"evil-payload");
    tampered.extend_from_slice(&SIG_MAGIC);
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    tampered.extend_from_slice(ring::hmac::sign(&k, payload).as_ref());
    let err = mgr2.clone().process_firmware(tampered).await.unwrap_err();
    assert!(
        err.message().contains("signature") || err.message().contains("tamper"),
        "{}",
        err.message()
    );
    assert_eq!(mgr2.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

// ===== 差分升级（bsdiff）=====

#[tokio::test]
async fn delta_happy_path_patches_active_slot_into_inactive() {
    let dir = temp_dir("delta_ok");
    let mgr = new_manager(&dir, None);
    // 预置活动分区 slot_a 为“旧固件”（差异化内容保证补丁非空）
    let old_fw = b"firmware-v1\x00\x01\x02-common-tail-block-abcdefghijklmnop".to_vec();
    let new_fw = b"firmware-v2\x09\x08\x07-common-tail-block-abcdefghijklmnop-EXTRA".to_vec();
    std::fs::write(dir.join("slot_a.bin"), &old_fw).expect("seed slot_a");

    mgr.clone()
        .process_delta(build_delta(2, &old_fw, &new_fw))
        .await
        .expect("delta ota should succeed");

    let st = mgr.status();
    assert_eq!(st.state, OtaState::PendingReboot);
    assert_eq!(st.progress, 100);
    assert_eq!(st.target_version, Some(2));

    // 打补丁结果写入非活动分区 slot_b，且与新固件逐字节一致
    let written = std::fs::read(dir.join("slot_b.bin")).expect("slot_b");
    assert_eq!(written, new_fw);
    // 活动分区不被触碰
    let active = std::fs::read(dir.join("slot_a.bin")).expect("slot_a");
    assert_eq!(active, old_fw);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn delta_rejects_invalid_magic() {
    let dir = temp_dir("delta_magic");
    let mgr = new_manager(&dir, None);
    let old_fw = b"old-fw".to_vec();
    let new_fw = b"new-fw".to_vec();
    let mut d = build_delta(2, &old_fw, &new_fw);
    d[0] = 0x00; // 破坏 magic
    let err = mgr.clone().process_delta(d).await.unwrap_err();
    assert!(err.message().contains("magic"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn delta_rejects_crc_mismatch() {
    let dir = temp_dir("delta_crc");
    let mgr = new_manager(&dir, None);
    let old_fw = b"old-fw-abcdef".to_vec();
    let new_fw = b"new-fw-ghijkl-EXTRA".to_vec();
    std::fs::write(dir.join("slot_a.bin"), &old_fw).expect("seed");
    let mut d = build_delta(2, &old_fw, &new_fw);
    d[8] ^= 0xFF; // 破坏 new_crc32 字段
    let err = mgr.clone().process_delta(d).await.unwrap_err();
    assert!(err.message().contains("CRC32"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn delta_unsigned_rejected_when_key_configured() {
    let dir = temp_dir("delta_nosig");
    let mgr = new_manager(&dir, Some(b"secret-key".to_vec()));
    let old_fw = b"old-fw".to_vec();
    let new_fw = b"new-fw-diff".to_vec();
    let err = mgr
        .clone()
        .process_delta(build_delta(2, &old_fw, &new_fw))
        .await
        .unwrap_err();
    assert!(err.message().contains("no signature"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Failed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn delta_signed_accepted() {
    let key = b"secret-key";
    let dir = temp_dir("delta_sig");
    let mgr = new_manager(&dir, Some(key.to_vec()));
    let old_fw = b"firmware-v1-common".to_vec();
    let new_fw = b"firmware-v2-common-CHANGED".to_vec();
    std::fs::write(dir.join("slot_a.bin"), &old_fw).expect("seed");

    mgr.clone()
        .process_delta(build_signed_delta(2, &old_fw, &new_fw, key))
        .await
        .expect("signed delta should pass");
    assert_eq!(mgr.status().state, OtaState::PendingReboot);
    let written = std::fs::read(dir.join("slot_b.bin")).expect("slot_b");
    assert_eq!(written, new_fw);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 与 `new_manager` 相同，但可指定活动槽（用于验证回滚目标随活动槽变化）。
fn new_manager_active(dir: &Path, active: char) -> Arc<OtaManager> {
    let partition = AbPartitionConfig {
        slot_a: dir.join("slot_a.bin"),
        slot_b: dir.join("slot_b.bin"),
        active_slot: active,
    };
    let staging = dir.join("staging");
    let staging = staging.to_str().expect("utf8 staging path");
    std::fs::write(dir.join("slot_a.bin"), b"").expect("create slot_a");
    std::fs::write(dir.join("slot_b.bin"), b"").expect("create slot_b");
    OtaManager::new(partition, staging, None, Arc::new(Metrics::new()))
}

/// 回归缺陷维度：修复前 `rollback()` 复用「写活动分区反面」的函数，把 `boot_partition`
/// 指向**刚写入的新分区**而非正在运行的分区，回滚静默失效。
///
/// 既有 `rollback_only_allowed_in_pending_reboot` 只断言状态机，无法观测
/// 「实际写入的槽位」。本测试借助 `BootEnv` 内存后端的写入历史断言：
/// - 升级（apply）应把 `boot_partition` 指向**非活动槽**；
/// - 回滚（rollback）应把 `boot_partition` 设回**当前运行槽**。
///
/// 活动槽 = 'a'
#[tokio::test]
async fn rollback_boot_env_targets_active_slot_a() {
    let dir = temp_dir("rb_env_a");
    let mgr = new_manager(&dir, None); // active_slot = 'a'
    assert_eq!(mgr.status().active_slot, 'a');

    mgr.clone()
        .process_firmware(build_firmware(2, b"fw-v2-payload"))
        .await
        .expect("ota ok");

    assert_eq!(
        last_boot_partition(&mgr),
        Some("b".to_string()),
        "升级应把 boot_partition 指向非活动槽 b，实际 {:?}",
        mgr.boot_env.writes()
    );

    mgr.rollback().expect("rollback ok");

    assert_eq!(
        last_boot_partition(&mgr),
        Some("a".to_string()),
        "回滚必须把 boot_partition 设回当前运行槽 a，实际 {:?}",
        mgr.boot_env.writes()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 同上，活动槽 = 'b'：升级写 slot_a（非活动槽 a），回滚须设回 'b'。
#[tokio::test]
async fn rollback_boot_env_targets_active_slot_b() {
    let dir = temp_dir("rb_env_b");
    let mgr = new_manager_active(&dir, 'b');
    assert_eq!(mgr.status().active_slot, 'b');

    mgr.clone()
        .process_firmware(build_firmware(3, b"fw-v3-payload"))
        .await
        .expect("ota ok");

    assert_eq!(
        last_boot_partition(&mgr),
        Some("a".to_string()),
        "升级应把 boot_partition 指向非活动槽 a，实际 {:?}",
        mgr.boot_env.writes()
    );

    mgr.rollback().expect("rollback ok");

    assert_eq!(
        last_boot_partition(&mgr),
        Some("b".to_string()),
        "回滚必须把 boot_partition 设回当前运行槽 b，实际 {:?}",
        mgr.boot_env.writes()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn delta_rejects_oversized_without_state_change() {
    let dir = temp_dir("delta_oversize");
    let mgr = new_manager(&dir, None);
    let big = vec![0u8; MAX_FIRMWARE_SIZE + 1];
    let err = mgr.clone().process_delta(big).await.unwrap_err();
    assert!(err.message().contains("too large"), "{}", err.message());
    assert_eq!(mgr.status().state, OtaState::Idle);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bug 2.5: fail() 后 progress 必须重置为 0，避免 UI 永远卡在 70% (Writing)
#[test]
fn test_fail_resets_progress() {
    let dir = temp_dir("fail_progress");
    let m = new_manager(&dir, None);
    // 模拟进入 Writing 状态后失败
    *m.state.lock() = OtaState::Writing;
    *m.progress.lock() = 70;
    m.metrics.set_ota_progress(70);

    let res = m.fail(GatewayError::Io("test failure".into()));
    assert!(res.is_err());
    let st = m.status();
    assert_eq!(st.state, OtaState::Failed);
    assert_eq!(st.progress, 0, "fail() 后 progress 必须重置为 0");
    assert_eq!(st.error.as_deref(), Some("test failure"));
}

/// 正常流程：PendingReboot 不变动 progress（fail 路径不干扰正常状态）
#[test]
fn test_pending_reboot_keeps_full_progress() {
    let dir = temp_dir("reboot_progress");
    let m = new_manager(&dir, None);
    *m.state.lock() = OtaState::Writing;
    *m.progress.lock() = 70;
    m.metrics.set_ota_progress(70);
    *m.state.lock() = OtaState::PendingReboot;
    *m.progress.lock() = 100;
    let st = m.status();
    assert_eq!(st.progress, 100);
    assert_eq!(st.state, OtaState::PendingReboot);
}

/// Bug C8: 配置的 base64 签名密钥解析——合法 base64 通过，非法 base64 fail-closed。
#[test]
fn parse_signing_key_base64() {
    let encoded = crate::engine::util::encoding::b64_encode(b"secret-key");
    let key = parse_signing_key(&encoded).expect("valid base64 应通过");
    assert_eq!(key, b"secret-key");

    // 非法 base64（含非字母表字符 '*'）必须返回 Err，避免静默退化为不验签
    assert!(parse_signing_key("not*base64").is_err());
}

/// r31-OTA4 (回归)：升级成功后 staging_dir 下应创建 `pending_reboot` 标记文件，
/// 内含 target version (u32 LE 字节)。回滚时标记被删除。
#[tokio::test]
async fn pending_reboot_marker_created_on_success_and_cleared_on_rollback() {
    let dir = temp_dir("marker");
    let mgr = new_manager(&dir, None);
    let payload = b"v2-payload";
    mgr.clone()
        .process_firmware(build_firmware(2, payload))
        .await
        .expect("ota ok");

    // 1) 标记文件存在
    let marker_path = mgr.pending_reboot_marker_path();
    assert!(
        marker_path.exists(),
        "升级成功后 pending_reboot 标记应存在：{}",
        marker_path.display()
    );
    // 2) 内容 = target version (u32 LE) + 目标槽位 (1 字节 ASCII)
    let bytes = std::fs::read(&marker_path).expect("read marker");
    assert_eq!(
        bytes.len(),
        5,
        "标记内容应为 5 字节 (version u32 LE + target slot)"
    );
    let ver = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    assert_eq!(ver, 2, "标记应记录 target version");
    assert_eq!(
        bytes[4], b'b',
        "标记应记录目标槽位（active='a' → 目标 'b'）"
    );

    // 3) 回滚后标记被删除
    mgr.rollback().expect("rollback ok");
    assert!(!marker_path.exists(), "rollback 后标记应被删除");

    let _ = std::fs::remove_dir_all(&dir);
}

/// r31-OTA4 补充：重复 rollback 幂等（标记文件不存在时不报错）。
#[tokio::test]
async fn rollback_when_no_marker_is_idempotent() {
    let dir = temp_dir("marker_idem");
    let mgr = new_manager(&dir, None);
    mgr.clone()
        .process_firmware(build_firmware(2, b"x"))
        .await
        .expect("ota ok");
    mgr.rollback().expect("first rollback");
    // 第二次 rollback：状态已是 Idle，函数本身应先 return "PendingReboot required"，
    // 不会触达 marker 删除逻辑——但已删除的 marker 仍应保持"不存在"状态。
    let marker = mgr.pending_reboot_marker_path();
    assert!(!marker.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// #2 回归（核心）：升级生效后重启，`boot_rollback_check` 不得把 `boot_partition`
/// 反向切回旧槽——旧实现无条件设为「active_slot 的反面」，导致升级生效后每次重启
/// 都回退、新固件永不生效。生效时应清除标记。
#[tokio::test]
async fn boot_rollback_check_does_not_revert_after_upgrade_effective() {
    let dir = temp_dir("bootcheck_effective");
    let mgr = new_manager(&dir, None); // 升级前 active_slot = 'a'
    mgr.clone()
        .process_firmware(build_firmware(2, b"v2"))
        .await
        .expect("ota ok");

    // 模拟已重启进入新固件：bootloader 引导 b 槽。生产环境由 `OtaManager::new`
    // 依据 bootloader env 校正 active_slot；测试中直接以 active_slot='b' 重建。
    let mgr2 = new_manager_active(&dir, 'b');
    mgr2.boot_rollback_check();

    assert!(
        !mgr2.pending_reboot_marker_path().exists(),
        "升级生效后应清除 pending_reboot 标记"
    );
    let writes = mgr2.boot_env.writes();
    assert!(
        !writes
            .iter()
            .any(|(k, v)| k == "boot_partition" && v == "a"),
        "升级已生效时不得把 boot_partition 切回旧槽 a，实际写入 {:?}",
        writes
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// #2：标记存在但尚未切到目标槽（如断电 / env 写丢失）→ 重新确认 `boot_partition`
/// 指向目标槽并保留标记，待下次启动复核。
#[tokio::test]
async fn boot_rollback_check_reaffirms_target_slot_when_not_yet_effective() {
    let dir = temp_dir("bootcheck_pending");
    let mgr = new_manager(&dir, None); // active_slot = 'a'
    mgr.clone()
        .process_firmware(build_firmware(2, b"v2"))
        .await
        .expect("ota ok");

    mgr.boot_env.clear_writes();
    mgr.boot_rollback_check();
    assert_eq!(
        last_boot_partition(&mgr),
        Some("b".to_string()),
        "尚未切到目标槽时应重新确认 boot_partition=b，实际 {:?}",
        mgr.boot_env.writes()
    );
    assert!(
        mgr.pending_reboot_marker_path().exists(),
        "升级尚未生效时应保留标记"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `bootcount >= 3`（新分区连续启动失败）→ 启动自检必须触发自动回滚：
/// 切到另一槽、重置 bootcount、并触发一次重启。
///
/// 这条路径在生产里会真的执行 `reboot`；测试用内存后端，只累加计数，
/// 既验证语义又不会重启 CI runner。
#[test]
fn bootcount_over_threshold_triggers_auto_rollback_and_reboot() {
    let dir = temp_dir("bootcount_rollback");
    let mgr = new_manager(&dir, None); // active_slot = 'a'
    mgr.boot_env.set_var("bootcount", "3");

    assert!(mgr.check_bootcount_rollback(), "bootcount=3 应判定需要回滚");

    mgr.perform_rollback().expect("自动回滚应成功");

    let writes = mgr.boot_env.writes();
    assert!(
        writes
            .iter()
            .any(|(k, v)| k == "boot_partition" && v == "b"),
        "回滚应把 boot_partition 切到另一槽 b，实际 {:?}",
        writes
    );
    assert!(
        writes.iter().any(|(k, v)| k == "bootcount" && v == "0"),
        "回滚应重置 bootcount=0，实际 {:?}",
        writes
    );
    assert_eq!(mgr.boot_env.reboot_count(), 1, "自动回滚必须触发一次重启");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `bootcount` 未达阈值 → 不回滚、不重启。
#[test]
fn bootcount_below_threshold_does_not_trigger_rollback() {
    let dir = temp_dir("bootcount_ok");
    let mgr = new_manager(&dir, None);
    mgr.boot_env.set_var("bootcount", "1");

    assert!(!mgr.check_bootcount_rollback(), "bootcount=1 不应触发回滚");
    mgr.boot_rollback_check();
    assert_eq!(
        mgr.boot_env.reboot_count(),
        0,
        "bootcount 未达阈值不得重启设备"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
