//! 行业 SKU × 授权档**采购推荐**表
//!
//! ## 定位：这是采购建议，不是运行时约束
//!
//! 本模块**不参与任何运行时配额判定**。它回答的是销售/售前问题：
//! 「客户买了电力版硬件（`sku-power`），应该配哪一档授权？」
//!
//! 为什么不做成运行时约束：SKU 是**编译期** Cargo feature 聚合
//! （见 `gateway-core/Cargo.toml` 的 `sku-*` 段），它决定二进制里有哪些协议；
//! 授权档决定**点位规模上限**。两者是**正交**的两件事——
//! `sku-minimal` 配 ultra 授权在技术上完全合法（只是多买了点位额度），
//! 反过来 `sku-power` 配 core 档则会因点位不够而买小了。
//!
//! 行业惯例同样如此：研华/映翰通把协议支持度做进**硬件型号**，
//! 把规模上限做进**软件授权**，两条售卖轴彼此独立（见 Cisco Smart Licensing
//! 的 tier × 型号对照表）。所以本模块给出的是**推荐组合**，供报价时参考。
//!
//! ## 单一口径原则
//!
//! 推荐档位只按**点数规模**推导——因为 v2.1 起配额只管点位，
//! 协议种类数与北向通道数在四档之间已无差异（见
//! `config::license::DEFAULT_TIER_QUOTAS`）。
//!
//! ## 编译期识别当前 SKU
//!
//! SKU 由 Cargo feature 决定，运行时可由 `cfg!(feature = "sku-*")` 判定。
//! 多个 SKU feature 同时启用时按**协议覆盖面从窄到宽**取第一个命中
//! （窄 SKU 的推荐档位更保守），并可用 [`Sku::current`] 自行校验。

use crate::domain::license::Tier;

/// 行业 SKU 枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sku {
    /// 精简版：仅计量三件套（Modbus / DL/T 645 / CJ/T 188）
    Minimal,
    /// 标准版：通用计量 + IEC104 远动 + OPC UA Client
    Standard,
    /// 楼宇版：BACnet/IP 楼控 + 现场计量 + OPC UA
    Building,
    /// 电力版：电能量采集 + 远动（IEC104 / IEC101）
    Power,
    /// PLC 版：西门子 S7comm + 三菱 MC + Modbus 兼容接入
    Plc,
    /// 未启用任何 `sku-*` feature（全协议构建 / 自主集成）
    Full,
}

impl Sku {
    /// 当前构建的 SKU（编译期 feature 判定）
    ///
    /// 判定顺序按协议覆盖面从窄到宽：`sku-minimal` → `sku-building` →
    /// `sku-plc` → `sku-standard` → `sku-power` → 全协议。
    /// 这样即使误启用了多个 SKU feature，也取最保守（推荐档最低）的那一个。
    pub fn current() -> Sku {
        if cfg!(feature = "sku-minimal") {
            Sku::Minimal
        } else if cfg!(feature = "sku-building") {
            Sku::Building
        } else if cfg!(feature = "sku-plc") {
            Sku::Plc
        } else if cfg!(feature = "sku-standard") {
            Sku::Standard
        } else if cfg!(feature = "sku-power") {
            Sku::Power
        } else {
            Sku::Full
        }
    }

