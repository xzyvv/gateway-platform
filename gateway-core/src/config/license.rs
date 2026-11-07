//! License 校验配置节（§12 + v2.0 配额配置）
//!
//! 全部可选：未设置时按环境变量 `GATEWAY_LICENSE_PATH` → 默认 `./license.lic` 查找，
//! 不存在则视为无 license（开源版/未授权版可用，tier=core）。
//!
//! v2.0 新增 tier_quotas：tier → 配额默认映射，license 自带 quota 时优先使用，
//! 缺失时按 tier 查表，core/standard/pro/ultra 全部缺失走 absent_quota。
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
/// tier_quotas 缺省时所有 tier 都按 absent_quota 兜底。
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
    /// 字段缺失或 None 的 tier 走 absent_quota
    #[serde(default)]
    pub tier_quotas: Option<Vec<TierQuotaEntry>>,

    /// v2.0 无 license / 失效 / 缺失时回退到的默认配额（开源版体验）
    /// 缺省走代码内 DEFAULT_ABSENT_QUOTA（100 点 / 1 协议 / 7 天 / 1 通道）
    #[serde(default)]
    pub absent_quota: Option<Quota>,
}

/// 代码内默认无 license 配额（与 docs/design/license-v2-quota §3 表格末行一致）
///
/// 选用保守值（100 点 / 1 协议 / 7 天 / 1 通道）——
/// 开源用户能跑最小演示，付费用户有动力升级。
/// 运维若需调整，可直接在 config.json 的 license.absent_quota 覆盖。
pub const DEFAULT_ABSENT_QUOTA: Quota = Quota {
    max_points: Some(100),
    max_protocols: Some(1),
    storage_days: Some(7),
    northbound_channels: Some(1),
};

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
    /// 优先级：tier_quotas[tier] → absent_quota（effective_absent_quota）
    /// 字段缺失语义：None 字段保留 None = 无限；只在配置层找不到 tier 映射时整体降级到 absent。
    pub fn quota_for_tier(&self, tier: Tier) -> Quota {
        let map = self.tier_quotas_map();
        if let Some(q) = map.get(&tier) {
            return q.clone();
        }
        // tier 在配置中无映射（运维没填），整体降级到 absent_quota
        self.effective_absent_quota()
    }

    /// 四档中尚未在 tier_quotas 里配置的档位（core / standard / pro / ultra）
    ///
    /// 运维漏配某档时，license 未自带 quota 的付费用户会被静默降到 absent 兜底配额。
    /// 启动告警与前端提示都靠本方法把「缺哪几档」暴露出来。
    pub fn missing_tiers(&self) -> Vec<Tier> {
        [Tier::Core, Tier::Standard, Tier::Pro, Tier::Ultra]
            .into_iter()
            .filter(|t| !self.tier_configured(*t))
            .collect()
    }
}

#[cfg(test)]
mod tests;
