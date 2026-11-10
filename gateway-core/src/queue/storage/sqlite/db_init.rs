//! 队列数据库初始化（建表、索引、备份、完整性检查）

use crate::engine::metrics::Metrics;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use rusqlite::Connection;

use crate::engine::error::{GatewayError, GatewayResult};

use super::wal::backup_db_consistent;

/// 是否允许在数据库打不开 / 完整性校验失败时自动「恢复备份或重建空库」。
///
/// 默认**关闭**：`integrity_check` 失败有两种成因——掉电/磁盘损坏，或 keyfile 被替换。
/// 旧实现无差别地「有备份就覆盖主库、无备份就删库重建」，换 key 时会静默清空全部
/// 未交付数据，现场只表现为「重启后队列莫名清空」，无法察觉也无法追溯。
/// 现在默认拒绝启动并保留全部文件；运维确认库确实不可用后，显式设置
/// `GATEWAY_QUEUE_REBUILD_ON_CORRUPTION=1` 才恢复旧的自愈行为。
fn rebuild_on_corruption_allowed() -> bool {
    matches!(
        std::env::var("GATEWAY_QUEUE_REBUILD_ON_CORRUPTION").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// 构造「拒绝启动」的错误：带上库、密钥路径与处置指引，便于现场区分
/// 「keyfile 被换了」和「掉电把库写坏了」这两种完全不同的故障。
fn refuse_unusable_db(db_path: &Path, reason: &str) -> GatewayError {
    // 未启用 encryption 时不存在 keyfile（`ensure_key` 返回空串），提示里不出现它。
    #[cfg(feature = "encryption")]
    let key_hint = format!(
        "，keyfile: {}",
        crate::queue::core::keyfile_path(db_path).display()
    );
    #[cfg(not(feature = "encryption"))]
    let key_hint = String::new();

    GatewayError::Queue(format!(
        "队列数据库无法使用：{reason}（库: {}{key_hint}）。\
         已按数据保护策略拒绝启动，未删除、未覆盖任何文件。\
         请先确认 keyfile 是否被替换；若确认库已不可用，设置 \
         GATEWAY_QUEUE_REBUILD_ON_CORRUPTION=1 重启，程序将尝试从备份恢复，\
         无备份时重建空库（未交付数据会丢失）",
        db_path.display()
    ))
}

/// 初始化队列数据库：打开/恢复连接、设置 WAL、建表/索引、校准计数器
///
/// 包含：
/// - SQLite 连接打开 + integrity_check（失败时默认拒绝启动，见
///   [`rebuild_on_corruption_allowed`]）
/// - WAL 模式设置
/// - data_queue 表 + schema 版本校验与迁移链（`PRAGMA user_version`）
/// - in_flight 恢复 + 队列深度计数器校准
/// - 索引创建 + 降采样聚合表
pub(super) fn init_queue_db(
    db_path: &Path,
    backup_path: &Path,
    depth: &AtomicI64,
    rows_total: &AtomicI64,
    key_hex: &str,
    metrics: &Arc<Metrics>,
) -> GatewayResult<Connection> {
    let mut conn = match open_with_key(db_path, key_hex) {
        Ok(conn) => {
            let integrity_ok: bool = conn
                .query_row("PRAGMA integrity_check", [], |row| {
                    let result: String = row.get(0)?;
                    Ok(result == "ok")
                })
                .unwrap_or(false);
            if integrity_ok {
                conn
            } else if rebuild_on_corruption_allowed() {
                tracing::error!("SQLite integrity check failed, attempting restore from backup...");
                drop(conn);
                if backup_path.exists() {
                    std::fs::copy(backup_path, db_path).map_err(|e| {
                        GatewayError::Queue(format!("restore from backup failed: {}", e))
                    })?;
                    tracing::warn!("restored data_queue.db from backup");
                    open_with_key(db_path, key_hex).map_err(|e| {
                        GatewayError::Queue(format!("reopen after restore failed: {}", e))
                    })?
                } else {
                    tracing::error!("no backup available, recreating empty database");
                    let _ = std::fs::remove_file(db_path);
                    open_with_key(db_path, key_hex)
                        .map_err(|e| GatewayError::Queue(format!("recreate db failed: {}", e)))?
                }
            } else {
                return Err(refuse_unusable_db(
                    db_path,
                    "PRAGMA integrity_check 未通过（库损坏或 keyfile 与库不匹配）",
                ));
            }
        }
        Err(e) => {
            if rebuild_on_corruption_allowed() {
                tracing::error!(
                    "open sqlite failed ({}), attempting restore from backup...",
                    e
                );
                if backup_path.exists() {
                    std::fs::copy(backup_path, db_path).map_err(|e2| {
                        GatewayError::Queue(format!("restore from backup failed: {}", e2))
                    })?;
                    tracing::warn!("restored data_queue.db from backup after open failure");
                    open_with_key(db_path, key_hex).map_err(|e2| {
                        GatewayError::Queue(format!("reopen after restore failed: {}", e2))
                    })?
                } else {
                    return Err(GatewayError::Queue(format!(
                        "open sqlite failed and no backup: {}",
                        e
                    )));
                }
            } else {
                return Err(refuse_unusable_db(
                    db_path,
                    &format!("打开数据库失败（库损坏或 keyfile 与库不匹配）: {e}"),
                ));
            }
        }
    };

    // r42-Q3: 启动备份必须先 checkpoint（TRUNCATE）把 -wal 中已 commit 但未落主库
    // 的行刷回主库，再复制。旧实现直接 fs::copy 只复制主库文件：上一生命周期若异常
    // 退出、-wal 残留已提交数据，备份会漏掉这些行并覆盖掉上一轮维护备份 → 之后从
    // 备份恢复将丢失最近一次运行的数据。与维护路径（maintenance 第 5 步）保持同序。
    let _ = backup_db_consistent(&mut conn, db_path, backup_path);

    // 空闲页回收：retention 只做 DELETE，SQLite 默认不把释放的页还给文件系统，
    // 主库文件因此只增不减。开启 `auto_vacuum=INCREMENTAL` 后，maintenance 周期
    // 调用 `PRAGMA incremental_vacuum(N)` 就能分小批把空闲页交还 OS。
    // 放在 WAL 设置与建表之前：已有库需要一次 VACUUM 才会把该设置写进库头。
    crate::support::sqlite_maintenance::ensure_incremental_auto_vacuum(&conn, "队列主库");

    // WAL 调优：综合设置 journal_mode + synchronous + busy_timeout + cache_size + wal_autocheckpoint + mmap_size
    // - journal_mode=WAL: 写入不阻塞读，崩溃恢复由 WAL 保证
    // - synchronous=FULL: 计费场景必须保证已 commit 数据不丢——NORMAL 下 WAL 不 fsync，
    //   嵌入式设备掉电会丢最近未 checkpoint 的计费行；FULL 在每事务 commit 时 fsync 写盘，
    //   性能开销对秒级采集网关可忽略，符合产品「稳定性第一」原则。
    // - busy_timeout=5000ms: 多连接争用时等待 5s 而非立即返回 SQLITE_BUSY（避免读池与主连接冲突）
    // - cache_size=-65536: 64MB 页缓存（负值=KB 单位），减少磁盘 I/O
    // - wal_autocheckpoint=1000: 每 1000 页（约 4MB）自动 checkpoint，防止 WAL 增长过大
    // - mmap_size=268435456: 256MB 内存映射，提升读取性能（仅在 64 位系统生效）
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; \
         PRAGMA synchronous=FULL; \
         PRAGMA busy_timeout=5000; \
         PRAGMA cache_size=-65536; \
         PRAGMA wal_autocheckpoint=1000; \
         PRAGMA mmap_size=268435456;",
    )
    .map_err(|e| GatewayError::Queue(format!("set WAL mode failed: {}", e)))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS data_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                point_id TEXT NOT NULL,
                device_id TEXT NOT NULL DEFAULT '',
                value REAL NOT NULL,
                raw_value REAL NOT NULL,
                raw_type TEXT NOT NULL,
                quality TEXT NOT NULL,
                quality_reason TEXT NOT NULL DEFAULT '',
                timestamp INTEGER NOT NULL,
                source_ts INTEGER NOT NULL DEFAULT 0,
                stale_count INTEGER NOT NULL DEFAULT 0,
                in_flight INTEGER NOT NULL DEFAULT 0,
                dequeued_at INTEGER NOT NULL DEFAULT 0,
                delivered INTEGER NOT NULL DEFAULT 0,
                value_text TEXT
            )",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create table failed: {}", e)))?;

    // schema 版本校验 + 迁移链。
    //
    // 旧实现只是在这里平铺一串 `ensure_column`：每次升级都要靠「逐列探测」判断
    // 走到哪一步，既无法表达「改列 / 迁数据」这类迁移，也无法证明升级链完整，
    // 更挡不住用旧程序打开新库（回滚部署）这种会静默丢数据的操作。
    //
    // 现在统一由 `PRAGMA user_version` 记录版本，迁移链逐步推进并在每一步成功后
    // 落盘版本号（中途崩溃时下次启动从已完成的那步继续）。
    migrate_schema(&conn)?;

    // Bug r08-DB2 (fix): 加 WHERE delivered = 0 限定只重置"未送达"的行。
    // 旧实现更新全表（包括 delivered=1 等 ack 等待 retention 删除的行），
    // 功能无影响（delivered=1 行不出队）但语义混乱，干扰后续读代码者。
    conn.execute(
        "UPDATE data_queue SET in_flight = 0 WHERE delivered = 0",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("recover in_flight failed: {}", e)))?;

    // P3: 启动时校准内存队列深度计数器（一次性 COUNT，之后纯内存维护）。
    //
    // 必须只统计 `delivered = 0` 的行：运行时 depth 的增减就是
    // 「enqueue_batch 加 inserted / ack_batch 减 ids_len」，语义恒等于
    // 「尚未送达（含 in_flight=1）的行数」。P0-3 之后 ack 不再删行，而是置
    // delivered=1 并保留到 retention 回收；若此处按 COUNT(*) 记账，重启后 depth
    // 会把已 ack 的行重新算进来 → 长期虚高且不可自愈（retention 删行不扣 depth）：
    // 指标失真、drain_and_wait 可能永不返回、enforce_queue_limit 误删仍在
    // 保留期内的历史行。
    //
    // P1-2：同一次扫描顺带取**全表行数**，作为 `rows_total` 计数器的初值。
    // 之后容量判定全部走内存计数器（`total > max_rows` 才算超限），入队热路径
    // 不再出现 `SELECT COUNT(*)` 全表扫描。
    ensure_dedup_unique_index(&conn)?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_ts ON data_queue(timestamp)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create index failed: {}", e)))?;

    // 复合索引：覆盖 history 查询 (point_id 等值 + timestamp 范围 + DESC 排序)
    // 原 idx_queue_ts(timestamp) 仅支持时间范围扫描，多点位场景退化为近全表扫描
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_point_ts ON data_queue(point_id, timestamp DESC)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create point_ts index failed: {}", e)))?;

    // P1-1：投递状态索引。(in_flight, delivered) 前缀让「未送达计数」与
    // 「stale in_flight 回收（WHERE in_flight = 1）」都走索引，避免每次入队
    // 触发的恢复 UPDATE 变成全表扫描。
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_delivery ON data_queue(in_flight, delivered)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create delivery index failed: {}", e)))?;

    // P1-1：容量淘汰专用**部分索引**。淘汰语句是
    // `WHERE in_flight = 0 ORDER BY timestamp ASC, id ASC LIMIT n`：
    // 普通 (in_flight, delivered) 索引无法支撑 timestamp 排序，超限时会对全部
    // 非在途行做一次排序（O(n log n)）；部分索引把「非在途」直接固化进索引，
    // 天然按 timestamp（末位隐含 rowid）有序 → 淘汰只需读 k 行。
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_evict ON data_queue(timestamp) WHERE in_flight = 0",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create evict index failed: {}", e)))?;

    // snapshot 上报模式的折叠索引：按 (device_id, point_id) 分组取 MAX(id)，
    // 再把非最新行标记 delivered。部分索引只覆盖未送达行，体积远小于全表索引；
    // stream 模式下不会被使用，仅带来少量写入开销。
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_snapshot ON data_queue(device_id, point_id, id) WHERE delivered = 0",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create snapshot index failed: {}", e)))?;

    // 计数器校准放在建索引与去重**之后**：去重会删行，先算会导致终值虚高。
    let (initial_total, initial_unsent): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(delivered = 0), 0) FROM data_queue",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0));
    depth.store(initial_unsent, Ordering::Relaxed);
    rows_total.store(initial_total, Ordering::Relaxed);
    metrics.set_queue_depth(initial_unsent);

    // 降采样聚合表（F5）与它的索引由 v2 → v3 迁移步骤创建，不在这里重复建。

    Ok(conn)
}

