//! 核心枚举定义（§3.11）
//!
//! 所有枚举对应 JSON 字符串，用于配置反序列化与运行时数据表达。

use serde::{Deserialize, Serialize};

/// 数据质量码（§7.1 四级质量码）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// 采集成功
    Good = 0,
    /// 采集成功但有疑点（BCD 含非法值、校验和 borderline）
    Uncertain = 1,
    /// 采集失败（超时、校验错误、帧格式错误）
    Bad = 2,
    /// 数据过时（退避中）
    Stale = 3,
}

impl Quality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Quality::Good => "good",
            Quality::Uncertain => "uncertain",
            Quality::Bad => "bad",
            Quality::Stale => "stale",
        }
    }
}

/// Modbus 数据类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModbusDataType {
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

impl ModbusDataType {
    /// 根据数据类型返回占用的寄存器数（§9.4）
    pub fn register_count(&self) -> u16 {
        match self {
            ModbusDataType::U16 | ModbusDataType::I16 => 1,
            ModbusDataType::U32 | ModbusDataType::I32 | ModbusDataType::F32 => 2,
            ModbusDataType::U64 | ModbusDataType::I64 | ModbusDataType::F64 => 4,
        }
    }
}

/// 字节序（§9.4），默认 ABCD
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
pub enum ByteOrder {
    /// 大端（默认）
    #[serde(rename = "ABCD")]
    #[default]
    Abcd,
    /// 小端
    #[serde(rename = "DCBA")]
    Dcba,
    /// 字交换大端
    #[serde(rename = "CDAB")]
    Cdab,
    /// 字交换小端
    #[serde(rename = "BADC")]
    Badc,
}

/// 聚合函数（§3.6）
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum AggFn {
    Last,
    Avg,
    Max,
    Min,
    Sum,
}

/// 运行时采集数据点（§3.7）
///
/// 全库共享领域模型：south 各协议采集器产出、north 各 sink/server 消费、
/// queue/pipeline/ha 与 gateway-api 的 web 面中转。原位于 south/protocol/common/model.rs，
/// 因 north 反向依赖 south 而上提至 engine 中立层。
///
/// Q13 扩展字段：
/// - `device_id`：所属设备 ID（按设备分组上送）
/// - `quality_reason`：失败原因字符串（device_timeout/parse_error/empty_window 等）
/// - `source_ts`：原始采集时刻（UTC 毫秒），stale_count>0 时为 last_good 的原始采集时间
/// - `stale_count`：连续重发次数（采集失败时复用 last_good 值，stale_count+=1）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataPoint {
    /// 对应 PointConfig.id
    pub point_id: String,
    /// 所属设备 ID（按设备分组上送用）
    pub device_id: String,
    /// 工程量值（经公式/scale换算后）
    pub value: f64,
    /// 原始解析值
    pub raw_value: f64,
    /// 原始类型：u16/i16/u32/i32/f32/u64/unknown
    pub raw_type: String,
    /// 质量码
    pub quality: Quality,
    /// 质量码原因（失败原因：device_timeout/parse_error/empty_window 等）
    #[serde(default)]
    pub quality_reason: String,
    /// 上送时刻时间戳（UTC 毫秒），平台按此排序
    pub timestamp: i64,
    /// 原始采集时刻（UTC 毫秒），诊断用，stale_count>0 时为 last_good 的原始采集时间
    #[serde(default = "default_source_ts")]
    pub source_ts: i64,
    /// 连续重发次数（采集失败时复用 last_good 值，stale_count+=1）
    #[serde(default)]
    pub stale_count: u32,
    /// 枚举转换文本（C1 协议转换 enum 模板命中时写入，如 "运行"/"故障"）。
    ///
    /// 追加式字段：None 时序列化省略该键，MQTT/RealtimeBus/前端契约零影响；
    /// 数值 `value` 始终保留（枚举文本是附加视图，不替代工程量）。
    /// 批次 C 起随数据落库（`data_queue.value_text` 列），使 /api/history 能返回
    /// 枚举标签；落库为 NULL 表示「该点无枚举标签」。
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_text: Option<String>,
}

