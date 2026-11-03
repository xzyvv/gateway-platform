//! 队列记录结构（§10）
//!
//! QueuedItem 是 SQLite 队列中一条记录的内存映射，包含 Q13 扩展的
//! device_id / quality_reason / source_ts / stale_count 字段。

use serde::Serialize;

use crate::engine::model::DataPoint;

/// 队列中的一条记录
#[derive(Debug, Clone, Serialize)]
pub struct QueuedItem {
    pub id: i64,
    pub point_id: String,
    pub device_id: String,
    pub value: f64,
    pub raw_value: f64,
    pub raw_type: String,
    pub quality: String,
    pub quality_reason: String,
    pub timestamp: i64,
    pub source_ts: i64,
    pub stale_count: u32,
    /// 批次 C：枚举转换的文本标签（如 "运行"/"停机"），随数据落库。
    ///
    /// 契约：
    /// - `None`（落库为 NULL）表示「该点无枚举标签」或「该行写入时尚未支持标签」。
    /// - 标签在**入队时**由 pipeline 的转换引擎写入（见 `DataPoint.value_text`），
    ///   此处只做原样透传，查询期**不做规则回查**——保证历史数据口径不随
    ///   enum 映射演化而漂移。
    /// - 序列化风格与 DataPoint 一致：`None` 时省略该键，API 响应结构语义不变。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_text: Option<String>,
}

impl From<&QueuedItem> for DataPoint {
    fn from(item: &QueuedItem) -> Self {
        use crate::engine::model::Quality;
        let quality = match item.quality.as_str() {
            "uncertain" => Quality::Uncertain,
            "bad" => Quality::Bad,
            "stale" => Quality::Stale,
            _ => Quality::Good,
        };
        DataPoint {
            point_id: item.point_id.clone(),
            device_id: item.device_id.clone(),
            value: item.value,
            raw_value: item.raw_value,
            raw_type: item.raw_type.clone(),
            quality,
            quality_reason: item.quality_reason.clone(),
            timestamp: item.timestamp,
            source_ts: item.source_ts,
            stale_count: item.stale_count,
            // 批次 C：回填枚举标签（落库时写入的值，原样带回）
            value_text: item.value_text.clone(),
        }
    }
}

impl QueuedItem {
    /// #16: 由 DataPoint 构造 QueuedItem（透传发布用，id=0 表示非队列来源）。
    ///
    /// 仅填充 build_payload 所需的基础字段；rich 字段（device_id/point_name 等）
    /// 在透传路径下从 DataPoint 的对应字段读取由 build_payload 自行处理。
    pub fn from_datapoint(dp: &DataPoint) -> Self {
        Self {
            id: 0,
            point_id: dp.point_id.clone(),
            device_id: dp.device_id.clone(),
            value: dp.value,
            raw_value: dp.raw_value,
            raw_type: dp.raw_type.clone(),
            quality: dp.quality.as_str().to_string(),
            quality_reason: dp.quality_reason.clone(),
            timestamp: dp.timestamp,
            source_ts: dp.source_ts,
            stale_count: dp.stale_count,
            // 批次 C：透传枚举标签（pipeline 转换阶段已写入 DataPoint.value_text）
            value_text: dp.value_text.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
