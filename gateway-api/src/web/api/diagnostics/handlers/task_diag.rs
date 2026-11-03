//! 采集任务诊断 handler
//!
//! 输入点位 ID，按采集链路逐步诊断：总线连接 → 从站响应 → 功能码 → 地址 → 解码 → 调度。
//! 真值源：scheduler 点位状态快照（consecutive_failures / last_good_ts /
//! last_good_value / quality / stale_count / current_interval_ms）+ collector
//! 运行状态 + 总线/点位配置。历史 V1 用 hash + 时间片派生稳定假状态——
//! 现场会按编造的「总线连接 pass」排查，比不诊断更有误导性。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;
use ts_rs::TS;

use gateway_core::config::GatewayConfig;

use crate::web::types::{ApiError, ApiResponse, AppState};

/// 诊断步骤状态
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiagStep {
    /// 步骤名（如"总线连接"）
    pub name: String,
    /// 状态：pass / fail / warning / pending
    pub status: String,
    /// 主消息
    pub message: String,
    /// 可选详情（错误细节等）
    pub detail: Option<String>,
}

/// 点位诊断结果
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiagResult {
    pub point_id: String,
    pub point_name: String,
    /// 总体状态：healthy / degraded / offline
    pub status: String,
    pub steps: Vec<DiagStep>,
    pub suggestion: String,
    /// 最近一次诊断时间戳（毫秒）
    #[ts(type = "number")]
    pub last_check_at_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn step(name: &str, status: &str, message: String, detail: Option<String>) -> DiagStep {
    DiagStep {
        name: name.to_string(),
        status: status.to_string(),
        message,
        detail,
    }
}

/// 采集链路逐步诊断（真实数据版）：按快照与 collector 运行态逐步判定，
/// 前序 fail 后后续步骤置 pending（链路中断，后续未真正执行）。
fn diagnose_steps(
    point_id: &str,
    cfg: &GatewayConfig,
    snap: Option<&serde_json::Value>,
    collector_running: bool,
    now: i64,
) -> (Vec<DiagStep>, String, String) {
    let point_cfg = cfg.points.iter().find(|p| p.id == point_id);
    let bus_name = snap
        .and_then(|s| s["bus"].as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| point_cfg.map(|p| p.bus.clone()))
        .unwrap_or_default();
    let bus_cfg = cfg.buses.iter().find(|b| b.port_name == bus_name);

    let cf = snap
        .map(|s| s["consecutive_failures"].as_u64().unwrap_or(0))
        .unwrap_or(0);
    let last_good_ts = snap
        .map(|s| s["last_good_ts"].as_i64().unwrap_or(0))
        .unwrap_or(0);
    let last_good_value = snap.and_then(|s| s["last_good_value"].as_f64());
    let stale_count = snap
        .map(|s| s["stale_count"].as_u64().unwrap_or(0))
        .unwrap_or(0);
    let current_interval_ms = snap
        .map(|s| s["current_interval_ms"].as_u64().unwrap_or(0))
        .unwrap_or(0);
    let ever_ok = last_good_ts > 0;
    let elapsed_s = if ever_ok {
        (now - last_good_ts) / 1000
    } else {
        0
    };

    let mut steps: Vec<DiagStep> = Vec::with_capacity(6);

    // 1. 总线连接：collector 是否在跑（采集引擎真实运行态）
    let s1 = if bus_name.is_empty() {
        step(
            "总线连接",
            "fail",
            "点位未关联总线，无法建立采集链路".to_string(),
            None,
        )
    } else if collector_running {
        let meta = bus_cfg
            .map(|b| format!("{} @ {} baud", b.protocol, b.baud_rate))
            .unwrap_or_default();
        step(
            "总线连接",
            "pass",
            format!("总线 {bus_name} 采集中（{meta}）"),
            None,
        )
    } else {
        step(
            "总线连接",
            "fail",
            format!("总线 {bus_name} 未在采集：collector 未运行或总线已停用"),
            Some("请检查总线配置是否启用、串口/网口是否被占用".to_string()),
        )
    };
    let s1_fail = s1.status == "fail";
    steps.push(s1);

    // 2. 从站响应：连续失败次数（快照真实计数）
    let s2 = if s1_fail {
        step(
            "从站响应",
            "pending",
            "未执行（前序失败）".to_string(),
            None,
        )
    } else if snap.is_none() {
        step(
            "从站响应",
            "pending",
            "采集引擎尚未对该点位调度，无响应记录".to_string(),
            None,
        )
    } else if cf >= 3 {
        step(
            "从站响应",
            "fail",
            format!("从站连续 {cf} 次无响应"),
            Some("检查设备是否上电、A/B 线是否接反、从站地址与波特率是否匹配".to_string()),
        )
    } else if cf > 0 {
        step(
            "从站响应",
            "warning",
            format!("从站响应不稳定：连续失败 {cf} 次（采集退避中）"),
            None,
        )
    } else if ever_ok {
        step(
            "从站响应",
            "pass",
            format!("从站响应正常，最近成功 {elapsed_s} 秒前"),
            None,
        )
    } else {
        step(
            "从站响应",
            "pending",
            "尚无成功响应记录（采集刚启动或从未成功）".to_string(),
            None,
        )
    };
    let s2_fail = s2.status == "fail";
    steps.push(s2);

    // 3. 功能码支持 / 4. 地址校验：快照不区分失败原因，以「是否有成功记录」判定
    let blocked = s1_fail || s2_fail;
    let s3 = if blocked {
        step(
            "功能码支持",
            "pending",
            "未执行（前序失败）".to_string(),
            None,
        )
    } else if ever_ok {
        step(
            "功能码支持",
            "pass",
            "历史有成功采集，功能码被设备支持".to_string(),
            None,
        )
    } else if cf >= 3 {
        step(
            "功能码支持",
            "fail",
            "从未成功：功能码可能不被设备支持".to_string(),
            Some("核对点位功能码与设备手册（如 03/04 保持/输入寄存器）".to_string()),
        )
    } else {
        step(
            "功能码支持",
            "pending",
            "无成功记录，无法确认".to_string(),
            None,
        )
    };
    let s3_fail = s3.status == "fail";
    steps.push(s3);

    let s4 = if blocked || s3_fail {
        step(
            "地址校验",
            "pending",
            "未执行（前序失败）".to_string(),
            None,
        )
    } else if ever_ok {
        let addr = point_cfg.map(|p| p.address.clone()).unwrap_or_default();
        step("地址校验", "pass", format!("地址 {addr} 可正常读取"), None)
    } else if cf > 0 {
        step(
            "地址校验",
            "warning",
            "从未成功：寄存器地址可能越界或数据格式不匹配".to_string(),
            point_cfg.map(|p| format!("当前配置地址：{}", p.address)),
        )
    } else {
        step(
            "地址校验",
            "pending",
            "无成功记录，无法确认".to_string(),
            None,
        )
    };
    steps.push(s4);

    // 5. 数据解码：最近成功值 + stale 计数
    let s5 = if steps.iter().any(|s| s.status == "fail") {
        step(
            "数据解码",
            "pending",
            "未执行（前序失败）".to_string(),
            None,
        )
    } else if let Some(v) = last_good_value {
        let dt = point_cfg
            .and_then(|p| p.data_type.clone())
            .unwrap_or_else(|| "float".to_string());
        if stale_count > 0 {
            step(
                "数据解码",
                "warning",
                format!("最近值 {v}（{dt}），但已连续 {stale_count} 个周期未刷新"),
                Some("数据过期：设备可能在返回缓存值或采集被退避拉长".to_string()),
            )
        } else {
            step("数据解码", "pass", format!("最近值 {v}（{dt}）"), None)
        }
    } else {
        step("数据解码", "pending", "无成功解码记录".to_string(), None)
    };
    steps.push(s5);

    // 6. 采集调度：最近成功时刻 vs 当前采集周期（退避后周期）
    let s6 = if steps.iter().any(|s| s.status == "fail") {
        step(
            "采集调度",
            "pending",
            "未执行（前序失败）".to_string(),
            None,
        )
    } else if !ever_ok {
        step("采集调度", "pending", "尚无成功采集记录".to_string(), None)
    } else {
        let interval_s = (current_interval_ms / 1000).max(1);
        if elapsed_s <= interval_s as i64 * 2 {
            step(
                "采集调度",
                "pass",
                format!("最近采集 {elapsed_s} 秒前，当前周期 {current_interval_ms}ms"),
                None,
            )
        } else {
            step(
                "采集调度",
                "warning",
                format!("最近采集 {elapsed_s} 秒前，超过当前周期 {current_interval_ms}ms"),
                Some("可能存在总线拥塞或采集退避（连续失败后周期自动拉长）".to_string()),
            )
        }
    };
    steps.push(s6);

    // 总体状态与建议：首个 fail 定位链路断点
    let first_fail = steps.iter().position(|s| s.status == "fail");
    let has_warn = steps.iter().any(|s| s.status == "warning");
    let (status, suggestion) = match first_fail {
        Some(0) => (
            "offline",
            "总线未在采集：请检查总线配置是否启用、端口是否被占用".to_string(),
        ),
        Some(1) => (
            "offline",
            "从站无响应，请检查：1) 设备是否上电 2) A/B 线是否接反 3) 从站地址是否正确 4) 波特率是否匹配".to_string(),
        ),
        Some(2) => (
            "degraded",
            "功能码可能不被支持：请核对点位功能码与设备手册".to_string(),
        ),
        Some(_) => ("degraded", "采集链路存在异常，建议检查相关配置".to_string()),
        None if has_warn => (
            "degraded",
            "采集链路存在告警：查看各步骤详情定位（退避/过期/延迟）".to_string(),
        ),
        None => ("healthy", "采集链路正常，无需处理".to_string()),
    };
    (steps, status.to_string(), suggestion)
}

pub(crate) async fn diagnose_point(
    State(state): State<Arc<AppState>>,
    Path(point_id): Path<String>,
) -> Result<Json<ApiResponse<DiagResult>>, ApiError> {
    let cfg = state.config.load_full();

    // 在配置中查点位（points 或 devices 派生）
    let point_name = cfg
        .points
        .iter()
        .find(|p| p.id == point_id)
        .and_then(|p| p.name.clone())
        .or_else(|| {
            cfg.devices
                .as_ref()
                .and_then(|d| d.iter().find(|x| x.id == point_id).map(|x| x.name.clone()))
        })
        .unwrap_or_else(|| point_id.clone());

    // 快照按 point_id 直查（含模板展开点位——device_status 以展开后点位为键）；
    // collector 运行态按点位所属总线判定（快照优先，回退点位配置）
    let statuses = state.south.scheduler.device_status();
    let snap = statuses
        .iter()
        .find(|s| s["point_id"].as_str() == Some(point_id.as_str()));
    let bus_name = snap
        .and_then(|s| s["bus"].as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            cfg.points
                .iter()
                .find(|p| p.id == point_id)
                .map(|p| p.bus.clone())
        })
        .unwrap_or_default();
    let collector_running =
        !bus_name.is_empty() && state.south.scheduler.collectors().contains_key(&bus_name);

    let (steps, status, suggestion) =
        diagnose_steps(&point_id, &cfg, snap, collector_running, now_ms());
    Ok(Json(ApiResponse::ok(DiagResult {
        point_id,
        point_name,
        status,
        steps,
        suggestion,
        last_check_at_ms: now_ms(),
    })))
}
