//! 点位域路由组：`/api/points*`、`/api/point-tools*`、`/api/point-trash*`
//!
//! **这三个前缀的唯一注册点**。handler 分散在三个子域
//! （`config` 的 CRUD/回收站、`points` 的即时读写、`points_ops` 的虚拟点位与批量工具），
//! 但路由只在本文件出现一次，所以 grep 任一相关路径只会命中这里。
//!
//! 路由清单（`/api/points` 下只有参数化路径）：
//! - PUT    /api/points/{id}                          -> update_point（config）
//! - DELETE /api/points/{id}                          -> delete_point（config，软删除进回收站）
//! - POST   /api/points/{id}/restore                  -> restore_point（config）
//! - GET    /api/points/{id}/read                     -> point_read_now
//!
//! 点位工具（批量写 / 虚拟点位 / 批量偏移 / 克隆 / 查找替换）：
//! - POST   /api/point-tools/write                    -> points_write_unified（唯一写入口）
//! - POST   /api/point-tools/batch                    -> points_batch
//! - GET    /api/point-tools/virtual                  -> list_virtual_points
//! - POST   /api/point-tools/virtual                  -> create_virtual_point
//! - PUT    /api/point-tools/virtual/{id}             -> update_virtual_point
//! - DELETE /api/point-tools/virtual/{id}             -> delete_virtual_point
//! - POST   /api/point-tools/virtual/{id}/recalc       -> recalc_virtual_point
//! - GET    /api/point-tools/offset                    -> offset_list
//! - POST   /api/point-tools/offset/apply              -> offset_apply
//! - GET    /api/point-tools/clone/devices             -> clone_devices
//! - POST   /api/point-tools/clone/apply               -> clone_apply
//! - POST   /api/point-tools/find-replace/search       -> find_replace_search
//! - POST   /api/point-tools/find-replace/apply        -> find_replace_apply
//!
//! 点位回收站：
//! - GET    /api/point-trash                          -> list_point_trash（config）
//! - DELETE /api/point-trash                          -> purge_all_point_trash（config）
//! - DELETE /api/point-trash/{id}                     -> purge_point（config）
//!
//! ## 为什么工具端点 / 回收站不在 `/api/points` 下
//!
//! matchit 的路由树里**静态段优先于参数段，且不会回落**：只要
//! `/api/points/write` 这个静态节点存在，`DELETE /api/points/write` 就会命中该静态
//! 节点、因方法不匹配返回 405，而**不会**回落到 `/api/points/:id` 的 DELETE。
//! 于是 id 恰好叫 `write` / `virtual` / `offset` / `clone` / `find-replace` / `batch`
//! 的点位永远无法更新或删除（回收站同型的 `trash` 事故更严重：会清空整个回收站）。
//!
//! 想不改 URL 只在 handler 里校验 id 是无效的——请求根本到不了 handler。
//! 另一种「同一路径挂不同 method」的写法也不可行：鉴权与分类标记是 `route_layer`
//! 施加的，作用于整条路由（所有 method），无法对 `GET /api/points/virtual`（匿名可读）
//! 与 `DELETE /api/points/virtual`（需 token + 配置变更标记）分别设层。
//!
//! 结论：只能按前缀分离。`/api/points` 下除 `:id` 外不再注册任何静态段，
//! 因此任意 id（含上述保留字）行为一致。
//! 回归用例见 `gateway-api/tests/integration.rs::point_ids_shadowed_by_static_segments_*`
//! 与 `delete_point_with_id_trash_does_not_purge_trash`。
//!
//! 第 1 期冗余收口：原 `POST /api/points/:id/write` 与 `POST /api/point-tools/write-batch`
//! 与统一写入口语义重复（内部复用同一 `write_points_core`），前端已全部切到
//! `POST /api/point-tools/write`，两个旧端点已删除。
//!
//! 鉴权与分类标记按子 Router 分别施加：
//! - 点位 CRUD / 回收站：读写各一层，且打 `ConfigEntry` 分类标记（与收口前一致）；
//! - 即时读写 / 批量：写类一律 `require_auth`，**无**分类标记。其中
//!   `/api/points/:id/read` 会真实发起一次现场采集（占用总线），与纯读缓存不同，
//!   故虽为 GET 也放在写层（F-13），与 `POST /api/diagnostics/task-diag/:point_id` 同级；
//! - 虚拟点位 / 偏移 / 克隆 / 查找替换：读 `require_auth_if_anon_disabled`、
//!   写 `require_auth`，无分类标记。
//!
//! License gate（v2.0）：批量与虚拟点位功能全部免费，不挂中间件。配额校验
//! （max_points 等）不在此层，而由 `web::config_apply::reload_then_save` 统一前置校验。

