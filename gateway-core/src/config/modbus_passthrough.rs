//! Modbus TCP↔RTU 透传配置
//!
//! 网关作为 Modbus TCP 从站，将 TCP 请求透明转发到 RTU 串口设备，
//! 不做协议解析，只做帧格式转换（MBAP ↔ CRC）。

use serde::{Deserialize, Serialize};

/// Modbus TCP↔RTU 透传配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct ModbusPassthroughConfig {
    /// 是否启用
    pub enabled: bool,

    /// TCP 监听地址（如 "0.0.0.0:5020"，避免与 Modbus Server 的 502 冲突）
    pub listen_addr: String,

    /// 目标 RTU 串口总线名（对应 BusConfig.port_name）
    pub target_bus: String,

    /// 请求超时（毫秒），默认 2000
    #[serde(default = "default_timeout")]
    #[ts(type = "number")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    2000
}

impl Default for ModbusPassthroughConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: "0.0.0.0:5020".to_string(),
            target_bus: String::new(),
            timeout_ms: 2000,
        }
    }
}
