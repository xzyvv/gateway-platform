//! 工程文件整包导入导出（决策表 #8）
//!
//! 「工程文件」是**可移植的三层子集 + 自描述信封**，用于把一套网关工程
//! 在设备/现场之间搬运；与 `GET /api/config`（导出**完整**配置，
//! 含 web 鉴权与凭据）刻意区分：
//!
//! - 总线层 `buses`
//! - 北向层 `northbound`：mqtt / webhook / cellular / opcua /
//!   modbus_server / modbus_passthrough / telemetry
//! - 变量映射层 `mappings`：points / devices / virtual_points /
//!   device_groups / device_assets
//!
//! 导入时**只覆盖这三层**，保留本机 `web`（鉴权/凭据）、`ntp`、
//! `license`、`self_heal` —— 工程文件不携带、也不篡改本机安全/环境配置。
//!
//! 路由：
//! - GET  /api/project/export -> export_project
//! - POST /api/project/import -> import_project

use serde::{Deserialize, Serialize};

use super::*;
use axum::extract::Query;
use axum::http::HeaderMap;
use gateway_core::config::{
    BusConfig, CellularConfig, DeviceAsset, DeviceGroup, ModbusPassthroughConfig,
    ModbusServerConfig, MqttConfig, OpcuaConfig, TelemetryConfig, VirtualPoint, WebhookConfig,
    CURRENT_SCHEMA_VERSION,
};

/// 工程文件信封标识（导入时据此拒绝误传的其它 JSON）
const PROJECT_KIND: &str = "gateway-project";

/// 北向层：所有「把采集数据送出去」的通道配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NorthboundLayer {
    /// MQTT 上传配置（GatewayConfig 中为必填，此处 Option 以表达"工程文件缺省时不覆盖"）
    pub mqtt: Option<MqttConfig>,
    pub webhook: Option<WebhookConfig>,
    pub cellular: Option<CellularConfig>,
    pub opcua: Option<OpcuaConfig>,
    pub modbus_server: Option<ModbusServerConfig>,
    pub modbus_passthrough: Option<ModbusPassthroughConfig>,
    pub telemetry: Option<TelemetryConfig>,
}

/// 变量映射层：点位、设备模板与派生/分组/资产元数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MappingsLayer {
    pub points: Vec<PointConfig>,
    pub devices: Option<Vec<DeviceConfig>>,
    pub virtual_points: Option<Vec<VirtualPoint>>,
    pub device_groups: Option<Vec<DeviceGroup>>,
    pub device_assets: Option<Vec<DeviceAsset>>,
}

/// 工程文件信封（自描述：kind + schema_version + exported_at）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectFile {
    /// 固定为 "gateway-project"，导入时强校验
    pub kind: String,
    /// 工程文件自身 Schema 版本（解析端据此拒绝"来自未来"的文件）
    pub schema_version: u32,
    /// 导出时刻（Unix 秒），仅供追溯用
    pub exported_at: i64,
    pub buses: Vec<BusConfig>,
    pub northbound: NorthboundLayer,
    pub mappings: MappingsLayer,
}

impl ProjectFile {
    /// 从完整配置抽取三层，构造工程文件信封。
    fn from_config(cfg: &GatewayConfig) -> Self {
        Self {
            kind: PROJECT_KIND.to_string(),
            schema_version: CURRENT_SCHEMA_VERSION,
            exported_at: now_secs(),
            buses: cfg.buses.clone(),
            northbound: NorthboundLayer {
                mqtt: Some(cfg.mqtt.clone()),
                webhook: cfg.webhook.clone(),
                cellular: cfg.cellular.clone(),
                opcua: cfg.opcua.clone(),
                modbus_server: cfg.modbus_server.clone(),
                modbus_passthrough: cfg.modbus_passthrough.clone(),
                telemetry: cfg.telemetry.clone(),
            },
            mappings: MappingsLayer {
                points: cfg.points.clone(),
                devices: cfg.devices.clone(),
                virtual_points: cfg.virtual_points.clone(),
                device_groups: cfg.device_groups.clone(),
                device_assets: cfg.device_assets.clone(),
            },
        }
    }

