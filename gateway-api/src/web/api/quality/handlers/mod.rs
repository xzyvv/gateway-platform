//! 数据质量规则 handler
//!
//! 规则对象字段与前端 `gateway-web/src/api/dataQuality.ts` 的 `QualityRule`
//! 一致（camelCase）。写操作记审计日志（`AuditLog`），随后重装管道快照。
//!
//! 执行口径提示：`stale` 规则由调度器的连续失败/超时机制负责，管道只执行
//! `range` / `rate` / `nan`，因此这里仍允许配置 stale，但不承诺由规则引擎降级。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use gateway_core::config::QualityRule;

use crate::web::error::ApiError;
use crate::web::services::install_quality_rules;
use crate::web::types::{ApiResponse, AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityRuleReq {
    pub name: String,
    pub condition: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub max_rate: Option<f64>,
    #[serde(default)]
    pub stale_seconds: Option<u32>,
    pub result_quality: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// PUT 的局部更新载荷：仅提供的字段被合并，未提供字段保持原值。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityRulePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub max_rate: Option<f64>,
    #[serde(default)]
    pub stale_seconds: Option<u32>,
    #[serde(default)]
    pub result_quality: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl QualityRuleReq {
    fn into_rule(self) -> QualityRule {
        QualityRule {
            id: String::new(),
            name: self.name.trim().to_string(),
            condition: self.condition.trim().to_string(),
            min: self.min,
            max: self.max,
            max_rate: self.max_rate,
            stale_seconds: self.stale_seconds,
            result_quality: self.result_quality.trim().to_string(),
            enabled: self.enabled,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// 校验规则：名称、条件枚举、数值参数与结果质量码。
fn validate_rule(rule: &QualityRule) -> Result<(), ApiError> {
    if rule.name.trim().is_empty() || rule.name.chars().count() > 64 {
        return Err(ApiError::bad_request("规则名称不能为空且不超过 64 字符"));
    }
    match rule.condition.as_str() {
        "range" => {
            let (Some(min), Some(max)) = (rule.min, rule.max) else {
                return Err(ApiError::bad_request("取值范围规则必须填写 min 与 max"));
            };
            if !min.is_finite() || !max.is_finite() {
                return Err(ApiError::bad_request("min / max 必须是有限数值"));
            }
            if min > max {
                return Err(ApiError::bad_request("min 不能大于 max"));
            }
        }
        "rate" => {
            let Some(max_rate) = rule.max_rate else {
                return Err(ApiError::bad_request("变化率规则必须填写 maxRate"));
            };
            if !max_rate.is_finite() || max_rate < 0.0 {
                return Err(ApiError::bad_request("maxRate 必须是 >= 0 的有限数值"));
            }
        }
        "nan" => {}
        "stale" => {
            if let Some(secs) = rule.stale_seconds {
                if secs == 0 {
                    return Err(ApiError::bad_request("staleSeconds 必须 >= 1"));
                }
            }
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "未知检查条件: {}（可选 range/rate/stale/nan）",
                other
            )))
        }
    }
    match rule.result_quality.as_str() {
        "good" | "bad" | "uncertain" | "old" => Ok(()),
        other => Err(ApiError::bad_request(format!(
            "未知结果质量码: {}（可选 good/bad/uncertain/old）",
            other
        ))),
    }
}

/// 把局部更新合并到现有规则，并按条件收敛数值字段（避免脏参数残留）。
fn merge_patch(mut rule: QualityRule, patch: &QualityRulePatch) -> QualityRule {
    if let Some(name) = &patch.name {
        rule.name = name.trim().to_string();
    }
    if let Some(condition) = &patch.condition {
        rule.condition = condition.trim().to_string();
    }
    if let Some(min) = patch.min {
        rule.min = Some(min);
    }
    if let Some(max) = patch.max {
        rule.max = Some(max);
    }
    if let Some(max_rate) = patch.max_rate {
        rule.max_rate = Some(max_rate);
    }
    if let Some(secs) = patch.stale_seconds {
        rule.stale_seconds = Some(secs);
    }
    if let Some(result) = &patch.result_quality {
        rule.result_quality = result.trim().to_string();
    }
    if let Some(enabled) = patch.enabled {
        rule.enabled = enabled;
    }
    match rule.condition.as_str() {
        "range" => {
            rule.max_rate = None;
            rule.stale_seconds = None;
        }
        "rate" => {
            rule.min = None;
            rule.max = None;
            rule.stale_seconds = None;
        }
        "nan" => {
            rule.min = None;
            rule.max = None;
            rule.max_rate = None;
            rule.stale_seconds = None;
        }
        "stale" => {
            rule.min = None;
            rule.max = None;
            rule.max_rate = None;
        }
        _ => {}
    }
    rule
}

/// GET /api/quality/rules — 列出全部规则（按创建时间升序）
pub(crate) async fn list_quality_rules(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<QualityRule>>>, ApiError> {
    Ok(Json(ApiResponse::ok(
        state.platform.quality_rule_store.list_rules()?,
    )))
}

/// POST /api/quality/rules — 新建规则
pub(crate) async fn create_quality_rule(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QualityRuleReq>,
) -> Result<Json<ApiResponse<QualityRule>>, ApiError> {
    let rule = req.into_rule();
    validate_rule(&rule)?;
    let created = state.platform.quality_rule_store.create_rule(rule)?;
    state.platform.audit.log(
        "quality_rule.create",
        &format!("quality_rule:{}", created.id),
        &format!(
            "{} condition={} result={}",
            created.name, created.condition, created.result_quality
        ),
        "web",
    );
    install_quality_rules(&state.platform.quality_rule_store);
    Ok(Json(ApiResponse::ok(created)))
}

/// PUT /api/quality/rules/:id — 局部更新规则（未提供字段保持原值）
pub(crate) async fn update_quality_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<QualityRulePatch>,
) -> Result<Json<ApiResponse<QualityRule>>, ApiError> {
    let existing = state
        .platform
        .quality_rule_store
        .get_rule(&id)?
        .ok_or_else(|| ApiError::not_found(format!("规则 {} 不存在", id)))?;
    let merged = merge_patch(existing, &patch);
    validate_rule(&merged)?;
    let updated = state
        .platform
        .quality_rule_store
        .update_rule(&id, &merged)?
        .ok_or_else(|| ApiError::not_found(format!("规则 {} 不存在", id)))?;
    state.platform.audit.log(
        "quality_rule.update",
        &format!("quality_rule:{}", id),
        &format!(
            "{} condition={} enabled={}",
            updated.name, updated.condition, updated.enabled
        ),
        "web",
    );
    install_quality_rules(&state.platform.quality_rule_store);
    Ok(Json(ApiResponse::ok(updated)))
}

/// DELETE /api/quality/rules/:id — 删除规则
pub(crate) async fn delete_quality_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    if !state.platform.quality_rule_store.delete_rule(&id)? {
        return Err(ApiError::not_found(format!("规则 {} 不存在", id)));
    }
    state.platform.audit.log(
        "quality_rule.delete",
        &format!("quality_rule:{}", id),
        "",
        "web",
    );
    install_quality_rules(&state.platform.quality_rule_store);
    Ok(Json(ApiResponse::ok(true)))
}
