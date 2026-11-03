//! 协议转换引擎（C1：通用值变换模板运行时）
//!
//! 挂在处理管道 **Lua 之后、实时广播/入队之前**（见 `pipeline/mod.rs`），
//! 对边缘过滤 + Lua 计算后的最终值执行三档免代码模板：
//!
//! - `raw`   ：直通——值不变换，仅记账命中（把点位纳入转换分组）。
//! - `linear`：`value = value * scale + offset`（结果非有限时降级 Uncertain，不落脏数据）。
//! - `enum`  ：数值命中 `enum_map.key` 时把文本写入 `DataPoint.value_text`（数值保留）。
//!
//! 匹配键唯一：`source_point_id == DataPoint.point_id`，且仅处理 `Quality::Good` 点。
//! `source_proto` / `target_proto` / `target_tag` / `name` 是管理元数据，不参与运行时。
//!
//! 热重载：`Gateway::reload_config` 检测 `convert_rules` 差异后调 `update_rules`
//! 原子替换规则集（ArcSwap），命中记账（hits）跨重载保留。

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;

use crate::config::ConvertRuleConfig;
use crate::engine::model::{DataPoint, Quality};

/// 单条规则的命中记账（API list 时合并进 DTO 展示「最近一次转换」）
#[derive(Debug, Clone)]
pub struct ConvertHit {
    /// 转换后的展示值（linear=换算后数值；enum=映射文本；raw=原值）
    pub last_value: String,
    /// 命中时刻（UTC 毫秒）
    pub last_converted_at_ms: i64,
}

/// 编译后的规则（运行态）：enum_map 预解析为 HashMap，transform 已收窄为枚举。
///
/// 配置合法性由 `ConvertRuleConfig::validate_rule` 在保存期保证（fail-closed）；
/// 此处编译是**防御深度**：配置被手工改坏并绕过校验直接落盘时，
/// 非法规则在编译期跳过 + warn，不进入运行态。
struct RuleCompiled {
    id: String,
    source_point_id: String,
    transform: Transform,
}

enum Transform {
    Raw,
    Linear { scale: f64, offset: f64 },
    Enum(HashMap<String, String>),
}

impl RuleCompiled {
    fn compile(rule: &ConvertRuleConfig) -> Option<Self> {
        if !rule.enabled {
            return None;
        }
        let transform = match rule.transform.as_str() {
            "raw" => Transform::Raw,
            "linear" => {
                let (Some(scale), Some(offset)) = (rule.scale, rule.offset) else {
                    tracing::warn!("convert rule {}: linear 缺 scale/offset，跳过", rule.id);
                    return None;
                };
                if !scale.is_finite() || !offset.is_finite() {
                    tracing::warn!("convert rule {}: linear scale/offset 非有限，跳过", rule.id);
                    return None;
                }
                Transform::Linear { scale, offset }
            }
            "enum" => {
                let map: HashMap<String, String> = rule
                    .enum_map
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .filter(|item| {
                        item.key
                            .trim()
                            .parse::<f64>()
                            .map(|v| v.is_finite())
                            .unwrap_or(false)
                            && !item.label.is_empty()
                    })
                    .map(|item| (item.key.trim().to_string(), item.label.clone()))
                    .collect();
                if map.is_empty() {
                    tracing::warn!("convert rule {}: enum_map 为空或全部非法，跳过", rule.id);
                    return None;
                }
                Transform::Enum(map)
            }
            other => {
                tracing::warn!(
                    "convert rule {}: 未知 transform \"{}\"，跳过",
                    rule.id,
                    other
                );
                return None;
            }
        };
        if rule.source_point_id.trim().is_empty() {
            tracing::warn!("convert rule {}: source_point_id 为空，跳过", rule.id);
            return None;
        }
        Some(Self {
            id: rule.id.clone(),
            source_point_id: rule.source_point_id.clone(),
            transform,
        })
    }
}

/// 协议转换引擎（线程安全；apply 只读规则快照，热重载无锁切换）
pub struct ConvertEngine {
    rules: ArcSwap<Vec<Arc<RuleCompiled>>>,
    hits: Mutex<HashMap<String, ConvertHit>>,
}

