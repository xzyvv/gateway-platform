//! NTP 失败回退行为验证（决策清单 P1-6 的 NTP 部分）。
//!
//! 现场含义：设备无外网时 NTP 必然失败。要求是**静默降级到 RTC、不阻断启动**，
//! 同时状态里保留可读的失败原因（`error: ...` + `source=RTC`），
//! 否则运维只能看到「时间不对」而不知道是 NTP 没同步。

use super::*;

/// NTP 服务器不可达时必须返回 Ok（回退 RTC），且状态可读、基准不被污染。
#[tokio::test]
async fn ntp_sync_failure_falls_back_to_rtc_without_error() {
    let mgr = NtpManager::new(NtpConfig {
        // 本机回环 + 端口 1：立即 connection refused，不依赖外网也不等待超时
        server: "127.0.0.1:1".to_string(),
        ..Default::default()
    });

    let initial = mgr.status();
    assert_eq!(initial.source, "RTC");
    assert_eq!(initial.status, "idle");

    let result = mgr.sync_now().await;
    assert!(
        result.is_ok(),
        "NTP 失败必须回退 RTC，不能返回 Err 阻断启动"
    );

    let after = mgr.status();
    assert_eq!(after.source, "RTC", "失败后时间源应保持 RTC");
    assert!(after.last_sync.is_none(), "失败不应伪造同步时间戳");
    assert!(
        after.status.starts_with("error:"),
        "状态应保留可读失败原因，实际: {}",
        after.status
    );
}
