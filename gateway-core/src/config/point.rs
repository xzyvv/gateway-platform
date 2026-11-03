//! 点位 / 模板设备 / 聚合配置

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::engine::model::{AggFn, ByteOrder, ModbusDataType};

use super::{
    validate_byte_order, validate_function_code, validate_non_zero_interval, validate_protocol,
    validate_scale,
};

/// 模板模式设备配置（§3.5）
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct DeviceConfig {
    /// 设备唯一标识（Q16 Web API CRUD 主键；旧配置无此字段时反序列化为空串，
    /// 由 Web 层在 create/update 时显式赋值；不影响模板展开逻辑，展开仍以 `name` 为点位 ID 前缀）
    #[serde(default)]
    pub id: String,

    /// 设备名（展开时点 ID 前缀）
    pub name: String,

    /// 模板名
    pub template: String,

    /// 总线名
    pub bus: String,

    /// 协议
    #[validate(custom(function = "validate_protocol"))]
    pub protocol: String,

    /// 从站地址/表地址
    pub address: String,

    /// Modbus 从站号
    pub slave_id: Option<u8>,

    /// 参数名→地址覆盖
    #[serde(default)]
    pub address_map: Option<HashMap<String, u16>>,

    /// 采集周期覆盖（秒），None=用模板默认
    #[ts(type = "number")]
    pub interval: Option<u64>,
}

/// 聚合配置（§3.6）
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct AggregationConfig {
    /// 聚合窗口（秒，>=1；0 会让窗口永不闭合）
    #[validate(range(min = 1))]
    #[ts(type = "number")]
    pub window: u64,

    /// 聚合函数
    pub function: AggFn,
}

/// 点位配置（§3.7，核心）
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct PointConfig {
    /// 点位唯一标识
    pub id: String,

    /// 中文名（显示用）
    pub name: Option<String>,

    /// 所属设备 ID（用于按设备分组上送报文；为空时回退到 bus 名）
    #[serde(default)]
    pub device_id: Option<String>,

    /// 总线名
    pub bus: String,

    /// 协议
    #[validate(custom(function = "validate_protocol"))]
    pub protocol: String,

    /// 地址（Modbus=slave_id字符串 / DLT645=BCD地址 / GPIO=GPIO编号）
    pub address: String,

    /// Modbus 从站号 1~247
    pub slave_id: Option<u8>,

    /// Modbus 寄存器地址
    pub address_num: Option<u16>,

    /// DLT645/CJT188 数据标识
    #[serde(default)]
    pub data_id: Option<String>,

    /// Modbus 功能码 3=保持寄存器 / 4=输入寄存器 / 1,2=线圈 / 5,6,15,16=写
    #[validate(custom(function = "validate_function_code"))]
    pub function_code: Option<u8>,

    /// 数据类型
    #[serde(default)]
    pub data_type: Option<String>,

    /// 字节序 ABCD/DCBA/CDAB/BADC，默认 ABCD
    #[serde(default)]
    #[validate(custom(function = "validate_byte_order"))]
    pub byte_order: Option<String>,

    /// 占用寄存器数
    pub register_count: Option<u16>,

    /// 采集周期（秒，>=1）
    #[validate(custom(function = "validate_non_zero_interval"))]
    #[ts(type = "number")]
    pub interval: u64,

    /// 倍率（formula 为空时使用）
    #[serde(default = "default_scale")]
    #[validate(custom(function = "validate_scale"))]
    pub scale: f64,

    /// 偏移
    #[serde(default = "default_offset")]
    pub offset: f64,

    /// 公式（优先于 scale+offset）
    pub formula: Option<String>,

    /// CT 变比（电流互感器，如 150/5=30），None=不应用
    /// 二次侧工程量 × ct_ratio = 一次侧真实电流值
    #[serde(default)]
    pub ct_ratio: Option<f64>,

    /// PT 变比（电压互感器，如 10000/100=100），None=不应用
    /// 二次侧工程量 × pt_ratio = 一次侧真实电压值
    #[serde(default)]
    pub pt_ratio: Option<f64>,

    /// 死区阈值
    #[serde(default = "default_deadband")]
    pub deadband: f64,

    /// 变化才上传
    #[serde(default)]
    pub on_change: bool,

    pub unit: Option<String>,

    /// 报警高限（值超过此阈值触发报警），None=不启用高限报警
    #[serde(default)]
    pub alarm_high: Option<f64>,

    /// 报警低限（值低于此阈值触发报警），None=不启用低限报警
    #[serde(default)]
    pub alarm_low: Option<f64>,

    /// 聚合配置，None=不聚合
    pub aggregation: Option<AggregationConfig>,

    /// Modbus 北向寄存器地址（显式指定，None=自动分配）
    #[serde(default)]
    pub modbus_register: Option<u16>,

    /// S11: 写入白名单标记。
    ///
    /// 仅当该点为 `true` 时才允许通过 API / 远程控制下发写入（默认 `false` = 拒绝）。
    /// 工业现场默认拒绝写入，避免未授权的任意寄存器被误写。
    #[serde(default)]
    pub writable: bool,
}