impl ConvertEngine {
    /// 从配置构建引擎（非法规则跳过 + warn，见 `RuleCompiled::compile`）
    pub fn new(rules: Vec<ConvertRuleConfig>) -> Arc<Self> {
        Arc::new(Self {
            rules: ArcSwap::from_pointee(Self::compile_all(rules)),
            hits: Mutex::new(HashMap::new()),
        })
    }

    /// 空引擎（测试/无配置场景；apply 早退零开销）
    pub fn empty() -> Arc<Self> {
        Self::new(Vec::new())
    }

    fn compile_all(rules: Vec<ConvertRuleConfig>) -> Vec<Arc<RuleCompiled>> {
        rules
            .iter()
            .filter_map(|r| RuleCompiled::compile(r).map(Arc::new))
            .collect()
    }

    /// 热重载：原子替换规则集；命中记账保留（规则 id 不变则记账延续）
    pub fn update_rules(&self, rules: Vec<ConvertRuleConfig>) {
        self.rules.store(Arc::new(Self::compile_all(rules)));
    }

    /// 规则数（编译后、仅启用的）
    pub fn rule_count(&self) -> usize {
        self.rules.load().len()
    }

    /// 对一批数据点执行转换模板（仅 Good 点；空规则集早退）
    pub fn apply(&self, points: &mut [DataPoint]) {
        let rules = self.rules.load();
        if rules.is_empty() {
            return;
        }
        let now = DataPoint::now();
        for dp in points.iter_mut() {
            if dp.quality != Quality::Good {
                continue;
            }
            for rule in rules.iter() {
                if rule.source_point_id != dp.point_id {
                    continue;
                }
                match &rule.transform {
                    Transform::Raw => {
                        self.record_hit(&rule.id, format!("{}", dp.value), now);
                    }
                    Transform::Linear { scale, offset } => {
                        let v = dp.value * scale + offset;
                        if v.is_finite() {
                            dp.value = v;
                            self.record_hit(&rule.id, format!("{}", v), now);
                        } else {
                            // 防御深度：换算结果 NaN/Inf 时降级，不让脏数据进北向/落库
                            dp.quality = Quality::Uncertain;
                            dp.quality_reason = "convert_non_finite".to_string();
                            tracing::warn!(
                                "convert rule {}: linear({}*{}+{}) 非有限，点 {} 降级 Uncertain",
                                rule.id,
                                dp.value,
                                scale,
                                offset,
                                dp.point_id
                            );
                        }
                    }
                    Transform::Enum(map) => {
                        // Rust f64 Display 已归一化整数（1.0 → "1"），一次查找同时覆盖
                        // 精确匹配（1.5 → "1.5"）与整数匹配（1.0 命中 key "1"）。
                        let key = format!("{}", dp.value);
                        if let Some(label) = map.get(&key) {
                            dp.value_text = Some(label.clone());
                            self.record_hit(&rule.id, label.clone(), now);
                        }
                    }
                }
            }
        }
    }

    /// 回查某点位某原始值在当前 enum 规则下的文本标签（北向 payload 构建期补 value_text 用）。
    ///
    /// 背景：enum 转换在 `apply` 时把标签写入 `DataPoint.value_text`，但 QueuedItem
    /// 无该字段，入队即丢；北向（MQTT 等）从 QueuedItem 构建报文时调用本方法补回。
    ///
    /// 判定口径与 `apply` 的 enum 分支完全一致：读**当前**规则集（ArcSwap 快照，
    /// 热重载后即新规则），`source_point_id == point_id`，且
    /// `format!("{}", raw_value)`（Display 归一化：1.0 → "1"）命中 enum_map 的 key。
    /// 不命中/无规则返回 None；只读不记账（不污染 hits）。
    pub fn enum_label_for(&self, point_id: &str, raw_value: f64) -> Option<String> {
        let rules = self.rules.load();
        if rules.is_empty() {
            return None;
        }
        let key = format!("{}", raw_value);
        for rule in rules.iter() {
            if rule.source_point_id != point_id {
                continue;
            }
            if let Transform::Enum(map) = &rule.transform {
                if let Some(label) = map.get(&key) {
                    return Some(label.clone());
                }
            }
        }
        None
    }

