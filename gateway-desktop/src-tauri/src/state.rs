//! 桌面客户端配置模型与应用状态
//!
//! 职责：
//! 1. 网关地址列表的内存表示（GatewayConfig / GatewayEntry）
//! 2. AppState：共享 HTTP 客户端 + 配置读写 + 加载错误诊断
//! 3. 配置相关 Tauri 命令：get_config / save_config / set_active_gateway / get_config_load_error

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// ===== 配置模型 =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// 已保存的网关列表
    #[serde(default)]
    pub gateways: Vec<GatewayEntry>,
    /// 当前激活的网关名称
    #[serde(default)]
    pub active: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayEntry {
    pub name: String,
    pub url: String,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            gateways: vec![GatewayEntry {
                name: "默认".to_string(),
                url: "http://127.0.0.1:8080".to_string(),
            }],
            active: Some("默认".to_string()),
        }
    }
}

// ===== 应用状态 =====

pub struct AppState {
    pub config: RwLock<GatewayConfig>,
    pub config_path: PathBuf,
    /// 共享 HTTP 客户端（连接池复用，避免每次请求新建）。
    ///
    /// 修复 A2: connect_timeout 2s + 总超时 8s；
    /// 增加 danger_accept_invalid_certs 让工业网关自签证书可用。
    pub http: reqwest::Client,
    /// Bug-D2 (fix)：webhook 出站专用 HTTP 客户端——**强制校验 TLS**。
    /// 与 `http` 字段不同：webhook 接收方通常是用户配置在公网的接收端
    /// （Slack / Lark / 企业 IM），不存在工业自签证书场景，
    /// 必须正常校验证书链避免 MITM。
    /// 仍保留 connect_timeout(2s) + timeout(8s) 与主客户端一致。
    pub webhook_http: reqwest::Client,
    /// 配置加载错误（启动 / 重新读取时填充，仅供前端诊断展示）。
    /// None 表示加载成功。
    pub last_load_error: std::sync::Mutex<Option<String>>,
    /// F6: 响应体大小上限（字节）。可通过环境变量 GATEWAY_MAX_RESPONSE_BYTES
    /// 配置，默认 65536（64 KiB）。超限返回 BODY_TOO_LARGE 错误，
    /// 引导前端走专门的导出通道（流式 / chunked）。
    pub max_response_bytes: usize,
}

impl AppState {
    /// 加载配置。
    ///
    /// 修复 A1：原实现对读盘失败 / JSON 反序列化失败统一静默吞掉用 default 兜底，
    /// 用户第一次启动若权限不足 / 磁盘满，看起来"能用"但所有请求都失败。
    /// 现区分 4 类错误：文件不存在 / 读取失败 / 反序列化失败 / 默认配置也解析失败。
    /// 失败信息记日志，runtime 通过 `last_load_error` 暴露给前端用于诊断。
    pub fn load(config_path: PathBuf) -> Self {
        let (config, load_error) = if !config_path.exists() {
            (GatewayConfig::default(), None)
        } else {
            match std::fs::read_to_string(&config_path) {
                Ok(text) => match serde_json::from_str::<GatewayConfig>(&text) {
                    Ok(cfg) => (cfg, None),
                    Err(e) => {
                        tracing::error!("desktop config: 反序列化失败 ({e}), 使用默认配置兜底");
                        (GatewayConfig::default(), Some(format!("反序列化失败: {e}")))
                    }
                },
                Err(e) => {
                    tracing::error!("desktop config: 读取失败 ({e}), 使用默认配置兜底");
                    (GatewayConfig::default(), Some(format!("读取失败: {e}")))
                }
            }
        };

        let max_response_bytes = std::env::var("GATEWAY_MAX_RESPONSE_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(65536);

        Self {
            config: RwLock::new(config),
            config_path,
            max_response_bytes,
            http: reqwest::Client::builder()
                // 工业现场首次启动网关还没起来，给 connect 单独的 2s 预算
                .connect_timeout(std::time::Duration::from_secs(2))
                .timeout(std::time::Duration::from_secs(8))
                // 工业网关常用自签证书，桌面端默认跳过校验；
                // 需要严格 TLS 校验时由后续版本在 gateway-switcher 加 verify 选项
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap_or_else(|e| {
                    // opt-4 sibling 修复：原 fallback reqwest::Client::new() 丢失了
                    // danger_accept_invalid_certs / 超时配置，行为与主客户端不一致，
                    // 自签证书网关在 fallback 路径下会 TLS 失败。
                    //
                    // Bug-D1 (fix)：二次 builder 兜底也要带 connect_timeout / timeout，
                    // 否则 builder 链全部失败（极罕见但可达，例如系统 TLS 后端严重异常）
                    // 时回退到 reqwest::Client::new()，**完全无超时**——proxy_request /
                    // forward_webhook / check_url_health 一旦发出，网络 hang 时永久挂起。
                    // 二次兜底保留相同超时 + TLS 跳过；最终 Client::new() 仅作为
                    // "TLS 后端完全不可用"的最后兜底（行为退化但不会再 spin）。
                    tracing::warn!("desktop http client builder 失败，使用基础配置: {}", e);
                    reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(8))
                        .danger_accept_invalid_certs(true)
                        .build()
                        .unwrap_or_else(|_| {
                            tracing::error!("desktop http client 二次 builder 仍失败，回退无超时客户端（极罕见）");
                            reqwest::Client::new()
                        })
                }),
            last_load_error: std::sync::Mutex::new(load_error),
            webhook_http: build_webhook_http_client(),
        }
    }

