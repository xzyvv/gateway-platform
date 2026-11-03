//! 通讯参数自动探测 handler（P0-5：真实总线验证式探测）
//!
//! V1 实现：对 `device_id` 做 hash 滚动派生，从候选表里挑波特率/校验位/字节序/字序/
//! 超时，5 个 `DetectStep` 恒为 `success`，从不触达总线。
//!
//! 本实现（契约 p0-5）：在**当前总线配置**下真实发帧，验证设备可达性与可观测参数
//! （连通性 / 从站地址 / 功能码支持 / 响应耗时 / 字节序），逐项给出真实判定。
//!
//! 能力边界（契约 §1.1）：`DebugBusTrait` 只能按 `BusConfig` 打开时的参数收发，
//! 运行期没有改波特率/校验位的接口，因此本期不做"扫描未知波特率"，只输出
//! 「已确认参数」；真正的参数扫描列入后续（契约 §9）。
//!
//! 安全策略：全部为只读 FC03/FC04、目标地址 0，不发任何写帧，零状态扰动。
//! 传输层与 p0-3/p0-4 同构：start 返回 id + 前端轮询 GET /run/:id。
//!
//! 子模块划分：
//! - `judge`：探测帧构造与 5 项单步判定
//! - `detect`：后台探测任务（逐项发帧并增量回写）
//! - `device`：按 `device_id` 从配置解析设备 / 总线 / 从站地址

mod detect;
mod device;
mod judge;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::web::services::protocol_capabilities_of as protocol_capabilities;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::web::api::protocol_tools::{acquire_bus, audit_send, error_response};
use crate::web::debug_bus::{clamp_scan_interval_ms, clamp_scan_timeout_ms, BusErrorKind};
use crate::web::types::{ApiError, ApiResponse, AppState};

use detect::spawn_auto_detect;
use device::resolve_device;

/// 单个探测步骤（V1 字段不变；`status` 取值扩展为 pending/testing/success/fail）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DetectStep {
    pub id: i32,
    pub param: String,
    pub value: String,
    pub status: String,
    pub detail: Option<String>,
}

/// 探测结果（V1 只有裸 DetectStep[]，此处新增任务对象，与 ScanResult 同构）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AutoDetectResult {
    pub id: String,
    pub device_id: String,
    pub device_name: String,
    pub bus: String,
    pub slave_id: i32,
    pub steps: Vec<DetectStep>,
    /// running / done / failed / aborted
    pub status: String,
    /// 0-100，已完成步数 / 5 × 100
    pub progress: i32,
    #[ts(type = "number")]
    pub start_time_ms: i64,
    #[ts(type = "number")]
    pub end_time_ms: Option<i64>,
}

#[derive(Deserialize)]
pub struct DetectBody {
    /// 目标设备 ID（必填）；后端从配置解析 bus/protocol/slave_id/name
    pub device_id: String,
}

/// 探测任务在注册表中的条目
struct DetectEntry {
    result: Arc<Mutex<AutoDetectResult>>,
    cancel: Arc<AtomicBool>,
}

/// 进程内探测结果注册表（**临时态**：不入库、不落盘，重启即失，
/// 口径见 `docs/api-protocol-tools.md` §3）
static RESULTS: Lazy<Mutex<HashMap<String, DetectEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 初始 5 个探测步骤（pending）
fn initial_steps() -> Vec<DetectStep> {
    ["总线连通性", "从站地址", "功能码支持", "响应耗时", "字节序"]
        .iter()
        .enumerate()
        .map(|(i, param)| DetectStep {
            id: i as i32 + 1,
            param: (*param).into(),
            value: String::new(),
            status: "pending".into(),
            detail: None,
        })
        .collect()
}

pub(crate) async fn start_auto_detect(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DetectBody>,
) -> Result<Response, ApiError> {
    // ── 解析设备 → bus + slave_id
    let (device, bus_name, slave_id) = match resolve_device(&state, &body.device_id) {
        Ok(v) => v,
        Err(e) => return Ok(error_response(e.kind, e.detail)),
    };

    // ── 总线准入
    let lease = match acquire_bus(&state, &bus_name).await {
        Ok(l) => l,
        Err(e) => {
            audit_send(
                &state,
                &bus_name,
                &format!("auto-detect {}", e.kind.as_str()),
            );
            return Ok(error_response(e.kind, e.detail));
        }
    };

    // ── 协议路由（B3）：按注册表能力位判断；探测步骤（FC03/FC04）由实现方保证
    // 口径必须取「设备自身协议」而不是总线协议：一条总线上可能挂不同协议的设备
    // （例如 modbus 总线 + dlt645 表计），按总线协议放行会把不属于该协议的 FC03/FC04
    // 探针打到现场设备上，既探测不准也可能干扰正常采集。
    let device_protocol = if device.protocol.is_empty() {
        lease.bus.protocol.as_str()
    } else {
        device.protocol.as_str()
    };
    match protocol_capabilities(device_protocol) {
        Some(caps) if caps.supports_auto_detect => {}
        Some(_) => {
            let detail = format!(
                "设备 {} 协议为 {}，未声明支持参数自动识别；请改用「设备诊断」确认链路",
                device.id, device_protocol
            );
            audit_send(&state, &bus_name, "auto-detect unsupported_protocol");
            return Ok(error_response(BusErrorKind::UnsupportedProtocol, detail));
        }
        None => {
            let detail = format!(
                "设备 {} 的协议 {} 未在注册表中登记，无法执行参数自动识别",
                device.id, device_protocol
            );
            audit_send(&state, &bus_name, "auto-detect unknown_protocol");
            return Ok(error_response(BusErrorKind::UnsupportedProtocol, detail));
        }
    }

    // ── 建结果条目 + spawn 后台任务
    let id = format!("detect_{}", now_ms());
    let result = AutoDetectResult {
        id: id.clone(),
        device_id: body.device_id.clone(),
        device_name: device.name.clone(),
        bus: bus_name.clone(),
        slave_id: slave_id as i32,
        steps: initial_steps(),
        status: "running".into(),
        progress: 0,
        start_time_ms: now_ms(),
        end_time_ms: None,
    };
    let entry = Arc::new(Mutex::new(result));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        // 结果表容量上限：只淘汰已结束的旧条目，运行中的任务仍持有 Arc 克隆。
        let mut results = RESULTS.lock();
        super::prune_finished_results(&mut results, super::MAX_RESULT_ENTRIES, |e| {
            let r = e.result.lock();
            (r.status != "running", r.start_time_ms)
        });
        results.insert(
            id.clone(),
            DetectEntry {
                result: entry.clone(),
                cancel: cancel.clone(),
            },
        );
    }

    let timeout_ms = clamp_scan_timeout_ms(None);
    let interval_ms = clamp_scan_interval_ms(None);

    spawn_auto_detect(
        Arc::new(move |action, dev, detail| {
            state.platform.audit.log(action, dev, detail, "api");
        }),
        body.device_id,
        device.name,
        bus_name,
        lease,
        entry,
        cancel,
        slave_id,
        timeout_ms,
        interval_ms,
    );
    Ok(Json(ApiResponse::ok(id)).into_response())
}

/// 中止运行中的探测：置 cancel 标志，任务在下一探测项前停止并释放租约。
pub(crate) async fn stop_auto_detect(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let entry = RESULTS.lock().get(&id).map(|e| e.cancel.clone());
    if let Some(cancel) = entry {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn get_auto_detect_result(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Option<AutoDetectResult>>>, ApiError> {
    let r = RESULTS.lock().get(&id).map(|e| e.result.lock().clone());
    Ok(Json(ApiResponse::ok(r)))
}
