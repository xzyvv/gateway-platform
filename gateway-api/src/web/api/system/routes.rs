//! 系统级路由组（版本 / 状态 / 配置管理 / OTA / NTP / 指标 / 证书 / 看门狗 / webhook）
//!
//! 鉴权策略：
//! - read_routes：`allow_anon_read=false` 时要求 token（require_auth_if_anon_disabled）
//! - write_routes：始终要求 token（require_auth）
//!
//! Bug 102：axum 0.7 中 route_layer 在被 merge 的子 Router 上保持生效，但顺序敏感——
//! 必须保持"先 route_layer 再 merge"的写法，否则鉴权可能失效。
//! 因此本函数先分别对 read/write 子 Router 应用 route_layer，再 merge 进外层 Router。
//!
//! 说明：各业务子域的路由已下沉到各自 `<子域>/routes.rs`（audit / auth / history /
//! points / realtime / license / devices），由 `build_router` 直接 merge，本函数只保留
//! 系统域自身的路由。健康检查（live/ready）与登录端点保留在 `build_router` 中。
//! `/api/config*` 归 `api/config/routes.rs`（`config` 子域是该前缀的唯一注册点），
//! 本文件不再出现 `/api/config` 路径。
//!
//! 第 1 期冗余收口（删除的重复端点）：
//! - `GET /api/cert/status`：与 `GET /api/certificates` 同源（同一 CertManager +
//!   x509 解析），其字段（days_remaining / cert_path）已并入证书列表项；
//! - `GET /api/north/servers`：其内容（server 存活列表）已并入 `GET /api/north/status`。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{delete, get, post},
    Router,
};

use crate::web::api::{
    cellular_status, disk_health_history, get_health_report, get_metrics, get_status, get_version,
    list_certificates, list_dead_letters, list_heal_events, list_services, north_status,
    ntp_status, ntp_sync, ota_rollback, ota_status, ota_upload, passthrough_sessions,
    purge_dead_letters, renew_certificate, replay_dead_letter, restart_service, test_webhook,
    upload_certificate,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建系统级路由组（read_routes + write_routes）
///
/// 鉴权分层：read 子 Router 先 `route_layer(require_auth_if_anon_disabled)` 再 merge；
/// write 子 Router 先 `route_layer(require_auth)` 再 merge。
pub fn system_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // 只读路由（GET）：allow_anon_read=false 时要求 token
    //
    // `/api/config*` 全部迁到 `api/config/routes.rs`（同一前缀只能有一个注册点）。
    let read_routes = Router::new()
        .route("/api/version", get(get_version))
        .route("/api/status", get(get_status))
        .route("/api/health/report", get(get_health_report))
        .route("/api/ota/status", get(ota_status))
        .route("/api/ntp/status", get(ntp_status))
        .route("/api/metrics", get(get_metrics))
        .route("/api/cellular/status", get(cellular_status))
        .route("/api/passthrough/sessions", get(passthrough_sessions))
        .route("/api/north/status", get(north_status))
        .route("/api/north/dead-letters", get(list_dead_letters))
        .route("/api/health/disk/history", get(disk_health_history))
        .route("/api/watchdog/services", get(list_services))
        .route("/api/watchdog/events", get(list_heal_events))
        .route("/api/certificates", get(list_certificates))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    // 写入路由（POST/PUT/DELETE）：始终要求鉴权
    let write_routes = Router::new()
        .route("/api/ota/upload", post(ota_upload))
        .route("/api/ota/rollback", post(ota_rollback))
        .route("/api/ntp/sync", post(ntp_sync))
        .route(
            "/api/watchdog/services/:name/restart",
            post(restart_service),
        )
        .route("/api/certificates/upload", post(upload_certificate))
        .route("/api/certificates/renew", post(renew_certificate))
        .route("/api/webhook/test", post(test_webhook))
        // 死信运维：重投（重新对外发包）与清理（销毁唯一副本）都必须强鉴权
        .route(
            "/api/north/dead-letters/:id/replay",
            post(replay_dead_letter),
        )
        .route("/api/north/dead-letters", delete(purge_dead_letters))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    Router::new().merge(read_routes).merge(write_routes)
}
