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

    /// 是否允许北向 Modbus 写回（FC06/FC16）。
    ///
    /// 默认拒绝写，与 OPC UA 侧 write_enabled 对齐：存量依赖写操作的 SCADA
    /// 必须在配置中显式启用本项，且点位需 writable=true 才可被写。
    #[serde(default)]
    pub write_enabled: bool,
}

impl Default for ModbusServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_endpoint(),
            base_register: 0,
            mode: default_mode(),
            write_enabled: false,
        }
    }
}

fn default_endpoint() -> String {
    "tcp://0.0.0.0:502".to_string()
}

fn default_mode() -> String {
    "auto".to_string()
}

#[cfg(test)]
mod tests {
    use super::ModbusServerConfig;

    /// 契约：旧配置（不含 write_enabled 字段）反序列化后必须默认为 false。
    /// 这是有意的行为变更——存量依赖 FC06/FC16 的 SCADA 将失效，必须显式开启。
    #[test]
    fn write_enabled_defaults_false_for_legacy_config() {
        let cfg: ModbusServerConfig =
            serde_json::from_str(r#"{"enabled":true,"endpoint":"tcp://0.0.0.0:502"}"#)
                .expect("legacy modbus server config parse");
        assert!(
            !cfg.write_enabled,
            "缺省 write_enabled 必须为 false（默认拒绝写，与 OPC UA 对齐）"
        );
        // 其余新增字段仍保持旧行为默认值
        assert!(cfg.enabled);
        assert_eq!(cfg.endpoint, "tcp://0.0.0.0:502");
        assert_eq!(cfg.base_register, 0);
        assert_eq!(cfg.mode, "auto");
    }
}
