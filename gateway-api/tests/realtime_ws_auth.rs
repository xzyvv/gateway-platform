//! `/api/realtime` WebSocket 握手鉴权回归测试
//!
//! 回归点：B1 修复把浏览器端 token 从 URL query 移到 `bearer.<token>` 子协议，
//! 但 `require_auth_if_anon_disabled` 中间件只认 Authorization / X-API-Token 头，
//! 导致浏览器 WS 握手在中间件层被 401，实时数据页永远显示「重连中」。
//! 本测试断言：匿名读关闭（生产默认）时，携带有效子协议 token 的 WS 升级请求
//! 能通过中间件到达 handler；错误 / 缺失 token 仍在中间件层 401。
//!
//! 关于断言 426 而非 101：tower `oneshot` 直连 router，不经过 hyper HTTP server，
//! 请求里不会有 hyper 注入的 upgrade extension，`WebSocketUpgrade` 提取器因此以
//! 426 Upgrade Required 拒绝。故在本测试台架中，「426」唯一意味着「已通过中间件、
//! 到达 handler 提取器」（修复前为中间件层 401，路由不存在则为 404）。完整 101
//! 握手由端到端探针（scratch/ws_probe.py 对运行中的 gatewayd）验证。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::mock_state::mock_app_state;
use gateway_api::web::AppState;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

/// 匿名读关闭的 mock 状态（与生产默认 fail-closed 一致）
fn strict_state() -> Arc<AppState> {
    let state = mock_app_state();
    let mut cfg = (*state.config.config.load_full()).clone();
    if let Some(web) = cfg.web.as_mut() {
        web.allow_anon_read = false;
    }
    state.config.config.store(Arc::new(cfg));
    state
}

/// 构造一次最小的 WebSocket Upgrade 握手请求
fn ws_handshake(subprotocol: Option<String>) -> Request<Body> {
    let mut b = Request::builder()
        .method("GET")
        .uri("/api/realtime")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    if let Some(p) = subprotocol {
        b = b.header("sec-websocket-protocol", p);
    }
    b.body(Body::empty()).expect("build request")
}

#[tokio::test]
async fn ws_handshake_accepts_subprotocol_token() {
    let app = gateway_api::web::build_router(strict_state());
    let resp = app
        .oneshot(ws_handshake(Some(format!("bearer.{TOKEN}"))))
        .await
        .expect("router call");
    assert_eq!(
        resp.status(),
        StatusCode::UPGRADE_REQUIRED,
        "有效子协议 token 应通过中间件到达 handler（修复前在中间件层 401；\
         oneshot 无 hyper upgrade extension，提取器以 426 收尾）"
    );
}

#[tokio::test]
async fn ws_handshake_rejects_wrong_token() {
    let app = gateway_api::web::build_router(strict_state());
    let resp = app
        .oneshot(ws_handshake(Some("bearer.wrong-token".to_string())))
        .await
        .expect("router call");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ws_handshake_rejects_missing_token() {
    let app = gateway_api::web::build_router(strict_state());
    let resp = app.oneshot(ws_handshake(None)).await.expect("router call");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ws_handshake_still_accepts_authorization_header() {
    let app = gateway_api::web::build_router(strict_state());
    let mut req = ws_handshake(None);
    req.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {TOKEN}").parse().expect("header value"),
    );
    let resp = app.oneshot(req).await.expect("router call");
    assert_eq!(
        resp.status(),
        StatusCode::UPGRADE_REQUIRED,
        "Authorization 头路径不应受子协议支持影响（同样穿过中间件到达提取器）"
    );
}
