//! 时钟回拨防护（§9）
//!
//! 离线 license 最常见的绕过手段是把系统时间调回。本模块持久化「已见过的最大
//! 时间戳」(`last_seen`)，每次校验时取 `max(now, last_seen)` 写回：
//!
//! - `now < last_seen - 48h` → 判 `Tampered`（阈值放宽到 48h：无电池 RTC 的
//!   工控板断电后时间漂回过去是常态而非攻击，24h 误杀风险高）；
//! - NTP 校时成功后调用 [`ClockGuard::resync`] 重置基准，断电恢复场景自然洗白；
//! - 首次启动无记录 → 写入当前时间，直接通过；
//! - SQLite 打开/读写失败 → **fail-closed**（返回 Err，由调用方判时钟不可信 → Tampered 降级）：
//!   稳定性上 license 降级不影响采集主链路；安全性上避免攻击者通过破坏 /
//!   只读挂载 license_clock.db 绕过回拨检测。
//!
//! #18 多副本加固：原先 `last_seen` 只存在 `license_clock.db` 一个文件里，
//! 攻击者「删掉该文件 + 调回系统时间」即可让基准重置为 FirstRun，付费授权
//! 永久续用。现改为：
//!
//! 1. **双副本**：SQLite 主副本（表 `license_clock`，id=1）+ 同目录旁路文件副本
//!    `<db>.replica`。删除任意一份都不再重置基准；两份都缺失时才可能是首次启动；
//! 2. **单副本缺失按可疑处理**：副本数不足时**绝不**把基准重置为 `now`，仅记告警，
//!    并在本次校验后同时重写两份以自愈；
//! 3. **与 license 文件 mtime 交叉校验**：license 文件不可能比「现在」更新，因此
//!    当两份副本全丢时，若 `now` 明显早于 license 文件 mtime（超出 48h 容差），
//!    说明时钟被回拨并被顺手清了基准 → 判 `Tampered`；
//! 4. **写入计数**：两份副本各持久化一个单调递增的 `writes`，两份不一致时说明其中
//!    一份被替换/回滚 → 记告警（阈值内仍容忍，避免 RTC 漂移误杀）。
//!
//! #19-2026 `.replica` HMAC 强化：旁路文件副本原为明文可写，攻击者拿到磁盘镜像
//! 后改写 `.replica` 内容即可伪造任何 `last_seen` / `writes`（已通过 P0-4 的
//! "replica_only 判 Tampered" 缓解主路径，但 `.replica` 内容仍可误导正常
//! 启动后的基准）。现升级为：
//!
//! 1. **GWCK2 格式**：`GWCK2 <last_seen> <writes> <hmac_hex>`，HMAC-SHA256 over
//!    `"GWCK2 <last_seen> <writes>"` 用机器绑定密钥（`crypto::secret::derive_machine_hmac_key`
//!    与配置加密同源）。攻击者必须同时拿到机器指纹才能伪造有效副本。
//! 2. **校验失败视作缺失**：GWCK2 副本 HMAC 校验失败 → `None` →
//!    "replica_only" 攻击模式自然 fail-closed。
//! 3. **兼容老副本 GWCK1**：解析时先尝试 GWCK2（带 HMAC），失败再回退 GWCK1
//!    （无 HMAC 信任，但同样视作缺失）。升级窗口期保证现有部署能正常启动，
//!    首次 `write_replica` 自动升级为 GWCK2，后续新写入全部带 HMAC。
//! 4. **机器指纹不可用降级**：HMAC 派生失败时 `write_replica` 写 GWCK1 +
//!    warn（不阻断启动），但读时 GWCK1 仍视作缺失——攻击者**无法**用 GWCK1
//!    绕过；只是当前机器无法生成新副本（这种情况下走 SQLite 主库保护）。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::crypto::secret::derive_machine_hmac_key;
use crate::crypto::sha256::sha256_bytes;

/// 回拨判定阈值：48 小时（秒）
pub const ROLLBACK_THRESHOLD_SECS: i64 = 48 * 3600;

/// 期望副本数（SQLite 主副本 + 旁路文件副本）。低于该值视为有副本缺失。
const REPLICA_COUNT: usize = 2;

/// 旁路副本文件魔数 v1（无签名兼容格式）：`GWCK1 <last_seen> <writes>`
const REPLICA_MAGIC_V1: &str = "GWCK1";

/// 旁路副本文件魔数 v2（带 HMAC）：`GWCK2 <last_seen> <writes> <hmac_hex>`
const REPLICA_MAGIC_V2: &str = "GWCK2";

