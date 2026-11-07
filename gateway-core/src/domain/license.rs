//! 授权领域模型（档位 + 配额）
//!
//! 本模块存放**授权档位与配额上限**的领域类型。它们原先定义在
//! `license` 模块（`Tier` 在 `license/mod.rs`，`Quota` 等在 `license/verify.rs`），
//! 但配置层 `config::license`（`LicenseConfig` / `TierQuotaEntry`）同样需要引用它们，
//! 于是形成 `config → license → config` 的双向循环。
//!
//! 下沉到 `domain` 中立层后：
//! - `config` 引用 `crate::domain::{Tier, Quota}`；
//! - `license` 引用同一份 `crate::domain::license::{Tier, Quota, ...}`；
//! - 两侧都只依赖中立层 `domain`，反向依赖被消除。
//!
//! 与 [`VALID_PROTOCOLS`](crate::domain::VALID_PROTOCOLS) 同属"下沉到中立层消除
//! 层间反向依赖"的先例。
//!
//! # 对外路径兼容
//!
//! `gateway_core::license::Tier` / `gateway_core::license::Quota` 等原公开路径
//! 由 `license` 模块的 `pub use` 转出保持不变，外部调用方（gateway-api、
//! gatewayd、tools/license-issuer）无需改动。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 档位（与前端 `type Tier` 一致，小写序列化）
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    TS,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Tier {
    #[default]
    Core,
    Standard,
    Pro,
    Ultra,
}

/// 配额上限（v2.0 license 模型，替换原 v1.2 的 features 细粒度开关）
///
/// 各字段语义：
/// - `None` = 无限配额（serialized 为 JSON `null`）
/// - `Some(n)` = 上限为 n
///
/// 字段缺失 = 无限，向后兼容老 payload：旧 license 没 quota 字段时，
/// `LicensePayload.quota` 解析为 `None`，网关按"该配额无上限"处理。
///
/// 序列化说明：`Option<T>` 字段保留 None（输出 JSON null），不省略——前端类型
/// 与后端字段一一对应，且 `null` 是 ts-rs 生成的 TS 类型 `T | null` 的合法值。
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[ts(export, export_to = "Quota.ts")]
pub struct Quota {
    /// 最大点位总数（None = 无限）
    pub max_points: Option<u32>,
    /// 最大并发协议数（None = 无限）
    pub max_protocols: Option<u32>,
    /// 历史数据保留天数（None = 无限；超限按 FIFO 清理）
    pub storage_days: Option<u32>,
    /// 北向上行并发通道数（None = 无限）
    pub northbound_channels: Option<u32>,
}

impl Quota {
    /// 所有 4 个字段都是 None（license 自带 quota 全空时的判定）
    /// LicenseManager 用此判断"license 没配 quota"→ 走配置兜底
    pub fn is_all_none(&self) -> bool {
        self.max_points.is_none()
            && self.max_protocols.is_none()
            && self.storage_days.is_none()
            && self.northbound_channels.is_none()
    }

    /// 剩余额度 = 上限 - 已用（逐项饱和减法，已超限记 0）
    ///
    /// 上限为 None（无限）时该项剩余也是 None（前端显示"不限"）。
    pub fn available(&self, usage: &QuotaUsage) -> QuotaAvailable {
        fn rest(limit: Option<u32>, used: u32) -> Option<u32> {
            limit.map(|l| l.saturating_sub(used))
        }
        QuotaAvailable {
            points: rest(self.max_points, usage.points),
            protocols: rest(self.max_protocols, usage.protocols),
            storage_days: rest(self.storage_days, usage.storage_days),
            northbound_channels: rest(self.northbound_channels, usage.northbound_channels),
        }
    }

