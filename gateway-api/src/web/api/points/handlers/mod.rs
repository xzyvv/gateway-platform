//! 点位 API 处理函数

use std::collections::BTreeMap;

use gateway_core::config::PointConfig;
use gateway_core::config::PointTrashEntry;

use super::*;
use crate::web::api::config::{now_ms, push_trash};

/// 点位批量操作（统一入口）
///
/// 路由：`POST /api/points/batch`
/// - 裸数组 `[{...}]`：批量 upsert（同 id 覆盖），兼容旧调用；
/// - `{"action":"upsert","points":[...]}`：同上；
/// - `{"action":"edit","add":[...],"update":[...],"delete":["id"]}`：批量增/改/删。
///
/// R3 合并前是两个端点（`/batch` + `/batch/edit`），语义重叠、返回结构不一致；
/// 合并后仅保留本端点，`edit` 的 delete 分支仍进回收站。
#[axum::debug_handler]
pub async fn points_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PointsBatchReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match req {
        PointsBatchReq::BareUpsert(points)
        | PointsBatchReq::Action(PointsBatchAction::Upsert { points }) => {
            upsert_points(&state, points).await
        }
        PointsBatchReq::Action(PointsBatchAction::Edit {
            add,
            update,
            delete,
        }) => edit_points(&state, add, update, delete).await,
    }
}

/// 批量导入点位（同 id 覆盖）
async fn upsert_points(
    state: &Arc<AppState>,
    points: Vec<gateway_core::config::PointConfig>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // D2: 串行化「读→改→写」，防止并发批量导入丢更新
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut config = old.as_ref().clone();
    let before = config.points.len();
    for p in points {
        if let Some(existing) = config.points.iter_mut().find(|x| x.id == p.id) {
            *existing = p;
        } else {
            config.points.push(p);
        }
    }
    let _ = crate::web::config_apply::reload_then_save(state, &old, &config).await?;
    let after = config.points.len();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "before": before,
        "after": after,
        "added": after.saturating_sub(before),
    })))
}

/// F3: 数据点位批量编辑（增/改/删）并热重载
async fn edit_points(
    state: &Arc<AppState>,
    add: Vec<gateway_core::config::PointConfig>,
    update: Vec<gateway_core::config::PointConfig>,
    delete: Vec<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut config = old.as_ref().clone();
    let (mut added, mut updated, mut deleted) = (0usize, 0usize, 0usize);
    for id in &delete {
        if let Some(pos) = config.points.iter().position(|p| p.id == *id) {
            let point = config.points.remove(pos);
            // 与 delete_point 同源：批量删除同样进入回收站（含 TTL/容量清理）
            push_trash(
                &mut config,
                PointTrashEntry {
                    point,
                    deleted_at_ms: now_ms(),
                },
            );
            deleted += 1;
        }
    }
    for p in &update {
        if let Some(existing) = config.points.iter_mut().find(|x| x.id == p.id) {
            *existing = p.clone();
            updated += 1;
        }
    }
    for p in &add {
        if config.points.iter().any(|x| x.id == p.id) {
            continue;
        }
        config.points.push(p.clone());
        added += 1;
    }
    let (outcome, _effective) =
        crate::web::config_apply::reload_then_save(state, &old, &config).await?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "added": added,
        "updated": updated,
        "deleted": deleted,
        "notes": outcome.notes,
        "applied": outcome.scope.applied,
        "restart_required": outcome.scope.restart_required,
    })))
}

