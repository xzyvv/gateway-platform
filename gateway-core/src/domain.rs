//! 中立领域常量
//!
//! 本模块不依赖 `config` / `south` / gateway-api 的 `web` 等任何业务层，用于存放同时被多层引用的
//! 领域常量与领域类型，避免层间反向依赖。
//!
//! 目前包含：
//!
//! - 协议白名单 [`VALID_PROTOCOLS`]：它原先定义在南向 `south::protocol::registry`，
//!   导致 `config::validate` 需要反向依赖 `south`。下沉到本模块后，`config` 与 `south`
//!   引用同一份常量，反向依赖被消除。
//! - 授权档位与配额模型 [`license`]（[`Tier`] / [`Quota`] 等）：它们原先定义在
//!   `license` 模块，但配置层 `config::license`（`LicenseConfig` / `TierQuotaEntry`）
//!   同样需要引用，于是形成 `config → license → config` 的双向循环。下沉到本模块后，
//!   `config` 与 `license` 共同引用同一份定义，反向依赖被消除。

pub mod license;

pub use license::{Quota, QuotaAvailable, QuotaKind, QuotaUsage, Tier};

/// 合法协议名白名单（配置校验与 API 校验的唯一来源）
///
/// 包含所有在配置中合法的协议字符串。其中 gpio 为点位级数字 IO，
/// 不走 Collector 注册流程（不在 `south::protocol::registry::PROTOCOL_ENTRIES` 中），
/// 但在配置中合法。
///
/// 不变式：本数组必须与 `south::protocol::registry::PROTOCOL_ENTRIES` 保持同步
/// （由 `protocol::registry` 的单元测试 `test_valid_protocols_match_entries` 校验）。
///
/// 新增协议时：若为普通协议，在 `PROTOCOL_ENTRIES` 追加项即可（本数组同步由测试校验）；
/// 若为点位级特殊协议（如 gpio），需在此数组手动追加名字。
pub const VALID_PROTOCOLS: &[&str] = &[
    #[cfg(feature = "modbus")]
    "modbus",
    #[cfg(feature = "dlt645")]
    "dlt645",
    #[cfg(feature = "fins")]
    "fins",
    #[cfg(feature = "cjt188")]
    "cjt188",
    #[cfg(feature = "opcua-client")]
    "opcua",
    #[cfg(feature = "iec104")]
    "iec104",
    #[cfg(feature = "iec101")]
    "iec101",
    #[cfg(feature = "mc")]
    "mc",
    #[cfg(feature = "bacnet")]
    "bacnet",
    #[cfg(feature = "s7comm")]
    "s7comm",
    #[cfg(feature = "bacnet-mstp")]
    "bacnet_mstp",
    "gpio", // 点位级协议，始终合法，不受 feature gate
];

/// 全部曾支持的协议名（**不随 feature 裁剪**）。
///
/// 用途：区分「协议根本不存在（配置拼错）」与「协议存在但未编入当前发行 SKU」，
/// 后者应给出可操作的提示（升级 SKU / 重新构建并启用 feature），而不是笼统的
/// "unknown protocol"。新增协议时需同步追加本表。
pub const KNOWN_PROTOCOLS: &[&str] = &[
    "modbus",
    "dlt645",
    "cjt188",
    "opcua",
    "iec104",
    "iec101",
    "mc",
    "bacnet",
    "bacnet_mstp",
    "s7comm",
    "fins",
    "gpio",
];

/// 为配置校验 / 运行时建链失败生成统一的可读错误信息。
///
/// - 协议已知但未编入当前发行版：提示该协议被 SKU 裁剪或处于退役观察，并给出
///   需要启用的 Cargo feature；
/// - 协议未知（拼错）：列出当前版本可用协议。
pub fn protocol_error_message(protocol: &str) -> String {
    let lower = protocol.to_ascii_lowercase();
    if KNOWN_PROTOCOLS.contains(&lower.as_str()) {
        // 配置协议名与 Cargo feature 名的两处不一致：
        // - bacnet_mstp → bacnet-mstp
        // - opcua → opcua-client：`opcua` 配置项是**南向采集**，由 opcua-client 编译；
        //   `opcua` feature 只是 client+server 的合并别名，不能直接作为提示
        let feature = match lower.as_str() {
            "bacnet_mstp" => "bacnet-mstp",
            "opcua" => "opcua-client",
            _ => lower.as_str(),
        };
        format!(
            "协议 '{}' 未包含在当前发行版（已按 SKU 裁剪或处于退役观察）。本版可用协议: {:?}；\
             如确需该协议，请重新构建并启用 feature '{}'，或改用包含它的发行包。",
            protocol, VALID_PROTOCOLS, feature
        )
    } else {
        format!(
            "unknown protocol '{}', valid protocols in this build: {:?}",
            protocol, VALID_PROTOCOLS
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不变式：当前版本可用协议必须是已知协议的子集（防止新增协议漏登记 KNOWN）。
    #[test]
    fn valid_protocols_are_known() {
        for p in VALID_PROTOCOLS {
            assert!(
                KNOWN_PROTOCOLS.contains(p),
                "VALID_PROTOCOLS 中的 '{}' 未登记到 KNOWN_PROTOCOLS",
                p
            );
        }
    }

    /// 未编入 opcua-client 时，提示必须指向真正需要启用的 feature（而非合并别名）。
    #[cfg(not(feature = "opcua-client"))]
    #[test]
    fn disabled_opcua_points_to_opcua_client_feature() {
        let msg = protocol_error_message("opcua");
        assert!(msg.contains("未包含在当前发行版"), "实际信息: {msg}");
        assert!(msg.contains("feature 'opcua-client'"), "实际信息: {msg}");
    }

    /// 已知但未编入当前发行版的协议必须给出「未包含在本版本」的可操作提示。
    #[cfg(not(feature = "fins"))]
    #[test]
    fn known_but_disabled_protocol_has_actionable_message() {
        let msg = protocol_error_message("fins");
        assert!(msg.contains("未包含在当前发行版"), "实际信息: {msg}");
        assert!(msg.contains("feature 'fins'"), "实际信息: {msg}");
    }

    /// 拼错的协议名不应被误报为「已裁剪」。
    #[test]
    fn unknown_protocol_message_is_distinct() {
        let msg = protocol_error_message("modbu");
        assert!(msg.contains("unknown protocol"), "实际信息: {msg}");
        assert!(!msg.contains("未包含在当前发行版"), "实际信息: {msg}");
    }
}
