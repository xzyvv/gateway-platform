//! 计数式北向桩（决策 1A）。
//!
//! 只统计 `publish_batch` 的批次与点位数，不外联、零依赖。
//! 经 `ConversionManager` 订阅 `RealtimeBus` 后，即可验证
//! “南向采集 → 管道 → 实时总线 → 北向扇出”整条链路真实打通。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use gateway_core::engine::error::GatewayResult;
use gateway_core::engine::model::DataPoint;
use gateway_core::north::{ConnectorHealth, NorthConnector};

/// 北向计数桩。
#[derive(Default)]
pub struct CountingSink {
    batches: AtomicU64,
    points: AtomicU64,
}

impl CountingSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 已收到的批次数。
    pub fn batches(&self) -> u64 {
        self.batches.load(Ordering::Relaxed)
    }

    /// 已收到的点位数。
    pub fn points(&self) -> u64 {
        self.points.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl NorthConnector for CountingSink {
    fn name(&self) -> &str {
        "counting_sink"
    }

    async fn publish_batch(&self, batch: &[DataPoint]) -> GatewayResult<()> {
        self.batches.fetch_add(1, Ordering::Relaxed);
        self.points.fetch_add(batch.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn health(&self) -> ConnectorHealth {
        ConnectorHealth {
            connected: true,
            last_error: None,
        }
    }

    fn is_enabled(&self) -> bool {
        true
    }
}
