//! 自愈执行器配置（`[self_heal]` 节）
//!
//! 配置类型归属 `config` 层；运行时决策参数 [`SelfHealSettings`] 由本结构体转换而来
//! （见 `crate::web::heal`），避免 `config` 反向依赖 `web` 层。
//!
//! 默认 **关闭**（`enabled = false`）：存量部署升级后行为零变化，opt-in 启用。
//! 字段顺序保持序列化兼容性——新增字段必须追加在末尾。

#[derive(
    Debug,
    Clone,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    validator::Validate,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[ts(export)]
pub struct SelfHealConfig {
    /// 是否启用自愈执行器（默认关闭）
    #[serde(default)]
    pub enabled: bool,
    /// 快照采集/评估周期（秒），必须 >= 1
    #[serde(default = "default_check_interval")]
    #[validate(range(min = 1))]
    #[ts(type = "number")]
    pub check_interval_secs: u64,
    /// 连续异常达到该次数才触发动作（防抖），必须 >= 1
    #[serde(default = "default_debounce_k")]
    #[validate(range(min = 1))]
    pub debounce_k: u32,
    /// 同一 target 两次动作的最小间隔（冷却，秒），允许 0 表示无冷却
    #[serde(default = "default_cooldown")]
    #[ts(type = "number")]
    pub cooldown_secs: u64,
    /// 限速滚动窗口长度（秒），必须 >= 1
    #[serde(default = "default_window")]
    #[validate(range(min = 1))]
    #[ts(type = "number")]
    pub window_secs: u64,
    /// 滚动窗口内允许的最大动作数（限速），必须 >= 1
    #[serde(default = "default_max_actions")]
    #[validate(range(min = 1))]
    pub max_actions_per_window: u32,
    /// 连续恢复失败达到该次数则熔断该 target（停止自动动作），必须 >= 1
    #[serde(default = "default_fuse")]
    #[validate(range(min = 1))]
    pub fail_streak_to_fuse: u32,
}

impl Default for SelfHealConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            check_interval_secs: default_check_interval(),
            debounce_k: default_debounce_k(),
            cooldown_secs: default_cooldown(),
            window_secs: default_window(),
            max_actions_per_window: default_max_actions(),
            fail_streak_to_fuse: default_fuse(),
        }
    }
}

fn default_check_interval() -> u64 {
    15
}
fn default_debounce_k() -> u32 {
    3
}
fn default_cooldown() -> u64 {
    300
}
fn default_window() -> u64 {
    3600
}
fn default_max_actions() -> u32 {
    10
}
fn default_fuse() -> u32 {
    3
}

#[cfg(test)]
mod tests;