    fn record_hit(&self, rule_id: &str, last_value: String, at_ms: i64) {
        self.hits.lock().insert(
            rule_id.to_string(),
            ConvertHit {
                last_value,
                last_converted_at_ms: at_ms,
            },
        );
    }

    /// 命中记账快照（rule_id → 最近一次转换），供 API list 合并展示
    pub fn hits_snapshot(&self) -> HashMap<String, ConvertHit> {
        self.hits.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConvertEnumItemConfig;

    fn rule(id: &str, point_id: &str, transform: &str) -> ConvertRuleConfig {
        ConvertRuleConfig {
            id: id.to_string(),
            name: id.to_string(),
            source_proto: "modbus-tcp".to_string(),
            source_point_id: point_id.to_string(),
            source_point_name: String::new(),
            target_proto: "mqtt".to_string(),
            target_tag: String::new(),
            transform: transform.to_string(),
            scale: None,
            offset: None,
            enum_map: None,
            enabled: true,
            created_at_ms: 0,
        }
    }

    fn dp(point_id: &str, value: f64) -> DataPoint {
        DataPoint::good(point_id, "dev1", value, value, "f32")
    }

    #[test]
    fn raw_rule_records_hit_without_changing_value() {
        let engine = ConvertEngine::new(vec![rule("cv1", "p1", "raw")]);
        let mut points = vec![dp("p1", 25.3)];
        engine.apply(&mut points);
        assert_eq!(points[0].value, 25.3);
        assert!(points[0].value_text.is_none());
        let hits = engine.hits_snapshot();
        assert_eq!(hits["cv1"].last_value, "25.3");
        assert!(hits["cv1"].last_converted_at_ms > 0);
    }

    #[test]
    fn linear_rule_scales_value() {
        let mut r = rule("cv2", "p1", "linear");
        r.scale = Some(100.0);
        r.offset = Some(1.5);
        let engine = ConvertEngine::new(vec![r]);
        let mut points = vec![dp("p1", 1.6)];
        engine.apply(&mut points);
        assert_eq!(points[0].value, 161.5);
        assert_eq!(engine.hits_snapshot()["cv2"].last_value, "161.5");
    }

    #[test]
    fn enum_rule_writes_value_text_and_keeps_numeric() {
        let mut r = rule("cv3", "p1", "enum");
        r.enum_map = Some(vec![
            ConvertEnumItemConfig {
                key: "0".to_string(),
                label: "停机".to_string(),
            },
            ConvertEnumItemConfig {
                key: "1".to_string(),
                label: "运行".to_string(),
            },
        ]);
        let engine = ConvertEngine::new(vec![r]);
        // 浮点 1.0 应命中 key "1"（Display 归一化）
        let mut points = vec![dp("p1", 1.0)];
        engine.apply(&mut points);
        assert_eq!(points[0].value, 1.0);
        assert_eq!(points[0].value_text.as_deref(), Some("运行"));
        assert_eq!(engine.hits_snapshot()["cv3"].last_value, "运行");

        // 非整数精确匹配
        let mut r2 = rule("cv4", "p2", "enum");
        r2.enum_map = Some(vec![ConvertEnumItemConfig {
            key: "1.5".to_string(),
            label: "半速".to_string(),
        }]);
        let engine2 = ConvertEngine::new(vec![r2]);
        let mut points2 = vec![dp("p2", 1.5)];
        engine2.apply(&mut points2);
        assert_eq!(points2[0].value_text.as_deref(), Some("半速"));

        // 未命中不写 value_text、不更新记账（上次命中保留）
        let mut points3 = vec![dp("p2", 9.0)];
        engine2.apply(&mut points3);
        assert!(points3[0].value_text.is_none());
        assert_eq!(engine2.hits_snapshot()["cv4"].last_value, "半速");
    }

    #[test]
    fn only_good_points_and_matching_id_are_converted() {
        let mut r = rule("cv5", "p1", "linear");
        r.scale = Some(10.0);
        r.offset = Some(0.0);
        let engine = ConvertEngine::new(vec![r]);
        let mut bad = dp("p1", 5.0);
        bad.quality = Quality::Bad;
        let mut other = dp("p_other", 5.0);
        let mut points = vec![bad.clone(), other.clone()];
        engine.apply(&mut points);
        assert_eq!(points[0].value, 5.0); // Bad 点直通
        assert_eq!(points[1].value, 5.0); // point_id 不匹配
        assert!(engine.hits_snapshot().is_empty());
        // 引用保持，消除未使用告警
        let _ = (&mut bad, &mut other);
    }

    #[test]
    fn invalid_rules_are_skipped_at_compile() {
        // linear 缺 scale/offset → 跳过
        let engine = ConvertEngine::new(vec![rule("bad1", "p1", "linear")]);
        assert_eq!(engine.rule_count(), 0);
        // 未知 transform → 跳过
        let engine = ConvertEngine::new(vec![rule("bad2", "p1", "wat")]);
        assert_eq!(engine.rule_count(), 0);
        // enum_map 空 → 跳过
        let engine = ConvertEngine::new(vec![rule("bad3", "p1", "enum")]);
        assert_eq!(engine.rule_count(), 0);
        // 禁用 → 跳过
        let mut disabled = rule("bad4", "p1", "raw");
        disabled.enabled = false;
        let engine = ConvertEngine::new(vec![disabled]);
        assert_eq!(engine.rule_count(), 0);
    }

    #[test]
    fn update_rules_swaps_atomically_and_keeps_hits() {
        let engine = ConvertEngine::new(vec![rule("cv6", "p1", "raw")]);
        let mut points = vec![dp("p1", 1.0)];
        engine.apply(&mut points);
        assert_eq!(engine.rule_count(), 1);

        // 热重载：换成 linear；同 id 记账保留
        let mut r = rule("cv6", "p1", "linear");
        r.scale = Some(2.0);
        r.offset = Some(0.0);
        engine.update_rules(vec![r]);
        assert_eq!(engine.rule_count(), 1);
        let mut points = vec![dp("p1", 3.0)];
        engine.apply(&mut points);
        assert_eq!(points[0].value, 6.0);
        assert_eq!(engine.hits_snapshot()["cv6"].last_value, "6");

        // 清空规则集后早退
        engine.update_rules(vec![]);
        assert_eq!(engine.rule_count(), 0);
        let mut points = vec![dp("p1", 3.0)];
        engine.apply(&mut points);
        assert_eq!(points[0].value, 3.0);
    }

    #[test]
    fn enum_label_for_matches_current_rules() {
        let mut r = rule("cv8", "p1", "enum");
        r.enum_map = Some(vec![
            ConvertEnumItemConfig {
                key: "0".to_string(),
                label: "停机".to_string(),
            },
            ConvertEnumItemConfig {
                key: "1".to_string(),
                label: "运行".to_string(),
            },
        ]);
        let engine = ConvertEngine::new(vec![r]);

        // 命中：整数 Display 归一化（1.0 命中 key "1"）
        assert_eq!(engine.enum_label_for("p1", 1.0).as_deref(), Some("运行"));
        assert_eq!(engine.enum_label_for("p1", 0.0).as_deref(), Some("停机"));
        // 未命中：值不在映射表 / point_id 不匹配 / 非 enum 规则点位
        assert!(engine.enum_label_for("p1", 9.0).is_none());
        assert!(engine.enum_label_for("p_other", 1.0).is_none());
        // 空引擎
        assert!(ConvertEngine::empty().enum_label_for("p1", 1.0).is_none());

        // 热重载后读当前规则（不是构建期快照）
        let mut r2 = rule("cv8", "p1", "enum");
        r2.enum_map = Some(vec![ConvertEnumItemConfig {
            key: "1".to_string(),
            label: "运转".to_string(),
        }]);
        engine.update_rules(vec![r2]);
        assert_eq!(engine.enum_label_for("p1", 1.0).as_deref(), Some("运转"));
    }

    #[test]
    fn linear_non_finite_result_degrades_to_uncertain() {
        let mut r = rule("cv7", "p1", "linear");
        r.scale = Some(f64::MAX);
        r.offset = Some(0.0);
        let engine = ConvertEngine::new(vec![r]);
        let mut points = vec![dp("p1", f64::MAX)];
        engine.apply(&mut points);
        assert_eq!(points[0].quality, Quality::Uncertain);
        assert_eq!(points[0].quality_reason, "convert_non_finite");
    }
}
