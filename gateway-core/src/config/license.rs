//! License 校验配置节（§12 + v2.0 配额配置）
//!
//! 全部可选：未设置时按环境变量 `GATEWAY_LICENSE_PATH` → 默认 `./license.lic` 查找，
//! 不存在则视为无 license（开源版/未授权版可用，tier=core）。
//!
//! v2.0 新增 tier_quotas：tier → 配额默认映射，license 自带 quota 时优先使用，
//! 缺失时按 tier 查表；配置里没写该档时回落到代码内置的四档默认表
//! （`DEFAULT_TIER_QUOTAS`），不会静默降到 absent_quota。
//! 字段值 `null` = 无限配额。运维改配额不用发版，直接编辑 config.json 的 license 节。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::{Quota, Tier};

/// tier → 配额 配置项（serde 友好的 Vec 形式，BTreeMap 内部加速查找）
///
/// 用 Vec 而不是 BTreeMap 或 HashMap 是因为 serde 对 enum key 的 BTreeMap
/// 支持较弱（需要 `serde_with`），Vec 在 toml/json 都直接可用。
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct TierQuotaEntry {
    pub tier: Tier,
    pub quota: Quota,
}

/// License 校验配置（§12）
///
/// 缺省时所有字段均为 None，按环境变量 → 默认路径查找；
/// tier_quotas 缺省时所有 tier 都按代码内置默认表（`DEFAULT_TIER_QUOTAS`）兜底。
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct LicenseConfig {
    /// license 文件路径（相对路径相对于进程 CWD）
    /// 优先级：环境变量 `GATEWAY_LICENSE_PATH` > 此处 > 默认 `./license.lic`
    #[serde(default)]
    pub path: Option<String>,

    /// v2.0 tier → 配额默认映射（运维可调，license 文件未带 quota 时按此兜底）
    /// 未显式配置的 tier 走代码内置默认表 `DEFAULT_TIER_QUOTAS`
    #[serde(default)]
    pub tier_quotas: Option<Vec<TierQuotaEntry>>,

    /// v2.0 无 license / 失效 / 缺失时回退到的默认配额（开源版体验）
    /// 缺省走代码内 DEFAULT_ABSENT_QUOTA（100 点 / 1 协议 / 1 通道；保留天数不属授权）
    #[serde(default)]
    pub absent_quota: Option<Quota>,
}

/// 代码内默认无 license 配额（与 docs/design/license-v2-quota §3 表格末行一致）
///
/// 选用保守值（100 点 / 1 协议 / 1 通道）——
/// 开源用户能跑最小演示，付费用户有动力升级。
/// `storage_days` 仅为 .lic 格式兼容保留，v2.1 起与授权无关（历史保留固定 30 天）。
/// 运维若需调整，可直接在 config.json 的 license.absent_quota 覆盖。
pub const DEFAULT_ABSENT_QUOTA: Quota = Quota {
    max_points: Some(100),
    max_protocols: Some(1),
    storage_days: Some(30),
    northbound_channels: Some(1),
};