    /// 已超出上限的配额项（已用 > 上限）——**会拒绝保存配置**的硬约束
    ///
    /// v2.1 起只有「采集点数」是硬约束：协议种类数与北向通道数反映的是
    /// 部署形态而非产能规模，按它们拦保存配置会在现场反复咬人
    /// （买的是高档位，配置里多挂一个协议就被拒），且这两项从 v2.1 起
    /// 在默认档位表里本就是 `None`（不限）。
    ///
    /// 显式配置了 `max_protocols` / `northbound_channels` 上限的部署，
    /// 其超限情况改由 [`Quota::warned_kinds`] 报告（只告警，不拒绝）。
    ///
    /// `storage_days` 不在此列：它由运行时 FIFO 截断保证，不会拒绝配置保存，
    /// 拿它报警只会制造噪音。
    ///
    /// 典型场景：导入低档位 license 后，原有配置已超出新档位配额。
    pub fn exceeded_kinds(&self, usage: &QuotaUsage) -> Vec<QuotaKind> {
        let mut out = Vec::new();
        if self.max_points.is_some_and(|l| usage.points > l) {
            out.push(QuotaKind::Points);
        }
        out
    }

    /// 已超出上限但**只告警不拒绝**的配额项
    ///
    /// v2.1 起协议种类数 / 北向通道数不再阻断配置保存：默认值本就无限，
    /// 只有运维在 `license.tier_quotas` 或 license 文件里显式写了上限时
    /// 才可能触发。这种显式配置通常是「本项目的约定」而非「授权边界」，
    /// 阻断保存会直接卡住交付，改为告警 + 提示更符合现场预期。
    pub fn warned_kinds(&self, usage: &QuotaUsage) -> Vec<QuotaKind> {
        let mut out = Vec::new();
        if self.max_protocols.is_some_and(|l| usage.protocols > l) {
            out.push(QuotaKind::Protocols);
        }
        if self
            .northbound_channels
            .is_some_and(|l| usage.northbound_channels > l)
        {
            out.push(QuotaKind::NorthboundChannels);
        }
        out
    }
}

/// 配额已用量（当前生效配置/数据的实际占用）
///
/// 与 `Quota` 的 4 个上限一一对应：
/// - `points`：展开后的采集点数（模板展开口径，与配额校验同源）
/// - `protocols`：已配置协议种类数
/// - `storage_days`：实际保留的历史数据跨度（天）
/// - `northbound_channels`：已启用的北向通道数
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[ts(export, export_to = "QuotaUsage.ts")]
pub struct QuotaUsage {
    /// 已用采集点数
    pub points: u32,
    /// 已用并发协议数
    pub protocols: u32,
    /// 已用数据保留天数（实际历史跨度）
    pub storage_days: u32,
    /// 已用北向通道数
    pub northbound_channels: u32,
}

/// 配额剩余量（None = 该项无上限）
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[ts(export, export_to = "QuotaAvailable.ts")]
pub struct QuotaAvailable {
    /// 剩余可用采集点数
    pub points: Option<u32>,
    /// 剩余可用并发协议数
    pub protocols: Option<u32>,
    /// 剩余可用数据保留天数
    pub storage_days: Option<u32>,
    /// 剩余可用北向通道数
    pub northbound_channels: Option<u32>,
}

/// 配额类型（LicenseManager::check_quota 第一个参数）
///
/// 与 Quota 字段一一对应；增加字段时同步更新 match 分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaKind {
    Points,
    Protocols,
    StorageDays,
    NorthboundChannels,
}

impl QuotaKind {
    /// 当前 kind 在 Quota 中的字段值
    pub fn value_in(&self, q: &Quota) -> Option<u32> {
        match self {
            QuotaKind::Points => q.max_points,
            QuotaKind::Protocols => q.max_protocols,
            QuotaKind::StorageDays => q.storage_days,
            QuotaKind::NorthboundChannels => q.northbound_channels,
        }
    }

    /// 客户可读的中文标签（用于 4021 错误 message）
    pub fn label_zh(&self) -> &'static str {
        match self {
            QuotaKind::Points => "采集点数",
            QuotaKind::Protocols => "并发协议数",
            QuotaKind::StorageDays => "数据保留天数",
            QuotaKind::NorthboundChannels => "北向通道数",
        }
    }
}
