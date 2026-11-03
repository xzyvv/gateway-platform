//! 队列 retention 清理：删除超过保留期的聚合历史记录
//!
//! v2.0 Step 5：保留天数由 license 配额 `storage_days` 驱动（bootstrap 的
//! 配额对账任务周期推送），不再硬编码。`None` = 不限，跳过清理。
//!
//! 注意：data_queue（原始待上报表）的 FIFO 截断由 maintenance 第 6 步的
//! `cleanup_old_records` 执行——它是 depth 计数器的同步点，raw 删除必须
//! 走它，避免两处删除导致计数漂移。本函数只负责聚合历史表。

use crate::engine::model::DataPoint;
use rusqlite::{params, Connection};

/// S-4: retention 清理——删除超过保留期的聚合历史记录
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
}
