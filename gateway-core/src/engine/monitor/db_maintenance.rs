//! 业务库存储维护与占用采样
//!
//! 配置域共享库（`config_versions.db`）承载 `config_versions` / `api_access_logs`
//! / `disk_history` / `uplink_test_history` 四张表，它们都有行数 retention，但
//! SQLite 默认（`auto_vacuum=NONE`）不会把 `DELETE` 释放的页还给文件系统 →
//! 行数受控、**文件却只增不减**。队列主库（`data_queue.db`）走同一套逻辑，
//! 公共实现收口在 [`crate::support::sqlite_maintenance`]；这里只管配置库这条链：
//!
//! 1. 启动时把 `auto_vacuum` 切到 `INCREMENTAL`（已有库必须跑一次 `VACUUM`
//!    才会写进库头，这是 SQLite 的语义）；
//! 2. 之后每 60s 调用 `PRAGMA incremental_vacuum(N)` 分小批回收空闲页；
//! 3. 同一任务采样两个库的文件大小 / 空闲页数与聚合表行数，暴露为 Prometheus
//!    gauge——没有这些数，现场无法判断「回收是否真的生效」。
//!
//! 所有失败都只告警、不阻断：维护任务的职责是兜底，不应影响采集与上报。

use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
use rusqlite::Connection;

use crate::engine::metrics::Metrics;
use crate::support::sqlite_maintenance::{
    ensure_incremental_auto_vacuum as shared_ensure_auto_vacuum, file_size_and_freelist,
    open_short_lived, run_incremental_vacuum,
};

/// 维护/采样周期（秒）。与磁盘监控的 30s 周期错开，避免同一时刻集中抢 IO。
const MAINTENANCE_INTERVAL_SECS: u64 = 60;

/// 每次回收的配置库空闲页数（4KB/页，128 页≈512KB）。
///
/// 分小批而不是一次 `VACUUM`：配置库是请求热路径（鉴权/限流）的库，
/// 长时间持锁会直接抬高接口延迟。
const CONFIG_DB_VACUUM_PAGES: i64 = 128;

/// 聚合表行数（`COUNT(*)` 需扫索引）每 5 个周期采样一次，降低常态 IO。
const AGG_ROW_COUNT_EVERY: u64 = 5;

/// 降采样聚合表名（与队列侧 retention 使用同一张表）。
const AGG_TABLE: &str = "data_queue_aggregated";

/// 开启配置库的增量 vacuum（幂等）。
///
/// 具体语义与队列主库共用 [`crate::support::sqlite_maintenance`]，
/// 这里只负责打开短生命周期连接。
/// 失败不阻断启动：磁盘不足/库被占用时降级为「本次运行不回收」，下次启动重试。
pub fn ensure_incremental_auto_vacuum(db_path: &Path) {
    let Some(conn) = open_short_lived(db_path, "配置库") else {
        return;
    };
    shared_ensure_auto_vacuum(&conn, "配置库");
}

/// 回收配置库空闲页（`auto_vacuum=NONE` 时为空操作，不会报错）。
fn reclaim_config_db(db_path: &Path, pages: i64) {
    let Some(conn) = open_short_lived(db_path, "配置库") else {
        return;
    };
    run_incremental_vacuum(&conn, pages, "配置库");
}

/// 统计聚合表行数（表不存在时返回 None，不告警）。
fn aggregated_rows(db_path: &Path) -> Option<i64> {
    let conn = open_short_lived(db_path, "配置库 aggregated rows")?;
    conn.query_row(&format!("SELECT COUNT(*) FROM {}", AGG_TABLE), [], |row| {
        row.get(0)
    })
    .ok()
}

