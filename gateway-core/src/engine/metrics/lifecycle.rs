//! 系统级横切指标（counter）
//!
//! 覆盖旧 `Metrics` 顶层字段：`task_panics_total` / `watchdog_timeouts_total` /
//! `shutdown_forced_total` / `config_reload_*` / `cert_acl_hardening_failures_total` /
//! `sink_reconnect_*`。
//!
//! 这些字段语义各异但同属「系统生命周期事件」，合并为单子结构即可；
//! 若再按 Task / ConfigReload / Sink 三块拆分会得到多个 1~3 字段的小结构，
//! 认知成本高于收益。

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct LifecycleMetrics {
    /// 后台任务 panic 次数（由 supervisor 统计）。
    pub task_panics: AtomicU64,
    /// watchdog 超时次数。
    pub watchdog_timeouts: AtomicU64,
    /// S04：优雅停机宽限超时被强制截断次数。
    pub shutdown_forced: AtomicU64,

    /// 配置热重载尝试总次数（含成功与失败）。
    pub config_reload_total: AtomicU64,
    /// Bug 185 (N08)：配置热重载成功次数。
    pub config_reload_succeeded: AtomicU64,
    /// Bug 185 (N08)：配置热重载被拒绝总次数。
    pub config_reload_rejected_total: AtomicU64,
    /// Bug P1-B7：拒绝原因细分——schema 校验失败。
    pub config_reload_rejected_schema: AtomicU64,
    /// Bug P1-B7：拒绝原因细分——凭据解密失败。
    pub config_reload_rejected_decrypt: AtomicU64,
    /// Bug P1-B7：拒绝原因细分——业务语义校验失败。
    pub config_reload_rejected_validate: AtomicU64,
    /// Bug P1-B7：拒绝原因细分——include 展开失败。
    pub config_reload_rejected_include: AtomicU64,
    /// Bug P1-B7：拒绝原因细分——其它 IO / 解析错误。
    pub config_reload_rejected_other: AtomicU64,

    /// Bug r11-CERT8：Windows 私钥 ACL 加固（icacls）失败次数。
    pub cert_acl_hardening_failures: AtomicU64,

    /// Bug 187 (N17)：server sink 内部 apply 失败触发重连/重启次数。
    pub sink_reconnect_attempts: AtomicU64,
    /// Bug 187 (N17)：server sink 重连后仍写不进去的次数。
    pub sink_reconnect_exhausted: AtomicU64,

    /// Bug-N2 (fix)：scheduler.start() 启动但 points 为空的次数。
    ///
    /// 不算 panic，但属于配置错误——空 points 通常意味着：
    /// (a) include 展开失败（init 阶段已被 schema 校验兜底）；或
    /// (b) 配置文件没有采集点位（用户没填 / points 段被误删 / schema 演化未迁移）。
    /// 旧实现仅 `info!("scheduler started with 0 points")`，无 metrics 信号，
    /// 运维看到「采集全掉」但无明确错误源——前端"全部设备离线"告警无指向。
    /// 显式递增此指标供 Grafana 告警（rate > 0 立即报"配置可能为空"）。
    pub scheduler_empty_starts: AtomicU64,
}

impl LifecycleMetrics {
    /// 输出到 Prometheus 文本缓冲区。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            buf,
            "# HELP gateway_watchdog_timeouts_total Watchdog timeout count\n\
             # TYPE gateway_watchdog_timeouts_total counter\n\
             gateway_watchdog_timeouts_total {}\n\
             # HELP gateway_task_panics_total Background task panic count\n\
             # TYPE gateway_task_panics_total counter\n\
             gateway_task_panics_total {}\n\
             # HELP gateway_shutdown_forced_total Graceful shutdown forcibly truncated after grace timeout\n\
             # TYPE gateway_shutdown_forced_total counter\n\
             gateway_shutdown_forced_total {}",
            self.watchdog_timeouts.load(Ordering::Relaxed),
            self.task_panics.load(Ordering::Relaxed),
            self.shutdown_forced.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_config_reload_total Config reload count\n\
             # TYPE gateway_config_reload_total counter\n\
             gateway_config_reload_total {}\n\
             # HELP gateway_config_reload_succeeded_total Config reloads succeeded\n\
             # TYPE gateway_config_reload_succeeded_total counter\n\
             gateway_config_reload_succeeded_total {}\n\
             # HELP gateway_config_reload_rejected_total Config reloads rejected\n\
             # TYPE gateway_config_reload_rejected_total counter\n\
             gateway_config_reload_rejected_total {}\n\
             # HELP gateway_config_reload_rejected_schema_total Reload rejected: schema validation failed\n\
             # TYPE gateway_config_reload_rejected_schema_total counter\n\
             gateway_config_reload_rejected_schema_total {}\n\
             # HELP gateway_config_reload_rejected_decrypt_total Reload rejected: credential decrypt failed\n\
             # TYPE gateway_config_reload_rejected_decrypt_total counter\n\
             gateway_config_reload_rejected_decrypt_total {}\n\
             # HELP gateway_config_reload_rejected_validate_total Reload rejected: semantic validation failed\n\
             # TYPE gateway_config_reload_rejected_validate_total counter\n\
             gateway_config_reload_rejected_validate_total {}\n\
             # HELP gateway_config_reload_rejected_include_total Reload rejected: include expansion failed\n\
             # TYPE gateway_config_reload_rejected_include_total counter\n\
             gateway_config_reload_rejected_include_total {}\n\
             # HELP gateway_config_reload_rejected_other_total Reload rejected: other IO/parse errors\n\
             # TYPE gateway_config_reload_rejected_other_total counter\n\
             gateway_config_reload_rejected_other_total {}",
            self.config_reload_total.load(Ordering::Relaxed),
            self.config_reload_succeeded.load(Ordering::Relaxed),
            self.config_reload_rejected_total.load(Ordering::Relaxed),
            self.config_reload_rejected_schema.load(Ordering::Relaxed),
            self.config_reload_rejected_decrypt.load(Ordering::Relaxed),
            self.config_reload_rejected_validate.load(Ordering::Relaxed),
            self.config_reload_rejected_include.load(Ordering::Relaxed),
            self.config_reload_rejected_other.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_cert_acl_hardening_failures_total Cert private-key ACL hardening failures\n\
             # TYPE gateway_cert_acl_hardening_failures_total counter\n\
             gateway_cert_acl_hardening_failures_total {}\n\
             # HELP gateway_sink_reconnect_attempts_total Sink reconnect attempts\n\
             # TYPE gateway_sink_reconnect_attempts_total counter\n\
             gateway_sink_reconnect_attempts_total {}\n\
             # HELP gateway_sink_reconnect_exhausted_total Sink reconnects exhausted\n\
             # TYPE gateway_sink_reconnect_exhausted_total counter\n\
             gateway_sink_reconnect_exhausted_total {}\n\
             # HELP gateway_scheduler_empty_starts_total Scheduler started with zero points\n\
             # TYPE gateway_scheduler_empty_starts_total counter\n\
             gateway_scheduler_empty_starts_total {}",
            self.cert_acl_hardening_failures.load(Ordering::Relaxed),
            self.sink_reconnect_attempts.load(Ordering::Relaxed),
            self.sink_reconnect_exhausted.load(Ordering::Relaxed),
            self.scheduler_empty_starts.load(Ordering::Relaxed),
        );
    }
}
