//! 虚拟点位引擎（公式派生量）
//!
//! 挂在处理管道 **协议转换之后、实时广播/入队之前**（见 `pipeline/mod.rs`）：
//! 每批数据落地后更新「点位实时值缓存」，再用缓存里的值求值虚拟点位公式，
//! 产出派生 `DataPoint`（`point_id` = 虚拟点位 ID）参与广播、入队与北向上报。
//!
//! # 语义
//!
//! - 公式语义由 [`super::vformula`] 定义（**唯一权威实现**），与前端
//!   `gateway-web/src/utils/formulaEngine.ts` 的浏览器端预演保持一致。
//! - 引用的点位取**最近一次成功采集**的值；引用值过期（TTL = 3 × 采集周期，
//!   下限 30 秒）或从未采集过，求值失败并记 `missing_point` 错误。
//! - 虚拟点位可**级联引用**其它虚拟点位；按固定点迭代求解，最多 8 轮，
//!   超限（存在环）记 `circular_reference` 错误。
//! - 求值失败不静默丢弃：产出 `Quality::Uncertain` +
//!   `quality_reason = "virtual_point:ERROR"` 的数据点，让平台能感知异常。
//!
//! # 上报节流
//!
//! 派生值只在**发生变化**时产出数据点（与点位级「变化才上传」语义一致），
//! 避免输入不变时反复上报同一数值。错误状态变化时同样只上报一次。
//!
//! # 热重载
//!
//! `GatewayRuntime::reload_config` 检测 `virtual_points` 差异后调 `update_rules`
//! 原子替换规则集（ArcSwap），求值状态（最近值 / 错误）跨重载保留。

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;

use crate::config::{PointConfig, VirtualPoint};
use crate::engine::model::{DataPoint, Quality};

use super::point_cache::PointValueCache;
use super::vformula::{self, CompiledFormula};

/// 级联求解的最大迭代轮数（超过即判定存在环）
const MAX_CASCADE_ROUNDS: usize = 8;

/// 虚拟点位的运行态规则
struct VirtualPointRule {
    id: String,
    device_id: String,
    decimals: u8,
    /// 是否启用（禁用的规则不参与求值）
    enabled: bool,
    /// 编译结果；`Err` 为编译期错误（公式非法），此时规则仍保留，
    /// 以便 Web 层如实展示「公式语法错误」而不是让点位凭空消失。
    compiled: Result<CompiledFormula, String>,
}

/// 虚拟点位的最近一次求值状态（供 Web 层展示）
#[derive(Debug, Clone, Default)]
pub struct VirtualPointState {
    /// 最近一次成功求值的结果（按 `decimals` 舍入）
    pub value: Option<f64>,
    /// 最近一次求值错误（无错为 `None`）
    pub error: Option<String>,
    /// 最近一次求值时刻（UTC 毫秒）
    pub updated_at_ms: i64,
}

/// 虚拟点位引擎（线程安全；求值只读规则快照，热重载无锁切换）
pub struct VirtualPointEngine {
    rules: ArcSwap<Vec<Arc<VirtualPointRule>>>,
    /// 点位实时值缓存（虚拟点位的求值输入）
    cache: PointValueCache,
    /// 各虚拟点位的最近求值状态
    states: Mutex<HashMap<String, VirtualPointState>>,
}

impl VirtualPointEngine {
    /// 从配置构建引擎（非法公式保留但标记错误，不进入求值）
    pub fn new(points: Vec<VirtualPoint>) -> Arc<Self> {
        Arc::new(Self {
            rules: ArcSwap::from_pointee(Self::compile_all(points)),
            cache: PointValueCache::new(),
            states: Mutex::new(HashMap::new()),
        })
    }

    /// 空引擎（无虚拟点位配置；求值零开销）
    pub fn empty() -> Arc<Self> {
        Self::new(Vec::new())
    }

    fn compile_all(points: Vec<VirtualPoint>) -> Vec<Arc<VirtualPointRule>> {
        points
            .iter()
            .map(|vp| {
                let compiled = match vformula::compile(&vp.formula) {
                    Ok(c) => Ok(c),
                    Err(e) => {
                        tracing::warn!("virtual point {}: 公式编译失败: {}", vp.id, e);
                        Err(e.to_string())
                    }
                };
                Arc::new(VirtualPointRule {
                    id: vp.id.clone(),
                    device_id: vp.device_id.clone().unwrap_or_default(),
                    decimals: vp.decimals,
                    enabled: vp.enabled,
                    compiled,
                })
            })
            .collect()
    }

