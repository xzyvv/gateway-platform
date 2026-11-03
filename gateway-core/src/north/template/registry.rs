//! 模板变量注册表：变量名 → 值的查找表
//!
//! 包含渲染上下文 [TemplateVars] 与扩展值类型 [ExtraValue]。
//! `lookup` 供 `renderer` 在渲染时按变量名取值。

use std::collections::HashMap;

use super::parser::TemplateEngine;

/// 模板渲染上下文（变量集）
#[derive(Debug, Clone, Default)]
pub struct TemplateVars {
    pub timestamp_ms: i64,
    pub timestamp_iso: String,
    pub source_ts: i64,
    pub device_id: String,
    pub device_name: String,
    pub device_group: String,
    pub point_id: String,
    pub point_name: String,
    pub unit: String,
    pub value: f64,
    pub raw_value: f64,
    pub quality_code: i32,
    pub quality_text: String,
    pub quality_reason: String,
    pub stale_count: u32,
    /// 枚举转换文本（C1 enum 规则命中的标签，如 "运行"）。
    /// 与 avg/max 等 Option 字段同风格：None 时 lookup 返回 None，
    /// 模板引用渲染为空串并计入未解析变量诊断。
    pub value_text: Option<String>,
    pub avg: Option<f64>,
    pub max: Option<f64>,
    pub min: Option<f64>,
    pub last: Option<f64>,
    pub sum: Option<f64>,
    pub count: Option<u64>,
    pub extra: HashMap<String, ExtraValue>,
}

/// r35-TPL3: 扩展变量数量上限。内置变量约 22 个，扩展项远超 256 属于异常
/// （配置错误或误用）。设置上限避免 `extra` HashMap 无界增长导致内存放大。
pub const MAX_EXTRA_ENTRIES: usize = 256;

/// 自定义扩展值类型
#[derive(Debug, Clone)]
pub enum ExtraValue {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl ExtraValue {
    fn render(&self) -> String {
        match self {
            ExtraValue::Str(s) => s.clone(),
            ExtraValue::Num(n) => super::render_num(*n),
            ExtraValue::Bool(b) => {
                if *b {
                    "true".into()
                } else {
                    "false".into()
                }
            }
        }
    }
}

impl TemplateVars {
    pub fn new() -> Self {
        Self {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            timestamp_iso: chrono::Utc::now().to_rfc3339(),
            ..Default::default()
        }
    }

    pub fn with_extra(mut self, key: impl Into<String>, val: ExtraValue) -> Self {
        // r35-TPL3: 数量上限保护。超出上限时拒绝新增键并告警（更新已有键不受限），
        // 避免无界增长；builder 无法返回 Result，故用 warn 而非静默丢弃。
        let key = key.into();
        if !self.extra.contains_key(&key) && self.extra.len() >= MAX_EXTRA_ENTRIES {
            tracing::warn!(
                "template extra vars exceed limit ({}), key '{}' ignored",
                MAX_EXTRA_ENTRIES,
                key
            );
            return self;
        }
        self.extra.insert(key, val);
        self
    }

    /// 按变量名取值（供 renderer 调用）
    pub(super) fn lookup(&self, name: &str) -> Option<String> {
        let s = match name {
            "timestamp_ms" => self.timestamp_ms.to_string(),
            "timestamp_iso" => self.timestamp_iso.clone(),
            "source_ts" => self.source_ts.to_string(),
            "device_id" => self.device_id.clone(),
            "device_name" => self.device_name.clone(),
            "device_group" => self.device_group.clone(),
            "point_id" => self.point_id.clone(),
            "point_name" => self.point_name.clone(),
            "unit" => self.unit.clone(),
            "value" => return Some(super::render_num(self.value)),
            "raw_value" => return Some(super::render_num(self.raw_value)),
            "quality_code" => self.quality_code.to_string(),
            "quality_text" => self.quality_text.clone(),
            "quality_reason" => self.quality_reason.clone(),
            "stale_count" => self.stale_count.to_string(),
            "value_text" => return self.value_text.clone(),
            "avg" => return self.avg.map(super::render_num),
            "max" => return self.max.map(super::render_num),
            "min" => return self.min.map(super::render_num),
            "last" => return self.last.map(super::render_num),
            "sum" => return self.sum.map(super::render_num),
            "count" => return self.count.map(|n| n.to_string()),
            _ => {
                return self.extra.get(name).map(|v| v.render());
            }
        };
        Some(s)
    }

    /// 用指定 engine 渲染（便利方法）
    pub fn render_with_engine(&self, engine: &TemplateEngine) -> String {
        engine.render(self)
    }
}
