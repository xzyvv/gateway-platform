//! 访问控制域 handler：限流规则 / 访问日志 / API 密钥 / 计划任务
//!
//! 数据全部落 SQLite（`default_version_db`），写操作记审计日志。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use gateway_core::config::{ApiKeyRecord, RateRule, ScheduledTask, TaskType};

use crate::web::error::ApiError;
use crate::web::types::{ApiResponse, AppState};

// ============================================================================
// 限流规则
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateRuleReq {
    pub name: String,
    pub path: String,
    pub method: String,
    pub limit_per_minute: u32,
    pub limit_per_hour: u32,
    #[serde(default)]
    pub burst: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 规则路径是否会覆盖 `/api/access/*` 管理入口（防自锁）。
///
/// 匹配语义与中间件 `rule_matches` 一致：无 `*` 为精确路径，`*` 结尾为前缀匹配。
/// 若允许规则覆盖限流/密钥/任务管理接口，误配后管理员将无法进入面板解除限制。
fn covers_access_admin(path: &str) -> bool {
    const ADMIN_PREFIX: &str = "/api/access/";
    match path.strip_suffix('*') {
        Some(prefix) => ADMIN_PREFIX.starts_with(prefix),
        None => path == "/api/access" || path.starts_with(ADMIN_PREFIX),
    }
}

/// 校验限流规则入参，归一化 method 为大写
fn validate_rule(req: &RateRuleReq) -> Result<String, ApiError> {
    if req.name.trim().is_empty() || req.name.len() > 64 {
        return Err(ApiError::bad_request("规则名称不能为空且不超过 64 字符"));
    }
    if !req.path.starts_with('/') || req.path.len() > 256 {
        return Err(ApiError::bad_request("路径必须以 / 开头且不超过 256 字符"));
    }
    if covers_access_admin(&req.path) {
        return Err(ApiError::bad_request(
            "限流规则不能覆盖 /api/access/*（限流/密钥/任务管理入口），否则误配后无法进入面板解除",
        ));
    }
    let method = req.method.to_uppercase();
    match method.as_str() {
        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "*" => {}
        _ => {
            return Err(ApiError::bad_request(
                "方法必须是 GET/POST/PUT/DELETE/PATCH/*",
            ))
        }
    }
    if req.limit_per_minute == 0 || req.limit_per_hour == 0 {
        return Err(ApiError::bad_request("每分钟/每小时限额必须 >= 1"));
    }
    if req.limit_per_minute > 1_000_000 || req.limit_per_hour > 100_000_000 {
        return Err(ApiError::bad_request("限额超出合理范围"));
    }
    Ok(method)
}

impl RateRuleReq {
    fn into_rule(self, method: String) -> RateRule {
        RateRule {
            id: String::new(),
            name: self.name.trim().to_string(),
            path: self.path.trim().to_string(),
            method,
            limit_per_minute: self.limit_per_minute,
            limit_per_hour: self.limit_per_hour,
            burst: self.burst,
            enabled: self.enabled,
            created_at: 0,
            updated_at: 0,
        }
    }
}

pub async fn list_rate_rules(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<RateRule>>>, ApiError> {
    Ok(Json(ApiResponse::ok(
        state.platform.rate_rules.list_rules()?,
    )))
}

pub async fn create_rate_rule(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RateRuleReq>,
) -> Result<Json<ApiResponse<RateRule>>, ApiError> {
    let method = validate_rule(&req)?;
    let rule = state
        .platform
        .rate_rules
        .create_rule(req.into_rule(method))?;
    state.platform.audit.log(
        "rate_rule.create",
        &format!("rate_rule:{}", rule.id),
        &format!(
            "{} {} {} per-min={}",
            rule.name, rule.method, rule.path, rule.limit_per_minute
        ),
        "web",
    );
    Ok(Json(ApiResponse::ok(rule)))
}

pub async fn update_rate_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<RateRuleReq>,
) -> Result<Json<ApiResponse<RateRule>>, ApiError> {
    let method = validate_rule(&req)?;
    let updated = state
        .platform
        .rate_rules
        .update_rule(&id, &req.into_rule(method))?
        .ok_or_else(|| ApiError::not_found(format!("规则 {} 不存在", id)))?;
    state.platform.audit.log(
        "rate_rule.update",
        &format!("rate_rule:{}", id),
        &format!("{} {}", updated.method, updated.path),
        "web",
    );
    Ok(Json(ApiResponse::ok(updated)))
}

pub async fn delete_rate_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    if !state.platform.rate_rules.delete_rule(&id)? {
        return Err(ApiError::not_found(format!("规则 {} 不存在", id)));
    }
    state
        .platform
        .audit
        .log("rate_rule.delete", &format!("rate_rule:{}", id), "", "web");
    Ok(Json(ApiResponse::ok(true)))
}

