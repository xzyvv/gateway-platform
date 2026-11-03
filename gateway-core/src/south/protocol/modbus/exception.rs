//! Modbus 标准异常码定义（GB/T 19582 附录）
//!
//! 从 mod.rs 拆出，便于异常码逻辑独立维护。
//! 外部通过 `crate::south::protocol::modbus::ModbusExceptionCode` 或
//! `crate::south::protocol::ModbusExceptionCode`（protocol::mod 重新导出）访问。

/// Modbus 标准异常码（GB/T 19582 附录）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModbusExceptionCode {
    /// 0x01 非法功能码（配置错误，不可恢复）
    IllegalFunction,
    /// 0x02 非法数据地址（配置错误，不可恢复）
    IllegalDataAddress,
    /// 0x03 非法数据值（配置错误，不可恢复）
    IllegalDataValue,
    /// 0x04 从站设备故障（临时性，可重试）
    SlaveDeviceFailure,
    /// 0x05 确认（临时性）
    Acknowledge,
    /// 0x06 从站忙（临时性，可重试）
    SlaveDeviceBusy,
    /// 0x08 存储奇偶校验错误（临时性）
    MemoryParityError,
    /// 0x0A 网关路径不可用
    GatewayPathUnavailable,
    /// 0x0B 网关目标设备无响应
    GatewayTargetFailed,
    /// 未知异常码
    Unknown(u8),
}

impl ModbusExceptionCode {
    /// 从原始字节解析
    pub fn from_code(code: u8) -> Self {
        match code {
            0x01 => Self::IllegalFunction,
            0x02 => Self::IllegalDataAddress,
            0x03 => Self::IllegalDataValue,
            0x04 => Self::SlaveDeviceFailure,
            0x05 => Self::Acknowledge,
            0x06 => Self::SlaveDeviceBusy,
            0x08 => Self::MemoryParityError,
            0x0A => Self::GatewayPathUnavailable,
            0x0B => Self::GatewayTargetFailed,
            other => Self::Unknown(other),
        }
    }

    /// 是否为可恢复的临时性错误（设备忙/故障/奇偶错），可重试
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::SlaveDeviceFailure
                | Self::SlaveDeviceBusy
                | Self::Acknowledge
                | Self::MemoryParityError
        )
    }

    /// 是否为配置类错误（功能码/地址/值非法），通常不可通过重试恢复
    pub fn is_config_error(&self) -> bool {
        matches!(
            self,
            Self::IllegalFunction | Self::IllegalDataAddress | Self::IllegalDataValue
        )
    }

    /// PR-2: 人类可读的异常描述，便于日志/运维定位
    pub fn description(&self) -> &'static str {
        match self {
            Self::IllegalFunction => "非法功能码 (0x01): 从站不支持该功能码，检查协议配置",
            Self::IllegalDataAddress => {
                "非法数据地址 (0x02): 寄存器地址超出从站范围，检查 point.address"
            }
            Self::IllegalDataValue => "非法数据值 (0x03): 写入值或数量非法，检查 scale/raw_max",
            Self::SlaveDeviceFailure => "从站设备故障 (0x04): 设备内部错误，可重试",
            Self::Acknowledge => "确认 (0x05): 从站已接受但需更长时间处理",
            Self::SlaveDeviceBusy => "从站忙 (0x06): 设备处理中，可重试",
            Self::MemoryParityError => "存储奇偶校验错误 (0x08): 设备存储区校验失败",
            Self::GatewayPathUnavailable => "网关路径不可用 (0x0A): 网关找不到目标",
            Self::GatewayTargetFailed => "网关目标设备无响应 (0x0B): 目标从站未响应",
            Self::Unknown(_c) => "未知异常码",
        }
    }

    /// 原始异常码数值
    pub fn code(&self) -> u8 {
        match self {
            Self::IllegalFunction => 0x01,
            Self::IllegalDataAddress => 0x02,
            Self::IllegalDataValue => 0x03,
            Self::SlaveDeviceFailure => 0x04,
            Self::Acknowledge => 0x05,
            Self::SlaveDeviceBusy => 0x06,
            Self::MemoryParityError => 0x08,
            Self::GatewayPathUnavailable => 0x0A,
            Self::GatewayTargetFailed => 0x0B,
            Self::Unknown(c) => *c,
        }
    }
}
