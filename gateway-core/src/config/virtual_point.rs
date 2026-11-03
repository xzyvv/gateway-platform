//! 虚拟点位（公式派生）配置
//!
//! 虚拟点位由 `core::pipeline::VirtualPointEngine` **真实求值**：公式在处理管道中
//! （协议转换之后、实时广播/入队之前）按采集周期求值，派生出的 `DataPoint`
//! 与普通点位一同广播、入队并北向上报。公式语义由 `core::pipeline::vformula`
//! 定义（唯一权威实现），前端 `utils/formulaEngine.ts` 只是浏览器端预演。
//!
//! 配置改动**热重载生效**：引擎内规则集用 `ArcSwap` 原子替换，无需重启网关
//! （见 `config/reload_scope.rs` 中 `virtual_points` 归入 `applied`）。
//!
//! 与 PointConfig 的 formula 字段区别：
//! - PointConfig.formula 在采集流程内生效（优先于 scale+offset），语义是**单变量**
//!   换算（白名单运算符 + `raw` 变量）
//! - VirtualPoint.formula 是**多变量**派生公式，支持 `{point_id}` 引用、级联引用
//!   其它虚拟点位，以及 12 个内置函数

use serde::{Deserialize, Serialize};

/// 虚拟点位配置（持久化）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct VirtualPoint {
    /// 唯一 ID
    pub id: String,

    /// 中文名（显示用）
    pub name: String,

    /// 所属设备 ID（用于按设备分组；可为空）
    #[serde(default)]
    pub device_id: Option<String>,

    /// 公式表达式，引用其他点位用 {point_id}
    pub formula: String,

    /// 单位
    #[serde(default)]
    pub unit: String,

    /// 保留小数位
    #[serde(default = "default_decimals")]
    pub decimals: u8,

    /// 是否启用
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// 创建时间戳（毫秒）
    #[ts(type = "number")]
    pub created_at: i64,
}

fn default_decimals() -> u8 {
    2
}

fn default_enabled() -> bool {
    true
}
