//! 采集结果处理：锁内决策 + 设备状态更新
//!
//! 从 scheduler/mod.rs 拆分。

use std::time::Duration;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::pipeline::CollectedBatch;

use super::task::PointAction;
use super::Scheduler;
use crate::south::scheduler::DeviceState;

impl Scheduler {
    /// tracing: 采集结果处理 span（n=points.len()），tracing feature off 时 cfg_attr 剥离为零成本。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, points, results), fields(n = points.len())))]
    pub(crate) async fn handle_results(
        &self,
        points: &[PointConfig],
        results: Vec<Result<DataPoint, crate::south::protocol::CollectorError>>,
    ) {
        // Bug 73 修复：原实现 points.iter().zip(results.into_iter()) 在长度不一致
        // 时静默截断（zip 行为），导致尾部点位无结果处理、状态不更新。
        // 现显式检查长度，不一致时按较短处理并 log warn。
        if points.len() != results.len() {
            tracing::warn!(
                "handle_results: points/results length mismatch ({} vs {})",
                points.len(),
                results.len()
            );
        }
        let n = points.len().min(results.len());
        let mut items: Vec<(PointConfig, DataPoint)> = Vec::with_capacity(n);
        let points_iter = points.iter().take(n);
        let results_iter = results.into_iter().take(n);

        for (point, result) in points_iter.zip(results_iter) {
            // 锁作用域 1：仅做状态更新（record_success/failure），不做边缘处理
            // 边缘处理/入队交由 ProcessingPipeline 异步执行，保证采集周期精准
            let action: PointAction = {
                let mut states = self.device_states.lock();
                let state = states.entry(point.id.clone()).or_insert_with(|| {
                    DeviceState::new(Duration::from_secs(point.interval.max(1)))
                });

                match result {
                    Ok(mut dp) => {
                        if dp.quality == Quality::Good {
                            // NaN/Inf 检测：非法值降级为 Bad，避免污染下游
                            if !DataPoint::is_value_valid(dp.value)
                                || !DataPoint::is_value_valid(dp.raw_value)
                            {
                                tracing::warn!(
                                    "point {} produced non-finite value ({}), degrading",
                                    point.id,
                                    dp.value
                                );
                                dp.quality = Quality::Bad;
                                state.record_failure();
                            } else {
                                state.record_success(dp.value);
                            }
                        } else {
                            // Bug D2: 采集成功但质量码非 Good（如解码失败返回 Ok+Bad）。
                            // 原实现不更新 DeviceState → consecutive_failures 恒为 0 →
                            // current_quality() 恒为 Good，点位在线统计虚高且永不进入退避。
                            // 此处按失败计入，使设备状态与质量码保持一致。
                            state.record_failure();
                        }
                        PointAction::Ok(dp)
                    }
                    Err(e) => {
                        state.record_failure();
                        let quality = state.current_quality();
                        let last_good = state.last_good_value;
                        let consecutive_failures = state.consecutive_failures;
                        let last_good_ts = state.last_good_ts;
                        let stale_count = state.stale_count;
                        PointAction::Err {
                            quality,
                            last_good,
                            err_msg: e.to_string(),
                            point_id: point.id.clone(),
                            consecutive_failures,
                            last_good_ts,
                            stale_count,
                        }
                    }
                }
            }; // 锁在此释放

            // 锁外构造数据点，收集到 items 后提交给处理管道
            match action {
                PointAction::Ok(dp) => {
                    items.push((point.clone(), dp));
                }
                PointAction::Err {
                    quality,
                    last_good,
                    err_msg,
                    point_id,
                    consecutive_failures,
                    last_good_ts,
                    stale_count,
                } => {
                    let dp = DataPoint::failed(
                        &point_id,
                        &point.device_id.clone().unwrap_or_else(|| point.bus.clone()),
                        last_good,
                        last_good_ts,
                        quality,
                        "collect_failed",
                        stale_count,
                    );
                    items.push((point.clone(), dp));
                    tracing::debug!(
                        "point {} read failed: {} (failures={})",
                        point_id,
                        err_msg,
                        consecutive_failures
                    );
                }
            }
        }

        // 更新在线/离线点位指标
        {
            let states = self.device_states.lock();
            let mut online = 0i64;
            let mut offline = 0i64;
            for s in states.values() {
                if s.current_quality() == crate::engine::model::Quality::Good {
                    online += 1;
                } else {
                    offline += 1;
                }
            }
            self.metrics.set_point_status(online, offline);
        }

        // 提交给处理管道（边缘处理 + 入队 + 实时广播异步执行）
        if !items.is_empty() {
            self.pipeline.submit(CollectedBatch::new(items)).await;
        }
    }
}