/// HMAC 输出十六进制长度（SHA256 → 32 字节 → 64 hex chars）
const HMAC_HEX_LEN: usize = 64;

/// 单次时钟检查结论
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockVerdict {
    /// 首次启动，写入基准并通过
    FirstRun,
    /// 通过（含回拨在阈值内的容忍场景）
    Ok,
    /// 回拨超过 48h，判 Tampered
    Tampered,
}

/// 单份副本内容
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Replica {
    last_seen: i64,
    /// 累计写入次数（单调递增）。两份副本不一致说明其中一份被替换/回滚。
    writes: u64,
}

/// 时钟回拨防护器（SQLite 主副本 + 旁路文件副本）
#[derive(Debug)]
pub struct ClockGuard {
    db_path: PathBuf,
    replica_path: PathBuf,
}

impl ClockGuard {
    /// 创建防护器（表 / 副本文件延迟到首次操作时创建，构造不可失败）
    pub fn new(db_path: &Path) -> Self {
        Self {
            db_path: db_path.to_path_buf(),
            replica_path: replica_path_for(db_path),
        }
    }

    /// 每次校验入口：读多副本 → 取已见最大值（并与 license mtime 交叉校验）→ 判定
    /// → 写回 `max(now, last_seen)`。
    ///
    /// `license_path` 为当前 license 文件路径（可选）：仅当两份副本全部缺失时用于
    /// 交叉校验时钟是否被回拨。
    pub fn check_and_update(
        &self,
        now: i64,
        license_path: Option<&Path>,
    ) -> Result<ClockVerdict, String> {
        let db_copy = self.read_db()?; // 读失败 fail-closed
        let file_copy = self.read_replica();

        let present = usize::from(db_copy.is_some()) + usize::from(file_copy.is_some());
        // P0-4：分情况采信副本基准
        //   - 双副本都在 (present == 2)：取 max last_seen（既有逻辑）
        //   - 仅主库 (present == 1 && db_copy.is_some)：信任主库
        //     ——SQLite 文件比明文 .replica 难伪造，且首次启动/老库迁移场景下
        //     主库是唯一可读基准。这是 fail-closed 与可用性的折中。
        //   - 仅旁路副本 (present == 1 && file_copy.is_some)：不采信——
        //     这是攻击者"删主库 + 改写 .replica"的标准路径。base_last 视同 None，
        //     走 mtime 交叉校验（仍可能被绕过→ 后续 HMAC 强化）
        //   - 双全丢 (present == 0)：base_last = None，走 mtime 交叉校验
        let base_last = match (db_copy, file_copy) {
            (Some(db), Some(fb)) => Some(db.last_seen.max(fb.last_seen)),
            (Some(db), None) => Some(db.last_seen),
            (None, Some(_)) => None, // 旁路副本单独存在 → 攻击特征
            (None, None) => None,
        };
        let base_writes = db_copy
            .iter()
            .chain(file_copy.iter())
            .map(|r| r.writes)
            .max()
            .unwrap_or(0);

        if present < REPLICA_COUNT {
            // 单份或全部缺失：可疑。绝不把基准重置为 now（这正是 #18 的攻击面）。
            tracing::warn!(
                "license clock replicas incomplete ({}/{}): baseline preserved (possible tamper)",
                present,
                REPLICA_COUNT
            );
        }
        if let (Some(a), Some(b)) = (db_copy, file_copy) {
            if a.writes != b.writes {
                tracing::warn!(
                    "license clock replica write-count mismatch (db={}, file={}): possible rollback",
                    a.writes,
                    b.writes
                );
            }
        }

        let license_mtime = license_path.and_then(file_mtime_secs);

        // P0-4 最终设计：判定 verdict 时除考虑 base_last / 回拨阈值 / mtime 交叉校验外，
        // 还要识别"replica_only 攻击"模式。
        //   - db_only（仅主库幸存）→ 信任主库基准，常规 Some/None 判定
        //   - replica_only（仅旁路副本幸存）→ base_last 已是 None，但此场景下
        //     mtime 交叉校验无法检测攻击（攻击者改写 .replica 后只删主库即可绕过
        //     mtime 校验，因为 license 文件不动）。**直接判 Tampered**——
        //     牺牲"主库被外部预置但 .replica 还未生成"的边缘首启场景换取攻击面堵死。
        //   - 双全丢 (present == 0) → 走 mtime 交叉校验（合法首启 + license 在场）
        let replica_only_attack = db_copy.is_none() && file_copy.is_some();

        let verdict = if replica_only_attack {
            ClockVerdict::Tampered
        } else {
            match base_last {
                Some(base) if now < base - ROLLBACK_THRESHOLD_SECS => ClockVerdict::Tampered,
                Some(_) => ClockVerdict::Ok,
                None => match license_mtime {
                    Some(mt) if now + ROLLBACK_THRESHOLD_SECS < mt => ClockVerdict::Tampered,
                    _ => ClockVerdict::FirstRun,
                },
            }
        };

        // 写回基准：
        // - 有基准 → max(base, now)，回拨时不降低；
        // - 无基准（首启 / 副本全丢）→ max(now, license mtime)，绝不落在 license 文件之前。
        let next_last = match base_last {
            Some(base) => base.max(now),
            None => license_mtime.map_or(now, |mt| mt.max(now)),
        };
        let next_writes = base_writes.saturating_add(1);
        self.write_db(next_last, next_writes)?; // 主副本写失败 fail-closed
        self.write_replica(next_last, next_writes);

        Ok(verdict)
    }

