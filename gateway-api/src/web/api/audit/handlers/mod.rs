//! 审计日志 API 处理函数

use super::*;

/// GET /api/audit/logs — 查询审计日志（支持过滤 + 条数上限）
pub async fn list_audit_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
) -> Result<impl IntoResponse, ApiError> {
    // Bug 100: limit 服务端硬上限 1000。
    const MAX_LIMIT: usize = 1000;
    let effective_limit = q.limit.map(|n| n.min(MAX_LIMIT));
    // Bug 101: 时间范围校验——ts_from > ts_to 返回 400。
    if let (Some(from), Some(to)) = (q.ts_from, q.ts_to) {
        if from > to {
            return Err(ApiError::bad_request(format!(
                "ts_from ({}) > ts_to ({})",
                from, to
            )));
        }
    }
    let filter = AuditFilter {
        action: q.action,
        target: q.target,
        source: q.source,
        ts_from: q.ts_from,
        ts_to: q.ts_to,
        limit: effective_limit,
    };
    // 审计查询包含「等待落盘屏障 + 读取文件尾部」两步同步 IO：直接在 async
    // handler 里执行会占住 tokio worker，一次查询即可拖慢同 worker 上的
    // 采集上行 / MQTT 发布。放 to spawn_blocking，把阻塞隔离到专用线程池。
    let audit = state.platform.audit.clone();
    let logs: Vec<AuditEntry> = tokio::task::spawn_blocking(move || audit.list(filter))
        .await
        .map_err(|e| ApiError::internal(format!("audit list join failed: {}", e)))??;
    Ok(Json(serde_json::json!({"logs": logs})))
}
