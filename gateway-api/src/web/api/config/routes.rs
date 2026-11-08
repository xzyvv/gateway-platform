//! 配置域路由组：`/api/config*` + `/api/project/*` + `/api/buses/*`
//!
//! **这三个前缀的唯一注册点**。设备/点位的 CRUD 路由已迁到
//! `web/api/devices/routes.rs` 与 `web/api/points/routes.rs`（同一前缀必须只有一个
//! 注册点，否则改端点要在多个文件里翻）。handler 仍在 `config` 子域内，路由与实现
//! 分居两处是有意为之：本文件是**按 URL 前缀**组织的入口层。
//!
//! 路由清单：
//! - POST   /api/config/backup           -> backup_config
//! - PUT    /api/config/import           -> import_config
//! - POST   /api/config/reload           -> reload_config
//! - POST   /api/config/validate         -> validate_config_report（只校验不落盘）
//! - GET    /api/config                  -> get_config（原 `system` 路由组）
//! - POST   /api/config                  -> post_config（原 `system` 路由组）
//! - GET    /api/config/versions         -> list_config_versions
//! - GET    /api/config/versions/{id}    -> get_config_version
//! - GET    /api/config/versions/{id}/compare -> compare_config_versions
//! - POST   /api/config/versions/{id}/rollback -> rollback_config_version
//! - POST   /api/config/diff             -> config_diff
//! - GET    /api/project/export          -> export_project
//! - POST   /api/project/import          -> import_project
//! - GET    /api/buses/paused            -> list_paused_buses（E1）
//! - POST   /api/buses/{name}/pause      -> pause_bus_collect（E1，幂等）
//! - POST   /api/buses/{name}/resume     -> resume_bus_collect（E1，幂等）
//!
//! 分类标记刻意**不一致**，这是既有行为不是疏漏：
//! - `/api/config/{backup,import,reload,validate}` 与 `/api/project/*` 打
//!   `ConfigManagement`；
//! - `/api/config`、`/api/config/versions*`、`/api/config/diff` 来自 `system` 路由组，
//!   历史上没有分类标记，迁移时保持无标记，指标/审计口径才可跨版本比对；
//! - `/api/buses/*` 打 `ConfigEntry`（总线暂停/恢复属配置入口的调试动作）。
//!
//! F1: 按 配置入口(CRUD) vs 配置管理(import/reload) 拆路由 + 中间件

use std::sync::Arc;

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{get, post, put},
    Router,
};

use super::{
    backup_config, export_project, import_config, import_project, list_paused_buses,
    pause_bus_collect, reload_config, resume_bus_collect, validate_config_report,
};
use crate::web::api::{
    compare_config_versions, config_diff, get_config, get_config_version, list_config_versions,
    post_config, rollback_config_version,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// F1: Config entry (buses) + config management (import/export/reload) routes
pub fn config_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // === Config entry：总线暂停/恢复（调试需独占总线）===
    let entry_read = Router::new()
        .route("/api/buses/paused", get(list_paused_buses))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    let entry_write = Router::new()
        // E1：暂停/恢复幂等，可反复调用
        .route("/api/buses/:name/pause", post(pause_bus_collect))
        .route("/api/buses/:name/resume", post(resume_bus_collect))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    // === Config management: import/reload（始终要求 token）===
    let mgmt = Router::new()
        .route("/api/config/backup", post(backup_config))
        .route("/api/config/import", put(import_config))
        .route("/api/config/reload", post(reload_config))
        // P4：只校验不落盘（前端校验规则唯一来源）
        .route("/api/config/validate", post(validate_config_report))
        // 工程文件（三层子集 + 信封）：与整份 config 导入导出并存，语义不同
        .route("/api/project/export", get(export_project))
        .route("/api/project/import", post(import_project))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
        .route_layer(from_fn(crate::web::middleware::tag_config_management));

    // === 整份配置读取/写入/版本（原 `system` 路由组，无分类标记）===
    let sys_read = Router::new()
        .route("/api/config", get(get_config))
        .route("/api/config/versions", get(list_config_versions))
        .route("/api/config/versions/:id", get(get_config_version))
        .route(
            "/api/config/versions/:id/compare",
            get(compare_config_versions),
        )
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let sys_write = Router::new()
        .route("/api/config", post(post_config))
        .route(
            "/api/config/versions/:id/rollback",
            post(rollback_config_version),
        )
        .route("/api/config/diff", post(config_diff))
        .route_layer(from_fn_with_state(state, require_auth));

    Router::new()
        .merge(entry_read)
        .merge(entry_write)
        .merge(mgmt)
        .merge(sys_read)
        .merge(sys_write)
}
