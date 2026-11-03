//! 设备分组与资产标签（持久化元数据）
//!
//! 与 WebConfig.users 同模式：serde default = 空 Vec，老配置无字段自动兼容。
//! 存放在 GatewayConfig.device_groups / device_assets 字段，由 [device_ops] 模块管理。

use serde::{Deserialize, Serialize};

/// 设备分组（用户自定义的设备集合）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct DeviceGroup {
    /// 唯一 ID（前端用 ulid/uuid）
    pub id: String,
    /// 组名
    pub name: String,
    /// 描述
    #[serde(default)]
    pub description: String,
    /// 显示颜色（hex，例如 "#409eff"）
    #[serde(default)]
    pub color: String,
    /// 设备 ID 列表（关联 DeviceConfig.id）
    #[serde(default)]
    pub device_ids: Vec<String>,
    /// 创建时间戳（毫秒）
    pub created_at: i64,
}

/// 设备资产标签（资产生命周期：厂家/位置/校准日期等）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct DeviceAsset {
    /// 关联的设备 ID（关联 DeviceConfig.id）
    pub device_id: String,
    /// 资产编号（业务唯一编号）
    #[serde(default)]
    pub asset_no: String,
    /// 厂家
    #[serde(default)]
    pub vendor: String,
    /// 型号
    #[serde(default)]
    pub model: String,
    /// 安装位置
    #[serde(default)]
    pub location: String,
    /// 责任人
    #[serde(default)]
    pub owner: String,
    /// 采购日期（ISO8601 字符串，例 "2024-01-15"）
    #[serde(default)]
    pub purchased_at: String,
    /// 上次校准日期
    #[serde(default)]
    pub calibrated_at: String,
    /// 资产标签列表（如 ["电力", "关键", "温控"]）
    #[serde(default)]
    pub tags: Vec<String>,
}
