//! 协议能力元数据（`ProtocolCapabilities`）与命名构造器
//!
//! 每个协议声明自身支持的能力位，Web 前端据此决定显示哪些操作面板。

/// 协议能力元数据：供 Web UI 动态生成协议相关界面（S3）
///
/// 每个协议声明自身支持的能力位，Web 前端据此决定显示哪些操作面板。
///
/// TS 类型统一由 `web::types::export_ts::export_ts_types` 测试导出到
/// `gateway-web/src/api/generated/`，不要在此使用 `#[ts(export, export_to = ...)]`
/// 单独导出（路径基准不同，会生成到错误的嵌套目录）。
#[derive(Debug, Clone, Copy, serde::Serialize, ts_rs::TS)]
pub struct ProtocolCapabilities {
    pub supports_batch_read: bool,
    /// 是否有专用帧提取器（`frame_extractor`）。
    ///
    /// **当前仅作元数据预留**：前端未消费该位（`gateway-web` 无任何 `supports_frame_extract`
    /// 分支），因为它描述的是「采集引擎内部如何切帧」，不是用户可选择的操作。保留该位
    /// 是为了运维/诊断侧能区分「协议自己切帧」与「网关按通用规则切帧」，不做删除。
    pub supports_frame_extract: bool,
    pub supports_write: bool,
    pub supports_scan_slaves: bool,
    /// 是否支持「寄存器扫描」调试工具（按地址段探测可读寄存器）。
    ///
    /// 工具端按此能力位路由，而不是按协议名硬编码；新增协议只要在注册表声明
    /// 该位为 true，扫描入口即可用。
    pub supports_register_scan: bool,
    /// 是否支持「自动识别设备」调试工具（遍历从站号 + 协议探测）。
    pub supports_auto_detect: bool,
    /// 是否实现「一次事务写多点」（覆写 `write_multi`）；false 表示后端退化为逐点写
    pub supports_write_multi: bool,
    /// 是否实现真实诊断（覆写 `diag` + `diag_supported`）；false 表示诊断只会报「未实现」
    pub supports_diag: bool,
}

/// 协议能力命名构造器（供 [`super::PROTOCOL_ENTRIES`] 复用，避免重复 4 个 bool 字面量）
///
/// 按协议**形态**而非具体协议名归类；新增协议时优先匹配已有形态，
/// 若都不匹配再用 `ProtocolCapabilities { .. }` 字面量并考虑是否新增构造器。
impl ProtocolCapabilities {
    /// Modbus RTU/TCP：批量读 + 帧提取 + 写 + 扫从站 + FC15/FC16 事务写多点
    /// + 真实诊断（FC03 实读一次保持寄存器）
    pub const fn modbus() -> Self {
        Self {
            supports_batch_read: true,
            supports_frame_extract: true,
            supports_write: true,
            supports_scan_slaves: true,
            supports_register_scan: true,
            supports_auto_detect: true,
            supports_write_multi: true,
            supports_diag: true,
        }
    }

    /// 串行 BCD 计量协议（dlt645 / cjt188）：帧提取 + 写（决策表 #5/#6 已实现写能力）
    /// + 真实诊断（读通信地址，验证链路与表地址）。
    pub const fn serial_bcd_meter_writable() -> Self {
        Self {
            supports_batch_read: false,
            supports_frame_extract: true,
            supports_write: true,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }

    /// TCP 批量读写协议（mc / fins）：批量读 + 写 + 按软元件地址逐点探测
    pub const fn tcp_batch_write() -> Self {
        Self {
            supports_batch_read: true,
            supports_frame_extract: false,
            supports_write: true,
            supports_scan_slaves: false,
            supports_register_scan: true,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }

    /// 远动 TCP 批量读写协议（iec104）：在 [`Self::tcp_batch_write`] 基础上增加真实诊断
    /// （建链 + STARTDT + 总召唤）。不能直接把 `tcp_batch_write` 的 diag 改为 true——
    /// 该构造器同时被 mc / fins 复用，那两个协议尚未实现 `diag`。
    pub const fn telecontrol_tcp_write() -> Self {
        Self {
            supports_batch_read: true,
            supports_frame_extract: false,
            supports_write: true,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }

    /// 西门子 S7：批量读 + 写 + 单事务合并写多点 + 真实诊断（SZL 组件标识）
    pub const fn s7comm() -> Self {
        Self {
            supports_batch_read: true,
            supports_frame_extract: false,
            supports_write: true,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: true,
            supports_diag: true,
        }
    }

    /// TCP 批量读协议（opcua）：仅批量读，无写入实现
    ///
    /// Bug 175: 该协议当前未实现 `fn write`，若声明 supports_write=true 会让前端
    /// 显示写入入口而实际恒返回 Unsupported。声明为 false，如实反映能力。
    pub const fn tcp_batch_readonly() -> Self {
        Self {
            supports_batch_read: true,
            supports_frame_extract: false,
            supports_write: false,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }

    /// BACnet/IP：批量读（ReadPropertyMultiple）+ 写（WriteProperty）+ Who-Is 诊断。
    /// 不支持从站扫描：从站扫描是**协议工具的独立实现**
    /// （`gateway-api` 的 `slave_scan` handler 直接发探测帧，只对 modbus 总线开放，
    /// 非 modbus 总线返回 `UnsupportedProtocol`），不经过 `Collector`。
    /// 此处如实声明，避免前端按钮可点但请求必然失败。
    pub const fn bacnet_ip() -> Self {
        Self {
            supports_batch_read: true,
            supports_frame_extract: false,
            supports_write: true,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }

    /// BACnet MS/TP：帧提取 + 真实诊断（Who-Is 广播探针收 I-Am）。不支持从站扫描。
    pub const fn bacnet_mstp() -> Self {
        Self {
            supports_batch_read: false,
            supports_frame_extract: true,
            supports_write: false,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }

    /// 串行远动协议（iec101）：帧提取 + 总召唤批量读 + 真实诊断（链路复位 + 总召唤）
    pub const fn serial_telecontrol() -> Self {
        Self {
            supports_batch_read: true,
            supports_frame_extract: true,
            supports_write: false,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }
    /// GPIO（点位级协议）：读 DI + 写 DO + 芯片信息诊断，无帧提取、无批量读
    pub const fn gpio() -> Self {
        Self {
            supports_batch_read: false,
            supports_frame_extract: false,
            supports_write: true,
            supports_scan_slaves: false,
            supports_register_scan: false,
            supports_auto_detect: false,
            supports_write_multi: false,
            supports_diag: true,
        }
    }
}