    /// 热重载：原子替换规则集；求值状态与已产出值跨重载保留
    ///
    /// 被删除的虚拟点位其历史求值状态一并清理，避免状态表随配置变更无限增长。
    pub fn update_rules(&self, points: Vec<VirtualPoint>) {
        let compiled = Self::compile_all(points);
        {
            let mut states = self.states.lock();
            states.retain(|id, _| compiled.iter().any(|r| &r.id == id));
        }
        self.rules.store(Arc::new(compiled));
    }

    /// 规则数（含被禁用的规则）
    pub fn rule_count(&self) -> usize {
        self.rules.load().len()
    }

    /// 淘汰已过期的点位实时值缓存（DD-16）
    ///
    /// 由处理管道周期性调用：点位删除或长期不再上报后，缓存条目随 TTL 过期，
    /// 这里把它们真正移出内存，避免点位规模大时缓存单调增长。
    pub fn prune_cache(&self, now_ms: i64) {
        self.cache.prune(now_ms);
    }

    /// 更新点位实时值缓存
    ///
    /// **只写上报口径**：`final_points` 是经过边缘过滤 / 协议转换 / 质量规则后的
    /// 最终值，缓存只接受其中仍为 `Good` 的点。
    ///
    /// 被死区 / 变化阈值 / 聚合过滤掉（本轮未上报）或被质量规则降级的点**不写缓存**：
    /// 保留上一条上报口径值，由 TTL 自然过期。绝不回退到 `items` 里的采集原始值——
    /// 原始值未换算（量纲不同），直接喂给虚拟点位会让派生值忽大忽小、且与平台收到
    /// 的上报值对不上（I-28）。
    pub fn update_cache(&self, items: &[(PointConfig, DataPoint)], final_points: &[DataPoint]) {
        let now = DataPoint::now();
        // TTL 仍需按点位采集周期计算；interval 来自配置侧 items。
        let intervals: HashMap<&str, u64> = items
            .iter()
            .map(|(cfg, dp)| (dp.point_id.as_str(), cfg.interval))
            .collect();
        for dp in final_points {
            if dp.quality != Quality::Good {
                continue;
            }
            if let Some(interval) = intervals.get(dp.point_id.as_str()) {
                self.cache.put(&dp.point_id, dp.value, now, *interval);
            }
        }
    }

    /// 求值全部虚拟点位，返回需要产出的派生数据点
    ///
    /// 只在**值发生变化**（或首次求值 / 错误状态变化）时产出数据点。
    pub fn evaluate(&self) -> Vec<DataPoint> {
        let rules = self.rules.load();
        if rules.is_empty() {
            return Vec::new();
        }
        let now = DataPoint::now();
        // 级联求解的作用域 = 采集点缓存快照 + 已解出的虚拟点值。
        //
        // 性能：原实现在**每条规则**求值前都 `values.clone()` 再把 `resolved` 全量覆盖
        // 进去，于是每批次代价 ≈ 轮数 × 规则数 × 缓存条目的 String 克隆
        // （500 缓存 × 50 虚拟点 × 8 轮上限 ≈ 20 万次）。改为**增量维护同一份作用域**：
        // 采集点值只拷一次，虚拟点值在解出的瞬间插入。二者提供的可见性完全等价——
        // `resolved` 单调增长、从不删除，故「解出即加入」与「求值前全量重建」看到的
        // 键值集合相同（同名时虚拟点覆盖采集点，顺序也保持一致）。
        let mut scope = self.cache.snapshot(now);

        // 级联求解：虚拟点位可引用其它虚拟点位，按固定点迭代。
        // `resolved` 记录已求出的虚拟点位值（键为虚拟点位 ID）。
        let mut resolved: HashMap<String, f64> = HashMap::new();
        let mut pending: Vec<&Arc<VirtualPointRule>> = rules.iter().collect();
        let mut errors: HashMap<String, String> = HashMap::new();

        for _ in 0..MAX_CASCADE_ROUNDS {
            if pending.is_empty() {
                break;
            }
            let mut next_pending = Vec::new();
            let mut progressed = false;
            for &rule in pending.iter() {
                match &rule.compiled {
                    Err(e) => {
                        errors.insert(rule.id.clone(), e.clone());
                        progressed = true;
                    }
                    Ok(compiled) => {
                        match compiled.eval(&scope) {
                            Ok(v) => {
                                resolved.insert(rule.id.clone(), v);
                                // 立即并入作用域：同级后续规则 / 下一轮重试都能看到
                                scope.insert(rule.id.clone(), v);
                                progressed = true;
                            }
                            Err(e) => {
                                // 引用缺失可能是因为被引用的虚拟点位尚未解出，
                                // 留到下一轮再试；真正的错误（除零等）立即判定。
                                if let vformula::FormulaError::MissingPoint(_) = e {
                                    next_pending.push(rule);
                                } else {
                                    errors.insert(rule.id.clone(), e.to_string());
                                    progressed = true;
                                }
                            }
                        }
                    }
                }
            }
            if !progressed {
                // 本轮无任何进展：剩余规则全部因缺失引用（含环）无法求解
                for rule in &next_pending {
                    errors.insert(
                        rule.id.clone(),
                        "引用点位不可用（未采集 / 已过期 / 存在循环引用）".to_string(),
                    );
                }
                break;
            }
            pending = next_pending;
        }
        for rule in &pending {
            errors
                .entry(rule.id.clone())
                .or_insert_with(|| "引用点位不可用（未采集 / 已过期 / 存在循环引用）".to_string());
        }

        let mut out = Vec::new();
        let mut states = self.states.lock();
        for rule in rules.iter() {
            let prev = states.get(&rule.id).cloned().unwrap_or_default();
            if !rule.enabled {
                // 禁用的虚拟点位不参与求值，如实标记状态但不产出数据点
                states.insert(
                    rule.id.clone(),
                    VirtualPointState {
                        value: None,
                        error: Some("disabled".to_string()),
                        updated_at_ms: now,
                    },
                );
                continue;
            }
            let error = errors.get(&rule.id).cloned();
            let value = resolved
                .get(&rule.id)
                .copied()
                .map(|v| vformula::round_to_decimals(v, rule.decimals));
            states.insert(
                rule.id.clone(),
                VirtualPointState {
                    value,
                    error: error.clone(),
                    updated_at_ms: now,
                },
            );
            let changed = prev.value != value;
            let err_changed = prev.error != error;
            if !changed && !err_changed {
                continue;
            }
            match value {
                Some(v) => out.push(self.make_point(rule, v, now)),
                None => {
                    // 求值失败：产出 Uncertain 数据点，让平台感知异常而非静默消失
                    out.push(self.make_error_point(rule, now));
                }
            }
        }
        out
    }

