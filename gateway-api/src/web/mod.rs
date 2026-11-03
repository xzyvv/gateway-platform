//! Web Server（网关 Web API 面：REST + WebSocket）
//!
//! 基于 axum + tower ConcurrencyLimit。
//! 端点清单以 [`docs/api-surface.md`](../../../docs/api-surface.md) 为准——该文件由
//! `scripts/gen_api_surface.py` 从各子域 `routes.rs` 自动生成并由 CI 校验，
//! 勿在本注释里维护端点数量（历史上「14 个端点」已长期失真）。
//! 提供配置管理、状态查询、实时数据 WebSocket、IO 控制、OTA、NTP、历史、模板等接口。
//! Q16: 新增设备/测点 CRUD 路由组（api::config_routes），供 Go+Vue 调试软件调用。

pub mod api;
pub(crate) mod config_apply;
pub mod debug_bus;
pub mod error;
pub mod heal;
pub mod middleware;
pub(crate) mod services;
pub mod traits;
pub mod types;
pub mod uptime;

pub use traits::{
    DataQueueTrait, GatewayTrait, OtaStateView, OtaStatusView, OtaTrait, SchedulerTrait,
};
pub use types::AppState;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::{
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use middleware::{rate_limit, request_id};
use types::WEB_UI_HTML;
/// 根据 WebConfig.cors_origins 构建 CORS layer
fn build_cors_layer(origins: &Option<Vec<String>>) -> CorsLayer {
    use axum::http::HeaderValue;
    let layer = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_credentials(false)
        // 跨域直连（开发模式 vite 不走代理时）也要让前端读到 trace ID：
        // 浏览器默认只暴露 7 个简单响应头，x-request-id 必须显式 expose。
        //
        // P2-1④：历史导出（CSV）的截断标记同样放在响应头里，前端跨域下载时
        // 若不 expose 就读不到，因此一并列入。
        .expose_headers([
            axum::http::HeaderName::from_static(middleware::REQUEST_ID_HEADER),
            axum::http::HeaderName::from_static("x-history-total"),
            axum::http::HeaderName::from_static("x-history-returned"),
            axum::http::HeaderName::from_static("x-history-truncated"),
        ]);
    match origins {
        None => {
            // 开发模式：允许所有来源（向后兼容）
            layer.allow_origin(Any)
        }
        Some(list) if list.is_empty() => {
            // 生产模式：空列表 = 不允许任何跨域（不设置 allow_origin 即默认拒绝）
            layer
        }
        Some(list) => {
            // 生产模式：仅允许配置的来源
            let parsed: Vec<HeaderValue> = list
                .iter()
                .filter_map(|s| s.parse::<HeaderValue>().ok())
                .collect();
            layer.allow_origin(parsed)
        }
    }
}

/// 嵌入式调试控制台 HTML 响应（fallback 用）
fn embedded_html_response() -> axum::response::Response {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        WEB_UI_HTML,
    )
        .into_response()
}

use api::{health_live, health_ready};