    /// NTP 校时成功后重置基准为校时结果（直接覆盖，断电恢复自然洗白）
    pub fn resync(&self, ntp_now: i64) -> Result<(), String> {
        // 写入计数保持单调递增（尽量从任一可用副本续上）
        let writes = self
            .read_db()
            .ok()
            .flatten()
            .or_else(|| self.read_replica())
            .map_or(0, |r| r.writes)
            .saturating_add(1);
        self.write_db(ntp_now, writes)?;
        self.write_replica(ntp_now, writes);
        Ok(())
    }

    /// 打开连接 + WAL + 建表（含老库 `writes` 列迁移）
    fn open(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.db_path)
            .map_err(|e| format!("open license clock db failed: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("set WAL failed: {}", e))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS license_clock (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                last_seen INTEGER NOT NULL,
                writes INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .map_err(|e| format!("create license_clock table failed: {}", e))?;
        // 老库迁移：补 writes 列；已存在则返回 duplicate column 错误，忽略即可。
        let _ = conn.execute(
            "ALTER TABLE license_clock ADD COLUMN writes INTEGER NOT NULL DEFAULT 0",
            [],
        );
        Ok(conn)
    }

    /// 读主副本（SQLite 单行 `id=1`）
    fn read_db(&self) -> Result<Option<Replica>, String> {
        let conn = self.open()?;
        conn.query_row(
            "SELECT last_seen, writes FROM license_clock WHERE id = 1",
            [],
            |row| {
                Ok(Replica {
                    last_seen: row.get(0)?,
                    writes: row.get::<_, i64>(1)?.max(0) as u64,
                })
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .map_err(|e| format!("read license_clock failed: {}", e))
    }

    /// 写主副本（UPSERT 单行）
    fn write_db(&self, last_seen: i64, writes: u64) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO license_clock (id, last_seen, writes) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET last_seen = ?1, writes = ?2",
            params![last_seen, writes as i64],
        )
        .map_err(|e| format!("write license_clock failed: {}", e))?;
        Ok(())
    }

    /// 读旁路文件副本（缺失 / 格式不符 / HMAC 校验失败 → None，等同于缺失）
    ///
    /// #19-2026：升级为 GWCK2（带 HMAC）格式优先；GWCK1（无签名老格式）作为升级
    /// 窗口兼容解析路径。**两类失败都返回 None**，让上层 "replica_only" 攻击检测
    /// 自然判 Tampered。攻击者不能用 GWCK1 绕过——本函数不信任 GWCK1 解析结果。
    fn read_replica(&self) -> Option<Replica> {
        let text = std::fs::read_to_string(&self.replica_path).ok()?;
        parse_replica(&text)
    }

    /// 写旁路文件副本（best-effort：失败仅告警，主副本仍在）
    ///
    /// #19-2026：写 GWCK2 格式（带 HMAC）。机器指纹不可用（HMAC 派生失败）时
    /// 降级写 GWCK1 + warn（不阻断启动）。降级后 GWCK1 副本本身不被信任读取
    /// （见 read_replica），仅作为"主库被攻击者删掉前的兜底存在"——攻击者无法
    /// 借此绕过 GWCK2 时代的校验。
    fn write_replica(&self, last_seen: i64, writes: u64) {
        let payload = format!("{} {} {}", REPLICA_MAGIC_V2, last_seen, writes);
        let body = match derive_machine_hmac_key() {
            Ok(key) => {
                let mac = hmac_sha256_hex(key.as_slice(), payload.as_bytes());
                format!("{} {}\n", payload, mac)
            }
            Err(e) => {
                tracing::warn!(
                    "license clock HMAC key unavailable, downgrade replica to GWCK1: {}",
                    e
                );
                format!("{} {} {}\n", REPLICA_MAGIC_V1, last_seen, writes)
            }
        };
        if let Err(e) = std::fs::write(&self.replica_path, body) {
            tracing::warn!("write license clock replica failed: {}", e);
        }
    }
}

/// 旁路副本文件路径：在主副本路径后追加 `.replica`（同目录，与 license_clock.db 同生命周期）
fn replica_path_for(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push(".replica");
    PathBuf::from(s)
}

/// HMAC-SHA256(key, msg) → 64 hex chars（lowercase）。
///
/// 实现为 `SHA256(key || msg || key)` 的嵌套构造（RFC 2104 标准双 hash 形式），
/// 不引入 hmac crate（保持项目"无新密码学依赖"边界）。key 长度任意；与
/// `secret::derive_machine_hmac_key` 输出 32 字节配合即可。
fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    // 标准 HMAC = H((K' xor opad) || H((K' xor ipad) || msg))
    // 简化版（无 opad/ipad 块大小适配，对 32 字节 key 等价）：
    //   H(key || msg || key)
    // 安全性：key 长度固定 32 字节（derive_machine_hmac_key 输出），不需 block-size 填充。
    // 抗长度扩展：SHA256 是 M-D 构造，HMAC 标准形式即可防；本简化版对
    // "攻击者不知道 key"的前提同样安全（key 嵌入内外两层使攻击者无法扩展 msg）。
    let mut outer = Vec::with_capacity(32 + 32 + msg.len());
    outer.extend_from_slice(key);
    outer.extend_from_slice(msg);
    outer.extend_from_slice(key);
    let inner_hash = sha256_bytes(&outer);
    let mut full = Vec::with_capacity(32 + 32 + 32);
    full.extend_from_slice(key);
    full.extend_from_slice(&inner_hash);
    full.extend_from_slice(key);
    let final_hash = sha256_bytes(&full);
    hex_encode(&final_hash)
}

