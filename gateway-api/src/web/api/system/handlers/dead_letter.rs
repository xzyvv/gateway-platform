//! 死信队列运维端点（不可投递批次的查询 / 重投 / 清理）
//!
//! 存在意义：Webhook 收到 4xx 或重试用尽时，这批数据**永远不会再被自动发送**。
//! 原实现直接物理删除，现场表现为「接口返回 400 之后数据就消失了，且没有任何
//! 入口能看到丢了什么」。本组端点把这条链路补齐：
//!
//! - `GET    /api/north/dead-letters`              看丢了什么（payload 只回预览）
//! - `POST   /api/north/dead-letters/:id/replay`    修好接收端后重投
//! - `DELETE /api/north/dead-letters`               确认不要了才清理
//!
//! 关键语义：**移入死信 ≠ 已交付**。死信只保证「没有静默消失」。
//!
//! 不可用（outbox 库未打开）时一律返回 503，而不是空列表——死信端点返回
//! 「0 条」会被运维读成「从未丢过数据」，这与事实相反。

use super::*;
use crate::web::traits::DeadLetterDto;

use axum::extract::Query;

/// 列表查询参数
#[derive(Debug, serde::Deserialize)]
pub(crate) struct DeadLetterListQuery {
    /// 协议名（`webhook` / `mqtt`），缺省 `webhook`
    pub protocol: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// 清理查询参数
#[derive(Debug, serde::Deserialize)]
pub(crate) struct DeadLetterPurgeQuery {
    /// 协议名；缺省清理全部协议
    pub protocol: Option<String>,
    /// 绝对时间戳（epoch 毫秒）：只清理 `failed_at` 早于该时刻的记录。
    ///
    /// 传绝对时间而非「N 天前」是有意的：由调用方决定「现在」，
    /// 避免网关时钟与运维设备时钟不一致时清理范围与预期不符。
    pub older_than_ms: Option<i64>,
}

/// 默认协议：必须与 outbox 表里实际写入的值一致。
///
/// 注意这里存的是 `http` 而不是 `webhook`——`WebhookClient` 落库时写的是传输协议名
/// `http`。二者不一致会导致不带 `protocol` 参数的查询永远返回 0 条，而这是本模块
/// 最不能出现的故障模式（让人以为"没有未送达数据"）。
const DEFAULT_PROTOCOL: &str = "http";
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

#[derive(Debug, serde::Serialize)]
pub(crate) struct DeadLetterListResponse {
    pub protocol: String,
    /// 该协议死信总数（不受 limit/offset 影响，供前端分页）
    pub total: i64,
    pub items: Vec<DeadLetterDto>,
    /// 因超过行数上限被**自动裁剪**（永久删除）的累计条数。
    ///
    /// 不返回这个数字，界面就只能看到"一张正常的表"——更早的数据已经因超限
    /// 被删掉了，运维却以为历史上从未丢过数据。
    pub trimmed_total: u64,
    /// 最近一次自动裁剪的时间（毫秒 epoch，0 = 从未裁剪）
    pub last_trim_ms: u64,
    /// 最近一次自动裁剪删除的条数
    pub last_trim_rows: u64,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DeadLetterReplayResponse {
    /// 死信记录 id
    pub id: i64,
    /// 重投后生成的新 outbox 行 id；`0` 表示同一 data_id 已有在途记录
    pub outbox_id: i64,
    /// 是否真正重新入队（`outbox_id == 0` 时为 false）
    pub requeued: bool,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DeadLetterPurgeResponse {
    /// 实际删除的死信条数
    pub deleted: usize,
}

/// 把可选字符串协议归一化（空串视为缺省）
fn norm_protocol(p: Option<String>) -> String {
    p.filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PROTOCOL.to_string())
}

fn norm_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// GET /api/north/dead-letters
pub(crate) async fn list_dead_letters(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DeadLetterListQuery>,
) -> Result<Json<ApiResponse<DeadLetterListResponse>>, ApiError> {
    let dl = &state.north.dead_letter;
    if !dl.available() {
        return Err(ApiError::service_unavailable(
            "死信库不可用（outbox 数据库未能打开），无法确认是否存在丢失数据；\
             请检查 GATEWAY_OUTBOX_DB 路径与磁盘状态",
        ));
    }
    let protocol = norm_protocol(q.protocol);
    let limit = norm_limit(q.limit);
    let offset = q.offset.unwrap_or(0);

    let total = dl
        .count(Some(&protocol))
        .await
        .map_err(|e| ApiError::internal(format!("统计死信失败: {e}")))?;
    let items = dl
        .list(&protocol, limit, offset)
        .await
        .map_err(|e| ApiError::internal(format!("查询死信失败: {e}")))?;

    let (trimmed_total, last_trim_ms, last_trim_rows) =
        state.platform.metrics.dead_letter_trim_stats();

    Ok(Json(ApiResponse::ok(DeadLetterListResponse {
        protocol,
        total,
        items,
        trimmed_total,
        last_trim_ms,
        last_trim_rows,
    })))
}

/// POST /api/north/dead-letters/:id/replay
///
/// 重投是把数据重新交给北向通道再发一次，属于**对外发起请求**的敏感动作，
/// 必须落审计（谁、什么时候、把哪条数据重新发出去）。
pub(crate) async fn replay_dead_letter(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<DeadLetterReplayResponse>>, ApiError> {
    let dl = &state.north.dead_letter;
    if !dl.available() {
        return Err(ApiError::service_unavailable(
            "死信库不可用（outbox 数据库未能打开），无法重投",
        ));
    }

    let outbox_id = dl
        .replay(id)
        .await
        .map_err(|e| ApiError::internal(format!("重投死信失败: {e}")))?;

    if outbox_id == 0 {
        // 两种可能都不能算成功：死信不存在，或同一 data_id 已有在途行。
        // 后者若谎报成功，运维会以为数据在路上了，而实际上死信还躺在表里。
        //
        // #5：409 必须计数——旧实现只回文本，「有人反复点重投但一直没生效」
        // 在 Grafana 上完全不可见，只能靠用户投诉发现。
        state.platform.metrics.inc_dead_letter_replay_conflict();
        state.platform.audit.log(
            "dead_letter.replay",
            &format!("dead_letter:{id}"),
            "result=conflict (dead letter gone or data_id still pending)",
            "api",
        );
        return Err(ApiError::conflict(
            "重投未生效：该死信不存在，或相同 data_id 已有在途（pending）记录。\
             请稍后重试，或先确认该批次是否正在发送",
        ));
    }

    state.platform.metrics.inc_dead_letter_replay();
    state.platform.audit.log(
        "dead_letter.replay",
        &format!("dead_letter:{id}"),
        &format!("outbox_id={outbox_id}"),
        "api",
    );

    Ok(Json(ApiResponse::ok(DeadLetterReplayResponse {
        id,
        outbox_id,
        requeued: true,
    })))
}

/// DELETE /api/north/dead-letters
///
/// 清理是**不可逆**的数据销毁：死信表是这批数据唯一的副本。
pub(crate) async fn purge_dead_letters(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DeadLetterPurgeQuery>,
) -> Result<Json<ApiResponse<DeadLetterPurgeResponse>>, ApiError> {
    let dl = &state.north.dead_letter;
    if !dl.available() {
        return Err(ApiError::service_unavailable(
            "死信库不可用（outbox 数据库未能打开），无法清理",
        ));
    }

    let protocol = q
        .protocol
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    let deleted = dl
        .purge(protocol.as_deref(), q.older_than_ms)
        .await
        .map_err(|e| ApiError::internal(format!("清理死信失败: {e}")))?;

    // #5：主动清理量必须计数——它是「确认丢弃」的数据量，
    // 与 dead_letter_rows 存量配合才能看出「清得比进得快」还是相反。
    state
        .platform
        .metrics
        .add_dead_letter_purged(deleted as u64);

    state.platform.audit.log(
        "dead_letter.purge",
        protocol.as_deref().unwrap_or("*"),
        &format!(
            "deleted={}, older_than_ms={}",
            deleted,
            q.older_than_ms
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        "api",
    );

    Ok(Json(ApiResponse::ok(DeadLetterPurgeResponse { deleted })))
}

#[cfg(test)]
mod tests {
    use super::{norm_limit, norm_protocol, DEFAULT_LIMIT, MAX_LIMIT};

    #[test]
    fn protocol_defaults_to_http() {
        // 默认值必须等于 WebhookClient 落库时写入的协议名，否则查询恒为 0 条
        assert_eq!(norm_protocol(None), "http");
        assert_eq!(norm_protocol(Some(String::new())), "http");
        assert_eq!(norm_protocol(Some("  ".into())), "http");
        assert_eq!(norm_protocol(Some("mqtt".into())), "mqtt");
    }

    /// 分页上限必须被夹紧：死信 payload 可能几十 KB，放任 limit 会打爆
    /// 4G 链路与浏览器内存。
    #[test]
    fn limit_is_clamped() {
        assert_eq!(norm_limit(None), DEFAULT_LIMIT);
        assert_eq!(norm_limit(Some(0)), 1);
        assert_eq!(norm_limit(Some(10_000)), MAX_LIMIT);
        assert_eq!(norm_limit(Some(20)), 20);
    }
}