    /// 立即对单个虚拟点位重算一次（供 Web 端「重算」按钮）
    ///
    /// 使用当前缓存中的点位值 + 其它虚拟点位的最近值作为输入。
    pub fn recalc(&self, id: &str) -> Option<VirtualPointState> {
        let rules = self.rules.load();
        let rule = rules.iter().find(|r| r.id == id)?;
        let now = DataPoint::now();
        if !rule.enabled {
            let state = VirtualPointState {
                value: None,
                error: Some("disabled".to_string()),
                updated_at_ms: now,
            };
            self.states.lock().insert(id.to_string(), state.clone());
            return Some(state);
        }
        let mut scope = self.cache.snapshot(now);
        // 其它虚拟点位的最近值参与级联
        {
            let states = self.states.lock();
            for (k, st) in states.iter() {
                if k == id {
                    continue;
                }
                if let Some(v) = st.value {
                    scope.insert(k.clone(), v);
                }
            }
        }
        let (value, error) = match &rule.compiled {
            Err(e) => (None, Some(e.clone())),
            Ok(compiled) => match compiled.eval(&scope) {
                Ok(v) => (Some(vformula::round_to_decimals(v, rule.decimals)), None),
                Err(e) => (None, Some(e.to_string())),
            },
        };
        let state = VirtualPointState {
            value,
            error,
            updated_at_ms: now,
        };
        self.states.lock().insert(id.to_string(), state.clone());
        Some(state)
    }

    /// 全部虚拟点位的最近求值状态快照（供 Web 层展示）
    pub fn states(&self) -> HashMap<String, VirtualPointState> {
        self.states.lock().clone()
    }

    fn make_point(&self, rule: &VirtualPointRule, value: f64, now: i64) -> DataPoint {
        DataPoint {
            point_id: rule.id.clone(),
            device_id: rule.device_id.clone(),
            value,
            raw_value: value,
            raw_type: "virtual".to_string(),
            quality: Quality::Good,
            quality_reason: String::new(),
            timestamp: now,
            source_ts: now,
            stale_count: 0,
            value_text: None,
        }
    }

