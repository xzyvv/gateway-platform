//! 数据质量规则热路径执行快照
//!
//! 规则由 API 写入 `QualityRuleStore` 后，经 `install_rules` 替换本模块的
//! 模块级快照（`RwLock<Arc<Vec<QualityRule>>>`），处理管道在 `process_batch`
//! 出口处调用 [`apply`]，因此规则变更无需改 `ProcessingPipeline` 构造签名。
//!
//! 执行口径：
//! - `range`：`value` 落在 `[min, max]` 之外 → 降级为 `resultQuality`；
//! - `rate`：与**同一 point_id 上一次采样值**的绝对变化 > `maxRate` → 降级；
//!   上值表容量有上限（见 [`MAX_RATE_TRACKED_POINTS`]），超限时按时间淘汰旧点；
//! - `nan`：`value` 非有限（NaN/±Inf）→ 降级；
//! - `stale`：**不在本模块执行**。数据超时未更新由调度器的连续失败/超时机制
//!   负责（`stale_count` / 退避），此处不重复判定，避免两套口径互相覆盖。
//!
//! 只对当前质量码为 `Good` 的点执行规则：上游已判定为 `Bad/Uncertain/Stale`
//! 的数据保留原质量与原因，避免被质量规则「洗白」或覆盖故障原因。

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};

use crate::config::QualityRule;
use crate::engine::model::{DataPoint, Quality};

/// 变化率上值表最多跟踪的点位数。工业网关点位数通常远小于该值；
/// 超限时按采样时间淘汰过期条目，保证长时间运行内存有界。
const MAX_RATE_TRACKED_POINTS: usize = 4096;
/// 上值条目的保留时长：超过该时长未再采样的点位在扩容时被清理。
const RATE_STATE_TTL_MS: i64 = 60 * 60 * 1000;

/// 当前生效规则快照（启动/写接口更新后整体替换）
static RULES: Lazy<RwLock<Arc<Vec<QualityRule>>>> = Lazy::new(|| RwLock::new(Arc::new(Vec::new())));

/// 变化率规则的上值表：point_id → 上一次采样值 + 采样时刻
#[derive(Debug, Clone, Copy)]
struct RateState {
    last_value: f64,
    ts_ms: i64,
}

static RATE_STATE: Lazy<Mutex<HashMap<String, RateState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 安装新的规则快照（通常传入 `QualityRuleStore::list_enabled_rules()`）
pub fn install_rules(rules: Vec<QualityRule>) {
    *RULES.write() = Arc::new(rules);
}

/// 对一批数据点执行质量规则（原地修改质量码与 `quality_reason`）
///
/// `now_ms`：本批处理时刻（毫秒），用于变化率上值表的时间戳与老化。
/// 无规则时直接返回，对数据点零改动。
pub fn apply(points: &mut [DataPoint], now_ms: i64) {
    let rules = RULES.read().clone();
    if rules.is_empty() || points.is_empty() {
        return;
    }
    // 只有存在 rate 规则时才需要触碰上值表，避免无谓加锁。
    let has_rate = rules.iter().any(|r| r.enabled && r.condition == "rate");
    let mut rate_state = if has_rate {
        Some(RATE_STATE.lock())
    } else {
        None
    };

    for dp in points.iter_mut() {
        if dp.quality != Quality::Good {
            continue;
        }
        for rule in rules.iter() {
            if !rule.enabled {
                continue;
            }
            let hit_quality = match rule.condition.as_str() {
                "range" => {
                    if range_hit(rule, dp.value) {
                        quality_from_str(&rule.result_quality)
                    } else {
                        None
                    }
                }
                "rate" => {
                    let hit = match rate_state.as_mut() {
                        Some(state) => {
                            let prev = state.get(&dp.point_id).copied();
                            let max_rate = rule.max_rate.unwrap_or(f64::NAN);
                            let hit = prev
                                .map(|p| {
                                    max_rate.is_finite()
                                        && max_rate >= 0.0
                                        && (dp.value - p.last_value).abs() > max_rate
                                })
                                .unwrap_or(false);
                            record_rate(&mut *state, &dp.point_id, dp.value, now_ms);
                            hit
                        }
                        None => false,
                    };
                    if hit {
                        quality_from_str(&rule.result_quality)
                    } else {
                        None
                    }
                }
                "nan" => {
                    if !dp.value.is_finite() {
                        quality_from_str(&rule.result_quality)
                    } else {
                        None
                    }
                }
                // stale 由调度器负责（见模块文档），此处不执行。
                _ => None,
            };
            if let Some(q) = hit_quality {
                dp.quality = q;
                dp.quality_reason = format!("quality_rule:{}", rule.name);
                break;
            }
        }
    }
}

/// 取值范围命中：非法数据（非有限）交给 nan 规则，这里不判定。
fn range_hit(rule: &QualityRule, value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    match (rule.min, rule.max) {
        (Some(min), Some(max)) => value < min || value > max,
        (Some(min), None) => value < min,
        (None, Some(max)) => value > max,
        (None, None) => false,
    }
}

