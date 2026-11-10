//! 点位 / 模板设备 / 聚合配置

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::engine::model::{AggFn, ByteOrder, ModbusDataType};

use super::{
    validate_byte_order, validate_non_zero_interval, validate_protocol, validate_ratio,
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

    /// 功能码（语义随协议而变）
    ///
    /// - Modbus：3=保持寄存器 / 4=输入寄存器 / 1,2=线圈 / 5,6,15,16=写
    /// - DLT645：0x04=写数据 / 0x0C=写通信地址 / 0x0F=写密码
    /// - CJT188：0x04=写数据 / 0x15=写通信地址
    ///
    /// 校验必须按协议分流（见 [`allowed_function_codes`]），字段级校验拿不到
    /// protocol，只能放到结构级（`gate.rs`）做。
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
    /// 二次侧工程量 × ct_ratio = 一次侧真实电流值（仅电流/功率/电量类单位生效）
    #[serde(default)]
    #[validate(custom(function = "validate_ratio"))]
    pub ct_ratio: Option<f64>,

    /// PT 变比（电压互感器，如 10000/100=100），None=不应用
    /// 二次侧工程量 × pt_ratio = 一次侧真实电压值（仅电压/功率/电量类单位生效）
    #[serde(default)]
    #[validate(custom(function = "validate_ratio"))]
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

/// 各协议允许的功能码集合（`None` = 该协议的功能码无需校验）
///
/// 功能码在不同协议下语义完全不同，不能共用一份白名单：
/// - **Modbus**：功能码直接决定读/写哪一类寄存器
///   （1/2 读离散、3 读保持、4 读输入、5/6 写单点、15 写多线圈、16 写多寄存器），
///   取值错误会长期采不到值或写错寄存器，必须严格校验
/// - **DLT645**：只作「写控制码」路由——0x04 写数据、0x0C(12) 写通信地址、
///   0x0F(15) 写密码；其余取值会落到「写数据」分支，不会写错设备
/// - **CJT188**：只作「写控制码」路由——0x04 写数据、0x15(21) 写通信地址
/// - **其它协议**：不使用功能码
///
/// 历史实现用 Modbus 的白名单（1~6/15/16）校验所有协议，导致 DLT645 的
/// 写通信地址(0x0C)、写密码(0x0F) 与 CJT188 的写通信地址(0x15) 根本存不进配置。
pub fn allowed_function_codes(protocol: &str) -> Option<&'static [u8]> {
    match protocol.trim().to_ascii_lowercase().as_str() {
        "modbus" => Some(&[1, 2, 3, 4, 5, 6, 15, 16]),
        _ => None,
    }
}

// ===== CT/PT 变比适用范围 =====

/// CT/PT 变比适用范围（按点位单位判定）
///
/// 变比只对「二次侧 → 一次侧」有物理意义的量生效：
/// - 电压（V/kV/mV）：只乘 PT
/// - 电流（A/kA/mA）：只乘 CT
/// - 功率/电量（W/kW/var/kWh/kVarh…）：CT × PT（P = U × I）
/// - 功率因数（PF/cosφ…）：无量纲，两者都不乘
/// - 单位为空或不在上述清单：两者都乘（兼容改造前的无条件连乘）
///
/// 判定放在配置层：解码（`convert_value` / `reverse_convert`）与 Web 体检
/// 必须共用同一张单位表，各自复制一份必然漂移。
/// 前端 `useRatio.ts::ratioScopeOf` 使用同一套判定，改动需两边同步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RatioScope {
    /// 只乘 CT（电流类）
    Ct,
    /// 只乘 PT（电压类）
    Pt,
    /// CT × PT（功率/电量/未知单位）
    Both,
    /// 都不乘（功率因数等无量纲量）
    None,
}

pub(crate) fn ratio_scope(unit: Option<&str>) -> RatioScope {
    // 用 Unicode 小写而不是 ASCII 小写："COSΦ" 只有转成 "cosφ" 才认得出来
    let unit = unit.unwrap_or("").trim().to_lowercase();
    match unit.as_str() {
        "" => RatioScope::Both,
        "v" | "kv" | "mv" => RatioScope::Pt,
        "a" | "ka" | "ma" => RatioScope::Ct,
        "pf" | "cos" | "cosφ" | "cosphi" | "功率因数" => RatioScope::None,
        _ => RatioScope::Both,
    }
}

impl PointConfig {
    /// 本点位的变比适用范围（由 `unit` 决定）。
    pub(crate) fn ratio_scope(&self) -> RatioScope {
        ratio_scope(self.unit.as_deref())
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
mod tests;
