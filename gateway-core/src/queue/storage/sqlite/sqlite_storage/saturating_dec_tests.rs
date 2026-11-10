use super::SqliteStorage;
use std::sync::atomic::{AtomicI64, Ordering};

/// P2-c 回归：计数器饱和递减，绝不越减为负。
#[test]
fn dec_never_goes_negative() {
    let c = AtomicI64::new(0);
    // 计数器为 0 时扣减不应变负
    SqliteStorage::saturating_dec(&c, 10);
    assert_eq!(c.load(Ordering::Relaxed), 0);

    c.store(5, Ordering::Relaxed);
    SqliteStorage::saturating_dec(&c, 3);
    assert_eq!(c.load(Ordering::Relaxed), 2);
    SqliteStorage::saturating_dec(&c, 100);
    assert_eq!(c.load(Ordering::Relaxed), 0);

    // n <= 0 时为 no-op
    c.store(7, Ordering::Relaxed);
    SqliteStorage::saturating_dec(&c, 0);
    assert_eq!(c.load(Ordering::Relaxed), 7);
}
