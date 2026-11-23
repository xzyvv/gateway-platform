//! DD-52：诊断类队列命令的 handler（原 task.rs 内联，为控制行数预算外移）。
//!
//! 这些查询都是 SQLite 聚合，必须走 `spawn_blocking`，避免阻塞 tokio worker；
//! 错误原样回传调用方，由 handler 层决定降级方式（不得用估算值替代真实数据）。

use crate::engine::error::{GatewayError, GatewayResult};
use crate::queue::storage::QueueStorage;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// 最旧一条持久化数据的时间戳；join 失败只告警并返回 None（与旧行为一致）。
pub(super) async fn oldest_ts(storage: Arc<dyn QueueStorage>, reply: oneshot::Sender<Option<i64>>) {
    let storage_clone = storage.clone();
    let result = tokio::task::spawn_blocking(move || storage_clone.oldest_data_timestamp()).await;
    let ts = match result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("oldest_data_timestamp join failed: {}", e);
            None
        }
    };
    let _ = reply.send(ts);
}

/// 按点位统计窗口内实际落库样本数；SQL 错误原样回传。
pub(super) async fn stored_counts(
    storage: Arc<dyn QueueStorage>,
    start: i64,
    end: i64,
    reply: oneshot::Sender<GatewayResult<HashMap<String, i64>>>,
) {
    let storage_clone = storage.clone();
    let result = tokio::task::spawn_blocking(move || storage_clone.stored_counts(start, end)).await;
    let result = match result {
        Ok(v) => v,
        Err(e) => Err(GatewayError::Queue(format!(
            "stored_counts spawn_blocking failed: {}",
            e
        ))),
    };
    let _ = reply.send(result);
}
