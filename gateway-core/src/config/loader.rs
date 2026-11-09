//! 配置加载器：加载 / 保存 / 校验配置文件
//! Q18: 新增 reload() 方法用于热重载入口。
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::GatewayConfig;
use crate::engine::error::{GatewayError, GatewayResult};

/// Bug T13: 同进程内 tmp 文件名唯一性计数器。
///
/// 原实现仅用 `<pid>.<nanos>` 区分 tmp 文件，但 `SystemTime::now()` 的分辨率受限
/// （Windows 系统时钟粒度约 100ns 甚至更粗），并发 `save()` 完全可能取到相同纳秒，
/// 导致 tmp 路径冲突：线程 A 先 rename 走该 tmp，线程 B 随后 rename 同一路径即失败
/// （`save()` 返回 Err）；且 Windows 下 `remove_file` 对已打开文件的删除还会因共享
/// 冲突失败，残留 tmp 文件。附加单调递增计数器后，进程内 tmp 名必不重复。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 配置加载器。
#[derive(Clone)]
pub struct ConfigLoader {
    path: PathBuf,
}

impl ConfigLoader {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into() }
    }

    /// 返回配置文件路径（供 bootstrap 持有 path 用于热重载）
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 清理配置目录中历史残留的临时文件（`<file>.tmp.<pid>.<nanos>.<seq>`）。
    ///
    /// 只在 [`Self::load`] 启动路径调用：正常写入要么 rename 成正式文件，
    /// 要么在失败分支删除 tmp；只有进程被 kill/断电才会留下残骸。
    /// 按固定前缀匹配，不清扫目录里的其它文件。
    fn cleanup_stale_tmp_files(&self) {
        let (Some(parent), Some(file_name)) = (
            self.path.parent(),
            self.path.file_name().and_then(|n| n.to_str()),
        ) else {
            return;
        };
        let prefix = format!("{}.tmp.", file_name);
        let Ok(entries) = std::fs::read_dir(parent) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(&prefix) {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => tracing::warn!("清理残留临时配置 {}（上次异常退出）", name),
                Err(e) => tracing::warn!("清理残留临时配置 {} 失败: {}", name, e),
            }
        }
    }

    /// 加载配置（含 校验流程）
    pub fn load(&self) -> GatewayResult<GatewayConfig> {
        // 启动加载时清扫上次异常退出（写 tmp 后、rename 前被 kill/断电）
        // 留下的 `<file>.tmp.*`。此刻本进程尚未开始写入，删除是安全的；
        // 正常写入路径在 rename / 失败分支里已自行清理，无需周期扫描。
        self.cleanup_stale_tmp_files();
        match self.try_load() {
            Ok(cfg) => Ok(cfg),
            Err(e) => {
                if !self.path.exists() {
                    tracing::warn!("config file not found, generating default: {}", e);
                    let cfg = GatewayConfig::default();
                    // Bug 66 修复：原代码 `let _ = self.save(&cfg)` 静默忽略保存错误。
                    // 若磁盘满 / 权限不足，文件仍未生成，下次启动再次进入 fallback 分支。
                    // 改为检查返回值：保存失败时返回 Err，让运维知晓首次配置未落盘。
                    if let Err(save_err) = self.save(&cfg) {
                        tracing::error!(
                            "config: failed to write default config to {}: {}",
                            self.path.display(),
                            save_err
                        );
                        return Err(save_err);
                    }
                    return Ok(cfg);
                }
                tracing::error!("config file invalid, load failed: {}", e);
                let bak = self.path.with_extension("json.bak");
                // Bug 115: 仅在 bak 不存在时复制——保留历史失败现场便于回溯。
                // 原实现 `let _ = fs::copy(...)` 每次启动失败都覆盖 bak。
                // 导致连续失败时只能看到最后一次状态。
                if !bak.exists() {
                    if let Err(copy_err) = fs::copy(&self.path, &bak) {
                        tracing::warn!("config: backup copy failed: {}", copy_err);
                    } else {
                        tracing::info!("config: original config backed up to {}", bak.display());
                    }
                } else {
                    tracing::warn!(
                        "config: backup {} already exists, keeping historical failure",
                        bak.display()
                    );
                }
                Err(e)
            }
        }
    }

    fn try_load(&self) -> GatewayResult<GatewayConfig> {
        if !self.path.exists() {
            return Err(GatewayError::Config("config file not found".to_string()));
        }
        // 配置分片：展开 include 引用，合并为单个 JSON Value。
        // expand_includes 内部处理文件读取、JSON 解析、递归 include 展开。
        // 返回的 Value 已移除 include 字段，可直接做 schema 校验 + 反序列化。
        let value = super::shard::expand_includes(&self.path)
            .map_err(|e| GatewayError::Config(format!("expand config includes failed: {}", e)))?;
        if let Err(errs) = super::schema::validate_value(&value) {
            return Err(GatewayError::Config(format!(
                "schema validate config failed: {}",
                errs.join("; ")
            )));
        }
        let mut cfg: GatewayConfig = serde_json::from_value(value)
            .map_err(|e| GatewayError::Config(format!("deserialize config failed: {}", e)))?;
        // Bug 177: migrate 现在返回 Result，未来版本写出的未知版本配置会报错并阻断加载。
        cfg.migrate().map_err(GatewayError::Config)?;
        // C-3: 解密失败直接阻断启动。旧实现仅 warn 保留原文 `enc:xxx`。
        // 下游模块（如 MQTT 客户端）拿到字面串当凭据用，导致"启动看似正常
        // 但链路失败且无任何告警。现在解密错误透传到顶层 load() 错误路径。
        crate::crypto::decrypt_config_secrets(&mut cfg)
            .map_err(|e| GatewayError::Config(format!("decrypt config secrets failed: {}", e)))?;
        sanitize_legacy_ratios(&mut cfg);
        cfg.validate_all().map_err(|errs| {
            GatewayError::Config(format!("validate config failed: {}", errs.join("; ")))
        })?;
        Ok(cfg)
    }

    /// 原子保存配置到文件。
    ///
    /// 流程：先写到 `<path>.tmp.<pid>.<nanos>.<seq>` → `sync_all` 刷盘 → `rename` 原子替换。
    /// 失败时清理 tmp。任一步骤失败都不会损坏现有 config.json，旧文件保持不变。
    ///
    /// 修复 Bug 116：原实现 `fs::write` 直接覆盖目标文件，写一半断电/磁盘满会损坏
    /// 现有配置。本实现保证要么写入完整新文件，要么旧文件完全不动。
    ///
    /// 并发安全：
    /// - 同进程多个 `save()` 并发：每个调用使用独立 tmp 文件名（pid + nanos + seq），
    ///   最后一个 rename 成功的版本生效。
    /// - 跨进程：依赖文件系统的 rename 原子性（POSIX 保证；Windows 同卷 NTFS 也保证）。
    pub fn save(&self, cfg: &GatewayConfig) -> GatewayResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                GatewayError::Config(format!(
                    "create config parent dir {} failed: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        // Bug C5: 内存中的 cfg 是 load()/reload() 解密后的明文，直接序列化会把
        // api_token / password 明文写回磁盘，`enc:` 保护在首次在线保存后即失效。
        // 在副本上重新加密后再落盘；加密失败则 fail-closed 拒绝写入，避免明文外泄。
        let mut to_write = cfg.clone();
        crate::crypto::encrypt_config_secrets(&mut to_write).map_err(|e| {
            GatewayError::Config(format!("encrypt config secrets before save failed: {}", e))
        })?;
        let json = serde_json::to_string_pretty(&to_write)
            .map_err(|e| GatewayError::Config(format!("serialize config failed: {}", e)))?;

        atomic_write(&self.path, json.as_bytes())
    }

    /// Q18: reload 从磁盘重新读取配置并校验，返回最新的 GatewayConfig。
    ///
    /// 与 `load()` 的区别：
    /// - `load()` 在文件不存在时会自动生成默认配置并保存。
    /// - `reload()` 假设文件已存在（首次启动后），更严格：
    ///   - 文件不存在 -> 返回 Err
    ///   - 解析/校验失败 -> 返回 Err（不静默回退）。
    ///
    /// Bug 185 (N08): `reload` 现在返回 `Result` 已经 fail-closed，但调用方（API
    /// 端点）在 reload 失败时只把错误透传出去，没有任何指引/审计/版本快照。
    /// 现场运维需要：
    ///   1. 知道 reload 失败次数 —— `Metrics::config_reload_rejected_total`（增量）
    ///   2. reload 失败时能回退 —— 调用方应在调用 `reload()` 前先保存一份
    ///      version_store 快照（见 config_apply.rs / API handler）。
    ///
    /// 本函数本身只做校验与装载；不直接访问 metrics（保持 ConfigLoader 不依赖
    /// 全局状态），指标与审计由调用方集中处理。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn reload(&self) -> GatewayResult<GatewayConfig> {
        if !self.path.exists() {
            return Err(GatewayError::Config(format!(
                "reload failed: config file not found at {}",
                self.path.display()
            )));
        }
        // 配置分片：展开 include 引用，合并为单个 JSON Value
        let value = super::shard::expand_includes(&self.path).map_err(|e| {
            GatewayError::Config(format!("reload expand config includes failed: {}", e))
        })?;
        if let Err(errs) = super::schema::validate_value(&value) {
            return Err(GatewayError::Config(format!(
                "reload schema validate config failed: {}",
                errs.join("; ")
            )));
        }
        let mut cfg: GatewayConfig = serde_json::from_value(value).map_err(|e| {
            GatewayError::Config(format!("reload deserialize config failed: {}", e))
        })?;
        // Bug 177: migrate 现在返回 Result，未来版本写出的未知版本配置会报错并阻断加载。
        cfg.migrate().map_err(GatewayError::Config)?;
        // C-3: 同 load()，reload 也必须 fail-fast on decrypt failure。
        crate::crypto::decrypt_config_secrets(&mut cfg).map_err(|e| {
            GatewayError::Config(format!("reload decrypt config secrets failed: {}", e))
        })?;
        sanitize_legacy_ratios(&mut cfg);
        cfg.validate_all().map_err(|errs| {
            GatewayError::Config(format!(
                "reload validate config failed: {}",
                errs.join("; ")
            ))
        })?;
        Ok(cfg)
    }
}

