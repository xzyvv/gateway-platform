//! 协议描述符与 Web UI 协议清单 / 能力查询辅助

use super::capabilities::ProtocolCapabilities;
use super::entries::PROTOCOL_ENTRIES;

/// 协议描述符：注册表对外暴露的完整元数据（Web 前端的单一来源）。
///
/// `GET /api/protocols` 直接序列化此结构，前端据此生成协议下拉、标签、默认超时
/// 与地址提示；新增协议只改注册表，前端无需同步改协议清单。
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ProtocolDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub transport: &'static str,
    pub default_timeout_ms: u64,
    /// 设备/点位地址输入提示（表单 placeholder）
    pub address_hint: &'static str,
    /// 总线对端地址（`tcp_addr`）输入提示；纯串行协议为空字符串
    pub endpoint_hint: &'static str,
    /// 协议变体下拉项 (值, 展示名)；无变体概念时为空切片
    pub variants: &'static [(&'static str, &'static str)],
    pub tag_color: &'static str,
    pub capabilities: ProtocolCapabilities,
}

/// S3: 返回所有协议描述符（注册表是 Web UI 协议清单的单一来源）。
///
/// 包含展示名、链路形态、默认超时、地址提示与能力元数据；`gpio` 是点位级协议，
/// 不在 [`PROTOCOL_ENTRIES`] 中，单独追加，使前端能正确门控 GPIO 写入入口。
pub fn all_protocol_descriptors() -> Vec<ProtocolDescriptor> {
    let mut list: Vec<ProtocolDescriptor> = PROTOCOL_ENTRIES
        .iter()
        .map(|e| ProtocolDescriptor {
            name: e.name,
            label: e.label,
            transport: e.transport,
            default_timeout_ms: e.default_timeout_ms,
            address_hint: e.address_hint,
            endpoint_hint: e.endpoint_hint,
            variants: e.variants,
            tag_color: e.tag_color,
            capabilities: e.capabilities,
        })
        .collect();
    list.push(ProtocolDescriptor {
        name: "gpio",
        label: "GPIO",
        transport: "local",
        default_timeout_ms: 1000,
        address_hint: "如: 17（GPIO 编号）",
        endpoint_hint: "",
        variants: &[],
        tag_color: "info",
        capabilities: ProtocolCapabilities::gpio(),
    });
    list
}

/// 按协议名查询能力位（调试工具按能力路由的唯一入口）。
///
/// 未注册协议返回 `None`：调用方应报「协议未注册」而不是「不支持该操作」，
/// 两者对现场的含义不同（配置写错 vs 协议本身不提供该能力）。
pub fn protocol_capabilities(name: &str) -> Option<ProtocolCapabilities> {
    if name == "gpio" {
        return Some(ProtocolCapabilities::gpio());
    }
    PROTOCOL_ENTRIES
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.capabilities)
}

/// S3: 返回所有协议能力元数据（供 Web API / 测试）
pub fn all_protocol_capabilities() -> Vec<(&'static str, ProtocolCapabilities)> {
    all_protocol_descriptors()
        .into_iter()
        .map(|d| (d.name, d.capabilities))
        .collect()
}