/// 前端质量码字符串 → 核心枚举；`old` 对应 `Stale`
fn quality_from_str(s: &str) -> Option<Quality> {
    match s {
        "good" => Some(Quality::Good),
        "bad" => Some(Quality::Bad),
        "uncertain" => Some(Quality::Uncertain),
        "old" | "stale" => Some(Quality::Stale),
        _ => None,
    }
}

/// 记录本次采样值；容量超限时先淘汰过期条目，极端情况下整体清空以保证有界。
fn record_rate(state: &mut HashMap<String, RateState>, point_id: &str, value: f64, now_ms: i64) {
    if state.len() >= MAX_RATE_TRACKED_POINTS && !state.contains_key(point_id) {
        let cutoff = now_ms - RATE_STATE_TTL_MS;
        state.retain(|_, s| s.ts_ms >= cutoff);
        if state.len() >= MAX_RATE_TRACKED_POINTS {
            state.clear();
        }
    }
    state.insert(
        point_id.to_string(),
        RateState {
            last_value: value,
            ts_ms: now_ms,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;

    /// 规则快照与上值表是进程级全局态，测试必须串行执行。
    static TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn reset() {
        install_rules(Vec::new());
        RATE_STATE.lock().clear();
    }

    fn rule(
        condition: &str,
        result: &str,
        min: Option<f64>,
        max: Option<f64>,
        max_rate: Option<f64>,
    ) -> QualityRule {
        QualityRule {
            id: "qr_test".into(),
            name: format!("rule-{}", condition),
            condition: condition.into(),
            min,
            max,
            max_rate,
            stale_seconds: None,
            result_quality: result.into(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn point(id: &str, value: f64) -> DataPoint {
        DataPoint::good(id, "dev", value, value, "f64")
    }

    #[test]
    fn no_rules_keeps_points_untouched() {
        let _guard = TEST_LOCK.lock();
        reset();
        let mut points = vec![point("p1", 123.0), point("p2", 456.0)];
        let before: Vec<_> = points.clone();
        apply(&mut points, 1_000);
        for (after, before) in points.iter().zip(before.iter()) {
            assert_eq!(after.quality, before.quality);
            assert_eq!(after.value, before.value);
            assert_eq!(after.quality_reason, before.quality_reason);
        }
    }

    #[test]
    fn range_hits_and_passes_through() {
        let _guard = TEST_LOCK.lock();
        reset();
        install_rules(vec![rule("range", "bad", Some(-100.0), Some(100.0), None)]);

        let mut inside = vec![point("range_in", 50.0)];
        apply(&mut inside, 1_000);
        assert_eq!(inside[0].quality, Quality::Good, "范围内不得降级");
        assert_eq!(inside[0].quality_reason, "");

        let mut outside = vec![point("range_out", 150.0)];
        apply(&mut outside, 1_000);
        assert_eq!(outside[0].quality, Quality::Bad);
        assert_eq!(outside[0].quality_reason, "quality_rule:rule-range");
    }

    #[test]
    fn rate_compares_with_previous_value() {
        let _guard = TEST_LOCK.lock();
        reset();
        install_rules(vec![rule("rate", "uncertain", None, None, Some(50.0))]);

        let mut first = vec![point("rate_p", 10.0)];
        apply(&mut first, 1_000);
        assert_eq!(first[0].quality, Quality::Good, "首次采样无上值可比");

        let mut second = vec![point("rate_p", 20.0)];
        apply(&mut second, 2_000);
        assert_eq!(second[0].quality, Quality::Good, "变化 10 <= 50 不应命中");

        let mut third = vec![point("rate_p", 120.0)];
        apply(&mut third, 3_000);
        assert_eq!(third[0].quality, Quality::Uncertain, "变化 100 > 50 应命中");
        assert_eq!(third[0].quality_reason, "quality_rule:rule-rate");
    }

    #[test]
    fn nan_hits_only_non_finite_values() {
        let _guard = TEST_LOCK.lock();
        reset();
        install_rules(vec![rule("nan", "bad", None, None, None)]);

        let mut finite = vec![point("nan_ok", 1.0)];
        apply(&mut finite, 1_000);
        assert_eq!(finite[0].quality, Quality::Good);

        let mut non_finite = vec![point("nan_bad", 1.0)];
        non_finite[0].value = f64::NAN;
        apply(&mut non_finite, 1_000);
        assert_eq!(non_finite[0].quality, Quality::Bad);
        assert_eq!(non_finite[0].quality_reason, "quality_rule:rule-nan");
    }

    /// 上游已判 Bad 的点不参与规则判定，保留原质量与原因。
    #[test]
    fn non_good_points_are_skipped() {
        let _guard = TEST_LOCK.lock();
        reset();
        install_rules(vec![rule(
            "range",
            "uncertain",
            Some(0.0),
            Some(10.0),
            None,
        )]);

        let mut failed = vec![DataPoint::failed(
            "p_failed",
            "dev",
            Some(99.0),
            0,
            Quality::Bad,
            "device_timeout",
            0,
        )];
        apply(&mut failed, 1_000);
        assert_eq!(failed[0].quality, Quality::Bad);
        assert_eq!(failed[0].quality_reason, "device_timeout");
    }
}
