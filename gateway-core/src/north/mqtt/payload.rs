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

/// 解析上行报文的枚举文本标签（Bug 169 修复）
///
/// 优先级：
/// 1. `item.value_text` —— 入队时由 pipeline 转换引擎写下的标签，与实时值/历史表
///    **同源**，因此报文标签与历史查询结果必然一致；
/// 2. 回退到按当前规则集回查 —— 仅用于 `value_text` 为空的历史数据
///    （批次 C 之前落库的行，`value_text` 列为 NULL）。
///
/// 原实现跳过第 1 步、直接按 `raw_value` 回查当前规则，导致两个口径问题：
/// - **键不一致**：`ConvertEngine::apply` 用 `format!("{}", dp.value)` 查 enum_map，
///   这里用 `raw_value`。二者在「线性+枚举串联」或「点位 scale 后再枚举」时不同，
///   同一采样会出现历史有标签、报文没标签（或反之）。
/// - **快照漂移**：热重载 enum 映射后（如 `1 → 运行` 改为 `1 → 运转`），队列中
///   入队时的旧样本会在发送瞬间被按新规则重新贴标签 —— 同一采样的报文标签与
///   历史标签不一致；规则被删除后则连标签一起丢失。
///
/// 回退分支的取值同步对齐 `apply`，改用 `item.value` 而非 `raw_value`。
fn resolve_value_text(item: &QueuedItem, convert: Option<&ConvertEngine>) -> Option<String> {
    item.value_text
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| convert.and_then(|e| e.enum_label_for(&item.point_id, item.value)))
}

/// 把 QueuedItem + 可选 PointConfig 拼装成 TemplateVars（pub 供测试和上层使用）
///
/// `convert`：批次 B 注入的协议转换引擎，用于在 `value_text` 缺失时回查 enum 规则
/// 补文本（未命中/未注入则为 None，模板渲染为空串）。
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
        value_text: resolve_value_text(item, convert),
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
/// `convert`：批次 B 注入的协议转换引擎，用于在 `value_text` 缺失时回查 enum 标签；
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
                    value_text: resolve_value_text(item, convert),
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
