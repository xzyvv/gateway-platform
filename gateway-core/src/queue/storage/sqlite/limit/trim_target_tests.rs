use super::{trim_target, TRIM_COOLDOWN_MS, TRIM_MAX_ROWS_PER_RUN};

/// 未超限时不 trim（入队热路径的常态，必须零 DB 开销）。
#[test]
fn none_when_below_limit() {
    assert_eq!(trim_target(999, 1000, 0, 10_000, 10), None);
    assert_eq!(trim_target(1000, 1000, 0, 10_000, 10), None);
    assert_eq!(trim_target(0, 1000, 0, 10_000, 0), None);
}

/// 刚超限且缺口小于单轮预算时，一次削到上限的 90%（滞回水位）。
#[test]
fn trims_to_ninety_percent_within_budget() {
    assert_eq!(trim_target(1001, 1000, 0, 5, 0), Some(900));
    // 缺口刚好等于预算（900 + 2000 = 2900）：仍然一次削到 90%，不浪费预算
    assert_eq!(
        trim_target(900 + TRIM_MAX_ROWS_PER_RUN, 1000, 0, 5, 0),
        Some(900)
    );
}

/// 远超上限时单轮只删 `TRIM_MAX_ROWS_PER_RUN` 行，避免长时间占住写锁。
#[test]
fn caps_rows_deleted_per_run() {
    let total = 1_000_001;
    let target = trim_target(total, 1_000_000, 0, 5, 0).expect("超限必须 trim");
    assert_eq!(
        total - target,
        TRIM_MAX_ROWS_PER_RUN,
        "单轮删除量必须被预算限住"
    );
    // 否则会一次删掉 10 万行——CI 上正是这样把 s8000 的 p99 拖到 496ms
    assert_ne!(target, 900_000);
}

/// 预算随本批插入量放大（2×），保证高速入队下淘汰能力不被限死。
#[test]
fn budget_scales_with_inserted() {
    let total = 2_000_000;
    let inserted = 50_000;
    let target = trim_target(total, 1_000_000, 0, 5, inserted).expect("超限必须 trim");
    assert_eq!(total - target, inserted * 2, "预算 = 2×本批插入量");
    // 放大后的预算仍受 90% 水位约束，不会把表削到水位以下
    let target2 = trim_target(total, 1_000_000, 0, 5, 10_000_000).expect("超限必须 trim");
    assert_eq!(target2, 900_000, "预算再大也不能削到 90% 水位以下");
}

/// 冷却期内即使仍超限也不 trim；冷却到期才允许再次 trim。
#[test]
fn respects_cooldown_window() {
    let last = 10_000;
    assert_eq!(
        trim_target(2_000, 1_000, last, last + TRIM_COOLDOWN_MS - 1, 0),
        None,
        "距上次 trim 不足冷却期时必须跳过"
    );
    assert_eq!(
        trim_target(2_000, 1_000, last, last + TRIM_COOLDOWN_MS, 0),
        Some(900),
        "冷却到期后必须恢复 trim"
    );
}

/// `last_trim_ms == 0` 表示「从未 trim 过」，必须立即触发（启动后首次触顶）。
#[test]
fn zero_last_trim_means_immediate() {
    assert_eq!(trim_target(2_000, 1_000, 0, 0, 0), Some(900));
    // 时钟回拨（now < last）不应永久禁用 trim：饱和减法得 0 < 冷却期，
    // 行为上是「再等一个冷却周期」，不会 panic / 下溢。
    assert_eq!(trim_target(2_000, 1_000, 10_000, 9_000, 0), None);
}

/// 极小上限的边界：目标水位至少 1 行、且不超过上限本身。
#[test]
fn clamps_tiny_limits() {
    assert_eq!(trim_target(2, 1, 0, 0, 0), Some(1));
    assert_eq!(trim_target(1, 0, 0, 0, 0), Some(0));
}