pub async fn rate_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<gateway_core::config::RateLimitStats>>, ApiError> {
    Ok(Json(ApiResponse::ok(state.platform.rate_rules.stats()?)))
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default = "default_log_limit")]
    pub limit: usize,
}

fn default_log_limit() -> usize {
    100
}

pub async fn rate_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<ApiResponse<Vec<gateway_core::config::ApiAccessLog>>>, ApiError> {
    let limit = q.limit.clamp(1, 500);
    Ok(Json(ApiResponse::ok(
        state.platform.rate_rules.recent_access_logs(limit)?,
    )))
}

// ============================================================================
// API 密钥
// ============================================================================

/// 允许的权限范围白名单（与前端 `apiKeys.ts` / `docs/api-access.md` 保持一致）
///
/// 刻意不提供 `system:write`：系统域写端点（`/api/users`、`/api/access`、
/// `/api/system` 等）只接受主 api_token。若密钥可持系统域写权限，持有者可通过
/// 建用户或建密钥把权限提升到管理员，按域授权本身即失效。
const ALL_SCOPES: &[&str] = &[
    "devices:read",
    "devices:write",
    "points:read",
    "points:write",
    "config:read",
    "config:write",
    "history:read",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiKeyReq {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// 有效期（天）；0/None = 永不过期
    #[serde(default)]
    pub expires_days: Option<u32>,
}

/// 创建成功响应：完整 Key 仅此一次返回
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiKey {
    #[serde(flatten)]
    pub record: ApiKeyRecord,
    pub key: String,
}

pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ApiKeyRecord>>>, ApiError> {
    Ok(Json(ApiResponse::ok(state.platform.api_keys.list()?)))
}

pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateApiKeyReq>,
) -> Result<Json<ApiResponse<CreatedApiKey>>, ApiError> {
    if req.name.trim().is_empty() || req.name.len() > 64 {
        return Err(ApiError::bad_request("名称不能为空且不超过 64 字符"));
    }
    if let Some(bad) = req
        .scopes
        .iter()
        .find(|s| !ALL_SCOPES.contains(&s.as_str()))
    {
        return Err(ApiError::bad_request(format!("未知权限范围: {}", bad)));
    }
    if let Some(d) = req.expires_days {
        if d > 3650 {
            return Err(ApiError::bad_request("有效期不能超过 3650 天"));
        }
    }
    let (record, key) = state.platform.api_keys.create(
        req.name.trim(),
        &req.scopes,
        req.expires_days.filter(|d| *d > 0),
    )?;
    state.platform.audit.log(
        "api_key.create",
        &format!("api_key:{}", record.id),
        &format!("{} scopes={:?}", record.name, record.scopes),
        "web",
    );
    Ok(Json(ApiResponse::ok(CreatedApiKey { record, key })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateApiKeyReq {
    pub enabled: bool,
}

pub async fn update_api_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateApiKeyReq>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    if !state.platform.api_keys.set_enabled(&id, req.enabled)? {
        return Err(ApiError::not_found(format!("密钥 {} 不存在", id)));
    }
    state.platform.audit.log(
        if req.enabled {
            "api_key.enable"
        } else {
            "api_key.disable"
        },
        &format!("api_key:{}", id),
        "",
        "web",
    );
    Ok(Json(ApiResponse::ok(true)))
}

pub async fn delete_api_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    if !state.platform.api_keys.delete(&id)? {
        return Err(ApiError::not_found(format!("密钥 {} 不存在", id)));
    }
    state
        .platform
        .audit
        .log("api_key.delete", &format!("api_key:{}", id), "", "web");
    Ok(Json(ApiResponse::ok(true)))
}

// ============================================================================
// 计划任务
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedTaskReq {
    pub name: String,
    #[serde(rename = "type")]
    pub task_type: String,
    #[serde(default)]
    pub target: Option<String>,
    pub cron: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// 校验任务入参并计算下次执行时间（enabled 时）
fn validate_task(req: &SchedTaskReq) -> Result<(TaskType, Option<i64>), ApiError> {
    if req.name.trim().is_empty() || req.name.len() > 64 {
        return Err(ApiError::bad_request("任务名称不能为空且不超过 64 字符"));
    }
    let task_type = TaskType::parse(req.task_type.trim())
        .ok_or_else(|| ApiError::bad_request(format!("未知任务类型: {}", req.task_type)))?;
    match task_type {
        TaskType::ReadPoint | TaskType::ReadDevice
            if req
                .target
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty() =>
        {
            return Err(ApiError::bad_request("读取类任务必须填写目标 ID"));
        }
        _ => {}
    }
    let schedule = gateway_core::support::cron::parse(req.cron.trim())
        .ok_or_else(|| ApiError::bad_request(format!("cron 表达式无效: {}", req.cron)))?;
    let now_secs = chrono::Utc::now().timestamp();
    let next = schedule.next_after(now_secs);
    Ok((task_type, next))
}

pub async fn list_sched_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ScheduledTask>>>, ApiError> {
    Ok(Json(ApiResponse::ok(state.platform.sched_tasks.list()?)))
}

pub async fn create_sched_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SchedTaskReq>,
) -> Result<Json<ApiResponse<ScheduledTask>>, ApiError> {
    let (task_type, next) = validate_task(&req)?;
    let enabled = req.enabled.unwrap_or(true);
    let task = state.platform.sched_tasks.create(ScheduledTask {
        id: String::new(),
        name: req.name.trim().to_string(),
        task_type,
        target: req.target.map(|t| t.trim().to_string()),
        cron: req.cron.trim().to_string(),
        enabled,
        last_run_at: None,
        // 停用的任务不排程
        next_run_at: if enabled { next } else { None },
        run_count: 0,
        last_status: None,
        last_error: None,
        created_at: 0,
    })?;
    state.platform.audit.log(
        "sched_task.create",
        &format!("sched_task:{}", task.id),
        &format!("{} {} {}", task.name, task.task_type.as_str(), task.cron),
        "web",
    );
    Ok(Json(ApiResponse::ok(task)))
}

pub async fn update_sched_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SchedTaskReq>,
) -> Result<Json<ApiResponse<ScheduledTask>>, ApiError> {
    let existing = state
        .platform
        .sched_tasks
        .get(&id)?
        .ok_or_else(|| ApiError::not_found(format!("任务 {} 不存在", id)))?;
    let (task_type, next) = validate_task(&req)?;
    let enabled = req.enabled.unwrap_or(existing.enabled);
    let updated = state
        .platform
        .sched_tasks
        .update(
            &id,
            req.name.trim(),
            task_type,
            req.target.as_deref().map(str::trim),
            req.cron.trim(),
            enabled,
            if enabled { next } else { None },
        )?
        .ok_or_else(|| ApiError::not_found(format!("任务 {} 不存在", id)))?;
    state.platform.audit.log(
        "sched_task.update",
        &format!("sched_task:{}", id),
        &format!("enabled={}", updated.enabled),
        "web",
    );
    Ok(Json(ApiResponse::ok(updated)))
}

