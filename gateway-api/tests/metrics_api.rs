//! `/api/metrics` 端到端断言：北向 feed 与广播 Lagged 指标必须出现在抓取结果里。
//!
//! 背景：这两组计数此前只存在于进程内部（原子量 + 节流日志），Prometheus 侧
//! 看不到——积压、背压、掉队只能靠翻日志反推。本测试从 HTTP 入口抓取，
//! 锁定指标名、标签与数值口径，防止后续重构把导出悄悄删掉。

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::mock_app_state_with_north_feed;
use gateway_api::web::AppState;
use gateway_core::engine::bus::BusConsumerKind;
use gateway_core::engine::model::DataPoint;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

async fn fetch_metrics(state: Arc<AppState>) -> String {
    let app = gateway_api::web::build_router(state);
    let req = Request::builder()
        .method("GET")
        .uri("/api/metrics")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).expect("Prometheus 文本必须是 UTF-8")
}

fn batch(point_id: &str) -> Arc<Vec<DataPoint>> {
    Arc::new(vec![DataPoint::good(point_id, "d1", 1.0, 1.0, "u16")])
}

/// 4 个北向 feed 指标：积压 / 容量 / 已投递 / 背压次数。
#[tokio::test]
async fn metrics_expose_north_feed_counters() {
    // 容量 1：第一批入队，第二批在 `send` 入口看到剩余槽位为 0 → 记一次背压后挂起。
    let (state, tx, _rx) = mock_app_state_with_north_feed(1);
    tx.send(batch("p1"))
        .await
        .expect("接收端仍在，send 不应失败");

    let blocked = {
        let tx = tx.clone();
        tokio::spawn(async move { tx.send(batch("p2")).await })
    };
    // 容量检查在 await 之前执行，但任务调度是异步的，等计数落账再抓取。
    let started = Instant::now();
    while tx.stats().backpressure_events == 0 && started.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let text = fetch_metrics(state).await;
    for needle in [
        "gateway_north_feed_pending_batches 1",
        "gateway_north_feed_capacity_batches 1",
        "gateway_north_feed_sent_batches_total 1",
        "gateway_north_feed_backpressure_events_total 1",
    ] {
        assert!(text.contains(needle), "缺少 {needle}\n{text}");
    }

    // 释放被背压挂起的发送任务，避免测试结束时留下挂起任务。
    blocked.abort();
}

/// 广播 Lagged 丢批：按消费端分别打标签，不能合并成一个总数。
#[tokio::test]
async fn metrics_expose_broadcast_lagged_counts_per_consumer() {
    let (state, _tx, _rx) = mock_app_state_with_north_feed(4);
    let bus = &state.south.realtime_bus;
    bus.note_lagged(BusConsumerKind::North, 3);
    bus.note_lagged(BusConsumerKind::WebSocket, 5);
    bus.note_lagged(BusConsumerKind::Alarm, 1);

    let text = fetch_metrics(state).await;
    for (consumer, value) in [("north", 3), ("websocket", 5), ("alarm", 1)] {
        let needle =
            format!("gateway_realtime_bus_lagged_batches_total{{consumer=\"{consumer}\"}} {value}");
        assert!(text.contains(&needle), "缺少 {needle}\n{text}");
    }
}
