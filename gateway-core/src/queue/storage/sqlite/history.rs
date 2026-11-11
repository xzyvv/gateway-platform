//! 历史查询与降采样
//!
//! 提供按点位查询时间范围内的历史样本，以及按时间桶聚合的降采样查询。
//!
//! ## P2-1 / P2-3：两条查询路径
//!
//! | 请求 | 数据源 | 粒度 |
//! |------|--------|------|
//! | `bucket_secs = None` | `data_queue` 原始行 | 原始行 |
//! | `bucket_secs = Some(n)`，窗口在 24h 内 | `data_queue` 原始行 | `n` 秒桶 |
//! | `bucket_secs = Some(n >= 60)`，窗口跨过 24h | 旧段读 `data_queue_aggregated`（分钟桶），新段读原始行 | `n` 秒桶 |
//!
//! 24 小时以内保持原始精度（现场排障要看秒级抖动），更早的数据由
//! [`aggregate_minute_windows`] 周期性压成分钟桶，避免长窗口查询扫原始大表。

use rusqlite::{params, Connection};

use crate::queue::core::QueuedItem;
use crate::queue::storage::{HistoryQueryResult, HistorySample};

/// 历史查询硬上限（防御性：防止调用方传入过大 limit 导致内存爆炸）
pub const HISTORY_HARD_LIMIT: i64 = 100_000;

/// 原始精度保留窗口：24 小时。
///
/// 窗口内的数据不聚合（保留逐笔抖动，用于排障）；超过该窗口的原始行
/// 由 [`aggregate_minute_windows`] 压成分钟桶写入 `data_queue_aggregated`，
/// 查询时旧段直接读聚合表。
pub const AGG_HORIZON_MS: i64 = 24 * 3600 * 1000;

/// 分钟桶宽度（聚合表的最小粒度）
const MINUTE_MS: i64 = 60_000;

/// 降采样桶宽的取值范围（毫秒）：最短 1 秒，最长 1 天。
///
/// 上限与前端输入框的语义一致（`downsample` 单位为秒）；超出范围按边界钳制，
/// 避免调用方传 0 或极大值把 SQL 除法算成 0 桶宽 / 死循环式扫描。
const MIN_BUCKET_MS: i64 = 1_000;
const MAX_BUCKET_MS: i64 = 86_400_000;

/// 从 SQLite 读取某点位在 [start, end] 范围内的原始行，按 timestamp ASC 排序。
///
/// 仅作为 [`query_raw_ex`] 的内部原语（N-10：不再对外暴露独立的历史查询路径），
/// 返回的项按时间升序，limit 在最末裁剪（保留最新）。
pub(crate) fn query_raw_rows(
    conn: &Connection,
    point_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: usize,
) -> rusqlite::Result<Vec<QueuedItem>> {
    let lim = (limit as i64).clamp(1, HISTORY_HARD_LIMIT);
    let mut stmt = conn.prepare_cached(
        "SELECT id, point_id, device_id, value, raw_value, raw_type, quality, quality_reason, timestamp, source_ts, stale_count, value_text \
         FROM data_queue WHERE point_id = ?1 AND timestamp BETWEEN ?2 AND ?3 \
         ORDER BY timestamp DESC LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![point_id, start_ms, end_ms, lim], |row| {
        Ok(QueuedItem {
            id: row.get(0)?,
            point_id: row.get(1)?,
            device_id: row.get(2)?,
            value: row.get(3)?,
            raw_value: row.get(4)?,
            raw_type: row.get(5)?,
            quality: row.get(6)?,
            quality_reason: row.get(7)?,
            timestamp: row.get(8)?,
            source_ts: row.get(9)?,
            stale_count: row.get(10)?,
            // 批次 C：枚举文本标签。查询期**不做规则回查**——落库时是 NULL 就
            // 如实返回 None（表示该点无枚举标签或写入时尚未支持标签）。
            value_text: row.get(11)?,
        })
    })?;
    let mut items: Vec<QueuedItem> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    items.reverse(); // 转为 ASC
    Ok(items)
}

