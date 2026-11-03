//! 协议转换规则 CRUD handler（C1：配置持久化版）
//!
//! 规则持久化进 `config.json` 的 `convert_rules` 节，写操作遵循统一范式：
//! mutate load_full → `ConvertRuleConfig::validate_rule`（fail-closed）→
//! [`reload_then_save`]（结构校验 + 模板展开校验 + license 配额 + store
//! + 热重载，引擎热 swap）。
//!
//! 运行时执行由 `gateway_core::pipeline::convert::ConvertEngine` 承担
//! （广播/入队之前）；list 时把引擎命中记账合并进 DTO 的
//! `last_value` / `last_converted_at_ms`。

use crate::web::types::{ApiError, ApiResponse, AppState};
use axum::extract::{Path, State};
use axum::Json;
use gateway_core::config::{ConvertEnumItemConfig, ConvertRuleConfig};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

/// 枚举映射项（Web 契约，与 ConvertEnumItemConfig 字段对齐）
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConvertEnumItem {
    pub key: String,
    pub label: String,
}

/// 转换规则（Web 契约；last_* 字段来自运行时记账，不落盘）
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConvertRule {
    pub id: String,
    pub name: String,
    pub source_proto: String,
    pub source_point_id: String,
    pub source_point_name: String,
    pub target_proto: String,
    pub target_tag: String,
    pub transform: String,
    pub scale: Option<f64>,
    pub offset: Option<f64>,
    pub enum_map: Option<Vec<ConvertEnumItem>>,
    pub enabled: bool,
    #[ts(type = "number")]
    pub last_converted_at_ms: Option<i64>,
    pub last_value: Option<String>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

/// 规则条数上限（防已认证客户端无限追加导致配置无界增长；正常使用为手动逐条创建）。
const MAX_CONVERT_RULES: usize = 200;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Web DTO → 持久化配置（丢弃 last_* 记账字段）
fn to_config(r: &ConvertRule) -> ConvertRuleConfig {
    ConvertRuleConfig {
        id: r.id.clone(),
        name: r.name.clone(),
        source_proto: r.source_proto.clone(),
        source_point_id: r.source_point_id.clone(),
        source_point_name: r.source_point_name.clone(),
        target_proto: r.target_proto.clone(),
        target_tag: r.target_tag.clone(),
        transform: r.transform.clone(),
        scale: r.scale,
        offset: r.offset,
        enum_map: r.enum_map.as_ref().map(|m| {
            m.iter()
                .map(|i| ConvertEnumItemConfig {
                    key: i.key.clone(),
                    label: i.label.clone(),
                })
                .collect()
        }),
        enabled: r.enabled,
        created_at_ms: r.created_at_ms,
    }
}

/// 持久化配置 → Web DTO（合并运行时命中记账）
fn to_dto(
    c: &ConvertRuleConfig,
    hits: &std::collections::HashMap<String, crate::web::traits::ConvertHitView>,
) -> ConvertRule {
    let hit = hits.get(&c.id);
    ConvertRule {
        id: c.id.clone(),
        name: c.name.clone(),
        source_proto: c.source_proto.clone(),
        source_point_id: c.source_point_id.clone(),
        source_point_name: c.source_point_name.clone(),
        target_proto: c.target_proto.clone(),
        target_tag: c.target_tag.clone(),
        transform: c.transform.clone(),
        scale: c.scale,
        offset: c.offset,
        enum_map: c.enum_map.as_ref().map(|m| {
            m.iter()
                .map(|i| ConvertEnumItem {
                    key: i.key.clone(),
                    label: i.label.clone(),
                })
                .collect()
        }),
        enabled: c.enabled,
        last_converted_at_ms: hit.map(|h| h.last_converted_at_ms),
        last_value: hit.map(|h| h.last_value.clone()),
        created_at_ms: c.created_at_ms,
    }
}

/// 单条规则 fail-closed 校验（直接复用 core 侧规则体校验，双源零漂移）
fn validate_dto(r: &ConvertRule) -> Result<(), ApiError> {
    let errs = to_config(r).validate_rule();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "规则校验失败：{}",
            errs.join("；")
        )))
    }
}

