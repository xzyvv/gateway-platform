//! 协议转换规则配置（C1：通用值变换模板）
//!
//! 定位：对标主流网关的「协议转换」能力，提供三档**免代码**模板，
//! 省去为常用值变换编写自定义脚本的功夫：
//!
//! - `raw`   ：直通模板——值不变换，仅把点位纳入转换分组（记账命中）。
//! - `linear`：线性换算 `value = value * scale + offset`。
//! - `enum`  ：枚举映射——数值命中 `enum_map.key` 时把文本写入
//!   `DataPoint.value_text`（数值保留，序列化时附加 `value_text` 字段）。
//!
//! 边界（v1 显式收窄）：
//! - `source_proto` / `target_proto` / `name` / `source_point_name` / `target_tag`
//!   是**管理元数据**（前端展示/分组/审计），运行时不参与匹配与桥接；
//!   唯一匹配键是 `source_point_id`（对应采集点位 `PointConfig.id`）。
//! - 转换结果经既有北向通道（MQTT/Webhook/Server）上送，不做协议间桥接。
//!
//! 运行时执行见 `crate::pipeline::convert::ConvertEngine`（入队/广播之前）。

use serde::{Deserialize, Serialize};

/// 枚举映射项：`key` 为数值字符串（须可解析为有限 f64），`label` 为显示文本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct ConvertEnumItemConfig {
    pub key: String,
    pub label: String,
}

/// 协议转换规则（持久化进 `config.json` 的 `convert_rules` 节）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct ConvertRuleConfig {
    /// 规则 ID（唯一，空串由 API 层生成）
    pub id: String,
    /// 规则名称（展示用）
    pub name: String,
    /// 源协议标签（元数据，运行时不参与匹配）
    pub source_proto: String,
    /// 源点位 ID（**唯一运行时匹配键**，对应 `PointConfig.id`）
    pub source_point_id: String,
    /// 源点位名称（展示用）
    #[serde(default)]
    pub source_point_name: String,
    /// 目标协议/分组标签（元数据，不做协议桥接）
    pub target_proto: String,
    /// 目标标签（元数据，如 MQTT topic 片段 / OPC UA 节点名，仅展示与分组）
    #[serde(default)]
    pub target_tag: String,
    /// 变换模板：`raw` / `linear` / `enum`
    pub transform: String,
    /// 线性换算系数（transform=linear 时必填且有限）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scale: Option<f64>,
    /// 线性换算偏移（transform=linear 时必填且有限）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub offset: Option<f64>,
    /// 枚举映射表（transform=enum 时必填非空）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enum_map: Option<Vec<ConvertEnumItemConfig>>,
    /// 是否启用（停用规则保留配置但不参与转换）
    pub enabled: bool,
    /// 创建时间（Unix 毫秒）
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

impl ConvertRuleConfig {
    /// fail-closed 校验：返回人类可读错误列表（空 = 合法）。
    ///
    /// 由 `GatewayConfig::validate_all` 与 API 保存路径共同调用，
    /// 保证「能配必能跑」：非法规则在保存期即被拒绝，不进入运行时。
    pub fn validate_rule(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.id.trim().is_empty() {
            errs.push("id 不能为空".to_string());
        }
        // 名称仅作展示/分组元数据，但空名会让前端列表与审计记录无法辨认，
        // 启动期与 API 保存路径共用本校验，空名/纯空白名一律拒绝（400）。
        if self.name.trim().is_empty() {
            errs.push("name 不能为空".to_string());
        }
        if self.source_point_id.trim().is_empty() {
            errs.push("source_point_id 不能为空（运行时唯一匹配键）".to_string());
        }
        match self.transform.as_str() {
            "raw" => {}
            "linear" => {
                // r29-TFM1 教训：scale/offset 必须有限，NaN/Inf 会污染工程量直送北向
                for (name, v) in [("scale", self.scale), ("offset", self.offset)] {
                    match v {
                        Some(x) if x.is_finite() => {}
                        Some(x) => errs.push(format!(
                            "transform=linear 时 {} 必须为有限数值，got {}",
                            name, x
                        )),
                        None => errs.push(format!("transform=linear 时 {} 必填", name)),
                    }
                }
            }
            "enum" => match &self.enum_map {
                Some(map) if !map.is_empty() => {
                    for item in map {
                        match item.key.trim().parse::<f64>() {
                            Ok(v) if v.is_finite() => {}
                            _ => errs.push(format!(
                                "enum_map key \"{}\" 不是有限数值（运行时按数值匹配）",
                                item.key
                            )),
                        }
                        if item.label.is_empty() {
                            errs.push(format!("enum_map key \"{}\" 的 label 为空", item.key));
                        }
                    }
                }
                _ => errs.push("transform=enum 时 enum_map 必填且非空".to_string()),
            },
            other => errs.push(format!(
                "transform 只接受 raw/linear/enum，got \"{}\"",
                other
            )),
        }
        errs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条除 name 外完全合法的规则，便于单测聚焦 name 校验。
    fn valid_rule_with_name(name: &str) -> ConvertRuleConfig {
        ConvertRuleConfig {
            id: "r1".to_string(),
            name: name.to_string(),
            source_proto: "modbus".to_string(),
            source_point_id: "p1".to_string(),
            source_point_name: String::new(),
            target_proto: "mqtt".to_string(),
            target_tag: String::new(),
            transform: "raw".to_string(),
            scale: None,
            offset: None,
            enum_map: None,
            enabled: true,
            created_at_ms: 0,
        }
    }

    #[test]
    fn empty_or_blank_name_rejected() {
        // 空 name 与纯空白 name 都必须被拒（API 层会因此返回 400）
        for name in ["", "   ", "\t\n "] {
            let errs = valid_rule_with_name(name).validate_rule();
            assert!(
                errs.iter().any(|e| e.contains("name 不能为空")),
                "name={:?} 应触发 name 不能为空，got {:?}",
                name,
                errs
            );
        }
        // 正常名字不应产生任何错误
        assert!(valid_rule_with_name("温度换算").validate_rule().is_empty());
    }
}