/// 历史查询（P2-1）：原始行 / 时间桶降采样，并返回分页元信息。
///
/// 参数 `now_ms` 由调用方传入（而非在 SQL 内取当前时间），便于单测构造
/// 「24 小时前的数据」而无需等待真实时钟。
pub fn query_history_ex(
    conn: &Connection,
    point_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: usize,
    bucket_secs: Option<u32>,
    now_ms: i64,
) -> rusqlite::Result<HistoryQueryResult> {
    let lim = (limit as i64).clamp(1, HISTORY_HARD_LIMIT) as usize;
    // downsample=0 与缺省同义（关闭降采样）：前端输入框允许输入 0，但语义是"不聚合"
    let bucket_ms = bucket_secs
        .filter(|s| *s > 0)
        .map(|s| (s as i64 * 1000).clamp(MIN_BUCKET_MS, MAX_BUCKET_MS));

    let Some(bucket_ms) = bucket_ms else {
        return query_raw_ex(conn, point_id, start_ms, end_ms, lim);
    };

    let granularity = format!("second:{}", bucket_ms / 1000);
    let horizon = now_ms - AGG_HORIZON_MS;
    let mut items: Vec<HistorySample> = Vec::new();

    // ① 旧段：窗口延伸到 24h 之前，且桶宽不细于聚合表粒度 → 直接读分钟聚合表。
    //    桶宽细于 1 分钟（如 5 秒）时聚合表无能为力，整段退回原始行分桶。
    let older_end = end_ms.min(horizon);
    if start_ms < older_end && bucket_ms >= MINUTE_MS {
        items.extend(query_aggregated_buckets(
            conn,
            point_id,
            start_ms,
            older_end,
            bucket_ms,
            (lim + 1) as i64,
        )?);
    }

    // ② 新段：窗口落在 24h 之内（含跨界窗口的近期部分）→ 原始行分桶。
    let recent_start = if bucket_ms >= MINUTE_MS {
        start_ms.max(horizon)
    } else {
        start_ms
    };
    if recent_start <= end_ms {
        items.extend(query_raw_buckets(
            conn,
            point_id,
            recent_start,
            end_ms,
            bucket_ms,
            (lim + 1) as i64,
        )?);
    }

    // 两段在时间上不重叠且顺序递增（旧段在前），直接拼接即为 ASC
    let truncated = items.len() > lim;
    if truncated {
        items.drain(0..items.len() - lim);
    }
    let total = if truncated {
        // 只有发生截断时才需要精确总数（见 query_raw_ex 的同类处理）
        let mut total = 0i64;
        if bucket_ms >= MINUTE_MS && start_ms < older_end {
            total += count_aggregated_buckets(conn, point_id, start_ms, older_end, bucket_ms)?;
        }
        if recent_start <= end_ms {
            total += count_raw_buckets(conn, point_id, recent_start, end_ms, bucket_ms)?;
        }
        total
    } else {
        items.len() as i64
    };

    Ok(HistoryQueryResult::new(items, total, granularity))
}

/// 原始行查询（无降采样）：多取 1 条用于截断判定，命中时才跑 COUNT。
fn query_raw_ex(
    conn: &Connection,
    point_id: &str,
    start_ms: i64,
    end_ms: i64,
    lim: usize,
) -> rusqlite::Result<HistoryQueryResult> {
    let mut items = query_raw_rows(conn, point_id, start_ms, end_ms, lim + 1)?;
    let truncated = items.len() > lim;
    let total = if truncated {
        conn.query_row(
            "SELECT COUNT(*) FROM data_queue WHERE point_id = ?1 AND timestamp BETWEEN ?2 AND ?3",
            params![point_id, start_ms, end_ms],
            |row| row.get::<_, i64>(0),
        )?
    } else {
        items.len() as i64
    };
    if truncated {
        let drop_n = items.len() - lim;
        items.drain(0..drop_n);
    }
    let samples: Vec<HistorySample> = items.into_iter().map(HistorySample::from_raw).collect();
    Ok(HistoryQueryResult::new(samples, total, "raw"))
}

