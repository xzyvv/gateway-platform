//! 队列 retention 清理：删除超过保留期的聚合历史记录
//!
//! v2.0 Step 5：保留天数由 license 配额 `storage_days` 驱动（bootstrap 的
//! 配额对账任务周期推送），不再硬编码。`None` = 不限，跳过时间维清理。
//!
//! 聚合表是「时间界 + 行数界 + 磁盘水位」三重约束：
//! - 时间界：`window_start < now - storage_days`（配额驱动）；
//! - 行数界：超过按设备容量自适应的上限后，按 `window_start` 从旧到新淘汰
//!   （点位很多时分钟桶仍可能把库撑大，时间界内也可能超量）；
//! - 水位联动：磁盘预警（Warn）时保留天数收紧到 14 天、上限减半；
//!   严重/阻断（Critical/Blocking）时清空聚合历史，只保原始队列——
//!   原始队列是断点续传的唯一依据，聚合历史可以随时由新数据重建。
//!
//! 注意：data_queue（原始待上报表）的 FIFO 截断由 maintenance 第 6 步的
//! `cleanup_old_records` 执行——它是 depth 计数器的同步点，raw 删除必须
//! 走它，避免两处删除导致计数漂移。本函数只负责聚合历史表。
//!
//! 行数上界的实际淘汰逻辑在 `aggregated_trim`（拆出以守住单文件行数预算）。

use super::aggregated_trim::enforce_aggregated_row_cap;
use crate::engine::model::DataPoint;
use crate::queue::core::limit::MAX_AGGREGATED_ROWS;
use rusqlite::{params, Connection};

/// `DiskLevel` 的 `u8` 编码（Ok=0 / Warn=1 / Critical=2 / Blocking=3）。
const DISK_LEVEL_WARN: i64 = 1;
const DISK_LEVEL_CRITICAL: i64 = 2;

/// 聚合表占用磁盘的预算比例（2%）。
///
/// 取 2% 而不是固定行数：2GB eMMC 与 64GB 工控机的合理历史深度不同，
/// 固定 500 万行在 2GB 设备上会直接吃掉四分之一的盘。
const AGGREGATED_DISK_SHARE: f64 = 0.02;

/// 单行聚合记录的估算占用（值 + 时间戳 + UNIQUE 索引，约 96B）。
const AGGREGATED_BYTES_PER_ROW: f64 = 96.0;

/// 自适应上限的下界：再小就失去降采样历史的意义。
const MIN_AGGREGATED_ROWS: i64 = 200_000;

/// 容量探测失败时的保守上限（比固定 500 万行更保守）。
const FALLBACK_AGGREGATED_ROWS: i64 = 1_000_000;

/// 磁盘预警时聚合历史的保留天数上限（覆盖更长的 license 配额）。
const WARN_AGGREGATED_DAYS: u32 = 14;