/// 确保 `(point_id, timestamp)` 唯一索引存在（P2-1）。
///
/// 唯一约束让「重复入队」在存储层被拒绝，堵住 persist 重试、上游重复投递、
/// 同一批点被两条投递路径各写一次所导致的重复行。落库侧配合
/// `INSERT ... ON CONFLICT(point_id, timestamp) DO NOTHING`：重复行静默跳过，
/// 不算错误（不会触发逐行回退的告警风暴）。
///
/// 升级兼容：旧版本没有该约束，历史库里可能已存在重复行，直接建唯一索引会
/// 失败并使网关拒绝启动。因此仅在索引**尚不存在**时做一次去重（每组保留最早
/// 的 id），随后建索引。索引已存在说明此前已完成迁移，直接返回，不重复扫描。
fn ensure_dedup_unique_index(conn: &Connection) -> GatewayResult<()> {
    const INDEX: &str = "idx_queue_dedup";
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
            rusqlite::params![INDEX],
            |row| row.get(0),
        )
        .map_err(|e| GatewayError::Queue(format!("probe dedup index failed: {e}")))?;
    if exists > 0 {
        return Ok(());
    }

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| GatewayError::Queue(format!("begin dedup tx failed: {e}")))?;
    // 「保留每组最早一行」：重复删行必须一次性完成，否则后续建索引仍会失败。
    let removed = tx
        .execute(
            "DELETE FROM data_queue WHERE id NOT IN (
                 SELECT MIN(id) FROM data_queue GROUP BY point_id, timestamp
             )",
            [],
        )
        .map_err(|e| GatewayError::Queue(format!("dedup existing rows failed: {e}")))?;
    tx.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_queue_dedup ON data_queue(point_id, timestamp)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create dedup unique index failed: {e}")))?;
    tx.commit()
        .map_err(|e| GatewayError::Queue(format!("commit dedup tx failed: {e}")))?;

    if removed > 0 {
        tracing::warn!(
            "队列库升级：删除 {} 行重复 (point_id, timestamp) 记录后建立唯一索引",
            removed
        );
    } else {
        tracing::info!("队列库升级：已建立 (point_id, timestamp) 唯一索引");
    }
    Ok(())
}

mod migrate;

use migrate::migrate_schema;

#[cfg(test)]
use migrate::SCHEMA_VERSION;

/// 打开 SQLite 连接并设置 SQLCipher 加密密钥
///
/// 必须在任何其他 PRAGMA 或查询之前执行 `PRAGMA key`。
fn open_with_key(db_path: &Path, key_hex: &str) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(db_path)?;
    if !key_hex.is_empty() {
        conn.pragma_update(None, "key", key_hex)?;
    }
    Ok(conn)
}

#[cfg(test)]
#[path = "db_init_migration_tests.rs"]
mod migration_tests;
