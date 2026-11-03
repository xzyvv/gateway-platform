//! 统一诊断链路（P2-5）
//!
//! 一次性诊断报告：聚合配置加载错误 + 网关健康状态 + 版本/路径。
//! 前端启动 bootstrap 调用此命令，一次拿到所有诊断信息，用于"网关连不上/
//! 配置损坏"等现场场景的可视化排错，避免黑屏死锁。

use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;

/// 一次性诊断报告：聚合配置加载错误 + 网关健康状态 + 版本/路径。
///
/// 前端启动 bootstrap 调用此命令，一次拿到所有诊断信息，用于"网关连不上/
/// 配置损坏"等现场场景的可视化排错，避免黑屏死锁。
#[derive(Debug, Serialize)]
pub struct DiagnosticReport {
    /// 配置加载错误（反序列化/读取失败），None 表示加载成功
    pub config_load_error: Option<String>,
    /// 激活网关是否可达
    pub gateway_reachable: bool,
    /// 网关探测延迟（毫秒，不可达时为 0）
    pub gateway_latency_ms: u64,
    /// 网关不可达原因（可达时为 None）
    pub gateway_error: Option<String>,
    /// 配置文件路径（供现场工程师定位）
    pub config_path: String,
    /// 应用版本
    pub app_version: String,
}

#[tauri::command]
pub async fn get_diagnostic(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<DiagnosticReport, String> {
    let config_error = state
        .last_load_error
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    // 健康检测（内联，复用 state.http）
    let cfg = state.config.read().await;
    let active_name = cfg.active.clone();
    let gateway = active_name
        .as_ref()
        .and_then(|name| cfg.gateways.iter().find(|g| g.name == *name).cloned());
    drop(cfg);
    let (reachable, latency_ms, error) = match gateway {
        Some(g) => {
            let base = g.url.trim_end_matches('/');
            let start = std::time::Instant::now();
            match state.http.get(format!("{}/health/live", base)).send().await {
                Ok(r) if r.status().is_success() => {
                    (true, start.elapsed().as_millis() as u64, None)
                }
                Ok(r) => (
                    false,
                    start.elapsed().as_millis() as u64,
                    Some(format!("状态码 {}", r.status())),
                ),
                Err(e) => (
                    false,
                    start.elapsed().as_millis() as u64,
                    Some(format!("{}", e)),
                ),
            }
        }
        None => (false, 0, Some("未配置激活网关".to_string())),
    };
    Ok(DiagnosticReport {
        config_load_error: config_error,
        gateway_reachable: reachable,
        gateway_latency_ms: latency_ms,
        gateway_error: error,
        config_path: state.config_path.display().to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