/// 存量配置的变比兜底：把「非正 / 非有限」变比归一化为「未配置」。
///
/// `validate_ratio` 是 fail-fast 的字段级校验。历史配置里若存有
/// `ct_ratio: 0`（旧界面常用 0 表示"无变比"），新增校验后会在
/// `validate_all()` 处直接阻断加载 —— 现场表现为**升级后网关起不来**。
/// 因此在校验之前把这类值降级为 `None`（语义等同「未配置变比」）并逐点留痕：
/// 网关照常启动，运维可从日志定位具体点位后手工修正。
///
/// 只作用于磁盘加载 / 热重载路径；Web 保存路径仍由 API 层严格拒绝 0 与负数，
/// 避免新配置继续写入非法值（fail-fast 只对"新输入"生效，不对"存量"判死刑）。
fn sanitize_legacy_ratios(cfg: &mut GatewayConfig) {
    for p in cfg.points.iter_mut() {
        for (name, slot) in [("ct_ratio", &mut p.ct_ratio), ("pt_ratio", &mut p.pt_ratio)] {
            if let Some(v) = *slot {
                if !v.is_finite() || v <= 0.0 {
                    tracing::warn!(
                        "point '{}' {}={} 非法（变比必须 > 0），已按「未配置变比」加载；请留空或填写正确变比",
                        p.id,
                        name,
                        v
                    );
                    *slot = None;
                }
            }
        }
    }
}

