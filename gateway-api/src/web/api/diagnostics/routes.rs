//! 诊断运维路由组
//!
//! 路由清单（共 11 条，全部 GET/POST）：
//! - GET  /api/diagnostics/bus-load              -> list_bus_loads
//! - GET  /api/diagnostics/bus-load/{bus}        -> get_bus_load
//! - POST /api/diagnostics/bus-load/refresh       -> refresh_bus_loads
//! - GET  /api/diagnostics/modbus-errors         -> list_modbus_errors
//! - GET  /api/diagnostics/modbus-errors/issues   -> list_common_issues
//! - GET  /api/diagnostics/stats                  -> get_stats_report
//! - GET  /api/diagnostics/schedule-timeline      -> get_schedule_timeline
//! - POST /api/diagnostics/task-diag/{point_id}   -> diagnose_point
//! - GET  /api/diagnostics/gateway                -> get_gateway_diag
//! - GET  /api/diagnostics/logs/archives          -> list_log_archives
//! - GET  /api/diagnostics/logs/download          -> download_logs
//! - GET  /api/diagnostics/logs                   -> diagnostics_logs_ws（原 `realtime` 路由组）
//! - POST /api/diagnostics/exec                   -> diagnostics_exec（原 `realtime` 路由组）
//!
//! 抓包不再提供诊断侧入口：`/api/protocol-tools/packet-capture/*` 是唯一实现。
//!
//! 鉴权策略：
//! - GET 类：allow_anon_read=false 时要求 token
//! - POST 类（refresh / task-diag）：始终要求 token
//! - `POST /api/diagnostics/exec`：`require_admin_or_exec_scope`（白名单 shell，最严一级）
//!
//! License gate（v2.0）：诊断/性能监控功能全部免费，不挂中间件。
//! `gateway` 总览（运维基线）本就不进 gate。

use std::sync::Arc;

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{get, post},
    Router,
};

use super::{
    diagnose_point, download_logs, get_bus_load, get_gateway_diag, get_schedule_timeline,
    get_stats_report, list_bus_loads, list_common_issues, list_log_archives, list_modbus_errors,
    refresh_bus_loads,
};
// 日志流 / 命令执行 handler 原属 `realtime` 子域，经 `web::api` 扁平重导出取用。
use crate::web::api::{diagnostics_exec, diagnostics_logs_ws};
use crate::web::types::{
    require_admin_or_exec_scope, require_auth, require_auth_if_anon_disabled, AppState,
};

/// 构建诊断运维路由组
pub fn diagnostics_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // ===== 诊断/性能监控（v2.0 全部免费）=====
    let perf_read = Router::new()
        .route("/api/diagnostics/bus-load", get(list_bus_loads))
        .route("/api/diagnostics/bus-load/:bus", get(get_bus_load))
        .route("/api/diagnostics/modbus-errors", get(list_modbus_errors))
        .route(
            "/api/diagnostics/modbus-errors/issues",
            get(list_common_issues),
        )
        .route("/api/diagnostics/stats", get(get_stats_report))
        .route(
            "/api/diagnostics/schedule-timeline",
            get(get_schedule_timeline),
        )
        .route("/api/diagnostics/logs/download", get(download_logs))
        .route("/api/diagnostics/logs/archives", get(list_log_archives))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let perf_write = Router::new()
        .route("/api/diagnostics/bus-load/refresh", post(refresh_bus_loads))
        .route("/api/diagnostics/task-diag/:point_id", post(diagnose_point))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // ===== gateway 总览（运维基线，core 即可）=====
    let gateway_read = Router::new()
        .route("/api/diagnostics/gateway", get(get_gateway_diag))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    // ===== 日志流 / 命令执行（原 `realtime` 路由组）=====
    let log_stream = Router::new()
        .route("/api/diagnostics/logs", get(diagnostics_logs_ws))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let exec = Router::new()
        .route("/api/diagnostics/exec", post(diagnostics_exec))
        .route_layer(from_fn_with_state(state, require_admin_or_exec_scope));

    Router::new()
        .merge(perf_read)
        .merge(perf_write)
        .merge(gateway_read)
        .route_layer(from_fn(crate::web::middleware::tag_diagnostic))
        // 这两条必须在 `tag_diagnostic` **之后** merge：它们历史上就不带诊断分类标记，
        // 顺手补上会让指标/审计的分类统计在今昔之间断开。
        .merge(log_stream)
        .merge(exec)
}