/// 原始表按 `bucket_ms` 分桶（`ORDER BY DESC LIMIT` 取最新，返回时反转为 ASC）。
fn query_raw_buckets(
    conn: &Connection,
    point_id: &str,
    start_ms: i64,
    end_ms: i64,
    bucket_ms: i64,
    lim: i64,
) -> rusqlite::Result<Vec<HistorySample>> {
    let mut stmt = conn.prepare_cached(
        "SELECT (timestamp / ?1) * ?1 AS w, AVG(value), MIN(value), MAX(value), COUNT(*) \
         FROM data_queue WHERE point_id = ?2 AND timestamp >= ?3 AND timestamp <= ?4 \
         GROUP BY w ORDER BY w DESC LIMIT ?5",
    )?;
    let rows = stmt.query_map(
        params![
            bucket_ms,
            point_id,
            start_ms,
            end_ms,
            lim.clamp(1, HISTORY_HARD_LIMIT)
        ],
        |row| {
            Ok(HistorySample {
                timestamp: row.get(0)?,
                value: row.get(1)?,
                min_value: row.get(2)?,
                max_value: row.get(3)?,
                sample_count: row.get(4)?,
                quality: "aggregated".to_string(),
                raw_value: None,
                raw_type: None,
            })
        },
    )?;
    let mut items: Vec<HistorySample> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    items.reverse();
    Ok(items)
}

/// 聚合表按 `bucket_ms` 二次分桶。
///
/// 聚合表存的是分钟桶，桶宽更宽时按样本数加权平均（`SUM(avg*count)/SUM(count)`），
/// 避免"平均的平均"失真；`sample_count` 为桶内原始样本数之和。
fn query_aggregated_buckets(
    conn: &Connection,
    point_id: &str,
    start_ms: i64,
    end_ms: i64,
    bucket_ms: i64,
    lim: i64,
) -> rusqlite::Result<Vec<HistorySample>> {
    let mut stmt = conn.prepare_cached(
        "SELECT (window_start / ?1) * ?1 AS w, \
                SUM(avg_value * sample_count) / SUM(sample_count), \
                MIN(min_value), MAX(max_value), SUM(sample_count) \
         FROM data_queue_aggregated WHERE point_id = ?2 AND window_start >= ?3 AND window_start < ?4 \
         GROUP BY w ORDER BY w DESC LIMIT ?5",
    )?;
    let rows = stmt.query_map(
        params![
            bucket_ms,
            point_id,
            start_ms,
            end_ms,
            lim.clamp(1, HISTORY_HARD_LIMIT)
        ],
        |row| {
            Ok(HistorySample {
                timestamp: row.get(0)?,
                value: row.get(1)?,
                min_value: row.get(2)?,
                max_value: row.get(3)?,
                sample_count: row.get(4)?,
                quality: "aggregated".to_string(),
                raw_value: None,
                raw_type: None,
            })
        },
    )?;
    let mut items: Vec<HistorySample> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    items.reverse();
    Ok(items)
}

fn count_raw_buckets(
    conn: &Connection,
    point_id: &str,
    start_ms: i64,
    end_ms: i64,
    bucket_ms: i64,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(DISTINCT (timestamp / ?1) * ?1) FROM data_queue \
         WHERE point_id = ?2 AND timestamp >= ?3 AND timestamp <= ?4",
        params![bucket_ms, point_id, start_ms, end_ms],
        |row| row.get(0),
    )
}

fn count_aggregated_buckets(
    conn: &Connection,
    point_id: &str,
    start_ms: i64,
    end_ms: i64,
    bucket_ms: i64,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(DISTINCT (window_start / ?1) * ?1) FROM data_queue_aggregated \
         WHERE point_id = ?2 AND window_start >= ?3 AND window_start < ?4",
        params![bucket_ms, point_id, start_ms, end_ms],
        |row| row.get(0),
    )
}

