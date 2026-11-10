use super::throttle::{
    circuit_cooldown_active, throttle_wait_ms, token_bucket_cap, token_bucket_refill,
};

/// r41-MQTT2 回归：冷却判定对时钟回拨（now < opened_at）必须安全。
/// 旧实现 `now - opened_at` 在 u64 下溢（debug panic / release 回绕误判冷却已过）。
#[test]
fn circuit_cooldown_handles_clock_rollback() {
    // 正常：冷却期内 → 仍阻断
    assert!(circuit_cooldown_active(1_000, 1_010, 30));
    // 正常：冷却已过 → 放行（半开）
    assert!(!circuit_cooldown_active(1_000, 1_040, 30));
    // 时钟回拨：now < opened_at → 饱和为 0，保守视为仍在冷却期（不 panic / 不提前半开）
    assert!(circuit_cooldown_active(2_000, 1_990, 30));
    assert!(circuit_cooldown_active(2_000, 0, 30));
}

/// P0-3 契约：首次调用给满桶（cap = rate * 突发系数），额度单位是"条"。
#[test]
fn token_bucket_first_call_fills_to_cap() {
    // rate=10 → cap=20，首次调用直接给 20 条额度
    assert_eq!(token_bucket_refill(0, 0, 10, 1_000), (1_000, 20));
    // 突发系数恒为 2
    assert_eq!(token_bucket_cap(10), 20);
    assert_eq!(token_bucket_cap(1_000), 2_000);
}

/// P0-3 契约：补充量按 rate*elapsed/1000 计算，且令牌上限为 cap。
#[test]
fn token_bucket_refills_and_caps() {
    // 1s 补 1000 令牌（rate=1000），上限 2000
    assert_eq!(token_bucket_refill(1_000, 0, 1_000, 2_000), (2_000, 1_000));
    // 累积补充被上限 2000 截断（1900 + 补 2000 → 封顶 2000）
    assert_eq!(
        token_bucket_refill(1_000, 1_900, 1_000, 3_000),
        (3_000, 2_000)
    );
}

/// 契约：elapsed 的补充量取整为 0 时，不推进时间戳（避免高频调用抹平补充）
#[test]
fn token_bucket_small_elapsed_keeps_timestamp() {
    // rate=1000, elapsed=1ms → refill=1 成立，正常推进
    assert_eq!(token_bucket_refill(1_000, 0, 1_000, 1_001), (1_001, 1));
    // rate=1, elapsed=999ms → refill=0，时间戳保持不回退
    assert_eq!(token_bucket_refill(1_000, 0, 1, 1_999), (1_000, 0));
}

/// P0-3 契约：时钟回拨（now < last_refill）不得下溢，也不得补充令牌。
#[test]
fn token_bucket_handles_clock_rollback() {
    // now=900 < last_refill=1000 → elapsed 饱和为 0，时间戳与令牌都不变
    assert_eq!(token_bucket_refill(1_000, 50, 100, 900), (1_000, 50));
}

/// P0-3 契约：退避时长按"攒够一整批"计算，且单次等待不超过上限。
#[test]
fn throttle_wait_scales_with_batch_target() {
    // rate=1000，攒 500 条 → 500ms
    assert_eq!(throttle_wait_ms(500, 0, 1_000), 500);
    // rate=200，攒 500 条 → 2500ms，被 MAX_THROTTLE_WAIT_MS(1000) 截断
    assert_eq!(throttle_wait_ms(500, 0, 200), 1_000);
    // 已攒 400 条、还差 100 条，rate=1000 → 100ms
    assert_eq!(throttle_wait_ms(500, 400, 1_000), 100);
    // 至少等 1ms（available 已满时也保持最小退避）
    assert_eq!(throttle_wait_ms(0, 0, 1_000), 1);
}