/// 点位回收站条目（软删除）。
///
/// 已删点位**不留在** `GatewayConfig::points` 中，而是整体搬到这里，
/// 使采集引擎 / 配额统计 / 配置导出等 `cfg.points` 消费方无需任何过滤逻辑。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct PointTrashEntry {
    /// 被删除的完整点位配置（恢复时原样放回 `points`）
    pub point: PointConfig,

    /// 删除时间（Unix 毫秒），前端用 new Date(ms) 渲染
    #[ts(type = "number")]
    pub deleted_at_ms: i64,
}

/// Bug 227: 手写 `Default`，与 serde 的缺省值对齐。
///
/// 原 `#[derive(Default)]` 由字段类型推导默认值，`f64` 字段一律取 `0.0`，
/// 而 serde 侧 `scale` 的缺省值是 `1.0`（`#[serde(default = "default_scale")]`）。
/// 两者不一致带来的是**静默数据错误**而非编译错误：
/// - 读路径 `raw * point.scale + point.offset`（`convert_value`）在 scale=0.0 时
///   恒等于 offset，采集明明成功、质量 Good，上报值却一律为 0；
/// - 写路径 `reverse_convert` 又把 0.0 当作"不做缩放"直接透传，同一份配置两种语义。
///
/// 由 JSON 加载的点位走 serde 路径不受影响，但**任何用代码构造点位**的地方
/// （模板展开、`..Default::default()` 的测试与工具代码、后续新增的构造入口）都会踩中，
/// 且症状（值恒为 0）与"设备没数据"难以区分。
impl Default for PointConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: None,
            device_id: None,
            bus: String::new(),
            protocol: String::new(),
            address: String::new(),
            slave_id: None,
            address_num: None,
            data_id: None,
            function_code: None,
            data_type: None,
            byte_order: None,
            register_count: None,
            // 与 JSON 一致：interval 无 serde 缺省值（必填，且校验要求 >= 1），
            // 此处保留 0 以暴露"未显式赋值"，不擅自给合法值掩盖配置缺失。
            interval: 0,
            // 关键修复：缺省倍率必须为 1.0（原 derive 得到 0.0，会把采集值归零）
            scale: default_scale(),
            offset: default_offset(),
            formula: None,
            ct_ratio: None,
            pt_ratio: None,
            deadband: default_deadband(),
            on_change: false,
            unit: None,
            alarm_high: None,
            alarm_low: None,
            aggregation: None,
            modbus_register: None,
            writable: false,
        }
    }
}

/// 解析数据类型字符串（唯一解析点）。
///
/// 除配置里的规范写法（`u16`/`i16`/…）外，同时接受界面/模板沿用的别名：
/// `uint16`/`int16`/`uint32`/`int32`/`uint64`/`int64`/`float`/`float32`/`double`/`float64`。
///
/// 修复背景：点位创建向导（QuickSetup/Wizard）写入的是 `int16`/`uint16`/`float` 等别名，
/// 旧实现只认 `u16`/`f32` 等规范写法，别名一律落进 `_ => None`，采集时静默按 u16 解码
/// （32 位/浮点值被读成乱码，有符号值被当成无符号）。因此这里必须逐别名兼容，
/// 非法值由 `validate_all` 显式报错，而不是运行时静默回退。
pub(crate) fn parse_data_type(s: &str) -> Option<ModbusDataType> {
    match s.trim().to_ascii_lowercase().as_str() {
        "u16" | "uint16" => Some(ModbusDataType::U16),
        "i16" | "int16" => Some(ModbusDataType::I16),
        "u32" | "uint32" => Some(ModbusDataType::U32),
        "i32" | "int32" => Some(ModbusDataType::I32),
        "u64" | "uint64" => Some(ModbusDataType::U64),
        "i64" | "int64" => Some(ModbusDataType::I64),
        "f32" | "float" | "float32" => Some(ModbusDataType::F32),
        "f64" | "double" | "float64" => Some(ModbusDataType::F64),
        _ => None,
    }
}

