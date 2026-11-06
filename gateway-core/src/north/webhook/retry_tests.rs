//! webhook 重试分类与指数退避的回归测试。
//!
//! 这两条是北向"下游长期不可用"场景的全部防线：
//! - `is_retriable_status` 判错 → 4xx 被无限重发（打爆下游）或 5xx 被当成永久失败（静默丢数据）
//! - `retry_backoff` 算错 → 退避过短形成重试风暴，或退避过长导致数据迟迟不投递

use super::retry::{is_retriable_status, retry_backoff, MAX_WEBHOOK_ATTEMPTS};

fn status(code: u16) -> reqwest::StatusCode {
    reqwest::StatusCode::from_u16(code).unwrap()
}

/// 可重试 = 5xx（服务端故障）+ 408/425/429（明确的"稍后重试"语义）。
///
/// 旧实现把「非 2xx」一律视为可重试——端点持续返回 401/404 时会无限重发。
#[test]
fn retriable_status_codes_are_classified_correctly() {
    for code in [500u16, 501, 502, 503, 504, 408, 425, 429] {
        assert!(is_retriable_status(status(code)), "{code} 应判为可重试");
    }
    for code in [
        200u16, 201, 202, 204, 301, 302, 307, 308, 400, 401, 403, 404, 405, 409, 413, 422,
    ] {
        assert!(
            !is_retriable_status(status(code)),
            "{code} 不应判为可重试（否则会对永久性错误无限重发）"
        );
    }
}

/// 退避序列：`base * 2^(attempts-1)`，单调不减、封顶 5 分钟。
#[test]
fn retry_backoff_is_exponential_and_capped() {
    let base = 1_000u64;
    let mut prev = 0u64;
    for attempts in 1..=MAX_WEBHOOK_ATTEMPTS {
        let ms = retry_backoff(base, attempts).as_millis() as u64;
        assert!(ms >= base, "attempts={attempts} 退避不得小于 base");
        assert!(
            ms <= 300_000,
            "attempts={attempts} 退避不得超过 5 分钟（实际 {ms}ms）"
        );
        if attempts > 1 {
            assert!(
                ms >= prev,
                "退避必须单调不减（attempts={attempts}: {prev} → {ms}）"
            );
        }
        prev = ms;
    }
}

/// 前 8 次严格翻倍（base=1s → 1/2/4/8/16/32/64/128 秒）。
#[test]
fn retry_backoff_doubles_each_attempt() {
    let base = 1_000u64;
    assert_eq!(retry_backoff(base, 1).as_millis(), 1_000);
    assert_eq!(retry_backoff(base, 2).as_millis(), 2_000);
    assert_eq!(retry_backoff(base, 3).as_millis(), 4_000);
    assert_eq!(retry_backoff(base, 4).as_millis(), 8_000);
    assert_eq!(retry_backoff(base, 8).as_millis(), 128_000);
}

/// 超过 5 分钟后封顶；`attempts` 极大时也不得溢出。
#[test]
fn retry_backoff_caps_at_five_minutes() {
    assert_eq!(retry_backoff(1_000, 10).as_millis(), 300_000);
    assert_eq!(retry_backoff(1_000, 30).as_millis(), 300_000);
    assert_eq!(retry_backoff(1_000, u32::MAX).as_millis(), 300_000);
    // base 本身超过上限时以 base 为准（退避不得短于主循环的 flush 间隔）
    assert_eq!(retry_backoff(600_000, 1).as_millis(), 600_000);
}

/// `attempts = 0` 与 `base = 0` 的边界不得 panic。
#[test]
fn retry_backoff_handles_degenerate_inputs() {
    assert_eq!(retry_backoff(1_000, 0).as_millis(), 1_000);
    assert_eq!(retry_backoff(0, 0).as_millis(), 0);
    assert_eq!(retry_backoff(0, 5).as_millis(), 0);
}

/// 重试上限固定为 8 次（超出即放弃并丢弃该条）。
///
/// 这是"下游长时间不可用"时数据被丢弃的**唯一**闸门——上限被改大会拉长
/// 积压期，被改小会误杀只是偶发抖动的端点，因此锁死。
#[test]
fn max_webhook_attempts_is_eight() {
    assert_eq!(MAX_WEBHOOK_ATTEMPTS, 8);
}
