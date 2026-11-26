//! 数据质量规则热路径执行快照
//!
//! 规则由 API 写入 `QualityRuleStore` 后，经 [`QualityRuleRuntime::install`] 替换
//! 本实例的规则快照（`RwLock<Arc<Vec<QualityRule>>>`），处理管道在 `process_batch`
//! 出口处调用 [`QualityRuleRuntime::apply`]。
//!
//! 运行时实例由装配点创建一次，**处理管道与 Web 写接口共享同一个 `Arc`**：
//! 写规则时经 `QualityRuleInstaller` 端口装到该实例上，采集侧立即可见。
//! 这里不再使用进程级全局 static——多实例（测试/嵌入式宿主）之间互不串扰。
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

use parking_lot::{Mutex, RwLock};

use crate::config::QualityRule;
use crate::engine::model::{DataPoint, Quality};
use crate::engine::quality_rule_port::QualityRuleInstaller;

/// 变化率上值表最多跟踪的点位数。工业网关点位数通常远小于该值；
/// 超限时按采样时间淘汰过期条目，保证长时间运行内存有界。
const MAX_RATE_TRACKED_POINTS: usize = 4096;
/// 上值条目的保留时长：超过该时长未再采样的点位在扩容时被清理。
const RATE_STATE_TTL_MS: i64 = 60 * 60 * 1000;

/// 变化率规则的上值表：point_id → 上一次采样值 + 采样时刻
#[derive(Debug, Clone, Copy)]
struct RateState {
    last_value: f64,
    ts_ms: i64,
}

/// 数据质量规则运行时。
///
/// 每个处理管道持有一个实例；Web 写接口通过 [`QualityRuleInstaller`] 向同一实例
/// 安装新规则。规则快照与变化率上值表都是实例级状态，不存在跨实例共享。
pub struct QualityRuleRuntime {
    /// 当前生效规则快照（启动/写接口更新后整体替换）
    rules: RwLock<Arc<Vec<QualityRule>>>,
    /// 变化率规则的上值表：point_id → 上一次采样值 + 采样时刻
    rate_state: Mutex<HashMap<String, RateState>>,
}

impl QualityRuleRuntime {
    /// 以一组初始规则创建运行时（通常来自 `QualityRuleStore::list_enabled_rules()`）。
    pub fn new(rules: Vec<QualityRule>) -> Self {
        Self {
            rules: RwLock::new(Arc::new(rules)),
            rate_state: Mutex::new(HashMap::new()),
        }
    }

    /// 安装新的规则快照（整体替换当前规则）。
    pub fn install(&self, rules: Vec<QualityRule>) {
        *self.rules.write() = Arc::new(rules);
    }