    pub async fn save(&self) -> Result<()> {
        let cfg = self.config.read().await;
        let text = serde_json::to_string_pretty(&*cfg)?;
        if let Some(parent) = self.config_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.config_path, text).await?;
        Ok(())
    }
}

/// Bug-D2 (fix)：构造 webhook 出站专用 HTTP 客户端。
///
/// **强制 TLS 校验**（不调用 `danger_accept_invalid_certs`），超时配置与主
/// `http` 客户端保持一致。webhook 接收方通常是公网接收端（Slack / Lark / 自建
/// webhook），不像工业网关那样需要自签证书，TLS 校验可避免 MITM。
///
/// 失败兜底：与 `http` 字段同型——二次 builder 失败时使用最小配置 Client，
/// 最后兜底无超时 Client（极罕见）。所有兜底分支都保留 TLS 校验。
fn build_webhook_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(8))
        // 注意：不调用 danger_accept_invalid_certsts —— webhook 接收方是公网，
        // 必须正常校验证书链。工业网关自签证书场景走主 `http` 客户端。
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!("desktop webhook http client builder 失败，使用基础配置: {}", e);
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(2))
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .unwrap_or_else(|_| {
                    tracing::error!("desktop webhook http client 二次 builder 仍失败，回退无超时客户端（极罕见）");
                    reqwest::Client::new()
                })
        })
}

// ===== Tauri 命令 =====

/// A1 修复：暴露配置加载错误给前端，让诊断面板可见。
///
/// 当 desktop 启动时配置加载失败（反序列化 / 权限 / 磁盘满），UI 的 status bar
/// 会显示"网关配置加载失败"等提示，避免"看起来能用但请求全失败"的死锁体验。
#[tauri::command]
pub async fn get_config_load_error(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<String>, String> {
    let g = state.last_load_error.lock().map_err(|e| e.to_string())?;
    Ok(g.clone())
}

/// 获取当前配置（网关列表 + 激活项）
#[tauri::command]
pub async fn get_config(state: tauri::State<'_, Arc<AppState>>) -> Result<GatewayConfig, String> {
    Ok(state.config.read().await.clone())
}

/// 保存配置
#[tauri::command]
pub async fn save_config(
    state: tauri::State<'_, Arc<AppState>>,
    config: GatewayConfig,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        *cfg = config;
    }
    state.save().await.map_err(|e| e.to_string())
}

/// 切换激活的网关
#[tauri::command]
pub async fn set_active_gateway(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.gateways.iter().any(|g| g.name == name) {
            cfg.active = Some(name);
        } else {
            return Err(format!("网关 '{}' 不存在", name));
        }
    }
    state.save().await.map_err(|e| e.to_string())
}
