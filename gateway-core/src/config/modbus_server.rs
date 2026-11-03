//! Modbus Server（北向）配置
//!
//! 启用后网关作为 Modbus TCP 从站，把采集到的点位数据映射到保持寄存器，
//! 供 SCADA/HMI 通过 FC01/FC02（位读）、FC03/FC04（寄存器读）、
//! FC06/FC16（单/多寄存器写）访问。

use serde::{Deserialize, Serialize};

/// Modbus Server 配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct ModbusServerConfig {
    /// 是否启用 Modbus Server（北向）
    #[serde(default)]
    pub enabled: bool,

    /// 监听端点，格式 tcp://0.0.0.0:502
    #[serde(default = "default_endpoint")]
    pub endpoint: String,

    /// 起始寄存器地址（auto 模式下第一个点位的寄存器地址）
    #[serde(default)]
    pub base_register: u16,

    /// 映射模式：auto（自动按顺序分配）或 manual（使用 point.register_addr）
    #[serde(default = "default_mode")]
    pub mode: String,
}

impl Default for ModbusServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_endpoint(),
            base_register: 0,
            mode: default_mode(),
        }
    }
}

fn default_endpoint() -> String {
    "tcp://0.0.0.0:502".to_string()
}

fn default_mode() -> String {
    "auto".to_string()
}