    fn make_error_point(&self, rule: &VirtualPointRule, now: i64) -> DataPoint {
        DataPoint {
            point_id: rule.id.clone(),
            device_id: rule.device_id.clone(),
            value: 0.0,
            raw_value: 0.0,
            raw_type: "virtual".to_string(),
            quality: Quality::Uncertain,
            quality_reason: "virtual_point:ERROR".to_string(),
            timestamp: now,
            source_ts: now,
            stale_count: 0,
            value_text: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(id: &str, formula: &str) -> VirtualPoint {
        VirtualPoint {
            id: id.to_string(),
            name: id.to_string(),
            device_id: Some("dev1".to_string()),
            formula: formula.to_string(),
            unit: String::new(),
            decimals: 2,
            enabled: true,
            created_at: 0,
        }
    }

    fn item(id: &str, value: f64) -> (PointConfig, DataPoint) {
        let cfg = PointConfig {
            id: id.to_string(),
            interval: 10,
            ..PointConfig::default()
        };
        (cfg, DataPoint::good(id, "dev1", value, value, "f32"))
    }

    #[test]
    fn evaluates_formula_from_cached_values() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} * 2")]);
        let items = vec![item("p1", 21.0)];
        let final_points = vec![DataPoint::good("p1", "dev1", 21.0, 21.0, "f32")];
        engine.update_cache(&items, &final_points);
        let out = engine.evaluate();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].point_id, "vp1");
        assert_eq!(out[0].value, 42.0);
        assert_eq!(out[0].quality, Quality::Good);
        assert_eq!(out[0].device_id, "dev1");
    }

    /// DD-16：prune_cache 必须真正淘汰过期条目，虚拟点随之报「引用不可用」。
    #[test]
    fn prune_cache_removes_expired_inputs() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} * 2")]);
        let items = vec![item("p1", 21.0)];
        engine.update_cache(
            &items,
            &items.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        );
        // interval=10s → TTL=max(30s, 30s)；越过 TTL 后条目必须被真正移除
        engine.prune_cache(DataPoint::now() + 31_000);
        let out = engine.evaluate();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quality, Quality::Uncertain);
        assert_eq!(out[0].quality_reason, "virtual_point:ERROR");
    }

    #[test]
    fn unchanged_value_is_not_re_emitted() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} * 2")]);
        let items = vec![item("p1", 21.0)];
        engine.update_cache(
            &items,
            &items.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        );
        assert_eq!(engine.evaluate().len(), 1);
        // 第二次求值值未变 → 不再产出
        assert_eq!(engine.evaluate().len(), 0);
    }

    #[test]
    fn missing_reference_reports_error_point() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{nope} * 2")]);
        let out = engine.evaluate();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quality, Quality::Uncertain);
        assert_eq!(out[0].quality_reason, "virtual_point:ERROR");
        let states = engine.states();
        assert!(states["vp1"].error.is_some());
        assert!(states["vp1"].value.is_none());
    }

    #[test]
    fn invalid_formula_is_reported_not_panicked() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "1 +")]);
        let out = engine.evaluate();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quality, Quality::Uncertain);
        assert!(engine.states()["vp1"].error.is_some());
    }

    #[test]
    fn cascade_between_virtual_points_is_resolved() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} + 1"), vp("vp2", "{vp1} * 10")]);
        let items = vec![item("p1", 2.0)];
        engine.update_cache(
            &items,
            &items.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        );
        let out = engine.evaluate();
        let vp1 = out.iter().find(|d| d.point_id == "vp1").expect("vp1");
        let vp2 = out.iter().find(|d| d.point_id == "vp2").expect("vp2");
        assert_eq!(vp1.value, 3.0);
        assert_eq!(vp2.value, 30.0);
    }

    /// 回归（性能优化，语义等价性）：正序长级联链必须在**同一轮**内全部解出。
    ///
    /// 优化把「每条规则求值前重建 scope（values.clone + resolved 全量覆盖）」改成
    /// 「解出一个立刻并入 scope」。二者等价的关键就在**同级可见性**：前面规则刚解出
    /// 的值，后面的规则本轮必须能看见。若误改成「每轮开头统一注入」，这条 12 级链
    /// 每轮只能前进一级，8 轮（`MAX_CASCADE_ROUNDS`）后剩 4 个点解不出。
    #[test]
    fn deep_cascade_resolves_within_single_round() {
        const DEPTH: usize = 12;
        let mut rules = vec![vp("vp0", "{p1} + 1")];
        for i in 1..DEPTH {
            rules.push(vp(&format!("vp{i}"), &format!("{{vp{}}} + 1", i - 1)));
        }
        let engine = VirtualPointEngine::new(rules);
        let items = vec![item("p1", 2.0)];
        engine.update_cache(
            &items,
            &items.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        );

        let out = engine.evaluate();
        assert_eq!(out.len(), DEPTH, "12 级链应全部产出，实际: {}", out.len());
        for dp in &out {
            assert_eq!(
                dp.quality,
                Quality::Good,
                "{} 不应因级联轮数不足而失败: {:?}",
                dp.point_id,
                dp.quality_reason
            );
        }
        // vp0 = 2 + 1 = 3，逐级 +1 → vp11 = 14
        for i in 0..DEPTH {
            let dp = out
                .iter()
                .find(|d| d.point_id == format!("vp{i}"))
                .unwrap_or_else(|| panic!("vp{i} 缺失"));
            assert_eq!(dp.value, 3.0 + i as f64, "vp{i} 级联值错误");
        }
    }

    #[test]
    fn circular_reference_is_detected() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{vp2} + 1"), vp("vp2", "{vp1} + 1")]);
        let out = engine.evaluate();
        assert_eq!(out.len(), 2);
        for dp in &out {
            assert_eq!(dp.quality, Quality::Uncertain);
        }
    }

    #[test]
    fn decimals_are_applied() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} / 3")]);
        let items = vec![item("p1", 10.0)];
        engine.update_cache(
            &items,
            &items.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        );
        let out = engine.evaluate();
        assert_eq!(out[0].value, 3.33);
    }

    #[test]
    fn hot_reload_replaces_rules_and_keeps_state() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} * 2")]);
        let items = vec![item("p1", 21.0)];
        engine.update_cache(
            &items,
            &items.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        );
        engine.evaluate();
        assert_eq!(engine.states()["vp1"].value, Some(42.0));

        engine.update_rules(vec![vp("vp1", "{p1} * 3")]);
        assert_eq!(engine.rule_count(), 1);
        let out = engine.evaluate();
        assert_eq!(out[0].value, 63.0);
    }

    #[test]
    fn recalc_returns_current_state() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} * 2")]);
        let items = vec![item("p1", 4.0)];
        engine.update_cache(
            &items,
            &items.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(),
        );
        let st = engine.recalc("vp1").expect("state");
        assert_eq!(st.value, Some(8.0));
        assert!(st.error.is_none());
    }

    #[test]
    fn empty_engine_produces_nothing() {
        let engine = VirtualPointEngine::empty();
        assert_eq!(engine.rule_count(), 0);
        assert!(engine.evaluate().is_empty());
        assert!(engine.recalc("nope").is_none());
    }

    /// I-28（ROI-24）：缓存只存上报口径（换算后）值，不能用采集原始值。
    #[test]
    fn cache_uses_reported_caliber_not_raw_value() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} * 2")]);
        // 原始值 1000 → 上报口径 1.0（如 0.001 倍率）
        let items = vec![item("p1", 1000.0)];
        let reported = vec![DataPoint::good("p1", "dev1", 1.0, 1000.0, "f32")];
        engine.update_cache(&items, &reported);
        let out = engine.evaluate();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value, 2.0, "公式必须基于上报口径值 1.0");
    }

    /// I-28（ROI-24）：点位本轮被边缘过滤（未上报）时，缓存保留上一条上报口径值，
    /// 不得回退为本批采集原始值。
    #[test]
    fn filtered_point_keeps_last_reported_caliber() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} * 2")]);
        engine.update_cache(
            &[item("p1", 1000.0)],
            &[DataPoint::good("p1", "dev1", 1.0, 1000.0, "f32")],
        );
        assert_eq!(engine.evaluate()[0].value, 2.0);

        // 第二批：原始值变了，但被死区 / 变化阈值拦下 → final_points 为空
        engine.update_cache(&[item("p1", 1005.0)], &[]);
        assert!(
            engine.evaluate().is_empty(),
            "缓存应仍是上报口径 1.0，派生值不应因原始值变化而重算"
        );
    }

    /// I-28（ROI-24）：点位被质量规则降级（final 非 Good）时同样保留上一条上报口径值。
    #[test]
    fn downgraded_point_does_not_poison_cache_with_raw_value() {
        let engine = VirtualPointEngine::new(vec![vp("vp1", "{p1} + 1")]);
        engine.update_cache(
            &[item("p1", 100.0)],
            &[DataPoint::good("p1", "dev1", 10.0, 100.0, "f32")],
        );
        assert_eq!(engine.evaluate()[0].value, 11.0);

        let mut degraded = DataPoint::good("p1", "dev1", 20.0, 200.0, "f32");
        degraded.quality = Quality::Bad;
        engine.update_cache(&[item("p1", 200.0)], &[degraded]);
        assert!(
            engine.evaluate().is_empty(),
            "降级点不得把原始值 200 写进缓存（应保留 10.0）"
        );
    }
}
