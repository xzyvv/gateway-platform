//! 模板变量列表 + 变量映射
//!
//! 渲染预览已并入 `/api/uplink/test`（`send=false` 即只渲染不发送），
//! 本模块只保留变量目录与「JSON 值 → TemplateVars」映射，供上行测试复用。

use super::*;
use crate::web::services::{ExtraValue, TemplateVars};

pub async fn list_template_vars() -> Resp {
    let vars = vec![
        TemplateVarDto {
            name: "timestamp_ms".into(),
            var_type: "number",
            description: "上送时刻 UTC 毫秒（i64）",
            optional: false,
        },
        TemplateVarDto {
            name: "timestamp_iso".into(),
            var_type: "string",
            description: "上送时刻 ISO8601 字符串",
            optional: false,
        },
        TemplateVarDto {
            name: "source_ts".into(),
            var_type: "number",
            description: "设备侧原始采集时刻",
            optional: false,
        },
        TemplateVarDto {
            name: "device_id".into(),
            var_type: "string",
            description: "设备 ID",
            optional: false,
        },
        TemplateVarDto {
            name: "device_name".into(),
            var_type: "string",
            description: "设备显示名",
            optional: false,
        },
        TemplateVarDto {
            name: "device_group".into(),
            var_type: "string",
            description: "设备分组/产线",
            optional: false,
        },
        TemplateVarDto {
            name: "point_id".into(),
            var_type: "string",
            description: "点位 ID",
            optional: false,
        },
        TemplateVarDto {
            name: "point_name".into(),
            var_type: "string",
            description: "点位显示名（来自 PointConfig）",
            optional: false,
        },
        TemplateVarDto {
            name: "unit".into(),
            var_type: "string",
            description: "工程量单位",
            optional: false,
        },
        TemplateVarDto {
            name: "value".into(),
            var_type: "number",
            description: "工程量值（已应用 scale/offset 公式）",
            optional: false,
        },
        TemplateVarDto {
            name: "raw_value".into(),
            var_type: "number",
            description: "原始寄存器值（未换算）",
            optional: false,
        },
        TemplateVarDto {
            name: "quality_code".into(),
            var_type: "number",
            description: "质量码数字（0=Good / 1=Uncertain / 2=Bad / 3=Stale）",
            optional: false,
        },
        TemplateVarDto {
            name: "quality_text".into(),
            var_type: "string",
            description: "质量码小写字符串",
            optional: false,
        },
        TemplateVarDto {
            name: "quality_reason".into(),
            var_type: "string",
            description: "失败原因",
            optional: false,
        },
        TemplateVarDto {
            name: "stale_count".into(),
            var_type: "number",
            description: "连续失败重发次数",
            optional: false,
        },
        TemplateVarDto {
            name: "avg".into(),
            var_type: "number",
            description: "时间窗均值",
            optional: true,
        },
        TemplateVarDto {
            name: "max".into(),
            var_type: "number",
            description: "时间窗最大值",
            optional: true,
        },
        TemplateVarDto {
            name: "min".into(),
            var_type: "number",
            description: "时间窗最小值",
            optional: true,
        },
        TemplateVarDto {
            name: "last".into(),
            var_type: "number",
            description: "时间窗末值",
            optional: true,
        },
        TemplateVarDto {
            name: "sum".into(),
            var_type: "number",
            description: "时间窗累计值",
            optional: true,
        },
        TemplateVarDto {
            name: "count".into(),
            var_type: "number",
            description: "时间窗采样次数",
            optional: true,
        },
    ];
    ok(vars)
}

pub(crate) fn build_preview_vars(
    raw: &std::collections::HashMap<String, serde_json::Value>,
) -> TemplateVars {
    let mut vars = TemplateVars::new();
    for (k, v) in raw {
        apply_value(&mut vars, k, v);
    }
    vars
}

pub(crate) fn apply_value(vars: &mut TemplateVars, name: &str, value: &serde_json::Value) {
    match name {
        "timestamp_ms" => {
            if let Some(n) = value.as_i64() {
                vars.timestamp_ms = n;
            }
            return;
        }
        "timestamp_iso" => {
            if let Some(s) = value.as_str() {
                vars.timestamp_iso = s.to_string();
            }
            return;
        }
        "source_ts" => {
            if let Some(n) = value.as_i64() {
                vars.source_ts = n;
            }
            return;
        }
        "device_id" => {
            if let Some(s) = value.as_str() {
                vars.device_id = s.to_string();
            }
            return;
        }
        "device_name" => {
            if let Some(s) = value.as_str() {
                vars.device_name = s.to_string();
            }
            return;
        }
        "device_group" => {
            if let Some(s) = value.as_str() {
                vars.device_group = s.to_string();
            }
            return;
        }
        "point_id" => {
            if let Some(s) = value.as_str() {
                vars.point_id = s.to_string();
            }
            return;
        }
        "point_name" => {
            if let Some(s) = value.as_str() {
                vars.point_name = s.to_string();
            }
            return;
        }
        "unit" => {
            if let Some(s) = value.as_str() {
                vars.unit = s.to_string();
            }
            return;
        }
        "value" => {
            if let Some(n) = value.as_f64() {
                vars.value = n;
            }
            return;
        }
        "raw_value" => {
            if let Some(n) = value.as_f64() {
                vars.raw_value = n;
            }
            return;
        }
        "quality_code" => {
            if let Some(n) = value.as_i64() {
                vars.quality_code = n as i32;
            }
            return;
        }
        "quality_text" => {
            if let Some(s) = value.as_str() {
                vars.quality_text = s.to_string();
            }
            return;
        }
        "quality_reason" => {
            if let Some(s) = value.as_str() {
                vars.quality_reason = s.to_string();
            }
            return;
        }
        "stale_count" => {
            if let Some(n) = value.as_u64() {
                vars.stale_count = n as u32;
            }
            return;
        }
        "avg" => {
            if let Some(n) = value.as_f64() {
                vars.avg = Some(n);
            }
            return;
        }
        "max" => {
            if let Some(n) = value.as_f64() {
                vars.max = Some(n);
            }
            return;
        }
        "min" => {
            if let Some(n) = value.as_f64() {
                vars.min = Some(n);
            }
            return;
        }
        "last" => {
            if let Some(n) = value.as_f64() {
                vars.last = Some(n);
            }
            return;
        }
        "sum" => {
            if let Some(n) = value.as_f64() {
                vars.sum = Some(n);
            }
            return;
        }
        "count" => {
            if let Some(n) = value.as_u64() {
                vars.count = Some(n);
            }
            return;
        }
        _ => {}
    }
    let extra_val = match value {
        serde_json::Value::String(s) => ExtraValue::Str(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ExtraValue::Num(i as f64)
            } else if let Some(f) = n.as_f64() {
                ExtraValue::Num(f)
            } else {
                return;
            }
        }
        serde_json::Value::Bool(b) => ExtraValue::Bool(*b),
        _ => return,
    };
    vars.extra.insert(name.to_string(), extra_val);
}
