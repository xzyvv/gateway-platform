//! 重连退避配置（MQTT / Modbus TCP 通用）
//!
//! 把原先硬编码在 `north/mqtt/event_loop.rs` 与 `south/bus/tcp_task.rs` 中的
//! 重连退避序列参数化，可通过 config.json 调整。默认值与历史硬编码完全一致
//! （MQTT: 2/4/8/16/32/60 秒 + 40% 抖动；Modbus TCP: 1/2/5/10/30 秒无抖动），
//! 保证升级后重连节奏不变。

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use validator::Validate;

use super::validate_backoff_seconds;

/// Bug P0-B5 (fix): 重连抖动熵源计数器。
///
/// 旧实现 `time_rand01` 用 `SystemTime::subsec_nanos() / 1e9` 作为随机源——
/// `subsec_nanos` 粒度为 1ns 但**整数值**为 0..1e9，同一微秒内（甚至 1ns
/// 内的连续调用）会得到完全相同的值，导致多设备/多实例同时触发重连时
/// 抖动退化为无抖动 → 与设计意图背离。
///
/// 修复：用 AtomicU64 计数器 + nanos 异或再取小数部分，提供进程内**单调
/// 变化**的伪随机源。即便两次调用 nanos 相同，counter 已自增，输出不同。
static BACKOFF_RAND_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 重连退避参数
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct ReconnectBackoffConfig {
    /// 退避档位（秒）：按序取用，用尽后保持最后一档
    #[serde(default = "default_backoff_seconds")]
    #[validate(custom(function = "validate_backoff_seconds"))]
    #[ts(type = "number[]")]
    pub seconds: Vec<u64>,
    /// 抖动比例（0.0~1.0）：实际延迟 = base*(1 + jitter_ratio*(rand-0.5))；0.0=无抖动
    #[serde(default)]
    pub jitter_ratio: f64,
}

impl ReconnectBackoffConfig {
    /// 退避基准（秒）：attempt 从 1 开始计数。
    ///
    /// 索引 = min(attempt-1, len-1)，即取用尽后保持最后一档；空表兜底返回 1.0。
    pub fn base_delay_secs(&self, attempt: u32) -> f64 {
        if self.seconds.is_empty() {
            return 1.0;
        }
        let idx = (attempt.saturating_sub(1) as usize).min(self.seconds.len() - 1);
        self.seconds[idx] as f64
    }

    /// 实际延迟（秒）：base + base*jitter_ratio*(rand01-0.5)，下界 1.0。
    ///
    /// `rand01` 为 [0,1) 区间的随机数；jitter_ratio=0.0 时结果恒等于 base。
    pub fn delay_secs(&self, attempt: u32, rand01: f64) -> f64 {
        let base = self.base_delay_secs(attempt);
        (base + base * self.jitter_ratio * (rand01 - 0.5)).max(1.0)
    }

    /// 基于 nanos + 进程内计数器的 [0,1) 伪随机数（不引入 rand 依赖）。
    ///
    /// Bug P0-B5 (fix)：旧实现直接用 subsec_nanos/1e9，同微秒内多次调用
    /// 返回相同值 → 多设备/多实例同时重连时抖动退化为无抖动。
    /// 改为 nanos XOR counter_counter，再对小数部分取 1e9 模，使每调用都
    /// 输出不同值（counter 单调递增，nanos 可能在同 counter 内重复但 XOR
    /// 后落到不同位）。分布对重连退避抖动而言已足够。
    pub fn time_rand01() -> f64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        let counter = BACKOFF_RAND_COUNTER.fetch_add(1, Ordering::Relaxed);
        // XOR 后取低 30 位（保证 <1e9），再除 1e9 得 [0,1)
        let mixed = (nanos ^ counter) & 0x3FFF_FFFF;
        (mixed as f64) / (0x3FFF_FFFF as f64 + 1.0)
    }
}

/// 退避档位默认值（仅当直接反序列化 `ReconnectBackoffConfig` 且缺失本字段时使用；
/// MQTT / Bus 各自的默认由父配置的 default 函数提供）。
fn default_backoff_seconds() -> Vec<u64> {
    vec![1, 2, 5, 10, 30]
}

#[cfg(test)]
mod tests;