    /// 对一批数据点执行质量规则（原地修改质量码与 `quality_reason`）
    ///
    /// `now_ms`：本批处理时刻（毫秒），用于变化率上值表的时间戳与老化。
    /// 无规则时直接返回，对数据点零改动。
    pub fn apply(&self, points: &mut [DataPoint], now_ms: i64) {
        let rules = self.rules.read().clone();
        if rules.is_empty() || points.is_empty() {
            return;
        }
        // 只有存在 rate 规则时才需要触碰上值表，避免无谓加锁。
        let has_rate = rules.iter().any(|r| r.enabled && r.condition == "rate");
        let mut rate_state = if has_rate {
            Some(self.rate_state.lock())
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
}

impl Default for QualityRuleRuntime {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl QualityRuleInstaller for QualityRuleRuntime {
    fn install_from_store(&self, store: &crate::config::QualityRuleStore) -> Result<usize, String> {
        let rules = store
            .list_enabled_rules()
            .map_err(|e| format!("读取已启用质量规则失败: {e}"))?;
        let count = rules.len();
        self.install(rules);
        tracing::info!("quality rules re-installed: {}", count);
        Ok(count)
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

/// 前端质量码字符串 → 核心枚举。
///
/// `old` / `stale` 在这里降级为 `Uncertain`：I-20 规定 `Stale` 只能由调度器
/// 的连续失败/退避判定产生，质量规则路径不得把点标成 `Stale`（G-T2b 修复）。
fn quality_from_str(s: &str) -> Option<Quality> {
    match s {
        "good" => Some(Quality::Good),
        "bad" => Some(Quality::Bad),
        "uncertain" | "old" | "stale" => Some(Quality::Uncertain),
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
        let rt = QualityRuleRuntime::default();
        let mut points = vec![point("p1", 123.0), point("p2", 456.0)];
        let before: Vec<_> = points.clone();
        rt.apply(&mut points, 1_000);
        for (after, before) in points.iter().zip(before.iter()) {
            assert_eq!(after.quality, before.quality);
            assert_eq!(after.value, before.value);
            assert_eq!(after.quality_reason, before.quality_reason);
        }
    }

    #[test]
    fn range_hits_and_passes_through() {
        let rt =
            QualityRuleRuntime::new(vec![rule("range", "bad", Some(-100.0), Some(100.0), None)]);

        let mut inside = vec![point("range_in", 50.0)];
        rt.apply(&mut inside, 1_000);
        assert_eq!(inside[0].quality, Quality::Good, "范围内不得降级");
        assert_eq!(inside[0].quality_reason, "");

        let mut outside = vec![point("range_out", 150.0)];
        rt.apply(&mut outside, 1_000);
        assert_eq!(outside[0].quality, Quality::Bad);
        assert_eq!(outside[0].quality_reason, "quality_rule:rule-range");
    }

    #[test]
    fn rate_compares_with_previous_value() {
        let rt = QualityRuleRuntime::new(vec![rule("rate", "uncertain", None, None, Some(50.0))]);

        let mut first = vec![point("rate_p", 10.0)];
        rt.apply(&mut first, 1_000);
        assert_eq!(first[0].quality, Quality::Good, "首次采样无上值可比");

        let mut second = vec![point("rate_p", 20.0)];
        rt.apply(&mut second, 2_000);
        assert_eq!(second[0].quality, Quality::Good, "变化 10 <= 50 不应命中");

        let mut third = vec![point("rate_p", 120.0)];
        rt.apply(&mut third, 3_000);
        assert_eq!(third[0].quality, Quality::Uncertain, "变化 100 > 50 应命中");
        assert_eq!(third[0].quality_reason, "quality_rule:rule-rate");
    }

    #[test]
    fn nan_hits_only_non_finite_values() {
        let rt = QualityRuleRuntime::new(vec![rule("nan", "bad", None, None, None)]);

        let mut finite = vec![point("nan_ok", 1.0)];
        rt.apply(&mut finite, 1_000);
        assert_eq!(finite[0].quality, Quality::Good);

        let mut non_finite = vec![point("nan_bad", 1.0)];
        non_finite[0].value = f64::NAN;
        rt.apply(&mut non_finite, 1_000);
        assert_eq!(non_finite[0].quality, Quality::Bad);
        assert_eq!(non_finite[0].quality_reason, "quality_rule:rule-nan");
    }

    /// 上游已判 Bad 的点不参与规则判定，保留原质量与原因。
    #[test]
    fn non_good_points_are_skipped() {
        let rt = QualityRuleRuntime::new(vec![rule(
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
        rt.apply(&mut failed, 1_000);
        assert_eq!(failed[0].quality, Quality::Bad);
        assert_eq!(failed[0].quality_reason, "device_timeout");
    }

    /// DD-14：规则与变化率上值表都是实例级状态，两个运行时互不影响。
    #[test]
    fn quality_rules_runtime_is_instance_scoped() {
        let a =
            QualityRuleRuntime::new(vec![rule("range", "bad", Some(-100.0), Some(100.0), None)]);
        let b = QualityRuleRuntime::default();

        let mut pa = vec![point("same", 150.0)];
        a.apply(&mut pa, 1_000);
        assert_eq!(pa[0].quality, Quality::Bad, "a 应命中自身规则");

        let mut pb = vec![point("same", 150.0)];
        b.apply(&mut pb, 1_000);
        assert_eq!(pb[0].quality, Quality::Good, "b 未装规则，不得受 a 影响");

        // 只给 b 装 rate 规则，不得改动 a 的规则快照。
        b.install(vec![rule("rate", "uncertain", None, None, Some(1.0))]);
        let mut pa2 = vec![point("same", 151.0)];
        a.apply(&mut pa2, 2_000);
        assert_eq!(
            pa2[0].quality,
            Quality::Bad,
            "a 仍是 range 规则，150/151 超范围应降级"
        );
        // b 装 rate 规则后首次采样只记账，第二次变化超过阈值才命中。
        let mut pb2 = vec![point("same", 152.0)];
        b.apply(&mut pb2, 2_000);
        assert_eq!(pb2[0].quality, Quality::Good, "首次采样无上值");
        let mut pb3 = vec![point("same", 155.0)];
        b.apply(&mut pb3, 3_000);
        assert_eq!(
            pb3[0].quality,
            Quality::Uncertain,
            "b 的 rate 上值表独立记录后应命中变化"
        );
    }

    /// I-18：多条规则同时命中时，只有**列表顺序中的第一条**生效，后续规则不得
    /// 覆盖质量码与原因（命中即 break）。
    #[test]
    fn first_matching_quality_rule_wins() {
        // 两条 range 规则都会命中 150（分别超出 ±100 与 0..10），
        // 规则名不同，便于用 reason 判定到底是哪一条生效。
        let mut first = rule("range", "bad", Some(-100.0), Some(100.0), None);
        first.name = "first-range".into();
        let mut second = rule("range", "uncertain", Some(0.0), Some(10.0), None);
        second.name = "second-range".into();
        let rt = QualityRuleRuntime::new(vec![first, second]);

        let mut points = vec![point("p-order", 150.0)];
        rt.apply(&mut points, 1_000);
        assert_eq!(
            points[0].quality,
            Quality::Bad,
            "必须采用第一条命中规则的质量码"
        );
        assert_eq!(
            points[0].quality_reason, "quality_rule:first-range",
            "reason 必须属于第一条命中的规则，后续规则不得覆盖"
        );

        // 交换顺序后结果必须跟着变，证明判定依据确实是列表顺序而非规则种类。
        let mut second = rule("range", "uncertain", Some(0.0), Some(10.0), None);
        second.name = "second-range".into();
        let mut first = rule("range", "bad", Some(-100.0), Some(100.0), None);
        first.name = "first-range".into();
        let rt = QualityRuleRuntime::new(vec![second, first]);

        let mut points = vec![point("p-order-rev", 150.0)];
        rt.apply(&mut points, 1_000);
        assert_eq!(points[0].quality, Quality::Uncertain);
        assert_eq!(
            points[0].quality_reason, "quality_rule:second-range",
            "交换顺序后应由新顺序里的第一条规则生效"
        );
    }

    /// I-19：变化率上值表容量上限 4096 + TTL 1 小时——超过 TTL 的旧条目在扩容
    /// 清理时被淘汰，保证长时间运行内存有界。
    ///
    /// 直接驱动模块内私有上值表（同模块测试可访问私有项），不改变生产可见性。
    #[test]
    fn rate_history_table_expires_old_entries() {
        let mut state: std::collections::HashMap<String, RateState> =
            std::collections::HashMap::new();
        // 灌满容量，时间戳停留在很久以前（远超 TTL）。
        for i in 0..MAX_RATE_TRACKED_POINTS {
            record_rate(&mut state, &format!("p{i}"), 1.0, 0);
        }
        assert_eq!(state.len(), MAX_RATE_TRACKED_POINTS, "测试前提：上值表已满");

        // 新点的采样时刻比旧条目晚一个 TTL 以上 → 旧条目全部过期。
        let now = RATE_STATE_TTL_MS + 1_000;
        record_rate(&mut state, "fresh", 2.0, now);

        assert_eq!(state.len(), 1, "超过 TTL 的旧条目必须全部被清理");
        assert!(state.contains_key("fresh"), "新条目必须保留");
        assert!(!state.contains_key("p0"), "过期条目不得残留");
    }

    /// I-19：同一时刻所有条目都未过期时，TTL 清理腾不出空间 → 整体清空后重插，
    /// 保证上值表永不突破 4096 上限。
    #[test]
    fn rate_history_table_clears_when_over_capacity() {
        let mut state: std::collections::HashMap<String, RateState> =
            std::collections::HashMap::new();
        let now = 1_000_000;
        for i in 0..MAX_RATE_TRACKED_POINTS {
            record_rate(&mut state, &format!("p{i}"), 1.0, now);
        }
        assert_eq!(state.len(), MAX_RATE_TRACKED_POINTS, "测试前提：上值表已满");

        record_rate(&mut state, "overflow", 2.0, now + 1);

        assert_eq!(
            state.len(),
            1,
            "无法靠 TTL 腾出空间时必须整体清空，再插入新条目"
        );
        assert!(state.contains_key("overflow"));
    }

    /// I-20：质量规则路径**不得**产出 `Quality::Stale`（Stale 由调度器的连续
    /// 失败/退避判定负责）。
    ///
    /// 1. `condition = "stale"` 的规则在 `apply` 中不执行（match 落到 `_ => None`）；
    /// 2. `result_quality = "stale" / "old"` 在质量规则中被降级为 `Uncertain`
    ///    （G-T2b 修复：此前会落成 `Stale`，违反 I-20）；
    /// 3. 未知结果码（如 `perfect`）不产生任何降级，质量保持原值。
    #[test]
    fn quality_rules_never_produce_stale() {
        // ① 条件为 stale 的规则永不执行：质量与原因都保持原值。
        let rt = QualityRuleRuntime::new(vec![rule("stale", "bad", None, None, None)]);
        let mut points = vec![point("p-stale-cond", 42.0)];
        rt.apply(&mut points, 1_000);
        assert_eq!(
            points[0].quality,
            Quality::Good,
            "condition=stale 的规则不得执行，质量必须保持 Good"
        );
        assert_eq!(points[0].quality_reason, "");

        // ② 结果码为 stale / old：必须降级为 Uncertain，绝不产出 Stale。
        for result in ["stale", "old"] {
            let mut to_uncertain = rule("range", result, Some(-100.0), Some(100.0), None);
            to_uncertain.name = "to-uncertain".into();
            let rt = QualityRuleRuntime::new(vec![to_uncertain]);
            let mut points = vec![point("p-stale-result", 150.0)];
            rt.apply(&mut points, 1_000);
            assert_eq!(
                points[0].quality,
                Quality::Uncertain,
                "result_quality={result} 必须降级为 Uncertain，不得产出 Stale"
            );
            assert_eq!(points[0].quality_reason, "quality_rule:to-uncertain");
        }

        // ③ 未知结果码不降级：规则命中但质量码无法解析 → 保持原质量与空原因。
        let mut unknown = rule("range", "perfect", Some(-100.0), Some(100.0), None);
        unknown.name = "unknown-result".into();
        let rt = QualityRuleRuntime::new(vec![unknown]);
        let mut points = vec![point("p-unknown-result", 150.0)];
        rt.apply(&mut points, 1_000);
        assert_eq!(points[0].quality, Quality::Good, "未知结果码不得改变质量");
        assert_eq!(points[0].quality_reason, "");
    }
}
