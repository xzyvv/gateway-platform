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

/// I-08 / I-56（ROI-19）：把「采集返回 `Ok` 但质量非 Good」的点统一成失败点口径。
///
/// 旧实现把协议层构造的 `Ok+Bad` 点原样上送：`value=0`、`source_ts=0`（1970）、
/// `stale_count=0`、`raw_type=none`，与 `Err` 路径（复用 last_good + 上次成功时刻）
/// 字段语义完全分叉，平台会把「从没读到的 0」当成真实读数。
///
/// 统一后与 [`DataPoint::failed`] 完全一致：有 last_good 就复用其值与采集时刻并
/// 标记 `raw_type=last_good`；从未成功过才保留 `none` / 0（I-06 的「无成功历史」语义）。
/// 质量码与协议给出的 `quality_reason` 原样保留；仅当调用方已把 Good 降级
/// （如非有限值）时补 `value:NON_FINITE`。
pub(crate) fn failed_from_ok(
    dp: DataPoint,
    point: &PointConfig,
    last_good: Option<f64>,
    last_good_ts: i64,
    stale_count: u32,
) -> DataPoint {
    let was_good = dp.quality == Quality::Good;
    let quality = if was_good { Quality::Bad } else { dp.quality };
    let reason = if was_good && dp.quality_reason.is_empty() {
        "value:NON_FINITE".to_string()
    } else {
        dp.quality_reason
    };
    let device_id = point.device_id.clone().unwrap_or_else(|| point.bus.clone());
    DataPoint::failed(
        &point.id,
        &device_id,
        last_good,
        last_good_ts,
        quality,
        &reason,
        stale_count,
    )
}

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
                    Ok(dp) => {
                        let finite = DataPoint::is_value_valid(dp.value)
                            && DataPoint::is_value_valid(dp.raw_value);
                        if dp.quality == Quality::Good && finite {
                            state.record_success(dp.value);
                            PointAction::Ok(dp)
                        } else {
                            if dp.quality == Quality::Good {
                                // 正常点里混入 NaN/Inf：降级为 Bad（旧实现只改 quality，
                                // 却把 NaN 留在 value 里继续下发）。
                                tracing::warn!(
                                    "point {} produced non-finite value ({}), degrading",
                                    point.id,
                                    dp.value
                                );
                            }
                            // Bug D2：采集成功但质量码非 Good（如解码失败返回 Ok+Bad）
                            // 仍按失败计入设备状态，使退避与在线统计与质量码一致。
                            // ROI-19：随后走与 `Err` 完全相同的失败点口径，
                            // 复用 last_good 值/采集时刻，不再上送 value=0、source_ts=0。
                            state.record_failure();
                            PointAction::Ok(failed_from_ok(
                                dp,
                                point,
                                state.last_good_value,
                                state.last_good_ts,
                                state.stale_count,
                            ))
                        }
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
                            // R-1：把真实错因的稳定错误码一起上送，
                            // 不再让所有失败都压成同一个 "collect_failed"。
                            quality_reason: e.quality_reason("collect_failed"),
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
                    quality_reason,
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
                        &quality_reason,
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