/// 周期维护 + 占用采样任务（panic 自恢复）。
///
/// `queue_db` / `config_db` 用 `String` 而不是 `&Path`：路径解析结果是
/// `&'static str`，转成 owned 后可以安全 move 进闭包。
pub fn spawn_storage_maintenance(queue_db: String, config_db: String, metrics: Arc<Metrics>) {
    crate::engine::supervision::spawn_supervised("storage_maintenance", move || {
        let queue_db = queue_db.clone();
        let config_db = config_db.clone();
        let metrics = metrics.clone();
        async move {
            let mut tick: u64 = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(MAINTENANCE_INTERVAL_SECS)).await;
                tick = tick.wrapping_add(1);
                let count_agg = tick.is_multiple_of(AGG_ROW_COUNT_EVERY);
                let q = queue_db.clone();
                let c = config_db.clone();
                let m = metrics.clone();
                // SQLite 文件 IO 是阻塞调用，放到 blocking 池，避免占住运行时的 worker。
                let _ = tokio::task::spawn_blocking(move || {
                    let config_path = Path::new(&c);
                    reclaim_config_db(config_path, CONFIG_DB_VACUUM_PAGES);
                    if let Some((size, freelist)) =
                        file_size_and_freelist(config_path, "配置库 stats")
                    {
                        m.set_config_db_stats(size, freelist);
                    }
                    let queue_path = Path::new(&q);
                    if let Some((size, freelist)) =
                        file_size_and_freelist(queue_path, "队列主库 stats")
                    {
                        m.set_queue_db_stats(size, freelist);
                    }
                    if count_agg {
                        if let Some(rows) = aggregated_rows(queue_path) {
                            m.set_aggregated_rows(rows);
                        }
                    }
                })
                .await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join(format!("{}.db", tag));
        (dir, path)
    }

    /// 启停幂等：重复调用不重复 VACUUM，且最终 auto_vacuum=INCREMENTAL。
    #[test]
    fn ensure_auto_vacuum_is_idempotent() {
        let (_dir, path) = temp_db("config");
        ensure_incremental_auto_vacuum(&path);
        ensure_incremental_auto_vacuum(&path);

        let conn = Connection::open(&path).expect("open db");
        let mode: i64 = conn
            .pragma_query_value(None, "auto_vacuum", |row| row.get(0))
            .expect("read auto_vacuum");
        assert_eq!(mode, 2, "配置库必须切换为 INCREMENTAL(2)");
    }

    /// 已有数据的库也能转换：转换后数据保留。
    #[test]
    fn ensure_auto_vacuum_keeps_existing_rows() {
        let (_dir, path) = temp_db("config-data");
        {
            let conn = Connection::open(&path).expect("open db");
            conn.execute_batch("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (7);")
                .expect("seed");
        }
        ensure_incremental_auto_vacuum(&path);

        let conn = Connection::open(&path).expect("open db");
        let v: i64 = conn
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .expect("read row");
        assert_eq!(v, 7, "VACUUM 不能丢数据");
    }

    /// 采样：库大小 > 0，空闲页可读；聚合表不存在时行数返回 None。
    #[test]
    fn samples_db_stats_and_aggregated_rows() {
        let (_dir, path) = temp_db("stats");
        ensure_incremental_auto_vacuum(&path);
        {
            let conn = Connection::open(&path).expect("open db");
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
                INSERT INTO data_queue_aggregated
                    (point_id, window_start, avg_value, min_value, max_value, sample_count)
                    VALUES ('p', 1, 0.0, 0.0, 0.0, 1), ('p', 2, 0.0, 0.0, 0.0, 1);",
            )
            .expect("seed");
        }

        let (size, freelist) = file_size_and_freelist(&path, "测试库").expect("stats");
        assert!(size > 0, "库文件大小必须 > 0");
        assert!(freelist >= 0, "空闲页数必须可读");
        assert_eq!(aggregated_rows(&path), Some(2));
    }

    /// 表不存在时不 panic、不误报行数。
    #[test]
    fn aggregated_rows_returns_none_without_table() {
        let (_dir, path) = temp_db("no-agg");
        ensure_incremental_auto_vacuum(&path);
        assert_eq!(aggregated_rows(&path), None);
    }
}