/// S-4: retention 清理——删除超过保留期的聚合历史记录 + 行数界 + 水位联动
///
/// - `storage_days`：license 配额中的历史保留天数（`None` = 不限）；
/// - `disk_level`：当前磁盘水位档位（`DiskLevel as u8`）；
/// - `disk_total_bytes`：库所在分区总容量（0 表示探测失败，用保守上限）。
pub(crate) fn run_retention_cleanup(
    conn: &mut Connection,
    storage_days: Option<u32>,
    disk_level: i64,
    disk_total_bytes: u64,
) {
    // 严重/阻断：聚合历史全部丢弃，把空间让给原始队列（断点续传依赖）。
    if disk_level >= DISK_LEVEL_CRITICAL {
        match conn.execute("DELETE FROM data_queue_aggregated", []) {
            Ok(n) if n > 0 => tracing::warn!(
                "retention: 磁盘水位 level={}，已清空聚合历史 {} 行，仅保留原始队列",
                disk_level,
                n
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("retention: 清空聚合历史失败: {}", e),
        }
        return;
    }

    let days = effective_aggregated_days(storage_days, disk_level);
    let now = DataPoint::now();
    if let Some(days) = days {
        let cutoff = now - (days as i64) * 24 * 3600 * 1000;
        match conn.execute(
            "DELETE FROM data_queue_aggregated WHERE window_start < ?1",
            params![cutoff],
        ) {
            Ok(n) => {
                if n > 0 {
                    tracing::info!("retention: deleted {} aggregated rows (>{}d)", n, days);
                }
            }
            Err(e) => tracing::warn!("retention cleanup aggregated failed: {}", e),
        }
    }
    // 时间界之外再压一层行数界：`storage_days = None`（不限）时也要生效。
    enforce_aggregated_row_cap(conn, policy_aggregated_cap(disk_level, disk_total_bytes));
}

/// 水位联动后的聚合保留天数：预警时收紧到 [`WARN_AGGREGATED_DAYS`]。
fn effective_aggregated_days(storage_days: Option<u32>, disk_level: i64) -> Option<u32> {
    if disk_level < DISK_LEVEL_WARN {
        return storage_days;
    }
    Some(match storage_days {
        Some(days) => days.min(WARN_AGGREGATED_DAYS),
        None => WARN_AGGREGATED_DAYS,
    })
}

/// 当前水位下的聚合表行数上限：按分区容量取 2%，预警时减半。
fn policy_aggregated_cap(disk_level: i64, disk_total_bytes: u64) -> i64 {
    let cap = adaptive_aggregated_row_cap(disk_total_bytes);
    if disk_level >= DISK_LEVEL_WARN {
        (cap / 2).max(MIN_AGGREGATED_ROWS)
    } else {
        cap
    }
}

/// 按分区容量自适应计算聚合表行数上界。
///
/// 行数 ≈ 总容量 × [`AGGREGATED_DISK_SHARE`] ÷ [`AGGREGATED_BYTES_PER_ROW`]，
/// 再钳在 `[MIN_AGGREGATED_ROWS, MAX_AGGREGATED_ROWS]`：
/// 2GB ≈ 44 万行、4GB ≈ 89 万行、≥ 32GB 触顶 500 万行。
pub(crate) fn adaptive_aggregated_row_cap(disk_total_bytes: u64) -> i64 {
    if disk_total_bytes == 0 {
        return FALLBACK_AGGREGATED_ROWS;
    }
    let rows = (disk_total_bytes as f64 * AGGREGATED_DISK_SHARE / AGGREGATED_BYTES_PER_ROW) as i64;
    rows.clamp(MIN_AGGREGATED_ROWS, MAX_AGGREGATED_ROWS)
}

/// 回收空闲页（需库已启用 `auto_vacuum=INCREMENTAL`，见 `configure_auto_vacuum`）。
///
/// 每次只回收 `pages` 页（4KB/页），由 maintenance 周期调用：既能把 retention
/// 删除释放的页交还文件系统，又不会像全量 `VACUUM` 那样长时间锁表。
/// `auto_vacuum=NONE` 时该语句是空操作，不会报错。
pub(crate) fn run_incremental_vacuum(conn: &Connection, pages: i64) {
    if let Err(e) = conn.execute_batch(&format!("PRAGMA incremental_vacuum({});", pages)) {
        tracing::warn!("incremental_vacuum 失败: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGG_SCHEMA: &str = "CREATE TABLE data_queue_aggregated (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            point_id TEXT NOT NULL,
            window_start INTEGER NOT NULL,
            avg_value REAL NOT NULL,
            min_value REAL NOT NULL,
            max_value REAL NOT NULL,
            sample_count INTEGER NOT NULL,
            UNIQUE(point_id, window_start)
        );
        CREATE INDEX idx_agg_window ON data_queue_aggregated(window_start);";

    fn seeded(n: i64) -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(AGG_SCHEMA).expect("create agg table");
        for i in 0..n {
            conn.execute(
                "INSERT INTO data_queue_aggregated
                 (point_id, window_start, avg_value, min_value, max_value, sample_count)
                 VALUES (?1, ?2, 0.0, 0.0, 0.0, 1)",
                params![format!("p{}", i % 3), 1_700_000_000_000_i64 + i * 60_000],
            )
            .expect("insert agg row");
        }
        conn
    }

    fn count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM data_queue_aggregated", [], |r| {
            r.get(0)
        })
        .expect("count")
    }

    /// 容量越大上限越高，且始终钳在 [MIN, MAX] 内。
    #[test]
    fn adaptive_cap_scales_with_disk_capacity() {
        let gb = 1024u64 * 1024 * 1024;
        assert_eq!(adaptive_aggregated_row_cap(0), FALLBACK_AGGREGATED_ROWS);
        let cap_2g = adaptive_aggregated_row_cap(2 * gb);
        assert!(
            (400_000..=470_000).contains(&cap_2g),
            "2GB 上限异常: {}",
            cap_2g
        );
        let cap_4g = adaptive_aggregated_row_cap(4 * gb);
        assert!(cap_4g > cap_2g, "4GB 上限必须高于 2GB");
        assert_eq!(adaptive_aggregated_row_cap(32 * gb), MAX_AGGREGATED_ROWS);
        assert_eq!(
            adaptive_aggregated_row_cap(64 * 1024 * 1024),
            MIN_AGGREGATED_ROWS
        );
    }

    /// 水位预警：保留天数收紧到 14 天、行数上限减半。
    #[test]
    fn warn_level_tightens_days_and_cap() {
        assert_eq!(effective_aggregated_days(Some(30), 0), Some(30));
        assert_eq!(effective_aggregated_days(None, 0), None);
        assert_eq!(
            effective_aggregated_days(Some(30), DISK_LEVEL_WARN),
            Some(14)
        );
        assert_eq!(effective_aggregated_days(Some(7), DISK_LEVEL_WARN), Some(7));
        assert_eq!(effective_aggregated_days(None, DISK_LEVEL_WARN), Some(14));

        let gb = 1024u64 * 1024 * 1024;
        let normal = policy_aggregated_cap(0, 4 * gb);
        let warn = policy_aggregated_cap(DISK_LEVEL_WARN, 4 * gb);
        assert_eq!(warn, normal / 2, "预警时行数上限必须减半");
    }

    /// 严重/阻断水位：清空聚合历史，只保原始队列。
    #[test]
    fn critical_level_drops_all_aggregated_rows() {
        let mut conn = seeded(10);
        run_retention_cleanup(&mut conn, Some(30), DISK_LEVEL_CRITICAL, 0);
        assert_eq!(count(&conn), 0, "严重水位必须清空聚合历史");
    }

    /// 正常水位且无配额：不误删未超界的行。
    #[test]
    fn normal_level_keeps_recent_rows_without_quota() {
        let mut conn = seeded(6);
        run_retention_cleanup(&mut conn, None, 0, 0);
        assert_eq!(count(&conn), 6);
    }
}
