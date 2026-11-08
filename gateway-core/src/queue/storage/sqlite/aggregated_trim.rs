//! 聚合历史表的行数上界淘汰
//!
//! 从 `retention` 拆出：淘汰逻辑（分块删除 + 目标行数计算）与 retention 的
//! 时间界/水位策略是两件事，分开也便于守住单文件行数预算。
//!
//! 依赖 v4 迁移建立的 `idx_agg_window(window_start)`：删除条件是
//! 「按 window_start 从旧到新取 N 行」，没有该索引会退化为全表排序。

use rusqlite::{params, Connection};

/// 单次删除的行数：分块避免一条 DELETE 长时间持写锁（嵌入式 ARM 上大事务
/// 会造成采集侧入队等待）。
const AGGREGATED_TRIM_CHUNK: i64 = 100_000;

/// 单次 retention 最多执行的淘汰轮数（每轮 [`AGGREGATED_TRIM_CHUNK`] 行）。
/// retention 每小时跑一次，这里允许的回收量足以跟上正常写入速度。
const AGGREGATED_TRIM_ROUNDS: usize = 4;

/// 聚合表行数上界：超过 `max_rows` 时按 `window_start` 从旧到新分批删除。
///
/// 边界抽样（同一 `window_start` 上多个点位的行可能让结果略高于/低于上限）
/// 对「防止写满磁盘」这一目标无影响，不为精确性引入额外排序/游标复杂度。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(n: i64) -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(
            "CREATE TABLE data_queue_aggregated (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                point_id TEXT NOT NULL,
                window_start INTEGER NOT NULL,
                avg_value REAL NOT NULL,
                min_value REAL NOT NULL,
                max_value REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                UNIQUE(point_id, window_start)
            );
            CREATE INDEX idx_agg_window ON data_queue_aggregated(window_start);",
        )
        .expect("create agg table");
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

    /// 超过上界时只淘汰最旧的 `total - max_rows` 行，不多删。
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
}