/// 解析字节序字符串（唯一解析点），与 [`super::VALID_BYTE_ORDERS`] 保持同步。
///
/// 除了大写四字码，还接受模板字段沿用的长写法（`big-endian` 等）。
/// 旧实现只认大写四字码，`little-endian` 会落进 `_ => Abcd`——小端设备被按大端解码，
/// 值恒为错；这里补全长写法，非法值由 `validate_all` 显式报错。
pub(crate) fn parse_byte_order(s: &str) -> Option<ByteOrder> {
    match s.trim().to_ascii_lowercase().as_str() {
        "abcd" | "big-endian" => Some(ByteOrder::Abcd),
        "dcba" | "little-endian" => Some(ByteOrder::Dcba),
        "cdab" | "big-endian-byte-swapped" => Some(ByteOrder::Cdab),
        "badc" | "little-endian-byte-swapped" => Some(ByteOrder::Badc),
        _ => None,
    }
}

impl PointConfig {
    /// 返回数据类型枚举
    pub fn data_type_enum(&self) -> Option<ModbusDataType> {
        self.data_type.as_deref().and_then(parse_data_type)
    }

    /// 返回字节序枚举（未配置/非法值回退 ABCD；非法值在保存期已被校验拦截）
    pub fn byte_order_enum(&self) -> ByteOrder {
        self.byte_order
            .as_deref()
            .and_then(parse_byte_order)
            .unwrap_or(ByteOrder::Abcd)
    }
}

// ===== 默认值函数 =====