fn default_source_ts() -> i64 {
    0
}

impl DataPoint {
    pub fn now() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// 构造成功数据点（source_ts = now）
    ///
    /// 含 device_id 参数。
    ///
    /// r24-MOD1（防御深度）：value/raw_value 若非有限（NaN/+Inf/-Inf），
    /// 则回退为 `Quality::Uncertain` 并把无效值替换为 0.0，
    /// quality_reason 标注 "non_finite_value"。这样：
    ///   - NaN/Inf 不会进入 MQTT/Kafka payload（serde_json 对 NaN 序列化为 null，
    ///     下游 InfluxDB / Kafka Connect 通常拒收）
    ///   - SQLite REAL 列不会存 NaN（聚合/比较异常）
    ///   - alarm 评估阶段仍看到 0.0（finite），不会因 r21-ALM2 NaN 守卫
    ///     把整条记录静默跳过
    ///
    /// 注：`convert_value()`（r24-DEC1）已在出口层兜底，此处为防御深度，
    ///     覆盖未走 convert_value 的协议（gpio/skeleton 等）。
    pub fn good(
        point_id: &str,
        device_id: &str,
        value: f64,
        raw_value: f64,
        raw_type: &str,
    ) -> Self {
        let ts = Self::now();
        // 独立校验 value / raw：任一非 finite 仅清零对应字段，保留另一字段的合法值。
        // 原因字段优先记录 value 问题（value 直接影响报警 / 上行）。
        let (safe_value, value_reason) = if value.is_finite() {
            (value, None)
        } else {
            tracing::warn!(
                "DataPoint::good for point '{}' received non-finite value={}, replace with 0.0",
                point_id,
                value
            );
            (0.0, Some("non_finite_value"))
        };
        let (safe_raw, raw_reason) = if raw_value.is_finite() {
            (raw_value, None)
        } else {
            tracing::warn!(
                "DataPoint::good for point '{}' received non-finite raw_value={}, replace with 0.0",
                point_id,
                raw_value
            );
            (0.0, Some("non_finite_raw_value"))
        };
        // 任一非 finite 都降级为 Uncertain（value 非有限直接污染工程量，必须降级）
        let (quality, reason) = match (value_reason, raw_reason) {
            (None, None) => (Quality::Good, String::new()),
            (Some(v), _) => (Quality::Uncertain, v.to_string()),
            (None, Some(r)) => (Quality::Uncertain, r.to_string()),
        };
        Self {
            point_id: point_id.to_string(),
            device_id: device_id.to_string(),
            value: safe_value,
            raw_value: safe_raw,
            raw_type: raw_type.to_string(),
            quality,
            quality_reason: reason,
            timestamp: ts,
            source_ts: ts,
            stale_count: 0,
            value_text: None,
        }
    }

    /// 检查浮点值是否合法（非 NaN/Inf）
    pub fn is_value_valid(v: f64) -> bool {
        v.is_finite()
    }

    /// 构造失败数据点（value=last_good，quality=对应质量码，source_ts=last_good 的原始采集时间）
    ///
    /// 新签名（Q13）：含 device_id、last_source_ts、reason、stale_count。
    pub fn failed(
        point_id: &str,
        device_id: &str,
        last_value: Option<f64>,
        last_source_ts: i64,
        quality: Quality,
        reason: &str,
        stale_count: u32,
    ) -> Self {
        let safe_last = last_value.filter(|v| v.is_finite()).unwrap_or(0.0);
        Self {
            point_id: point_id.to_string(),
            device_id: device_id.to_string(),
            value: safe_last,
            raw_value: 0.0,
            raw_type: "unknown".to_string(),
            quality,
            quality_reason: reason.to_string(),
            timestamp: Self::now(),
            source_ts: last_source_ts,
            stale_count,
            value_text: None,
        }
    }
}

