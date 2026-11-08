//! 死信队列运维端点集成测试
//!
//! 覆盖新增的 3 个端点（`docs/api-surface.md`）：
//! - GET    /api/north/dead-letters
//! - POST   /api/north/dead-letters/:id/replay
//! - DELETE /api/north/dead-letters
//!
//! 重点关注两条容易写错的语义：
//! 1. **不可用时必须 503，不能返回空列表**——空列表会被运维读成「从未丢过数据」；
//! 2. 重投/清理必须真的作用在 outbox 库上（不是只回 200）。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use gateway_core::north::mqtt::outbox::{OutboxStore, OutgoingMsg};

const TOKEN: &str = "test-token-0123456789abcdef";

async fn call(app: &axum::Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// mock 态的 `NoopDeadLetter`：三个端点都必须 503，而不是「0 条死信」。
#[tokio::test]
async fn unavailable_store_returns_503_not_empty_list() {
    let app = gateway_api::web::build_router(common::mock_state::mock_app_state());

    let (status, body) = call(&app, "GET", "/api/north/dead-letters").await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "死信库不可用时必须 503（否则空列表会被误读为「没有丢失数据」）：{body}"
    );
    assert_eq!(body["code"], 5003);

    let (status, _) = call(&app, "POST", "/api/north/dead-letters/1/replay").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _) = call(&app, "DELETE", "/api/north/dead-letters").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// 真实 outbox 库：查询 → 重投 → 清理全链路。
#[tokio::test]
async fn real_store_list_replay_purge() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db = dir.path().join("outbox.db");

    // 造两条死信（模拟两条 4xx 永久失败批次）
    {
        let mut store = OutboxStore::open(&db).expect("open outbox");
        for (data_id, payload) in [(101, b"first".to_vec()), (102, b"second".to_vec())] {
            let id = store
                .persist(&OutgoingMsg {
                    data_id,
                    data_ids: vec![data_id],
                    topic: "http://receiver/api".into(),
                    payload,
                    qos: 1,
                    protocol: "http".into(),
                })
                .expect("persist");
            store.move_to_dead_letter(id, "http_404").expect("move");
        }
    }

    // 注入真实实现（mock 态默认是 Noop）
    let mut state = common::mock_state::mock_app_state();
    // #5：重投/清理此前没有任何指标，409 也看不见；这里直接断言计数器
    let metrics = state.platform.metrics.clone();
    Arc::get_mut(&mut state)
        .expect("mock state 应独占")
        .north
        .dead_letter =
        Arc::new(gateway_api::web::traits::SqliteDeadLetter::open(&db).expect("open dead letter"));
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(
        &app,
        "GET",
        "/api/north/dead-letters?protocol=http&limit=10",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["protocol"], "http");
    assert_eq!(body["data"]["total"], 2);
    // #6：自动裁剪统计必须随列表下发（0 = 从未裁剪），
    // 否则界面无法区分「没丢过数据」与「丢过但没记」
    assert_eq!(body["data"]["trimmed_total"], 0);
    assert_eq!(body["data"]["last_trim_ms"], 0);
    assert_eq!(body["data"]["last_trim_rows"], 0);
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // 最近失败在前
    assert_eq!(items[0]["data_id"], 102);
    assert_eq!(items[0]["reason"], "http_404");
    // 列表只给预览，不吐完整 payload
    assert_eq!(items[0]["payload_preview"], "second");
    let replay_id = items[0]["id"].as_i64().unwrap();

    // 重投：回到 outbox
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/north/dead-letters/{replay_id}/replay"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["requeued"], true);
    assert!(body["data"]["outbox_id"].as_i64().unwrap() > 0);
    assert_eq!(
        metrics
            .alarm
            .dead_letter_replay_total
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "重投成功必须计数，否则现场无法判断有人在处置死信"
    );
    assert_eq!(
        metrics
            .alarm
            .dead_letter_replay_conflict_total
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    // 重投后死信减少一条
    let (status, body) = call(&app, "GET", "/api/north/dead-letters?protocol=http").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 1);

    // 清理：未来时间戳命中剩余那条
    let future = 4_102_444_800_000i64; // 2100-01-01
    let (status, body) = call(
        &app,
        "DELETE",
        &format!("/api/north/dead-letters?protocol=http&older_than_ms={future}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["deleted"], 1);
    assert_eq!(
        metrics
            .alarm
            .dead_letter_purged_total
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "主动清理掉的数据量必须计数（这是确认丢弃的数据）"
    );

    let (status, body) = call(&app, "GET", "/api/north/dead-letters?protocol=http").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 0);
}

/// 重投不存在的死信必须报冲突，而不是假装成功。
#[tokio::test]
async fn replay_unknown_id_is_conflict() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db = dir.path().join("outbox.db");
    drop(OutboxStore::open(&db).expect("create db"));

    let mut state = common::mock_state::mock_app_state();
    let metrics = state.platform.metrics.clone();
    Arc::get_mut(&mut state)
        .expect("mock state 应独占")
        .north
        .dead_letter =
        Arc::new(gateway_api::web::traits::SqliteDeadLetter::open(&db).expect("open dead letter"));
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(&app, "POST", "/api/north/dead-letters/9999/replay").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], 4009);
    // 冲突也必须计数：否则「一直点重投、一直 409」在监控上是完全静默的
    assert_eq!(
        metrics
            .alarm
            .dead_letter_replay_conflict_total
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        metrics
            .alarm
            .dead_letter_replay_total
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

/// 分页参数夹紧：limit 超上限时不报错，按上限返回。
#[tokio::test]
async fn limit_is_clamped_by_server() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db = dir.path().join("outbox.db");
    drop(OutboxStore::open(&db).expect("create db"));

    let mut state = common::mock_state::mock_app_state();
    Arc::get_mut(&mut state)
        .expect("mock state 应独占")
        .north
        .dead_letter =
        Arc::new(gateway_api::web::traits::SqliteDeadLetter::open(&db).expect("open dead letter"));
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(&app, "GET", "/api/north/dead-letters?limit=99999").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["total"], 0);
    assert!(body["data"]["items"].as_array().unwrap().is_empty());
}