fn default_scale() -> f64 {
    1.0
}
fn default_offset() -> f64 {
    0.0
}
fn default_deadband() -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小合法点位 JSON：name/slave_id/address_num/function_code/register_count/
    /// formula/unit/aggregation 无 serde 缺省值（必填，可为 null），其余字段可省略。
    ///
    /// 协议取 `gpio`：它在任何协议裁剪构建里都合法（见 `domain::VALID_PROTOCOLS`），
    /// 而 `modbus` 只在 `--features modbus` 时存在。倍率缺省、scale=0 拒绝这些
    /// 与协议无关的不变式，不应因裁剪构建而失效。
    const MINIMAL_JSON: &str = r#"{
        "id": "p1",
        "name": null,
        "bus": "b1",
        "protocol": "gpio",
        "address": "1",
        "slave_id": null,
        "address_num": null,
        "function_code": null,
        "register_count": null,
        "interval": 1,
        "formula": null,
        "unit": null,
        "aggregation": null
    }"#;

    /// Bug 227 回归：`Default` 必须与 serde 缺省值逐字段一致。
    ///
    /// 原 `#[derive(Default)]` 让 scale 得到 0.0，而 JSON 缺省是 1.0——
    /// 代码构造的点位读路径 `raw * scale + offset` 恒为 0（静默数据错误）。
    #[test]
    fn default_matches_serde_defaults() {
        let d = PointConfig::default();
        let from_json: PointConfig =
            serde_json::from_str(MINIMAL_JSON).expect("最小点位 JSON 应可反序列化");

        assert_eq!(
            from_json.scale, 1.0,
            "serde 缺省倍率应为 1.0（default_scale）"
        );
        assert_eq!(
            d.scale, 1.0,
            "Default 倍率必须为 1.0；0.0 会把所有采集值归零"
        );

        // 有 serde 缺省值的字段：两种构造路径必须一致
        assert_eq!(d.scale, from_json.scale, "scale");
        assert_eq!(d.offset, from_json.offset, "offset");
        assert_eq!(d.deadband, from_json.deadband, "deadband");
        assert_eq!(d.on_change, from_json.on_change, "on_change");
        assert_eq!(d.writable, from_json.writable, "writable");
        assert_eq!(d.device_id, from_json.device_id, "device_id");
        assert_eq!(d.data_id, from_json.data_id, "data_id");
        assert_eq!(d.data_type, from_json.data_type, "data_type");
        assert_eq!(d.byte_order, from_json.byte_order, "byte_order");
        assert_eq!(d.ct_ratio, from_json.ct_ratio, "ct_ratio");
        assert_eq!(d.pt_ratio, from_json.pt_ratio, "pt_ratio");
        assert_eq!(d.alarm_high, from_json.alarm_high, "alarm_high");
        assert_eq!(d.alarm_low, from_json.alarm_low, "alarm_low");
        assert_eq!(
            d.modbus_register, from_json.modbus_register,
            "modbus_register"
        );
    }

    /// 契约：Default 构造的点位不得把 scale/offset/deadband 变成"会让值归零"的组合
    #[test]
    fn default_point_keeps_linear_pass_through() {
        let p = PointConfig::default();
        let raw = 42.5f64;
        assert_eq!(
            raw * p.scale + p.offset,
            raw,
            "缺省倍率下 raw*scale+offset 必须等于原始值"
        );
    }

    /// Bug 228 回归：显式配置 `"scale": 0` 必须被校验 fail-fast 拒绝。
    ///
    /// scale=0 会让读路径 `raw*scale+offset` 恒等于 offset（静默归零），
    /// 且与写路径 `reverse_convert`（把 0 当作"不缩放"直接透传）语义相反。
    #[test]
    fn explicit_zero_scale_rejected() {
        use validator::Validate;
        let json = MINIMAL_JSON.replace("\"interval\": 1", "\"interval\": 1, \"scale\": 0.0");
        let p: PointConfig =
            serde_json::from_str(&json).expect("scale=0 应能反序列化（拒绝发生在 validate 阶段）");
        assert_eq!(p.scale, 0.0);
        let err = p.validate().expect_err("scale=0 必须校验失败");
        assert!(
            err.to_string().contains("scale"),
            "错误信息应指向 scale 字段，实际: {}",
            err
        );
    }

    /// 省略 scale（serde 缺省 1.0）必须通过校验，不受 Bug 228 修复影响。
    #[test]
    fn omitted_scale_passes_validation() {
        use validator::Validate;
        let p: PointConfig =
            serde_json::from_str(MINIMAL_JSON).expect("最小点位 JSON 应可反序列化");
        assert_eq!(p.scale, 1.0);
        assert!(p.validate().is_ok(), "缺省 scale=1.0 应通过校验");
    }

    /// 回收站为空（None）时不得落盘 `point_trash` 键：存量配置零影响。
    #[test]
    fn empty_trash_is_not_serialized() {
        use crate::config::GatewayConfig;
        let cfg = GatewayConfig::default();
        let v = serde_json::to_value(&cfg).expect("配置应可序列化");
        assert!(
            v.get("point_trash").is_none(),
            "空回收站不应出现在序列化结果中, got {v}"
        );
    }

    /// 回收站条目可完整往返序列化（含 Unix 毫秒时间戳与原始 PointConfig）。
    #[test]
    fn trash_entry_roundtrip() {
        use crate::config::GatewayConfig;
        let cfg = GatewayConfig {
            point_trash: Some(vec![PointTrashEntry {
                point: PointConfig {
                    id: "p1".into(),
                    interval: 10,
                    ..Default::default()
                },
                deleted_at_ms: 1_700_000_000_000,
            }]),
            ..Default::default()
        };
        let v = serde_json::to_value(&cfg).expect("配置应可序列化");
        assert_eq!(v["point_trash"][0]["point"]["id"], "p1");
        assert_eq!(v["point_trash"][0]["deleted_at_ms"], 1_700_000_000_000i64);

        let back: GatewayConfig = serde_json::from_value(v).expect("应可反序列化");
        let entry = &back.point_trash.expect("回收站应还原")[0];
        assert_eq!(entry.point.id, "p1");
        assert_eq!(entry.deleted_at_ms, 1_700_000_000_000);
    }
}