/// F5: 手动触发单个点位即时采集
pub async fn point_read_now(
    State(state): State<Arc<AppState>>,
    Path(point_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let config = state.config.load_full();
    let point = config
        .points
        .iter()
        .find(|p| p.id == point_id)
        .cloned()
        // 模板展开点位不落 config.points：回退到展开结果查找，
        // 与采集循环实际读取的点位集合一致（否则模板点位「读取当前值」恒 404）
        .or_else(|| {
            crate::web::config_apply::expand_points(&state, &config)
                .into_iter()
                .find(|p| p.id == point_id)
        })
        .ok_or_else(|| ApiError::not_found(format!("point {} not found", point_id)))?;
    let bus_name = point.bus.clone();
    let collectors = state.south.scheduler.collectors();
    let collector = collectors
        .get(&bus_name)
        .ok_or_else(|| ApiError::not_found(format!("collector for bus {} not found", bus_name)))?;
    match collector.read(&point).await {
        Ok(dp) => Ok(Json(serde_json::json!({
            "status": "ok",
            "point_id": dp.point_id,
            "value": dp.value,
            "raw_value": dp.raw_value,
            "quality": dp.quality,
        }))),
        Err(e) => Ok(Json(serde_json::json!({
            "status": "error",
            "point_id": point_id,
            "error": e.to_string(),
            "quality": e.to_quality(),
        }))),
    }
}

/// B2 统一写入口
///
/// 路由：`POST /api/points/write`（鉴权与其它写路由一致，见 routes.rs）。
///
/// 请求体契约：`{ "writes": [ { "id": "<point_id>", "value": <f64> } ] }`
/// （`id` 亦可写作 `point_id`，两者等价）。`writes` 为空 → 400。
///
/// 语义（单/批分发由后端决定，前端只传数组）：
/// - **1 条** → 单写业务：与 `POST /api/devices/control/send` 共用同一核心
///   （同一 writable 白名单、同一查点/collector 校验、同一 `point.write` 审计）；
///   点位不存在 → 404、白名单外 → 400、写失败 → 业务码 5101（与旧单写端点一致）。
/// - **≥2 条** → 批写业务：按 `bus` 分组调用 `Collector::write_multi`，
///   逐条落审计；单条失败只体现在 `results` 对应位置，不影响其它条。
///
/// 成功响应统一为 `{ status, total, succeeded, failed, results }`，
/// `results` 与入参**等长且逐位对应**（status = ok | partial | error）。
pub async fn points_write_unified(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PointsWriteBatchReq>,
) -> Result<impl IntoResponse, ApiError> {
    let mode = if req.writes.len() == 1 {
        WriteMode::Single
    } else {
        WriteMode::Batch
    };
    let items = write_points_core(&state, &req.writes, mode).await?;

    // 单写：写失败沿用 send_command 的业务码 5101，让前端统一走错误提示路径
    if mode == WriteMode::Single {
        if let Some(err) = &items[0].error {
            return Ok(Json(ApiResponse::<serde_json::Value>::err(
                5101,
                format!("写入失败：{}", err.message),
            ))
            .into_response());
        }
    }
    Ok(Json(write_summary_json(&items)).into_response())
}

/// 单写与批写的**唯一**业务实现（B2）
///
/// - `WriteMode::Single`：要求恰好 1 条；「点位不存在 / 白名单外 / collector 缺失」
///   直接返回 [`ApiError`]（404/400），与 `send_command` 既有错误码完全一致；
/// - `WriteMode::Batch`：逐条独立校验，失败项记为该项 error 并继续处理其余条，
///   同一总线的点位一次 `write_multi` 下发。
///
/// 两种模式共用 [`ensure_writable`] 与 [`audit_point_write`]，
/// 仓库内不再存在第二份白名单或 `point.write` 审计实现。
pub(crate) async fn write_points_core(
    state: &Arc<AppState>,
    writes: &[PointWriteEntry],
    mode: WriteMode,
) -> Result<Vec<WriteItemResult>, ApiError> {
    if writes.is_empty() {
        return Err(ApiError::bad_request("writes must not be empty"));
    }
    if mode == WriteMode::Single && writes.len() != 1 {
        return Err(ApiError::bad_request(
            "single write requires exactly one entry",
        ));
    }

    let config = state.config.load_full();
    let collectors = state.south.scheduler.collectors();

    if mode == WriteMode::Single {
        let w = &writes[0];
        let point = config
            .points
            .iter()
            .find(|p| p.id == w.id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("point {} not found", w.id)))?;
        ensure_writable(&point)?;
        let collector = collectors.get(&point.bus).ok_or_else(|| {
            ApiError::not_found(format!("collector for bus {} not found", point.bus))
        })?;
        let error = match collector.write(&point, w.value).await {
            Ok(()) => {
                audit_point_write(state, &point, w.value, None);
                None
            }
            Err(e) => {
                let message = e.to_string();
                audit_point_write(state, &point, w.value, Some(&message));
                Some(WriteItemError {
                    message,
                    quality: Some(e.to_quality()),
                })
            }
        };
        return Ok(vec![write_item(&point, w.value, error)]);
    }

    let mut results: Vec<Option<WriteItemResult>> = (0..writes.len()).map(|_| None).collect();

    // 按 bus 分组（组内保持入参下标顺序），同总线点位一次 write_multi 下发
    let mut by_bus: BTreeMap<String, Vec<(usize, PointConfig)>> = BTreeMap::new();
    for (i, w) in writes.iter().enumerate() {
        match config.points.iter().find(|p| p.id == w.id) {
            Some(p) if p.writable => by_bus
                .entry(p.bus.clone())
                .or_default()
                .push((i, p.clone())),
            // S11: 写入白名单——未标记 writable 的点位记为该项失败，不放行写入
            Some(p) => {
                let message = whitelist_error(p);
                results[i] = Some(write_item(
                    p,
                    w.value,
                    Some(WriteItemError {
                        message,
                        quality: None,
                    }),
                ));
            }
            None => {
                results[i] = Some(WriteItemResult {
                    point_id: w.id.clone(),
                    point_name: None,
                    device_id: None,
                    value: w.value,
                    error: Some(WriteItemError {
                        message: format!("point {} not found", w.id),
                        quality: None,
                    }),
                });
            }
        }
    }

    for (bus_name, group) in by_bus {
        let collector = match collectors.get(&bus_name) {
            Some(c) => c,
            None => {
                for (i, p) in &group {
                    results[*i] = Some(write_item(
                        p,
                        writes[*i].value,
                        Some(WriteItemError {
                            message: format!("collector for bus {} not found", bus_name),
                            quality: None,
                        }),
                    ));
                }
                continue;
            }
        };

        let pairs: Vec<(&PointConfig, f64)> =
            group.iter().map(|(i, p)| (p, writes[*i].value)).collect();
        let outcomes = collector.write_multi(&pairs).await;

        for ((i, p), outcome) in group.iter().zip(outcomes) {
            let value = writes[*i].value;
            let error = match outcome {
                Ok(()) => {
                    audit_point_write(state, p, value, None);
                    None
                }
                Err(e) => {
                    let message = e.to_string();
                    audit_point_write(state, p, value, Some(&message));
                    Some(WriteItemError {
                        message,
                        quality: Some(e.to_quality()),
                    })
                }
            };
            results[*i] = Some(write_item(p, value, error));
        }
    }

    // 每个入参下标都会在下面三个分支之一填入结果，理论上不存在 None；
    // 万一遗漏也只记日志跳过，不 panic（handler panic 会带崩整个 web 服务）。
    Ok(results
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| {
            if r.is_none() {
                tracing::error!("write_points_core 漏填结果下标 i={}", i);
            }
            r
        })
        .collect())
}