use std::sync::Arc;

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post, put},
    Router,
};

use super::{point_read_now, points_batch, points_write_unified};
// CRUD handler 来自 `config`，批量/虚拟点位 handler 来自 `points_ops`，
// 均由 `web::api` 扁平重导出（见 `web/api/mod.rs`）。
use crate::web::api::{
    clone_apply, clone_devices, create_virtual_point, delete_point, delete_virtual_point,
    find_replace_apply, find_replace_search, list_point_trash, list_virtual_points, offset_apply,
    offset_list, purge_all_point_trash, purge_point, recalc_virtual_point, restore_point,
    update_point, update_virtual_point,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建点位域路由组
pub fn points_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // === 点位回收站（原 config 路由组，保留 ConfigEntry 分类标记）===
    //
    // 独立前缀而不是 `/api/points/trash`：静态段会遮蔽 `:id`，见文件头说明。
    let trash_read = Router::new()
        .route("/api/point-trash", get(list_point_trash))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    let trash_write = Router::new()
        .route("/api/point-trash", delete(purge_all_point_trash))
        .route("/api/point-trash/:id", delete(purge_point))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    // === 点位 CRUD（原 config 路由组，保留 ConfigEntry 分类标记）===
    let crud_write = Router::new()
        .route("/api/points/:id", put(update_point).delete(delete_point))
        .route("/api/points/:id/restore", post(restore_point))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    // === 即时读取 / 批量读写（原 points 路由组，写层统一 require_auth）===
    let live = Router::new()
        .route("/api/points/:id/read", get(point_read_now))
        .route("/api/point-tools/write", post(points_write_unified))
        .route("/api/point-tools/batch", post(points_batch))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // === 虚拟点位（基础能力：公式派生）===
    let virtual_read = Router::new()
        .route("/api/point-tools/virtual", get(list_virtual_points))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let virtual_write = Router::new()
        .route("/api/point-tools/virtual", post(create_virtual_point))
        // F-13：recalc 是 POST 且有副作用（同步求值并回写派生点位），
        // 原先误放在读层（allow_anon_read=true 时匿名即可触发），现归入写层。
        .route(
            "/api/point-tools/virtual/:id/recalc",
            post(recalc_virtual_point),
        )
        .route(
            "/api/point-tools/virtual/:id",
            put(update_virtual_point).delete(delete_virtual_point),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // === 批量运维（v2.0 全部免费，无中间件）===
    let batch_read = Router::new()
        .route("/api/point-tools/offset", get(offset_list))
        .route("/api/point-tools/clone/devices", get(clone_devices))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let batch_write = Router::new()
        .route(
            "/api/point-tools/find-replace/search",
            post(find_replace_search),
        )
        .route(
            "/api/point-tools/find-replace/apply",
            post(find_replace_apply),
        )
        .route("/api/point-tools/offset/apply", post(offset_apply))
        .route("/api/point-tools/clone/apply", post(clone_apply))
        .route_layer(from_fn_with_state(state, require_auth));

    Router::new()
        .merge(trash_read)
        .merge(trash_write)
        .merge(crud_write)
        .merge(live)
        .merge(virtual_read)
        .merge(virtual_write)
        .merge(batch_read)
        .merge(batch_write)
}
