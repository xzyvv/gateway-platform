//! 配置 API 共享类型
//!
//! 设备/测点/配置的请求体结构，下沉自原 mod.rs。各 handler 文件按职责使用对应类型。
//! 校验函数（validate_*）放在各自 handler 文件里就近定义，避免本文件膨胀。

use serde::Deserialize;

use gateway_core::config::{AggregationConfig, GatewayConfig};
use gateway_core::engine::model::AggFn;

/// 创建设备请求
#[derive(Deserialize)]
pub(crate) struct DevicePayload {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) template: String,
    pub(crate) bus: String,
    pub(crate) protocol: String,
    pub(crate) address: String,
    pub(crate) slave_id: Option<u8>,
    pub(crate) interval: Option<u64>,
}

/// 聚合参数入参：兼容前端两种写法。
///
/// - 简写 `"avg"`：只给聚合函数，窗口取默认 60 秒（前端「聚合」下拉的最常用形态）；
/// - 完整 `{ "window": 60, "function": "avg" }`：显式指定窗口。
#[derive(Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum AggregationPayload {
    /// 简写：聚合函数名
    Short(AggFn),
    /// 完整聚合配置
    Full(AggregationConfig),
}

impl AggregationPayload {
    /// 归一成落盘的聚合配置（简写形态补默认窗口 60s）。
    pub(crate) fn into_config(self) -> AggregationConfig {
        match self {
            Self::Short(function) => AggregationConfig {
                window: 60,
                function,
            },
            Self::Full(cfg) => cfg,
        }
    }
}

/// 创建 / 修改测点请求
///
/// BUG-B 修复：原结构体只声明了基础寻址字段，`alarm_high / alarm_low / aggregation /
/// deadband / on_change / formula / ct_ratio / pt_ratio / data_id / modbus_register`
/// 因 serde 默认忽略未知字段而被**静默丢弃**——用户填了告警阈值，界面上看着成功，
/// 落盘却是 null。现补齐 `PointConfig` 的全部可写字段，并加 `deny_unknown_fields`：
/// 之后再出现前端多发字段会直接 422 而不是悄悄吞掉，从根上杜绝同类问题。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PointPayload {
    pub(crate) id: String,
    pub(crate) device_id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) bus: String,
    pub(crate) protocol: String,
    pub(crate) address: String,
    pub(crate) slave_id: Option<u8>,
    pub(crate) address_num: Option<u16>,
    /// DLT645 / CJT188 数据标识
    pub(crate) data_id: Option<String>,
    pub(crate) function_code: Option<u8>,
    pub(crate) data_type: Option<String>,
    pub(crate) byte_order: Option<String>,
    pub(crate) register_count: Option<u16>,
    pub(crate) interval: u64,
    pub(crate) scale: Option<f64>,
    pub(crate) offset: Option<f64>,
    /// 公式（优先于 scale+offset）
    pub(crate) formula: Option<String>,
    /// 电流互感器变比
    pub(crate) ct_ratio: Option<f64>,
    /// 电压互感器变比
    pub(crate) pt_ratio: Option<f64>,
    /// 死区阈值
    pub(crate) deadband: Option<f64>,
    /// 变化才上传
    pub(crate) on_change: Option<bool>,
    pub(crate) unit: Option<String>,
    /// 报警高限
    pub(crate) alarm_high: Option<f64>,
    /// 报警低限
    pub(crate) alarm_low: Option<f64>,
    /// 聚合配置（字符串简写或完整对象）
    pub(crate) aggregation: Option<AggregationPayload>,
    /// Modbus 北向寄存器地址
    pub(crate) modbus_register: Option<u16>,
    /// S11: 写入白名单标记（缺省 false，可由测点编辑界面 / API 显式开启）
    pub(crate) writable: Option<bool>,
}

/// 配置导入请求
#[derive(Deserialize)]
pub(crate) struct ConfigImport {
    pub(crate) config: GatewayConfig,
}

/// 热重载请求
#[derive(Deserialize)]
pub(crate) struct ReloadRequest {}