    /// 把工程文件三层覆盖到 `cfg`（本机 web / ntp / license / self_heal 保持不动）。
    fn apply_to(&self, cfg: &mut GatewayConfig) {
        cfg.buses = self.buses.clone();

        let nb = &self.northbound;
        // mqtt 为 GatewayConfig 必填项：仅在工程文件显式提供时覆盖，避免把必填项清空。
        if let Some(mqtt) = &nb.mqtt {
            cfg.mqtt = mqtt.clone();
        }
        cfg.webhook = nb.webhook.clone();
        cfg.cellular = nb.cellular.clone();
        cfg.opcua = nb.opcua.clone();
        cfg.modbus_server = nb.modbus_server.clone();
        cfg.modbus_passthrough = nb.modbus_passthrough.clone();
        cfg.telemetry = nb.telemetry.clone();

        let m = &self.mappings;
        cfg.points = m.points.clone();
        cfg.devices = m.devices.clone();
        cfg.virtual_points = m.virtual_points.clone();
        cfg.device_groups = m.device_groups.clone();
        cfg.device_assets = m.device_assets.clone();
    }
}

/// 工程导出查询参数（D-01 后 `/api/config/export` 已并入 GET /api/config，
/// 本端点保留自己的 `include_secrets` 语义，不再借用已删除的 `ExportQuery`）
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ProjectExportQuery {
    #[serde(default)]
    pub include_secrets: bool,
}

/// GET /api/project/export — 导出工程文件（三层子集 + 信封）
///
/// 与 `GET /api/config` 采用同一安全口径：默认脱敏北向通道凭据
/// （mqtt / webhook.headers / cellular），避免只读 API Key 取走明文。
/// 工程文件本身设计为「可搬运到另一台设备」，凭据随目标机既有值还原，
/// 故默认脱敏通常是期望行为；确需携带凭据时由主 token 显式请求明文。
pub(crate) async fn export_project(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ProjectExportQuery>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let cfg = state.config.load_full();
    let project = ProjectFile::from_config(&cfg);
    let mut value = serde_json::to_value(&project)
        .map_err(|e| ApiError::internal(format!("serialize project failed: {}", e)))?;
    if q.include_secrets {
        ensure_primary_token(&state, &headers, "project.export.plaintext")?;
    } else {
        // 工程文件把北向通道挂在 /northbound 下，用带前缀的同一脱敏清单。
        crate::web::api::system::handlers::config::redact_secrets_at(&mut value, "/northbound");
    }
    Ok(Json(ApiResponse::ok(value)))
}

/// POST /api/project/import — 导入工程文件（仅覆盖总线/北向/变量映射三层）
pub(crate) async fn import_project(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ProjectFile>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    if payload.kind != PROJECT_KIND {
        return Err(ApiError::bad_request(format!(
            "not a gateway project file (kind = {:?}, expected {:?})",
            payload.kind, PROJECT_KIND
        )));
    }
    if payload.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(ApiError::bad_request(format!(
            "project schema_version {} is newer than supported {}",
            payload.schema_version, CURRENT_SCHEMA_VERSION
        )));
    }

    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut new = old.as_ref().clone();
    payload.apply_to(&mut new);
    // 默认导出的工程文件凭据是 `"********"` 占位符，导入时必须从目标机既有配置还原，
    // 否则会把占位串当作真实凭据落盘（北向通道与 `buses[].opcua` 同此口径）。
    crate::web::config_apply::restore_redacted_secrets(&old, &mut new);

    // 配额校验与整份配置替换路径（post_config / rollback）保持一致：
    // 工程导入同样是整份替换，禁止绕过 license 上限。
    crate::web::config_apply::check_config_quota(&state, &new)?;

    // A5 整份替换序：expand → reload → store → save
    // （工程导入不落版本快照，无需 effective，忽略其返回值）
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new).await?;

    state.platform.audit.log(
        "project.import",
        "gateway",
        "project file import (buses/northbound/mappings) via api",
        "api",
    );
    Ok(Json(ApiResponse::ok(())))
}

/// 当前 Unix 秒（导出时间戳；系统时钟异常时退化为 0，不阻断导出）
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