/// 原子写入：tmp + fsync + rename，失败清理。
///
/// 设计要点：
/// - 临时文件名带 nanos 时间戳 + pid，避免同进程并发保存冲突。
/// - 写入完成后 `sync_all()` 强制刷盘，保证 rename 后文件内容已落盘（防断电丢数据）。
/// - rename 在同一文件系统上是原子的（POSIX/NTFS），读者要么看到旧版本，要么看到完整新版本。
/// - Windows 上目标文件被并发读取 / 杀毒 / 索引器短暂持有句柄时，替换会返回
///   `ERROR_ACCESS_DENIED` / `ERROR_SHARING_VIOLATION`；这是瞬时失败，短暂重试即可（见 `rename_replace`）。
/// - rename 失败时清理 tmp（best-effort，清理失败仅 warn 不影响返回错误）。
fn atomic_write(target: &Path, data: &[u8]) -> GatewayResult<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let parent = target.parent().ok_or_else(|| {
        GatewayError::Config(format!(
            "config target path has no parent: {}",
            target.display()
        ))
    })?;

    // 构造唯一 tmp 文件名：<target_filename>.tmp.<pid>.<nanos>.<seq>
    //
    // Bug T13: 追加进程内单调递增序号。仅 `<pid>.<nanos>` 在并发 `save()` 下可能
    // 碰撞（见 TMP_SEQ 注释），导致 tmp 路径冲突、rename 失败或残留 tmp 文件。
    let file_name = target.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        GatewayError::Config(format!(
            "config target has invalid filename: {}",
            target.display()
        ))
    })?;
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!("{}.tmp.{}.{}.{}", file_name, pid, nanos, seq));

    // 1. 写入 tmp（写 + fsync 一起，任何步骤失败都清理 tmp）。
    let write_result: std::io::Result<()> = (|| {
        // Bug C5: config.json 含（加密后的）凭据，必须 0600。
        // r15-CFG1: 原实现 `File::create` 先以默认 0644 建档再 chmod 0600，
        // 两者之间存在竞态窗口（同机其它用户可短暂读到含凭据的临时文件）。
        // 改为**创建时即指定 0600**（OpenOptions::mode），窗口消失。
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp_path)?;
        f.write_all(data)?;
        // fsync：确保数据落盘再 rename，防止断电后看到 0 长度文件
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        // 写入失败：清理 tmp，保留旧目标文件不动。
        let _ = fs::remove_file(&tmp_path);
        return Err(GatewayError::Config(format!(
            "write config tmp {} failed: {}",
            tmp_path.display(),
            e
        )));
    }

    // 2. rename 原子替换
    if let Err(e) = rename_replace(&tmp_path, target) {
        // rename 失败：清理 tmp，保留旧目标文件。
        let _ = fs::remove_file(&tmp_path);
        return Err(GatewayError::Config(format!(
            "rename {} -> {} failed: {}",
            tmp_path.display(),
            target.display(),
            e
        )));
    }

    Ok(())
}

/// rename 替换目标文件。
///
/// POSIX 下 rename 覆盖是原子的，直接调用即可。Windows 的 `MoveFileEx` 在目标文件
/// 被其它句柄打开（并发读取、杀毒扫描、索引器）时会瞬时返回
/// `ERROR_ACCESS_DENIED (5)` / `ERROR_SHARING_VIOLATION (32)`；并发 `save()` 时
/// 读者刚打开旧文件就可能撞上。这类失败重试几十毫秒即可恢复，不应直接上报给
/// 现场造成「保存失败」。其它错误（路径不存在、跨卷等）不重试。
fn rename_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::io::ErrorKind;
        const ATTEMPTS: u32 = 6;
        let mut last_error = None;
        for attempt in 1..=ATTEMPTS {
            match fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let retryable = matches!(
                        e.kind(),
                        ErrorKind::PermissionDenied | ErrorKind::WouldBlock
                    );
                    if !retryable || attempt == ATTEMPTS {
                        return Err(e);
                    }
                    last_error = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| std::io::Error::other("rename retry exhausted")))
    }

    #[cfg(not(windows))]
    {
        fs::rename(from, to)
    }
}

#[cfg(test)]
mod tests;