/// 构造 Web 路由（含所有路由组、中间件、CORS、限速、CatchPanic）
///
/// 提取自 `start`，便于集成测试用 mock AppState 构造路由后用 `app.oneshot()` 发请求。
/// `start` 中保留启动校验（api_token 非空等），本函数不做事前校验。
pub fn build_router(state: Arc<AppState>) -> Router {
    // 系统级路由（read_routes + write_routes）：拆分到 api::system_routes。
    // 鉴权分层与原内联实现一致：read 用 require_auth_if_anon_disabled，
    // write 用 require_auth。详见 system/routes.rs。

    // 会话鉴权路由（登录 / whoami / 改密）与健康检查路由分别由
    // api::auth_routes 与下方 health_routes 提供。

    // 健康检查路由（无需鉴权，供 K8s/负载均衡探测）
    let health_routes = Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready));

    // 静态资源服务：优先托管 gateway-web/dist/（完整 Vue 应用），
    // 文件不存在时 fallback 到编译进二进制的嵌入式调试控制台（单文件 HTML）。
    // SPA fallback：非 /api、非 /health 的路径返回 index.html，支持 History 路由。
    // 生产部署将 dist/ 拷贝到 ./ui-dist 或设置 GATEWAY_WEB_DIR 环境变量。
    let static_dir = std::env::var("GATEWAY_WEB_DIR").unwrap_or_else(|_| "./ui-dist".to_string());
    // SPA fallback：未命中静态文件时返回 index.html
    // - dist/index.html 存在 → 服务完整 Vue 应用
    // - 不存在 → 回退到编译进二进制的嵌入式调试控制台
    let index_path = std::path::Path::new(&static_dir).join("index.html");
    let has_dist_index = index_path.exists();
    let index_path_clone = index_path.clone();
    let fallback = tower::service_fn(move |req: axum::http::Request<axum::body::Body>| {
        let index_path = index_path_clone.clone();
        async move {
            // /api、/health 未命中的路径必须回 404 JSON，不能落到 SPA 的 index.html：
            // 否则接口拼错或已下线会被前端当成 HTTP 成功，只在 JSON 解析阶段报错，
            // 既掩盖真实原因，也让监控统计不到 4xx。
            let path = req.uri().path();
            if path == "/api"
                || path.starts_with("/api/")
                || path == "/health"
                || path.starts_with("/health/")
            {
                return Ok::<_, std::convert::Infallible>(
                    error::ApiError::not_found(format!("no route for {path}")).into_response(),
                );
            }
            // 直接异步读取，省去原每请求一次同步 `Path::exists()`（stat）系统调用；
            // 文件缺失或读取失败统一回退到嵌入式控制台，语义与原先 exists() 分支一致。
            match tokio::fs::read(&index_path).await {
                Ok(bytes) => Ok::<_, std::convert::Infallible>(
                    // B1: body() 仅在 HeaderValue 非法时返回 Err；text/html 是合法常量，
                    // 理论上不会失败。但 unwrap() 在极端情况下（如 bytes 含非法字节触发
                    // HeaderValue 校验失败）会让整个静态资源服务 panic。改为 unwrap_or_else
                    // 回退到 embedded_html_response，保证 fallback 服务不挂。
                    axum::response::Response::builder()
                        .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                        .body(axum::body::Body::from(bytes))
                        .unwrap_or_else(|_| embedded_html_response()),
                ),
                Err(_) => Ok(embedded_html_response()),
            }
        }
    });
    let static_service = ServeDir::new(&static_dir).fallback(fallback);
    tracing::info!(
        "web static dir: {} (mode: {})",
        static_dir,
        if has_dist_index {
            "serving gateway-web dist"
        } else {
            "embedded UI fallback"
        }
    );

    // Q16: 设备/测点 CRUD + 配置导入导出 + 热重载路由组
    let config_crud_routes = api::config_routes(state.clone());

    // 合并：健康检查 + 各业务子域路由（每域自带鉴权分层）+ Q16 CRUD
    let app = Router::new()
        .merge(health_routes)
        .merge(api::auth_routes(state.clone()))
        .merge(api::system_routes(state.clone()))
        .merge(api::audit_routes(state.clone()))
        .merge(api::history_routes(state.clone()))
        .merge(api::points_routes(state.clone()))
        .merge(api::realtime_routes(state.clone()))
        .merge(api::license_routes(state.clone()))
        .merge(api::devices_routes(state.clone()))
        .merge(config_crud_routes)
        .merge(api::uplink_routes(state.clone()))
        .merge(api::device_ops_routes(state.clone()))
        .merge(api::points_ops_routes(state.clone()))
        .merge(api::diagnostics_routes(state.clone()))
        .merge(api::access_routes(state.clone()))
        .merge(api::protocol_tools_routes(state.clone()))
        .merge(api::quality_routes(state.clone()))
        .merge(api::sim_consumer_routes(state.clone()))
        .fallback_service(static_service)
        // P1-1：统一响应信封兜底。必须挂在 CompressionLayer **内层**（先包装后压缩），
        // 否则要在解压后的 body 上识别信封。豁免路径见 middleware::is_envelope_exempt。
        .layer(axum::middleware::from_fn(middleware::envelope))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(ConcurrencyLimitLayer::new(64))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rate_limit,
        ))
        // CORS：按 web.cors_origins 配置。None=开发模式允许所有（向后兼容），
        // Some(list) = 仅允许列表内来源，空列表=拒绝所有跨域（生产推荐）
        .layer(build_cors_layer(&state.web.cors_origins))
        .layer(CatchPanicLayer::custom(|err| {
            tracing::error!("handler panicked: {:?}", err);
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "internal server error (panic)"
                })),
            )
                .into_response()
        }))
        // 请求 ID 放在最外层：限速拒绝、鉴权失败、panic 兜底响应也带 trace ID。
        // 用户报障时只需提供一个 ID，即可在日志里定位到具体请求与耗时。
        .layer(axum::middleware::from_fn(request_id))
        .with_state(state);

    app
}

