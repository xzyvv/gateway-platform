//! MQTT payload 构建逻辑（从 publish.rs 拆分）

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::north::template::{TemplateEngine, TemplateVars};
use crate::pipeline::ConvertEngine;
use crate::queue::QueuedItem;

use super::MqttClient;

/// MQTT 上报点位（避免 serde_json::Value 中间层，直接序列化结构体）
#[derive(serde::Serialize)]
struct PayloadPoint<'a> {
    point_id: &'a str,
    value: f64,
    quality: &'a str,
    quality_reason: &'a str,
    stale_count: u32,
    source_ts: i64,
    /// 批次 B：枚举转换文本（C1 enum 规则命中时由转换引擎回查补回）。
    /// None 时序列化省略该键——无枚举标签的点位报文 schema 与旧版完全一致。
    #[serde(skip_serializing_if = "Option::is_none")]
    value_text: Option<String>,
}

/// MQTT 上报报文（按设备分组）
#[derive(serde::Serialize)]
struct PayloadReport<'a> {
    ts: i64,
    device_id: &'a str,
    points: Vec<PayloadPoint<'a>>,
}

/// 构建 MQTT 上传 JSON 负载
///
/// 行为分两种模式：
/// 1. **自定义模板模式**（`config.payload_template` 已配置且编译成功）：
///    按点位循环渲染模板，每个点位输出一行（JSON Lines 风格），整体以 `\n` 拼接。
///    单条记录即一个 payload，单点失败不影响其他点。
///
/// 2. **默认 JSON 模式**（无自定义模板）：保持 Q10 历史 schema — 按 device_id 分组嵌套输出，
///    顶层为设备报文数组（向后兼容）。
///
/// 返回字节数组（首字节为压缩标志位，由 maybe_compress 包裹）。
impl MqttClient {
    /// 构建上行 payload（实例方法）
    ///
    /// 通过 `self.payload_template()` 决定走自定义模板模式还是默认 JSON 模式。
    pub(crate) fn build_payload(&self, items: &[QueuedItem]) -> Vec<u8> {
        // 批次 B：加载转换引擎（未注入则为 None，payload 不含 value_text）
        let convert = self.convert_engine.load();
        let convert = convert.as_deref();
        if let Some(engine) = self.payload_template() {
            self.build_payload_with_template(items, engine, convert)
        } else {
            build_payload_default(items, convert)
        }
    }

    /// 按点位循环渲染自定义模板 → JSON Lines（每行一条）
    fn build_payload_with_template(
        &self,
        items: &[QueuedItem],
        engine: &TemplateEngine,
        convert: Option<&ConvertEngine>,
    ) -> Vec<u8> {
        let configs = self.point_configs.read();
        let mut out: Vec<u8> = Vec::with_capacity(items.len() * 64);
        let mut rendered_count = 0usize;
        for item in items {
            let vars = template_vars_from_item(item, configs.get(&item.point_id), convert);
            let (line, missing) = engine.render_checked(&vars);
            // R3: 模板引用了查不到值的变量（拼写错误 / 缺少聚合配置）会静默渲染成
            // 空串，现场极难排查。首次命中时告警一次（swap 保证只告警一次）。
            if !missing.is_empty()
                && !self
                    .unresolved_template_warned
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                tracing::warn!(
                    "MQTT payload 模板存在未解析变量（将渲染为空串，请检查变量名拼写或聚合配置）: {}",
                    missing.join(", ")
                );
            }
            if line.is_empty() {
                // 模板渲染出空字符串（如模板只有注释）—— 跳过，避免空行噪声
                continue;
            }
            if !out.is_empty() {
                out.push(b'\n');
            }
            out.extend_from_slice(line.as_bytes());
            rendered_count += 1;
        }
        if rendered_count == 0 && !items.is_empty() {
            tracing::warn!(
                "build_payload: template produced 0 lines from {} items (template content may be empty)",
                items.len()
            );
        }
        out
    }
}

/// 把 QueuedItem + 可选 PointConfig 拼装成 TemplateVars（pub 供测试和上层使用）
///
/// `convert`：批次 B 注入的协议转换引擎，用于回查 enum 规则文本写入
/// `vars.value_text`（未命中/未注入则为 None，模板渲染为空串）。
pub(crate) fn template_vars_from_item(
    item: &QueuedItem,
    pc: Option<&PointConfig>,
    convert: Option<&ConvertEngine>,
) -> TemplateVars {
    let mut vars = TemplateVars {
        timestamp_ms: item.timestamp,
        source_ts: item.source_ts,
        device_id: item.device_id.clone(),
        point_id: item.point_id.clone(),
        value: item.value,
        raw_value: item.raw_value,
        quality_text: item.quality.clone(),
        quality_reason: item.quality_reason.clone(),
        stale_count: item.stale_count,
        // 批次 B：构建期按当前规则回查补 enum 标签。
        // 批次 C 起 QueuedItem 已带 value_text 列（入队时落库的标签），
        // 但 MQTT 报文的取值来源仍保持批次 B 的规则回查不变（本批次不动北向口径）。
        value_text: convert.and_then(|e| e.enum_label_for(&item.point_id, item.raw_value)),
        ..Default::default()
    };
    vars.timestamp_iso = chrono::DateTime::from_timestamp_millis(item.timestamp)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    // quality_code: 0=Good / 1=Uncertain / 2=Bad / 3=Stale
    vars.quality_code = match item.quality.as_str() {
        "good" => 0,
        "uncertain" => 1,
        "bad" => 2,
        "stale" => 3,
        _ => -1,
    };

    // PointConfig 提供的中文名/单位
    if let Some(p) = pc {
        if let Some(name) = &p.name {
            vars.point_name = name.clone();
        }
        if let Some(unit) = &p.unit {
            vars.unit = unit.clone();
        }
        if let Some(dev_id) = &p.device_id {
            // 优先级：PointConfig.device_id 覆盖 QueuedItem.device_id
            vars.device_id = dev_id.clone();
        }
    }
    vars
}

/// 默认 JSON 格式（保持 Q10 历史兼容）
///
/// `convert`：批次 B 注入的协议转换引擎；每个点位回查 enum 标签，
/// 命中时附加 `value_text` 键，未命中/未注入时键不出现（schema 不变）。
fn build_payload_default(items: &[QueuedItem], convert: Option<&ConvertEngine>) -> Vec<u8> {
    use std::collections::HashMap;
    let mut groups: HashMap<&str, Vec<&QueuedItem>> = HashMap::new();
    for item in items {
        groups
            .entry(item.device_id.as_str())
            .or_default()
            .push(item);
    }
    let reports: Vec<PayloadReport> = groups
        .into_iter()
        .map(|(device_id, group_items)| {
            let ts = DataPoint::now();
            let points: Vec<PayloadPoint> = group_items
                .iter()
                .map(|item| PayloadPoint {
                    point_id: item.point_id.as_str(),
                    value: item.value,
                    quality: item.quality.as_str(),
                    quality_reason: item.quality_reason.as_str(),
                    stale_count: item.stale_count,
                    source_ts: item.source_ts,
                    value_text: convert
                        .and_then(|e| e.enum_label_for(&item.point_id, item.raw_value)),
                })
                .collect();
            PayloadReport {
                ts,
                device_id,
                points,
            }
        })
        .collect();
    match serde_json::to_vec(&reports) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                "build_payload_default serialize failed ({} batches, {} points): {}",
                reports.len(),
                reports.iter().map(|r| r.points.len()).sum::<usize>(),
                e
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests;
