//! 点位 API 处理函数

use gateway_core::config::PointTrashEntry;
use gateway_core::engine::write::{PointWriteResult, RejectReason, WriteContext, WriteRequest};

use super::*;
use crate::web::api::config::{now_ms, push_trash};

/// 点位批量操作（统一入口）
///
/// 路由：`POST /api/point-tools/batch`
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

/// 点位导出（`GET /api/point-tools/export`）
///
/// 返回**配置点位**（`cfg.points`）快照，供前端另存为 CSV / JSON。
///
/// 只导 `cfg.points`、不导模板设备展开出来的点位，是刻意的：
/// 导入端点 `POST /api/point-tools/batch` 是 upsert 进 `cfg.points`，
/// 若把展开点位一并导出，用户改完再导回就会把它们固化成显式点位，
/// 运行期同一批点位出现两份（模板展开一份 + 显式一份）。
/// 模板点位随设备模板走，整份工程用 `/api/project/export` 搬运。
pub async fn export_points(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<PointsExportResp>> {
    let cfg = state.config.load_full();
    Json(ApiResponse::ok(PointsExportResp {
        total: cfg.points.len(),
        points: cfg.points.clone(),
    }))
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
    let _ = crate::web::config_apply::persist_then_apply(state, &old, &config).await?;
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
        crate::web::config_apply::persist_then_apply(state, &old, &config).await?;
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
/// 路由：`POST /api/point-tools/write`（鉴权与其它写路由一致，见 routes.rs）。
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

    // ROI-09：幂等键命中直接返回首次结果，不再重复下发。上游「超时后整批重试」
    // 不会把已经成功的点写第二遍（对累加/脉冲类点位是实质重复动作）。
    let idempotency_key = req
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if let Some(key) = idempotency_key {
        if let Some(cached) = state.platform.write_idempotency.get(key) {
            return Ok(Json(cached).into_response());
        }
    }

    let items = write_points_core(&state, &req.writes, mode, req.operator.clone()).await?;

    // 单写：写失败沿用 send_command 的业务码，让前端统一走错误提示路径。
    // G-02：按写结果三态区分——5101 明确失败（可安全重试），
    // 5102 结果未知（设备可能已执行，禁止自动重试）。
    if mode == WriteMode::Single {
        if let Some(err) = &items[0].error {
            // R1：不要用 `Json(..)` 包裹——`Json` 会强制 HTTP 200，
            // 使 5101/5102 与 send_command 的 500 语义分叉。
            // 直接用 `ApiResponse` 的 IntoResponse，由 error_codes::http_status 统一映射。
            return Ok(ApiResponse::<serde_json::Value>::err(
                write_failure_code(err.outcome),
                write_failure_message(err),
            )
            .into_response());
        }
    }
    let body = write_summary_json(&items);
    if let Some(key) = idempotency_key {
        state.platform.write_idempotency.put(key, body.clone());
    }
    Ok(Json(body).into_response())
}

/// 单写与批写的唯一业务实现（B2 · ROI-35）
///
/// 执行全部委托给 `GatewayRuntime::execute_point_writes`（core 的
/// `runtime::write_service`）：白名单 / 范围校验 / 三态 / 读回核实 / 审计 /
/// 指标只有一份实现，OPC UA 写回与 Web 写入口走同一条路。
///
/// 本函数只做 Web 层适配：
/// - `WriteMode::Single`：被拒（点位不存在 / 不可写 / 越界 / 无 collector）
///   映射成与 `send_command` 一致的 `ApiError`（404 / 400 / 4011）；
/// - `WriteMode::Batch`：逐条独立，结果与入参**等长且逐位对应**。
pub(crate) async fn write_points_core(
    state: &Arc<AppState>,
    writes: &[PointWriteEntry],
    mode: WriteMode,
    operator: Option<String>,
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
    let requests: Vec<WriteRequest> = writes
        .iter()
        .map(|w| WriteRequest {
            id: w.id.clone(),
            value: w.value,
        })
        .collect();
    let ctx = WriteContext::new("api")
        .with_operator(operator)
        // U-4：记录写入时的配置代次，跨 reload 写可事后追溯（决议 A：允许跨代写）。
        .with_config_generation(Some(state.config.generation()));
    let env = crate::web::traits::WriteEnv {
        points: &config.points,
        collectors: &collectors,
        metrics: state.platform.metrics.as_ref(),
        audit: state.platform.audit.as_ref(),
    };
    let results = state
        .north
        .gateway
        .execute_point_writes(env, requests, mode, ctx)
        .await;

    if mode == WriteMode::Single {
        if let Some(reason) = results.first().and_then(|r| r.reject) {
            return Err(reject_to_api_error(
                reason,
                results[0].message.clone().unwrap_or_default(),
            ));
        }
    }

    Ok(results.into_iter().map(to_write_item).collect())
}

/// 把统一写服务的「未下发」拒绝原因映射成既有 HTTP 语义。
fn reject_to_api_error(reason: RejectReason, message: String) -> ApiError {
    match reason {
        RejectReason::NotFound | RejectReason::NoCollector => ApiError::not_found(message),
        RejectReason::NotWritable => ApiError::bad_request(message),
        RejectReason::OutOfRange => {
            ApiError::with_code(crate::web::error_codes::WRITE_OUT_OF_RANGE, message)
        }
    }
}

/// 统一写结果 → Web 契约的单条结果。
fn to_write_item(result: PointWriteResult) -> WriteItemResult {
    let error = result.message.map(|message| WriteItemError {
        message,
        quality: result.quality,
        outcome: result.outcome,
    });
    WriteItemResult {
        point_id: result.point_id,
        point_name: result.point_name,
        device_id: result.device_id,
        value: result.value,
        error,
    }
}

/// 统一写响应（单写 / 批写同形）
///
/// 顶层 `status` 是粗粒度结果（ok / partial / error），逐条 `outcome` 是
/// 写结果三态（ok / failed / uncertain）：
/// - `outcome == "failed"`：能证明写没有生效，**可安全重试**；
/// - `outcome == "uncertain"`：写帧已下发但没有可确认的应答，设备可能已执行，
///   **禁止自动重试**，必须先读回核实。
///
/// 顶层 `failed` 沿用既有语义 = 未确认成功条数（含 uncertain），
/// `uncertain` 是其中「结果未知」的子集计数，供上游一眼判断该不该重试。
///
/// ROI-09：显式声明批量语义，让上游不必猜——
/// - `atomic = false`：批量写**不是**全成功才算数，成功的条目已经生效、不会回滚，
///   逐条结果才是真相；
/// - `retryable`：**整批原样重发是否会造成重复动作**。仅当「没有任何条目成功、
///   也没有任何条目结果未知」时为 `true`（即全部明确失败，可安全整批重试）。
///   只要有一条成功或结果未知，整批重发就会重复写，必须按 `results` 逐条处理。
fn write_summary_json(items: &[WriteItemResult]) -> serde_json::Value {
    let mut results = Vec::with_capacity(items.len());
    let mut succeeded = 0usize;
    let mut uncertain = 0usize;
    for item in items {
        match &item.error {
            Some(e) => {
                if e.outcome.is_uncertain() {
                    uncertain += 1;
                }
                results.push(serde_json::json!({
                    "point_id": item.point_id,
                    // 粗粒度状态（既有契约，兼容旧客户端）
                    "status": "error",
                    // 写结果三态（G-02：新增，供上游判断该不该重试）
                    "outcome": e.outcome.as_str(),
                    "error": e.message,
                    "quality": e.quality,
                }));
            }
            None => {
                succeeded += 1;
                results.push(serde_json::json!({
                    "point_id": item.point_id,
                    "status": "ok",
                    "outcome": WriteOutcome::Ok.as_str(),
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
    // 批量写逐条独立生效，不存在整批回滚
    let atomic = false;
    // 整批重发安全的唯一情形：全部明确失败（没有已生效、也没有结果未知）
    let retryable = succeeded == 0 && uncertain == 0;
    serde_json::json!({
        "status": status,
        "total": items.len(),
        "succeeded": succeeded,
        "failed": failed,
        "uncertain": uncertain,
        "atomic": atomic,
        "retryable": retryable,
        "results": results,
    })
}