/// S11 写入白名单校验（单写直接拒绝；批写在调用前据此标记该项失败）
fn ensure_writable(point: &PointConfig) -> Result<(), ApiError> {
    if point.writable {
        Ok(())
    } else {
        Err(ApiError::bad_request(whitelist_error(point)))
    }
}

/// 白名单拒绝文案（单写与批写共用，避免两处措辞漂移）
fn whitelist_error(point: &PointConfig) -> String {
    format!(
        "point {} 不在写入白名单中：需在点位配置中将 writable 置为 true 后方可写入",
        point.id
    )
}

/// 单点下发 + 审计的代码内联在 [`write_points_core`] 单写分支：
/// 这里不保留独立函数签名，避免 handler 层直接书写引擎内部类型
/// `gateway_core::south::protocol::Collector`（arch_deps 分层规则禁止 web/api 触碰 south）。
///
/// 落写指标 + `point.write` 审计（成功/失败共用，仓库内唯一实现）
///
/// 失败记录 detail 带 ` [失败: ...]` 后缀，控制历史（list_commands）据此区分状态。
fn audit_point_write(state: &Arc<AppState>, point: &PointConfig, value: f64, error: Option<&str>) {
    state.platform.metrics.record_point_write(error.is_none());
    let name = point.name.clone().unwrap_or_else(|| point.id.clone());
    let detail = match error {
        Some(e) => format!("{}={} [失败: {}]", name, value, e),
        None => format!("{}={}", name, value),
    };
    state.platform.audit.log(
        "point.write",
        &point.device_id.clone().unwrap_or_else(|| point.bus.clone()),
        &detail,
        "api",
    );
}

/// 用点位配置与下发结果组装单条写结果
fn write_item(point: &PointConfig, value: f64, error: Option<WriteItemError>) -> WriteItemResult {
    WriteItemResult {
        point_id: point.id.clone(),
        point_name: point.name.clone(),
        device_id: point.device_id.clone(),
        value,
        error,
    }
}

/// 统一写响应（单写 / 批写同形）
fn write_summary_json(items: &[WriteItemResult]) -> serde_json::Value {
    let mut results = Vec::with_capacity(items.len());
    let mut succeeded = 0usize;
    for item in items {
        match &item.error {
            Some(e) => results.push(serde_json::json!({
                "point_id": item.point_id,
                "status": "error",
                "error": e.message,
                "quality": e.quality,
            })),
            None => {
                succeeded += 1;
                results.push(serde_json::json!({
                    "point_id": item.point_id,
                    "status": "ok",
                    "value": item.value,
                }));
            }
        }
    }
    let failed = items.len() - succeeded;
    let status = if failed == 0 {
        "ok"
    } else if succeeded == 0 {
        "error"
    } else {
        "partial"
    };
    serde_json::json!({
        "status": status,
        "total": items.len(),
        "succeeded": succeeded,
        "failed": failed,
        "results": results,
    })
}