pub async fn delete_sched_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    if !state.platform.sched_tasks.delete(&id)? {
        return Err(ApiError::not_found(format!("任务 {} 不存在", id)));
    }
    state.platform.audit.log(
        "sched_task.delete",
        &format!("sched_task:{}", id),
        "",
        "web",
    );
    Ok(Json(ApiResponse::ok(true)))
}

/// 立即执行一次任务（同步等待结果），执行结果同样回写任务记录。
pub async fn run_sched_task_now(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let task = state
        .platform
        .sched_tasks
        .get(&id)?
        .ok_or_else(|| ApiError::not_found(format!("任务 {} 不存在", id)))?;
    let result = super::executor::execute_and_record(&state, &task).await;
    state.platform.audit.log(
        "sched_task.run_now",
        &format!("sched_task:{}", id),
        &result.message,
        "web",
    );
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "status": if result.ok { "success" } else { "error" },
        "error": if result.ok { None } else { Some(result.message) },
    }))))
}

#[cfg(test)]
mod tests {
    use super::covers_access_admin;

    #[test]
    fn refuses_rules_covering_access_admin() {
        // 精确命中、前缀通配、更大范围的通配都应被拒（否则可自锁面板）
        assert!(covers_access_admin("/api/access"));
        assert!(covers_access_admin("/api/access/rate-rules"));
        assert!(covers_access_admin("/api/access*"));
        assert!(covers_access_admin("/api/*"));
        assert!(covers_access_admin("/api*"));
        assert!(covers_access_admin("/*"));
    }

    #[test]
    fn allows_rules_outside_access_admin() {
        assert!(!covers_access_admin("/api/points"));
        assert!(!covers_access_admin("/api/points*"));
        assert!(!covers_access_admin("/api/devices/*"));
        assert!(!covers_access_admin("/api/poin*"));
        assert!(!covers_access_admin("/api/access-log"));
        assert!(!covers_access_admin("/api/access-log*"));
        assert!(!covers_access_admin("/health"));
    }
}
