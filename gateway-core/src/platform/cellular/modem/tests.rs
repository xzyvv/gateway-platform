//! PPP 建链等待（细粒度超时）路径测试。
//!
//! 通过注入闭包覆盖三条判定路径，无需真实 pppd / 串口 / ppp 接口。
use super::*;

#[tokio::test]
async fn bringup_returns_up_when_ip_appears() {
    let outcome = wait_for_ppp_bringup(
        Duration::from_secs(1),
        Duration::from_millis(1),
        || Some("10.0.0.2".to_string()),
        || true,
    )
    .await;
    assert_eq!(outcome, PppBringup::Up("10.0.0.2".to_string()));
}

#[tokio::test]
async fn bringup_fails_early_when_process_exits() {
    // 超时设 5s，但进程一开始就退出：应立即返回 ProcessExited，而非等满 5s。
    let start = tokio::time::Instant::now();
    let outcome = wait_for_ppp_bringup(
        Duration::from_secs(5),
        Duration::from_millis(1),
        || None,
        || false,
    )
    .await;
    assert_eq!(outcome, PppBringup::ProcessExited);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn bringup_times_out_when_no_ip_but_alive() {
    let start = tokio::time::Instant::now();
    let outcome = wait_for_ppp_bringup(
        Duration::from_millis(60),
        Duration::from_millis(5),
        || None,
        || true,
    )
    .await;
    assert_eq!(outcome, PppBringup::Timeout);
    assert!(start.elapsed() >= Duration::from_millis(50));
}

// ============================================================
// R35: AT 参数净化 + 退避抖动
// ============================================================

#[test]
fn sanitize_at_arg_removes_quote_and_control_chars() {
    // 引号会被剔除，CR/LF 等控制字符不属白名单也会被过滤
    assert_eq!(sanitize_at_arg("12\"345"), "12345");
    assert_eq!(sanitize_at_arg("ab\r\ncd"), "abcd");
    // 注入尝试被中和：白名单外字符（`"` `;`）全部剔除
    assert_eq!(sanitize_at_arg("cmnet\";ATD123"), "cmnetATD123");
}

#[test]
fn sanitize_at_arg_keeps_normal_values_intact() {
    assert_eq!(sanitize_at_arg("cmnet"), "cmnet");
    assert_eq!(sanitize_at_arg("internet.telekom"), "internet.telekom");
    assert_eq!(sanitize_at_arg("1234"), "1234");
}

#[test]
fn jittered_backoff_secs_stays_within_range_and_nonzero() {
    // 退避抖动区间为 [75%, 100%]；base=1 时 .max(1) 保证不会退化为 0（r35-MDM3）
    for base in [1u64, 10, 60] {
        for _ in 0..50 {
            let j = crate::engine::supervision::jittered_backoff_secs(base).max(1);
            let lo = (base * 3 / 4).max(1);
            assert!(j >= lo && j <= base, "base={} jitter={}", base, j);
        }
    }
}

// ============================================================
// r15: failover 默认路由切换（静默断网修复）
// ============================================================

/// 默认路由目标选择：物理状态 → 路由目标（纯函数，任意平台可测）。
#[test]
fn default_route_target_follows_link_priority() {
    // 以太网在 → 走以太网
    assert_eq!(
        desired_default_route(LinkState::EthernetUp),
        Some(RouteTarget::Ethernet)
    );
    // 双链路都在 → 仍以以太网为主（工业现场默认不把 4G 作为主用）
    assert_eq!(
        desired_default_route(LinkState::BothUp),
        Some(RouteTarget::Ethernet)
    );
    // 只有 4G → 走 4G
    assert_eq!(
        desired_default_route(LinkState::CellularUp),
        Some(RouteTarget::Cellular)
    );
    // 全断 → 不下任何路由（避免把默认路由指向不存在的接口）
    assert_eq!(desired_default_route(LinkState::Down), None);
}