/// r74-SD1：Web 服务优雅停机信号（停机句柄）。
///
/// 与 r61/r62/r72/r73 的 `stopping + Notify` 模式一致：`shutdown()` 先置位
/// stopping 再 `notify_waiters()`，唤醒 `axum::serve(...).with_graceful_shutdown`
/// 等待的 future，让 serve 停止接受新连接并等待在飞请求结束后返回。
///
/// 为什么需要它：`start()` 生成的 `spawn_supervised("web_server")` 句柄此前被丢弃
/// 且无关闭原语——整个停机窗口（最坏 80s）内 HTTP 仍接受写请求，与
/// `queue.drain_and_wait()` 及陆续退出的子系统（mqtt / north_servers）并发，
/// 可能出现「drain 期间下发新配置 / 控制指令打到正在退出的子系统」。
#[derive(Clone, Default)]
pub struct ShutdownSignal {
    stopping: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl ShutdownSignal {
    /// 创建停机信号（未触发状态）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 触发优雅停机。幂等：重复调用无副作用。
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// 是否已触发停机（供测试/诊断）。
    pub fn is_shutting_down(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// 供 `with_graceful_shutdown` 使用的等待 future。
    ///
    /// 先 `enable()` 注册 waiter 再复查 stopping——覆盖「`notify_waiters` 在无 waiter
    /// 时通知丢失」的窗口（与 r60/r61/r62 同一教训）。
    async fn wait(&self) {
        if self.stopping.load(Ordering::SeqCst) {
            return;
        }
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // 注册后再复查：若 shutdown() 恰好发生在 enable 之前/之间，此处捕获。
        if self.stopping.load(Ordering::SeqCst) {
            return;
        }
        notified.await;
    }
}

/// 启动 Web Server
///
/// 兼容入口：不接外部停机信号（等价于「无优雅停机」的历史行为），
/// 供内部/测试保持原签名。生产路径请用 [`start_with_shutdown`]。
pub async fn start(
    state: Arc<AppState>,
    addr: SocketAddr,
) -> gateway_core::engine::error::GatewayResult<()> {
    start_with_shutdown(state, addr, ShutdownSignal::new()).await
}

/// 启动 Web Server（r74-SD1：支持外部优雅停机）。
pub async fn start_with_shutdown(
    state: Arc<AppState>,
    addr: SocketAddr,
    shutdown: ShutdownSignal,
) -> gateway_core::engine::error::GatewayResult<()> {
    // 启动前校验：必须配置 api_token，否则拒绝启动（fail-closed）
    if state.web.api_token.is_empty() {
        return Err(gateway_core::engine::error::GatewayError::Auth(
            "web.api_token 未配置，拒绝启动 Web 服务以避免无鉴权暴露控制接口".to_string(),
        ));
    }
    // Bug 173: 长度不足同样 fail-closed——原实现仅对 <16 字符告警后照常启动，
    // 1 字符弱 token 也能启用控制接口。弱 token 可被轻易暴力枚举，等同无鉴权，
    // 因此与空 token 一致处理：拒绝启动。
    if state.web.api_token.len() < 16 {
        return Err(gateway_core::engine::error::GatewayError::Auth(format!(
            "web.api_token 长度仅 {} 字符（< 16），拒绝启动 Web 服务以避免弱 token 被暴力枚举",
            state.web.api_token.len()
        )));
    }
    tracing::info!(
        "web API 鉴权已启用（POST 类接口需要 token，{} 只读匿名访问）",
        if state.web.allow_anon_read {
            "允许"
        } else {
            "禁止"
        }
    );

    let app = build_router(state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        gateway_core::engine::error::GatewayError::Io(format!("bind {} failed: {}", addr, e))
    })?;

    tracing::info!("web server listening on {}", addr);
    // P2: 注入 ConnectInfo，供限速中间件按客户端 IP 分桶（无此包装则拿不到来源 IP）。
    // r74-SD1：with_graceful_shutdown —— shutdown() 触发后停止接受新连接，
    // 待在飞请求处理完毕后 serve 返回 Ok(())，主循环随即正常退出（不触发 supervisor 重启）。
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move { shutdown.wait().await })
    .await
    .map_err(|e| {
        gateway_core::engine::error::GatewayError::Io(format!("web server error: {}", e))
    })?;
    tracing::info!("web server stopped (graceful shutdown)");
    Ok(())
}

