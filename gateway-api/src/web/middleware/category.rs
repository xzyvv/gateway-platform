//! 请求分类标记中间件（F1）：把路由组归属写进 request extensions，供指标/审计分层统计。

/// F1: Request category tag
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestCategory {
    ConfigEntry,
    ConfigManagement,
    ProtocolTool,
    Diagnostic,
}

pub(crate) async fn tag_config_entry(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    req.extensions_mut().insert(RequestCategory::ConfigEntry);
    next.run(req).await
}

pub(crate) async fn tag_config_management(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    req.extensions_mut()
        .insert(RequestCategory::ConfigManagement);
    next.run(req).await
}

pub(crate) async fn tag_protocol_tool(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    req.extensions_mut().insert(RequestCategory::ProtocolTool);
    next.run(req).await
}

/// 诊断运维端点标记（§10 perf-monitor feature gate 同源）
pub(crate) async fn tag_diagnostic(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    req.extensions_mut().insert(RequestCategory::Diagnostic);
    next.run(req).await
}