/// 写操作统一收尾：校验 → 配额 → 落盘 → store → 展开点位 → 热重载
/// （调用方须已持有 config_reload_lock）。
///
/// V-8：改用 [`reload_then_save`] 统一入口，与其它配置写端点共享
/// 「结构校验 + 模板展开校验 + license 配额」链路，不再直连 `config_loader.save`。
async fn persist_and_reload(
    state: &Arc<AppState>,
    old_cfg: &gateway_core::config::GatewayConfig,
    new_cfg: gateway_core::config::GatewayConfig,
) -> Result<(), ApiError> {
    let _ = crate::web::config_apply::reload_then_save(state, old_cfg, &new_cfg).await?;
    Ok(())
}

pub(crate) async fn list_convert_rules(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ConvertRule>>>, ApiError> {
    let cfg = state.config.load();
    let hits = state.north.gateway.convert_hits();
    let rules: Vec<ConvertRule> = cfg
        .convert_rules
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|c| to_dto(c, &hits))
        .collect();
    Ok(Json(ApiResponse::ok(rules)))
}

pub(crate) async fn create_convert_rule(
    State(state): State<Arc<AppState>>,
    Json(rule): Json<ConvertRule>,
) -> Result<Json<ApiResponse<ConvertRule>>, ApiError> {
    let mut r = rule;
    if r.id.is_empty() {
        r.id = format!("cv_{}", now_ms());
    }
    if r.created_at_ms == 0 {
        r.created_at_ms = now_ms();
    }
    validate_dto(&r)?;

    let _guard = state.config.config_reload_lock.lock().await;
    let old_cfg = state.config.load();
    let mut cfg = (*old_cfg).clone();
    let rules = cfg.convert_rules.get_or_insert_with(Vec::new);
    // 客户端可自带 id：拒绝重复，避免 update 只改首条 / delete 删全部的语义不一致
    if rules.iter().any(|x| x.id == r.id) {
        return Err(ApiError::bad_request(format!("规则 id 已存在: {}", r.id)));
    }
    if rules.len() >= MAX_CONVERT_RULES {
        return Err(ApiError::bad_request(format!(
            "规则条数已达上限 {}，请先删除不再使用的规则",
            MAX_CONVERT_RULES
        )));
    }
    rules.push(to_config(&r));
    persist_and_reload(&state, &old_cfg, cfg).await?;
    Ok(Json(ApiResponse::ok(r)))
}

pub(crate) async fn update_convert_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<ConvertRule>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old_cfg = state.config.load();
    let mut cfg = (*old_cfg).clone();
    let rules = cfg.convert_rules.get_or_insert_with(Vec::new);
    let Some(existing) = rules.iter().find(|r| r.id == id) else {
        return Err(ApiError::not_found(format!("rule not found: {}", id)));
    };
    let mut new = patch;
    new.id = id.clone();
    new.created_at_ms = existing.created_at_ms;
    validate_dto(&new)?;
    if let Some(slot) = rules.iter_mut().find(|r| r.id == id) {
        *slot = to_config(&new);
    }
    persist_and_reload(&state, &old_cfg, cfg).await?;
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn delete_convert_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old_cfg = state.config.load();
    let mut cfg = (*old_cfg).clone();
    let rules = cfg.convert_rules.get_or_insert_with(Vec::new);
    let before = rules.len();
    rules.retain(|r| r.id != id);
    if rules.len() == before {
        return Err(ApiError::not_found(format!("rule not found: {}", id)));
    }
    persist_and_reload(&state, &old_cfg, cfg).await?;
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn toggle_convert_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old_cfg = state.config.load();
    let mut cfg = (*old_cfg).clone();
    let rules = cfg.convert_rules.get_or_insert_with(Vec::new);
    let Some(r) = rules.iter_mut().find(|r| r.id == id) else {
        return Err(ApiError::not_found(format!("rule not found: {}", id)));
    };
    r.enabled = !r.enabled;
    let res = r.enabled;
    persist_and_reload(&state, &old_cfg, cfg).await?;
    Ok(Json(ApiResponse::ok(res)))
}