// ===== r74-SD1：Web 优雅停机信号回归 =====

#[cfg(test)]
mod r74_tests {
    use super::*;
    use std::time::Duration;

    /// r74-SD1：`shutdown()` 置位标志，幂等。
    #[tokio::test]
    async fn r74_shutdown_signal_sets_flag_and_is_idempotent() {
        let sig = ShutdownSignal::new();
        assert!(!sig.is_shutting_down(), "初始不应处于停机状态");
        sig.shutdown().await;
        assert!(sig.is_shutting_down(), "shutdown() 必须置位");
        sig.shutdown().await;
        sig.shutdown().await;
        assert!(sig.is_shutting_down());
    }

    /// r74-SD1（关键）：`shutdown()` 必须唤醒正在等待的 `wait()` future——
    /// 这是 `with_graceful_shutdown` 能从停机信号真正退出的前提。
    #[tokio::test]
    async fn r74_shutdown_signal_wakes_waiter() {
        let sig = ShutdownSignal::new();
        let sig_for_wait = sig.clone();
        let waiter = tokio::spawn(async move { sig_for_wait.wait().await });
        // 让 waiter 先注册（进入 notified().await）
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "wait() 应处于等待态");

        sig.shutdown().await;
        let res = tokio::time::timeout(Duration::from_secs(2), waiter).await;
        assert!(res.is_ok(), "r74: shutdown() 必须唤醒 wait()");
    }

    /// r74-SD1（回归核心）：`shutdown()` 先于 `wait()` 调用时（无 waiter，
    /// `notify_waiters` 通知会丢失）——`wait()` 必须先复查 stopping 并立即返回，
    /// 否则 serve 将永久挂起，停机序超时才退出。
    #[tokio::test]
    async fn r74_shutdown_signal_wait_returns_immediately_if_already_shutdown() {
        let sig = ShutdownSignal::new();
        sig.shutdown().await; // 无 waiter，notify_waiters 丢失

        let res = tokio::time::timeout(Duration::from_secs(1), sig.wait()).await;
        assert!(
            res.is_ok(),
            "r74: 已停机时 wait() 必须立即返回（否则丢失唤醒导致 serve 永久挂起）"
        );
    }
}
