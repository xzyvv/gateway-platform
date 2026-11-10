//! MQTT 发布侧的纯函数：断路器冷却判定、限速退避、令牌桶。
//!
//! 从 `publish.rs` 抽出：这些函数不持有状态、只做算术，单独成模块便于回归测试，
//! 也让发布主路径保持在行数预算内。

use super::{MAX_THROTTLE_WAIT_MS, TOKEN_BUCKET_BURST_FACTOR};

/// r41-MQTT2：断路器冷却判定（纯函数，便于回归测试）。
///
/// 旧实现直接做 `now - opened_at`（均为 u64）：系统时钟回拨使 `now < opened_at`
/// （NTP 校时场景可见）时下溢——debug 构建 panic，release 构建回绕成大数、
/// `now - opened_at < cooldown` 为假 → 误判"冷却已过"提前半开。
/// 改为饱和减法：时钟回拨时 `elapsed = 0`，按"刚打开"处理（保守地留在冷却期，
/// 不提前放行对故障 broker 的发布）。
pub(super) fn circuit_cooldown_active(opened_at: u64, now: u64, cooldown_secs: u64) -> bool {
    now.saturating_sub(opened_at) < cooldown_secs
}

/// P0-3：限速退避时长计算（纯函数）。
///
/// 目标是**攒够一整批**令牌再发，而不是"攒 1 条发 1 条"——后者虽然平均
/// 速率正确，但会让 MQTT 往返次数暴涨（每条一次 PubAck 等待），把串行
/// publish 路径的吞吐压到远低于配置值。
///
/// `need = batch_target - available`（至少 1），所需时间 = `need * 1000 / rate`
/// 毫秒；单次等待封顶 `MAX_THROTTLE_WAIT_MS`，保证关停响应不被拖住
/// （超时后再次计算，仍不足则继续短等待）。
pub(super) fn throttle_wait_ms(batch_target: usize, available: u64, rate: u64) -> u64 {
    let need = (batch_target as u64).saturating_sub(available).max(1);
    let ms = need
        .saturating_mul(1000)
        .saturating_add(rate.saturating_sub(1))
        / rate.max(1);
    ms.clamp(1, MAX_THROTTLE_WAIT_MS)
}

/// 令牌桶容量（突发上限，单位：条）。
///
/// P0-3：容量 = rate * 突发系数。允许补发开始时短时突发，长期平均速率仍受
/// rate（条/秒）约束。rate=0 时返回 0（调用方不应进入限速分支）。
pub(super) fn token_bucket_cap(rate: u64) -> u64 {
    rate.saturating_mul(TOKEN_BUCKET_BURST_FACTOR)
}

/// P0-3：令牌桶补充计算（纯函数，便于回归测试）。
///
/// 返回 `(新的补充时间戳, 新的令牌数)`：
/// - `last_refill == 0` 视为首次调用：直接给满桶（= cap），允许一次突发；
/// - 令牌上限 `cap`（= rate * 突发系数），补充量 `rate * elapsed_ms / 1000`
///   （整毫秒取整为 0 时不推进时间戳，避免高频调用把补充量抹平）；
/// - 时钟回拨（now < last_refill）时 elapsed 饱和为 0，保守地不补充也不回退。
///
/// 与旧实现的关键差异：旧 `token_bucket_step` 把 `take` 夹在 `max_size`
/// （= 单批条数上限）之内，令牌桶实际从未限制过发送量；本函数**只做补充**，
/// 可用额度由调用方按"批量目标 vs 可用令牌"取小得出。
pub(super) fn token_bucket_refill(
    last_refill: u64,
    tokens: u64,
    rate: u64,
    now_ms: u64,
) -> (u64, u64) {
    let cap = token_bucket_cap(rate);
    if last_refill == 0 {
        // 首次调用：满桶，允许一次突发（每 rate 条的 2 倍）。
        return (now_ms, cap);
    }
    let mut last_refill = last_refill;
    let mut tokens = tokens.min(cap);
    let elapsed_ms = now_ms.saturating_sub(last_refill);
    if elapsed_ms > 0 {
        let refill = rate.saturating_mul(elapsed_ms) / 1000;
        if refill > 0 {
            last_refill = now_ms;
            tokens = tokens.saturating_add(refill).min(cap);
        }
    }
    (last_refill, tokens)
}