/// 代码内置的四档默认配额（与 config.json.example 的 license.tier_quotas 一致）
///
/// 存在意义：运维在 config.json 漏配某一档时，license 未自带 quota 的付费设备
/// 原本会被静默降到 `absent_quota`（100 点 / 1 协议 / 7 天 / 1 通道）——
/// 现场只表现为「配额偏小」，很难联想到是配置漏项。
/// 内置默认表让漏配只降级到**该档应有的数值**，而不是降到开源版数值。
///
/// 优先级：config.json 的 `license.tier_quotas` > 本表 > `absent_quota`（仅无 license 场景）。
///
/// ## v2.1 口径：配额只管点位
///
/// 四档之间**唯一的差异是 `max_points`**（采集点数上限），其余三项统一：
/// - `max_protocols` / `northbound_channels` = `None`（不限）。
///   协议种类数与北向通道数反映的是**部署形态**而非产能规模，按它们分档会让
///   「买得起高档位却用不满」的现场反复超限；且行业惯例（Takebishi / Moxa）
///   也只对点位与设备连接数分档，不对协议种类数分档。
/// - `storage_days` = 30（四档统一，含 ultra）。
///   历史保留天数在边缘侧是**运维策略**而非授权卖点：现场磁盘容量有限，
///   30 天足够覆盖绝大多数抄表与追溯场景；按档位拉长到 90/365 天会让
///   低配设备被历史数据挤爆存储，且超期数据按 FIFO 直接丢弃、不可恢复。
///   v2.1 起运行时不再读取该字段（队列启动即固定 30 天），此处仅为 .lic 格式兼容。
///
/// 注：`max_protocols` / `northbound_channels` 字段本身保留在 `Quota` 结构里
/// （license 文件格式兼容，老 .lic 照常解析），但**不再作为拒绝保存配置的依据**，
/// 仅在显式配置了上限时降级为告警提示。
pub const DEFAULT_TIER_QUOTAS: [(Tier, Quota); 4] = [
    (
        Tier::Core,
        Quota {
            max_points: Some(500),
            max_protocols: None,
            storage_days: Some(30),
            northbound_channels: None,
        },
    ),
    (
        Tier::Standard,
        Quota {
            max_points: Some(1500),
            max_protocols: None,
            storage_days: Some(30),
            northbound_channels: None,
        },
    ),
    (
        Tier::Pro,
        Quota {
            max_points: Some(2500),
            max_protocols: None,
            storage_days: Some(30),
            northbound_channels: None,
        },
    ),
    (
        Tier::Ultra,
        Quota {
            max_points: Some(5000),
            max_protocols: None,
            storage_days: Some(30),
            northbound_channels: None,
        },
    ),
];

/// 取某档位的代码内置默认配额
pub fn default_quota_for_tier(tier: Tier) -> Quota {
    DEFAULT_TIER_QUOTAS
        .iter()
        .find(|(t, _)| *t == tier)
        .map(|(_, q)| q.clone())
        .unwrap_or_else(|| DEFAULT_ABSENT_QUOTA.clone())
}

impl LicenseConfig {
    /// 解析最终生效的"无 license 默认配额"
    /// 优先级：配置 absent_quota → 代码内 DEFAULT_ABSENT_QUOTA
    pub fn effective_absent_quota(&self) -> Quota {
        self.absent_quota
            .clone()
            .unwrap_or_else(|| DEFAULT_ABSENT_QUOTA.clone())
    }

    /// 内部 BTreeMap 加速查询（避免每次 quota_for_tier 都线性扫描 Vec）
    fn tier_quotas_map(&self) -> BTreeMap<Tier, Quota> {
        self.tier_quotas
            .as_ref()
            .map(|v| v.iter().map(|e| (e.tier, e.quota.clone())).collect())
            .unwrap_or_default()
    }

    /// 该 tier 是否已在 tier_quotas 中显式配置
    pub fn tier_configured(&self, tier: Tier) -> bool {
        self.tier_quotas_map().contains_key(&tier)
    }

    /// 按 tier 查有效配额（license 自带 quota 不在此函数内处理，那是 LicenseManager 逻辑）
    ///
    /// 优先级：tier_quotas[tier] → 代码内置默认表 `DEFAULT_TIER_QUOTAS` → absent_quota
    /// 字段缺失语义：None 字段保留 None = 无限；只在配置层与内置表都找不到 tier 映射时
    /// 才整体降级到 absent（内置表已覆盖全部四档，实际不会走到）。
    pub fn quota_for_tier(&self, tier: Tier) -> Quota {
        let map = self.tier_quotas_map();
        if let Some(q) = map.get(&tier) {
            return q.clone();
        }
        // tier 在配置中无映射（运维没填）→ 回落代码内置默认表（该档应有数值）
        default_quota_for_tier(tier)
    }

    /// 四档中尚未在 tier_quotas 里显式配置的档位（core / standard / pro / ultra）
    ///
    /// 这些档位生效配额来自代码内置默认表 `DEFAULT_TIER_QUOTAS`——
    /// 不会静默降到 absent 兜底，但配置值与内置值可能不一致，
    /// 启动告警与前端提示都靠本方法把「没显式配置哪几档」暴露出来。
    pub fn missing_tiers(&self) -> Vec<Tier> {
        [Tier::Core, Tier::Standard, Tier::Pro, Tier::Ultra]
            .into_iter()
            .filter(|t| !self.tier_configured(*t))
            .collect()
    }
}

#[cfg(test)]
mod tests;