/// 按分钟窗口聚合历史数据（P2-3）。
///
/// 把 `timestamp < now - 24h` 的原始行压成分钟桶（avg/min/max/count）写入
/// `data_queue_aggregated`，供长窗口查询直接读取。
///
/// 增量策略：从聚合表已有的最大桶（水位）**前一个桶**开始重算——
/// 边界桶可能刚跨过 24h 边界而尚未填满，重算一遍即可补齐；更早的桶不再触碰，
/// 避免每次维护都全表重扫。写入用 UPSERT 且**只允许样本数不减**：
/// 若原始行已被 retention 删掉一部分，重算出的残缺桶不会覆盖已有的完整桶。
///
/// 返回本次写入（含更新）的桶数，供维护日志按需打印。
pub fn aggregate_minute_windows(conn: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    let horizon = now_ms - AGG_HORIZON_MS;
    // 水位：聚合表里最大的桶起点；从「水位 - 1 分钟」重算（见函数文档）。
    // 注意 MAX() 在空表上返回 NULL，必须按 Option 取值，否则会得到
    // InvalidColumnType 而不是"没有水位"。
    let watermark: Option<i64> = conn.query_row(
        "SELECT MAX(window_start) FROM data_queue_aggregated",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    let from_ms = watermark.map(|w| w - MINUTE_MS).unwrap_or(i64::MIN);

    let written = conn.execute(
        "INSERT INTO data_queue_aggregated \
             (point_id, window_start, avg_value, min_value, max_value, sample_count) \
         SELECT point_id, (timestamp / ?1) * ?1 AS w, AVG(value), MIN(value), MAX(value), COUNT(*) \
         FROM data_queue WHERE timestamp < ?2 AND timestamp >= ?3 \
         GROUP BY point_id, w \
         ON CONFLICT(point_id, window_start) DO UPDATE SET \
             avg_value = excluded.avg_value, \
             min_value = excluded.min_value, \
             max_value = excluded.max_value, \
             sample_count = excluded.sample_count \
         WHERE excluded.sample_count >= data_queue_aggregated.sample_count",
        params![MINUTE_MS, horizon, from_ms],
    )?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建最小 schema 的内存库（只含查询/聚合涉及的两张表）
    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE data_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                point_id TEXT NOT NULL, device_id TEXT NOT NULL DEFAULT '',
                value REAL NOT NULL, raw_value REAL NOT NULL, raw_type TEXT NOT NULL,
                quality TEXT NOT NULL, quality_reason TEXT NOT NULL DEFAULT '',
                timestamp INTEGER NOT NULL, source_ts INTEGER NOT NULL DEFAULT 0,
                stale_count INTEGER NOT NULL DEFAULT 0, in_flight INTEGER NOT NULL DEFAULT 0,
                dequeued_at INTEGER NOT NULL DEFAULT 0, delivered INTEGER NOT NULL DEFAULT 0,
                value_text TEXT);
            CREATE TABLE data_queue_aggregated (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                point_id TEXT NOT NULL, window_start INTEGER NOT NULL,
                avg_value REAL NOT NULL, min_value REAL NOT NULL, max_value REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                UNIQUE(point_id, window_start));",
        )
        .unwrap();
        conn
    }

    fn insert_raw(conn: &Connection, ts: i64, value: f64) {
        conn.execute(
            "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp) \
             VALUES ('p', ?1, ?1, 'f64', 'good', ?2)",
            params![value, ts],
        )
        .unwrap();
    }

    #[test]
    fn aggregate_minute_windows_upserts() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        insert_raw(&conn, now - 30 * 3600 * 1000, 1.0);
        insert_raw(&conn, now - 30 * 3600 * 1000 + 5_000, 3.0);
        insert_raw(&conn, now - 60 * 60 * 1000, 9.0);

        let written = aggregate_minute_windows(&conn, now).unwrap();
        assert_eq!(written, 1, "只有 24h 之前的行参与聚合");

        // 重复执行：幂等（桶数仍为 1，样本数不被重复累加）
        assert_eq!(aggregate_minute_windows(&conn, now).unwrap(), 1);
        let (avg, min, max, count): (f64, f64, f64, i64) = conn
            .query_row(
                "SELECT avg_value, min_value, max_value, sample_count FROM data_queue_aggregated",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((avg, min, max, count), (2.0, 1.0, 3.0, 2));
    }

    #[test]
    fn query_ex_merges_agg_and_raw_segments() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        let old_ts = now - 30 * 3600 * 1000;
        insert_raw(&conn, old_ts, 1.0);
        insert_raw(&conn, old_ts + 5_000, 3.0);
        insert_raw(&conn, now - 60 * 60 * 1000, 9.0);
        aggregate_minute_windows(&conn, now).unwrap();

        let result =
            query_history_ex(&conn, "p", now - 48 * 3600 * 1000, now, 100, Some(60), now).unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[0].sample_count, 2);
        assert_eq!(result.items[0].value, 2.0);
        assert_eq!(result.items[1].value, 9.0);
        assert_eq!(result.total, 2);
        assert!(!result.truncated);

        // 聚合表读到的桶，其 timestamp 必须对齐到分钟
        assert_eq!(result.items[0].timestamp % MINUTE_MS, 0);
    }

    /// 双段（旧段读聚合表 + 新段原始分桶）同时命中且总量超过 limit 时，
    /// `total` 必须是两段计数的**精确和**，不能靠 items.len() 凑。
    ///
    /// 这条用例专门覆盖「只有截断时才跑 COUNT」的路径：不截断时 total 直接取
    /// items.len()，两个 count_* 函数根本不会被调用，它们的返回值改成常量
    /// （Ok(0)/Ok(1)/Ok(-1)）也就测不出来。
    #[test]
    fn query_ex_two_segments_truncated_total_is_exact() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        let horizon = now - AGG_HORIZON_MS;
        // 旧段：3 个不同的分钟桶，且相隔 15 分钟以保证在 10 分钟桶下仍各占一格
        let old = now - 30 * 3600 * 1000;
        insert_raw(&conn, old, 1.0);
        insert_raw(&conn, old + 15 * 60_000, 2.0);
        insert_raw(&conn, old + 30 * 60_000, 3.0);
        // 新段：2 个不同的 10 分钟桶（都落在 24h 之内，只能原始行分桶）
        insert_raw(&conn, now - 60 * 60 * 1000, 7.0);
        insert_raw(&conn, now - 80 * 60 * 1000, 8.0);
        assert_eq!(aggregate_minute_windows(&conn, now).unwrap(), 3);

        // 桶宽 10 分钟（≥ 1 分钟，旧段走聚合表）；limit=3 < 总桶数 5 → 截断
        let r = query_history_ex(&conn, "p", now - 48 * 3600 * 1000, now, 3, Some(600), now).unwrap();
        assert!(r.truncated, "5 个桶超过 limit=3，必须判为截断");
        assert_eq!(r.returned, 3);
        assert_eq!(r.total, 5, "total 应为聚合段 3 桶 + 原始段 2 桶");
        assert_eq!(r.granularity, "second:600");
        // 截断保留的是**最新**的 3 个桶：新段 2 个 + 旧段里最晚的 1 个
        let horizon_bucket = (horizon / MINUTE_MS) * MINUTE_MS;
        assert!(
            r.items.iter().any(|s| s.timestamp >= horizon_bucket),
            "截断后应保留新段（24h 内）的桶"
        );
    }

    /// 桶宽细于 1 分钟时不读聚合表，整段退回原始行分桶——即使窗口跨过 24h。
    ///
    /// 这条用例把 `旧段条件` 里的 `bucket_ms >= MINUTE_MS` 与 `start_ms < older_end`
    /// 两半分别钉住：桶宽 5 秒时两个条件都应为「不读聚合表」。
    #[test]
    fn query_ex_sub_minute_bucket_skips_aggregated_table() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        let old = now - 30 * 3600 * 1000;
        insert_raw(&conn, old, 1.0);
        insert_raw(&conn, old + 5_000, 3.0);
        insert_raw(&conn, now - 60 * 60 * 1000, 9.0);
        aggregate_minute_windows(&conn, now).unwrap();

        // 5 秒桶：三行原始数据分别在 3 个不同的 5 秒桶里
        let r = query_history_ex(&conn, "p", now - 48 * 3600 * 1000, now, 100, Some(5), now).unwrap();
        assert_eq!(r.items.len(), 3, "细桶必须扫原始表，不能走聚合表");
        assert_eq!(r.granularity, "second:5");
        assert_eq!(r.total, 3);
        assert!(!r.truncated);
    }

    /// 原始行查询：条数正好等于 limit 时不算截断（`>` 不能写成 `>=`）。
    #[test]
    fn query_raw_ex_exact_limit_is_not_truncated() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        for i in 0..6 {
            insert_raw(&conn, now - (6 - i) * 1_000, i as f64);
        }
        let r = query_history_ex(&conn, "p", now - 60_000, now, 6, None, now).unwrap();
        assert_eq!(r.returned, 6);
        assert_eq!(r.total, 6);
        assert!(!r.truncated, "正好 limit 条不算截断");
    }

    /// 原始行查询：limit=1 且只有溢出 1 条时，只丢弃溢出那一行（`-` 不能写成 `/`）。
    #[test]
    fn query_raw_ex_drops_only_overflow_row() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        insert_raw(&conn, now - 2_000, 1.0);
        insert_raw(&conn, now - 1_000, 2.0);
        let r = query_history_ex(&conn, "p", now - 60_000, now, 1, None, now).unwrap();
        assert_eq!(r.returned, 1, "只应丢掉溢出的 1 行，保留 1 行");
        assert_eq!(r.total, 2);
        assert!(r.truncated);
        assert_eq!(r.items[0].value, 2.0, "保留的应是最新那行");
    }

    /// 增量聚合：已有水位时只重算「水位所在的桶」，不回头重扫更早的桶。
    ///
    /// 水位减的是 `MINUTE_MS`（毫秒偏移），若误写成除法会退化成从
    /// 「时间戳/60000」这么小的位置重扫，把所有历史桶都重算一遍。
    #[test]
    fn aggregate_minute_windows_recomputes_only_watermark_bucket() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        let old = now - 30 * 3600 * 1000;
        insert_raw(&conn, old, 1.0);
        insert_raw(&conn, old + 180_000, 3.0); // 相隔 3 分钟 → 2 个桶
        assert_eq!(aggregate_minute_windows(&conn, now).unwrap(), 2, "首次聚合两个桶");

        // 第二次：水位 = 较晚那个桶，只重算它自己 → 写入 1 行
        assert_eq!(aggregate_minute_windows(&conn, now).unwrap(), 1, "只重算水位桶");
    }

    #[test]
    fn query_ex_raw_and_bucket_limits() {
        let conn = mem_db();
        let now = 1_700_000_000_000i64;
        for i in 0..6 {
            insert_raw(&conn, now - (6 - i) * 1_000, i as f64);
        }
        // 原始行 + limit 截断
        let raw = query_history_ex(&conn, "p", now - 60_000, now, 3, None, now).unwrap();
        assert_eq!((raw.returned, raw.total, raw.truncated), (3, 6, true));
        // 10 秒桶 → 6 条压成 1 个桶
        let bucketed = query_history_ex(&conn, "p", now - 60_000, now, 10, Some(10), now).unwrap();
        assert_eq!(bucketed.items.len(), 1);
        assert_eq!(bucketed.granularity, "second:10");
        assert_eq!(bucketed.items[0].sample_count, 6);
    }
}