/// 按协议地址直读的单点结果（未建点位场景下的调试读值）
///
/// 与 [`DataPoint`] 的区别：`DataPoint` 是**已建点位**的采集产物（带 point_id/
/// device_id/工程量换算），本结构是**按协议原始地址**（如 IEC104 的 IOA）直读的
/// 调试读值，不经过点位配置、公式换算与命名。
///
/// 未命中语义：**`value` 为 `None` 表示该地址本轮总召唤未上报**，
/// 绝不静默返回 0（0 是合法工程量，静默 0 会把「没这个点」误读成「值为 0」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressReadEntry {
    /// 协议原始地址（IEC104 为 IOA）
    pub address: u32,
    /// 读到的值；未命中为 None
    pub value: Option<f64>,
    /// 质量码；未命中为 None
    pub quality: Option<Quality>,
    /// 源时标（UTC 毫秒），无时标时为 0
    #[serde(default)]
    pub source_ts: i64,
}

/// 按地址直读结果：命中项 + 本轮全量快照
///
/// `snapshot` 为本轮总召唤返回的全部 IOA（未命中目标也能看到现场到底有哪些点），
/// 按地址升序排列，保证前端渲染稳定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressReadResult {
    /// 与请求地址列表一一对应的命中结果（保持请求顺序）
    pub entries: Vec<AddressReadEntry>,
    /// 本轮全量快照（按地址升序）
    pub snapshot: Vec<AddressReadEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// r24-MOD1：合法 finite 值应原样保留为 Good。
    #[test]
    fn good_passes_through_finite_values() {
        let p = DataPoint::good("p1", "d1", 42.5, 850.0, "u16");
        assert_eq!(p.quality, Quality::Good);
        assert_eq!(p.value, 42.5);
        assert_eq!(p.raw_value, 850.0);
        assert_eq!(p.quality_reason, "");
    }

    /// r24-MOD1：NaN value 必须降级为 Uncertain(0.0)，不可标记为 Good。
    #[test]
    fn good_demotes_nan_value_to_uncertain() {
        let p = DataPoint::good("p_nan", "d1", f64::NAN, 100.0, "u16");
        assert_eq!(p.quality, Quality::Uncertain);
        assert_eq!(p.value, 0.0, "NaN 应被替换为 0.0");
        assert_eq!(p.raw_value, 100.0);
        assert_eq!(p.quality_reason, "non_finite_value");
    }

    /// r24-MOD1：+Inf value 必须降级为 Uncertain(0.0)。
    #[test]
    fn good_demotes_positive_inf_to_uncertain() {
        let p = DataPoint::good("p_inf", "d1", f64::INFINITY, 50.0, "f32");
        assert_eq!(p.quality, Quality::Uncertain);
        assert_eq!(p.value, 0.0);
        assert_eq!(p.quality_reason, "non_finite_value");
    }

    /// r24-MOD1：-Inf value 必须降级为 Uncertain(0.0)。
    #[test]
    fn good_demotes_negative_inf_to_uncertain() {
        let p = DataPoint::good("p_ninf", "d1", f64::NEG_INFINITY, 50.0, "f32");
        assert_eq!(p.quality, Quality::Uncertain);
        assert_eq!(p.value, 0.0);
        assert_eq!(p.quality_reason, "non_finite_value");
    }

    /// r24-MOD1：value 合法但 raw 非有限时，记录 raw 原因。
    #[test]
    fn good_demotes_non_finite_raw_with_distinct_reason() {
        let p = DataPoint::good("p", "d1", 10.0, f64::NAN, "u16");
        assert_eq!(p.quality, Quality::Uncertain);
        assert_eq!(p.value, 10.0, "value 合法时应保留");
        assert_eq!(p.raw_value, 0.0);
        assert_eq!(p.quality_reason, "non_finite_raw_value");
    }
}
