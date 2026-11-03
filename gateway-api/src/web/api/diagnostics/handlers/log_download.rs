//! 日志快照下载与归档列表
//!
//! - `GET /api/diagnostics/logs/download?lines=N[&date=YYYY-MM-DD]`：下载日志文本
//! - `GET /api/diagnostics/logs/archives`：按日期列出真实存在的日志归档文件
//!
//! 与 WebSocket `/api/diagnostics/logs`（实时 tail）互补：前端诊断包需要在**故障之后**
//! 取一份"此刻的日志"，而 WebSocket 只能拿到订阅之后的新行。这里直接读日志文件末尾
//! 固定字节数，内存占用可控（见 `Diagnostics::tail_lines`）。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::web::types::AppState;

/// 默认导出行数：够覆盖一次故障窗口，又不会让前端 zip 体积失控
const DEFAULT_LINES: usize = 2000;

/// 单次导出行数上限：再多对排障没有增量信息，只是把整份文件搬一遍
const MAX_LINES: usize = 20_000;

#[derive(Debug, Deserialize)]
pub(crate) struct LogDownloadQuery {
    #[serde(default)]
    pub lines: Option<usize>,
    /// 归档日期（`YYYY-MM-DD`）；缺省表示当期日志
    #[serde(default)]
    pub date: Option<String>,
}

/// 按日期列出日志归档文件（真实目录扫描，新日期在前）
pub(crate) async fn list_log_archives(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let archives = state.platform.diagnostics.log_archives().await;
    Json(serde_json::json!({
        "archives": archives,
        "count": archives.len(),
    }))
}

pub(crate) async fn download_logs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LogDownloadQuery>,
) -> Response {
    let want = query.lines.unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);
    let date = query.date.filter(|d| !d.trim().is_empty());
    let lines = match &date {
        Some(d) => state.platform.diagnostics.tail_lines_of_date(d, want).await,
        None => state.platform.diagnostics.tail_lines(want).await,
    };

    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }

    // 指定日期时用日期做文件名，运维一眼能对上归档列表
    let filename = match &date {
        Some(d) => format!("gateway-{d}.log"),
        None => format!("gateway-{}.log", chrono::Utc::now().format("%Y%m%d-%H%M%S")),
    };
    let mut resp = Response::new(axum::body::Body::from(body));
    let headers = resp.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    // 浏览器直接打开该 URL 时按附件下载；前端 fetch 读 body 时不依赖此头
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    resp.into_response()
}
