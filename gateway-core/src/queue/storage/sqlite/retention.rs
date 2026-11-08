//! 队列 retention 清理：删除超过保留期的聚合历史记录
//!
//! v2.0 Step 5：保留天数由 license 配额 `storage_days` 驱动（bootstrap 的
//! 配额对账任务周期推送），不再硬编码。`None` = 不限，跳过时间维清理。
//!
//! 聚合表是「时间界 + 行数界」双约束：
//! - 时间界：`window_start < now - storage_days`（配额驱动）；
//! - 行数界：超过 [`MAX_AGGREGATED_ROWS`] 后按 `window_start` 从旧到新淘汰
//!   （点位很多时分钟桶仍可能把库撑大，时间界内也可能超量）。
//!
//! 注意：data_queue（原始待上报表）的 FIFO 截断由 maintenance 第 6 步的
//! `cleanup_old_records` 执行——它是 depth 计数器的同步点，raw 删除必须
//! 走它，避免两处删除导致计数漂移。本函数只负责聚合历史表。

use crate::engine::model::DataPoint;
use crate::queue::core::limit::MAX_AGGREGATED_ROWS;
use rusqlite::{params, Connection};

/// 行数上界淘汰的单次删除行数。
///
/// 分块删除避免一条 DELETE 长时间持写锁（嵌入式 ARM 上大事务会造成采集侧
/// 入队等待）。每次 retention 最多执行 [`AGGREGATED_TRIM_ROUNDS`] 轮。
const AGGREGATED_TRIM_CHUNK: i64 = 100_000;

/// 单次 retention 最多执行的淘汰轮数（每轮 [`AGGREGATED_TRIM_CHUNK`] 行）。
/// retention 每小时跑一次，这里允许的回收量足以跟上正常写入速度。
const AGGREGATED_TRIM_ROUNDS: usize = 4;

/// S-4: retention 清理——删除超过保留期的聚合历史记录，并执行行数上界淘汰
///
/// - data_queue_aggregated: 保留 storage_days 天（降采样历史表；
///   聚合 writer 已实现，每 2 ticks 把过期原始行压成分钟桶写入该表）
pub(crate) fn run_retention_cleanup(conn: &mut Connection, storage_days: Option<u32>) {
    let now = DataPoint::now();
    if let Some(days) = storage_days {
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
    enforce_aggregated_row_cap(conn, MAX_AGGREGATED_ROWS);
}

/// 聚合表行数上界：超过 `max_rows` 时按 `window_start` 从旧到新分批删除。
///
/// 边界抽样（同一 `window_start` 上多个点位的行可能让结果略高于/低于上限）
/// 对「防止写满磁盘」这一目标无影响，不为精确性引入额外排序/游标复杂度。
///
/// 需要 v4 迁移的 `idx_agg_window` 索引支撑，否则每次都要全表排序。
pub(crate) fn enforce_aggregated_row_cap(conn: &mut Connection, max_rows: i64) {
    let total: i64 = match conn.query_row("SELECT COUNT(*) FROM data_queue_aggregated", [], |row| {
        row.get(0)
    }) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("统计聚合表行数失败，跳过行数上界淘汰: {}", e);
            return;
        }
    };
    if total <= max_rows {
        return;
    }

    let target = total - max_rows;
    let mut removed: i64 = 0;
    for _ in 0..AGGREGATED_TRIM_ROUNDS {
        // 每轮删除量不得超过「仍需淘汰的行数」，否则会把未超界的行一起删掉。
        let batch = (target - removed).min(AGGREGATED_TRIM_CHUNK);
        match conn.execute(
            "DELETE FROM data_queue_aggregated WHERE id IN \
             (SELECT id FROM data_queue_aggregated ORDER BY window_start ASC LIMIT ?1)",
            params![batch],
        ) {
            Ok(0) => break,
            Ok(n) => {
                removed += n as i64;
                if removed >= target {
                    break;
                }
            }
            Err(e) => {
                tracing::warn!("聚合表行数上界淘汰失败: {}", e);
                break;
            }
        }
    }
    if removed > 0 {
        tracing::warn!(
            "retention: aggregated 行数 {} 超过上界 {}，已淘汰最旧 {} 行",
            total,
            max_rows,
            removed
        );
    }
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

    /// 造 `n` 行聚合数据，`window_start` 单调递增（越大越新）。
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

    /// 超过行数上界时只淘汰**最旧的** `total - max_rows` 行，不多删。
    #[test]
    fn trims_only_oldest_rows_beyond_cap() {
        let mut conn = seeded(10);
        enforce_aggregated_row_cap(&mut conn, 4);

        assert_eq!(count(&conn), 4, "必须恰好压到上界，不能多删");
        let min_ts: i64 = conn
            .query_row(
                "SELECT MIN(window_start) FROM data_queue_aggregated",
                [],
                |r| r.get(0),
            )
            .expect("min ts");
        assert_eq!(
            min_ts,
            1_700_000_000_000 + 6 * 60_000,
            "保留的必须是最新的 4 行"
        );
    }

    /// 未超上界时一行都不删（行数界是保护，不是常规清理）。
    #[test]
    fn keeps_all_rows_when_below_cap() {
        let mut conn = seeded(5);
        enforce_aggregated_row_cap(&mut conn, 5);
        assert_eq!(count(&conn), 5);

        enforce_aggregated_row_cap(&mut conn, 100);
        assert_eq!(count(&conn), 5);
    }

    /// 时间界清理与行数界互不干扰：storage_days 为 None 时行数界仍生效。
    #[test]
    fn retention_without_time_quota_still_enforces_row_cap() {
        let mut conn = seeded(6);
        run_retention_cleanup(&mut conn, None);
        // 上限是生产常量（5,000,000），6 行远未触顶 → 一行不删。
        assert_eq!(count(&conn), 6);
    }
}
