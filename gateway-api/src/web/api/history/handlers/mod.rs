//! 历史 API 处理函数

use super::*;

pub(crate) async fn history_export_csv(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let point_id = params.get("point_id").cloned().unwrap_or_default();
    let start: i64 = params
        .get("start")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let end: i64 = params
        .get("end")
        .and_then(|s| s.parse().ok())
        .unwrap_or(i64::MAX);
    let limit: usize = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000);
    // P2-1：导出与查询共用同一套降采样语义（桶宽秒数，0/缺省 = 原始行）
    let downsample: Option<u32> = params.get("downsample").and_then(|s| s.parse().ok());

    let result = state
        .south
        .queue
        .history_ex(&point_id, start, end, limit, downsample)
        .await
        .map_err(ApiError::from)?;
    // 列顺序保持既有前 6 列不变（外部脚本按列号解析），聚合列追加在尾部
    let mut csv = String::from(
        "timestamp,point_id,value,raw_value,raw_type,quality,min_value,max_value,sample_count\n",
    );
    for item in &result.items {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{}\n",
            item.timestamp,
            point_id,
            item.value,
            item.raw_value.map(|v| v.to_string()).unwrap_or_default(),
            item.raw_type.as_deref().unwrap_or_default(),
            item.quality,
            item.min_value.map(|v| v.to_string()).unwrap_or_default(),
            item.max_value.map(|v| v.to_string()).unwrap_or_default(),
            item.sample_count
        ));
    }
    // ④ 截断标记：CSV 正文保持纯净（可直接导入 Excel），元信息放响应头。
    // 导出默认 limit=10000，超过时 X-History-Truncated=true，运维可据此提示
    // 「文件只是窗口内最新的一部分」。
    let header = [
        (
            "Content-Type".to_string(),
            "text/csv; charset=utf-8".to_string(),
        ),
        (
            "Content-Disposition".to_string(),
            "attachment; filename=history.csv".to_string(),
        ),
        ("X-History-Total".to_string(), result.total.to_string()),
        (
            "X-History-Returned".to_string(),
            result.returned.to_string(),
        ),
        (
            "X-History-Truncated".to_string(),
            result.truncated.to_string(),
        ),
    ];
    Ok((header, csv))
}

/// 未指定时间窗时的默认回看长度：24 小时。
///
/// 直连调试常用 `curl /api/history?point_id=xxx` 快速验证链路，早期实现把
/// `start`/`end` 声明为必填，缺参直接 400，排查时反而多一步。
const DEFAULT_HISTORY_WINDOW_MS: i64 = 24 * 3600 * 1000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) async fn history(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HistoryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    // limit 上限钳制：防止恶意请求 limit=999999999 导致 OOM
    const MAX_HISTORY_LIMIT: usize = 10000;
    let limit = q.limit.min(MAX_HISTORY_LIMIT);
    // 缺省的窗口参数补成「最近 24 小时」；start/end 传反时按空区间处理
    let end = q.end.unwrap_or_else(now_ms);
    let start = q.start.unwrap_or(end - DEFAULT_HISTORY_WINDOW_MS).min(end);
    let result = state
        .south
        .queue
        .history_ex(&q.point_id, start, end, limit, q.downsample)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}