    /// SKU 标识串（与 Cargo feature 名一致，便于日志与文档对照）
    pub fn id(self) -> &'static str {
        match self {
            Sku::Minimal => "sku-minimal",
            Sku::Standard => "sku-standard",
            Sku::Building => "sku-building",
            Sku::Power => "sku-power",
            Sku::Plc => "sku-plc",
            Sku::Full => "full",
        }
    }

    /// 中文标签（对外展示用）
    pub fn label_zh(self) -> &'static str {
        match self {
            Sku::Minimal => "精简版",
            Sku::Standard => "标准版",
            Sku::Building => "楼宇版",
            Sku::Power => "电力版",
            Sku::Plc => "PLC 版",
            Sku::Full => "全协议版",
        }
    }

    /// 该 SKU 覆盖的南向协议（与 Cargo.toml 的 `sku-*` 定义对应）
    pub fn protocols(self) -> &'static [&'static str] {
        match self {
            Sku::Minimal => &["modbus", "dlt645", "cjt188"],
            Sku::Standard => &["modbus", "dlt645", "cjt188", "iec104", "opcua"],
            Sku::Building => &["modbus", "bacnet", "cjt188", "opcua"],
            Sku::Power => &["modbus", "dlt645", "iec104", "iec101", "opcua"],
            Sku::Plc => &["modbus", "s7comm", "mc", "opcua"],
            Sku::Full => &[
                "modbus", "dlt645", "cjt188", "opcua", "iec104", "bacnet", "s7comm", "mc", "iec101",
            ],
        }
    }

    /// 采购推荐档位
    ///
    /// 口径：**按该行业的典型点位规模选档**。
    /// - 精简版：小规模计量（单表/单回路）→ core
    /// - 楼宇版：单楼宇数百点 → standard
    /// - PLC 版：产线设备点 → standard
    /// - 标准版：通用计量 + 远动，数千点 → pro
    /// - 电力版：电表群 + 遥测遥信，点位规模最大 → pro
    /// - 全协议版：自主集成，规模不可预知 → ultra
    pub fn recommended_tier(self) -> Tier {
        match self {
            Sku::Minimal => Tier::Core,
            Sku::Building => Tier::Standard,
            Sku::Plc => Tier::Standard,
            Sku::Standard => Tier::Pro,
            Sku::Power => Tier::Pro,
            Sku::Full => Tier::Ultra,
        }
    }

    /// 采购推荐档位的点位上限（等于该档 `max_points`）
    pub fn recommended_max_points(self) -> u32 {
        match self.recommended_tier() {
            Tier::Core => 500,
            Tier::Standard => 1500,
            Tier::Pro => 2500,
            Tier::Ultra => 5000,
        }
    }

    /// 采购说明（一句中文，给报价单用）
    pub fn purchase_note(self) -> &'static str {
        match self {
            Sku::Minimal => "小规模计量场景；协议种类少，点位需求低",
            Sku::Building => "单楼宇楼控场景；数百点位",
            Sku::Plc => "产线 PLC 接入场景；数百至上千点位",
            Sku::Standard => "通用计量 + 远动；数千点位",
            Sku::Power => "电能量采集 + 远动；点位数最多",
            Sku::Full => "自主集成；规模不可预知，建议按最大档采购",
        }
    }

    /// 按标识串解析 SKU（供配置/签发端使用；未知值返回 None）
    pub fn from_id(id: &str) -> Option<Sku> {
        match id {
            "sku-minimal" | "minimal" => Some(Sku::Minimal),
            "sku-standard" | "standard" => Some(Sku::Standard),
            "sku-building" | "building" => Some(Sku::Building),
            "sku-power" | "power" => Some(Sku::Power),
            "sku-plc" | "plc" => Some(Sku::Plc),
            "full" => Some(Sku::Full),
            _ => None,
        }
    }

    /// 全部 SKU（用于文档/签发端枚举）
    pub fn all() -> [Sku; 6] {
        [
            Sku::Minimal,
            Sku::Standard,
            Sku::Building,
            Sku::Power,
            Sku::Plc,
            Sku::Full,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommended_tier_ordering_is_monotonic() {
        // 覆盖面越大 → 推荐档位越高（采购推荐不应出现「协议更多但推荐档更低」）
        assert!(Sku::Minimal.recommended_max_points() < Sku::Building.recommended_max_points());
        assert!(Sku::Building.recommended_max_points() <= Sku::Plc.recommended_max_points());
        assert!(Sku::Plc.recommended_max_points() < Sku::Standard.recommended_max_points());
        assert!(Sku::Standard.recommended_max_points() <= Sku::Power.recommended_max_points());
        assert!(Sku::Power.recommended_max_points() < Sku::Full.recommended_max_points());
    }

    #[test]
    fn test_from_id_roundtrip() {
        for sku in Sku::all() {
            assert_eq!(Sku::from_id(sku.id()), Some(sku));
        }
        assert_eq!(Sku::from_id("nope"), None);
    }

    /// 当前 SKU 的协议数必须与其 `protocols()` 一致（自洽性）
    #[test]
    fn test_current_sku_protocols_non_empty() {
        assert!(!Sku::current().protocols().is_empty());
    }
}
