//! 系统域的 OTA 与 NTP handler
//!
//! 路由（注册在 `system/routes.rs`）：
//! - POST /api/ota/upload              -> ota_upload
//! - GET  /api/ota/status              -> ota_status
//! - POST /api/ota/rollback            -> ota_rollback
//! - POST /api/ntp/sync                -> ntp_sync
//! - GET  /api/ntp/status              -> ntp_status
//!
//! DD-44 归位说明：这些 handler 物理上曾放在 `devices/handlers/discovery.rs`，
//! 但 OTA（网关固件升级）与 NTP（时钟校准）操作的都是**网关自身**，
//! 与设备发现、设备 IO 控制无关；路由也一直由系统域注册（`/api/ota/*`、`/api/ntp/*`，
//! 不在 `/api/devices/*` 前缀下）。故按「调用方归位」原则移入本模块，
//! 与证书 handler（`certificates.rs`）同处系统域。设备域只保留真正的设备能力
//! （发现 / IO 控制）。

use super::*;
use crate::web::types::OtaStatus;

/// POST /api/ota/upload — 上传固件包（整包或 GWDT 差分包）
pub(crate) async fn ota_upload(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let ota = state.north.ota.clone();
    let data = body.to_vec();
    if data.is_empty() {
        return Err(ApiError::bad_request("empty firmware"));
    }
    let is_delta = data.len() >= 4 && &data[0..4] == b"GWDT";
    let upload_type = if is_delta { "delta" } else { "full" };
    // DD-40 / ROI-43：先同步校验包头 / CRC / 签名，坏包当场拒绝并落审计。
    // 旧实现一律回 200「uploading」，验签失败也只体现在后台状态里，接口无法区分拒绝与受理。
    let version = match if is_delta {
        ota.precheck_delta(&data)
    } else {
        ota.precheck_firmware(&data)
    } {
        Ok(v) => v,
        Err(e) => {
            state.platform.audit.log(
                "ota.upgrade",
                "gateway",
                &format!(
                    "reject {} firmware ({} bytes): {}",
                    upload_type,
                    data.len(),
                    e
                ),
                "api",
            );
            return Err(ApiError::bad_request(format!("固件校验失败：{e}")));
        }
    };
    // D3: OTA 升级为敏感操作，须落审计（action=ota.upgrade，供自愈事件/审计查询消费）
    state.platform.audit.log(
        "ota.upgrade",
        "gateway",
        &format!(
            "accept {} firmware v{} ({} bytes)",
            upload_type,
            version,
            data.len()
        ),
        "api",
    );
    let audit = state.platform.audit.clone();
    gateway_core::engine::supervision::spawn_catch_unwind("ota_process", async move {
        let result = if is_delta {
            ota.process_delta(data).await
        } else {
            ota.process_firmware(data).await
        };
        // DD-46：安装结果必须落审计，不能只有开始记录
        match result {
            Ok(()) => audit.log(
                "ota.upgrade",
                "gateway",
                &format!("install {} firmware v{}: ok", upload_type, version),
                "api",
            ),
            Err(e) => {
                tracing::error!("OTA {} process failed: {}", upload_type, e);
                audit.log(
                    "ota.upgrade",
                    "gateway",
                    &format!(
                        "install {} firmware v{}: failed: {}",
                        upload_type, version, e
                    ),
                    "api",
                );
            }
        }
    });
    Ok(Json(
        serde_json::json!({"status": "uploading", "type": upload_type}),
    ))
}

/// GET /api/ota/status — 固件升级进度
pub(crate) async fn ota_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let s = state.north.ota.status();
    Json(OtaStatus {
        progress: s.progress,
        status: s.state.as_str().to_string(),
        error: s.error,
    })
}

/// POST /api/ntp/sync — 立即触发一次校时
pub(crate) async fn ntp_sync(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .platform
        .ntp
        .sync_now()
        .await
        .map_err(ApiError::from)?;
    // 校时会影响全站时间戳/历史数据对齐，属运维敏感动作
    state
        .platform
        .audit
        .log("ntp.sync", "gateway", "manual ntp sync", "api");
    Ok(Json(serde_json::json!({"status": "synced"})))
}

/// `/api/ntp/status` 响应：NTP 字段保持原结构（flatten），额外附上时钟可信度。
///
/// 时钟不可信期间调度器会暂停全部计划任务；现场需要能直接看到原因，
/// 而不是去翻日志。
///
/// NTP 原有字段以 JSON 值整体展开，不在此处声明其 Rust 类型：
/// `web/api` 层只允许引用 gateway-core 的白名单模块（见 tests/arch_deps.rs）。
#[derive(serde::Serialize)]
struct NtpStatusResponse {
    /// NTP 原有字段（整体展开，保持与旧契约一致）
    #[serde(flatten)]
    ntp: serde_json::Value,
    /// 系统时钟是否可信（不可信时计划任务暂停）
    clock_trusted: bool,
    /// 不可信原因；可信时为 null
    clock_untrust_reason: Option<String>,
    /// 计划任务当前是否处于暂停
    scheduled_tasks_paused: bool,
}

/// GET /api/ntp/status — NTP 状态与时钟可信度
pub(crate) async fn ntp_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use gateway_core::shared::clock::{evaluate_clock, NtpObservation};

    let ntp = state.platform.ntp.status();
    let observation = NtpObservation {
        // NtpStatus.last_sync 本身是毫秒时间戳（sync_ntp 返回 timestamp_millis）
        last_sync_ms: ntp.last_sync,
        synced: ntp.source == "NTP",
        offset_ms: ntp.offset_ms,
    };
    // 用与调度器相同的判据（gateway-core::shared::clock）
    let reason = evaluate_clock(chrono::Utc::now().timestamp_millis(), observation);

    Json(NtpStatusResponse {
        clock_trusted: reason.is_none(),
        scheduled_tasks_paused: reason.is_some(),
        clock_untrust_reason: reason,
        ntp: serde_json::to_value(ntp).unwrap_or(serde_json::Value::Null),
    })
}

/// POST /api/ota/rollback — 将固件回滚到上一个版本
pub(crate) async fn ota_rollback(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    state.north.ota.rollback().map_err(ApiError::from)?;

    // D3: OTA 回滚为敏感操作，须落审计
    state
        .platform
        .audit
        .log("ota.rollback", "gateway", "rollback firmware", "api");

    Ok(Json(serde_json::json!({
        "status": "rolled_back",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntp_response_keeps_old_fields_and_adds_clock_trust() {
        // flatten 后旧字段位置不变（前端/脚本按原契约解析），新增三个时钟字段
        let resp = NtpStatusResponse {
            ntp: serde_json::json!({
                "last_sync": 1_790_000_000_000i64,
                "status": "synced",
                "source": "NTP",
                "offset_ms": 3_600_000i64,
            }),
            clock_trusted: false,
            clock_untrust_reason: Some("系统时间与 NTP 偏差 3600 秒".to_string()),
            scheduled_tasks_paused: true,
        };
        let v = serde_json::to_value(resp).unwrap();
        assert_eq!(v["status"], "synced");
        assert_eq!(v["source"], "NTP");
        assert_eq!(v["offset_ms"], 3_600_000);
        assert_eq!(v["clock_trusted"], false);
        assert_eq!(v["scheduled_tasks_paused"], true);
        assert!(v["clock_untrust_reason"].as_str().unwrap().contains("NTP"));
    }
}
