//! DD-52：诊断查询 API——按点位统计窗口内实际落库样本数。
//!
//! 从 `queue/mod.rs` 外移（该文件已接近 800 行行数预算上限）。
//! 口径：调用方传入 `[start, end]`（unix 毫秒），返回过滤后的真实样本数；
//! 查询失败必须返回错误，禁止用估算值兜底。

use super::core::QueueCommand;
use super::DataQueue;
use crate::engine::error::{GatewayError, GatewayResult};
use std::collections::HashMap;
use tokio::sync::oneshot;

impl DataQueue {
    /// 按点位统计窗口内**实际落库**的样本数（`[start, end]`，unix 毫秒）。
    ///
    /// 供「数据压缩」页展示真实的「采集成功量 → 存储量」对比：管线过滤
    /// （死区 / 变化上传）发生在入库之前，本计数是过滤后的真实结果。
    /// 查询失败时返回错误，由调用方决定降级方式（不得用估算值替代）。
    pub async fn stored_counts(&self, start: i64, end: i64) -> GatewayResult<HashMap<String, i64>> {
        let (reply, rx) = oneshot::channel();
        if self
            .tx
            .send(QueueCommand::StoredCounts { start, end, reply })
            .await
            .is_err()
        {
            return Err(GatewayError::Queue(
                "queue task closed before stored_counts".into(),
            ));
        }
        rx.await
            .unwrap_or_else(|_| Err(GatewayError::Queue("stored_counts reply dropped".into())))
    }
}