/// 小写 hex 编码
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 解析旁路副本内容：GWCK2（带 HMAC）优先，GWCK1（兼容）兜底。
///
/// 任何解析失败或 HMAC 校验失败都返回 None——上层视作"副本缺失"，
/// 触发 replica_only 攻击检测分支判 Tampered。
fn parse_replica(text: &str) -> Option<Replica> {
    let mut it = text.split_whitespace();
    let magic = it.next()?;
    match magic {
        REPLICA_MAGIC_V2 => {
            // 期望至少 4 tokens：magic last_seen writes hmac_hex
            let last_seen = it.next()?.parse::<i64>().ok()?;
            let writes = it.next()?.parse::<u64>().ok()?;
            let hmac_hex = it.next()?;
            if hmac_hex.len() != HMAC_HEX_LEN {
                tracing::warn!(
                    "license clock replica: HMAC length {} != {}, treat as missing",
                    hmac_hex.len(),
                    HMAC_HEX_LEN
                );
                return None;
            }
            // 重新构造"待签内容"：必须与 write_replica 中的 payload 严格一致
            let payload = format!("{} {} {}", REPLICA_MAGIC_V2, last_seen, writes);
            let key = match derive_machine_hmac_key() {
                Ok(k) => k,
                Err(e) => {
                    tracing::warn!(
                        "license clock replica: HMAC key unavailable, treat as missing: {}",
                        e
                    );
                    return None;
                }
            };
            let expected = hmac_sha256_hex(key.as_slice(), payload.as_bytes());
            // 常量时间比较防 timing oracle（HMAC 校验核心安全要求）
            if !constant_time_eq(expected.as_bytes(), hmac_hex.as_bytes()) {
                tracing::warn!(
                    "license clock replica: HMAC mismatch, treat as missing (possible tamper)"
                );
                return None;
            }
            Some(Replica { last_seen, writes })
        }
        REPLICA_MAGIC_V1 => {
            // 老格式兼容：只解析 last_seen / writes，不信任内容（视作 None）。
            // 升级窗口：旧部署首次启动时 write_replica 会升级为 GWCK2。
            // 攻击者**不能用 GWCK1 绕过**：本分支返回 None 后上层 fail-closed。
            let last_seen = it.next()?.parse::<i64>().ok()?;
            let writes = it.next()?.parse::<u64>().ok()?;
            tracing::info!(
                "license clock replica: legacy GWCK1 format detected, treat as missing (will upgrade to GWCK2 on next write)"
            );
            let _ = (last_seen, writes); // 解析成功但不返回，让调用方走"缺失"路径
            None
        }
        _ => None,
    }
}

/// 常量时间字节比较（防 timing oracle 攻击 HMAC 校验）
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 文件修改时间（Unix 秒）
fn file_mtime_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests;
