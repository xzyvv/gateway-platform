//! IEC 104「未建点位、按 IOA 直接读」调试端点
//!
//! `POST /api/protocol-tools/iec104/ioa-read`
//!
//! ## 为什么需要这个端点
//!
//! IEC 60870-5-104 **没有**「读单个 IOA」的请求帧——协议唯一的取数机制是
//! 总召唤（C_IC_NA_1 / ASDU 100），一次返回全站快照。因此现场想确认
//! 「某个 IOA 现在到底有没有值、值是多少」时，原有路径必须先在点位配置表里
//! 建点、等采集入库、再去历史/实时里找——而调试往往发生在「设备刚上电、
//! 现场还没来得及建点」的窗口里。
//!
//! 本端点把这条最短路径接出来：给一条总线 + 一组 IOA，直接返回该轮总召唤
//! 的命中值与全站快照。
//!
//! ## 取数来源（复用采集器优先 + 自建连接兜底）
//!
//! - **该总线已有采集器实例**（说明已经建了点位）：复用该实例，按 `Collector`
//!   契约读，不新建第二条 TCP 连接（IEC 104 从站通常只接受一条主站连接）。
//!   此时若该总线仍在采集中，**拒绝执行**并提示先暂停采集——直读与采集循环
//!   会争用同一链路（与 `/api/protocols/:name/diag` 的既有约定一致）。
//! - **该总线没有采集器实例**（未建点位的典型场景）：自建连接直读，读完即
//!   断开（采集器 `Drop` 会中断 keepalive 并关闭 TCP 连接），不影响采集面。
//!
//! ## 未命中语义
//!
//! 目标 IOA 不在本轮快照里 → `value = null`（**不返回 0**：0 是合法工程量，
//! 静默 0 会把「从站没这个点」误读成「值为 0」）。同时返回本轮全量快照，
//! 让现场直接看到从站到底上报了哪些 IOA。

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use gateway_core::engine::model::{AddressReadEntry, AddressReadResult};

use crate::web::api::protocol_tools::{audit_send, error_response};
use crate::web::debug_bus::BusErrorKind;
use crate::web::error::ApiError;
use crate::web::services::{iec104_read_standalone, iec104_read_via_collector, Iec104ReadSource};
use crate::web::types::{ApiResponse, AppState};

/// IEC 104 信息体地址（IOA）合法上限：IOA 恒 3 字节
pub(crate) const IEC104_MAX_IOA: u32 = 0x00FF_FFFF;
/// 单次请求允许的 IOA 数量上限（防滥用）
pub(crate) const MAX_IOA_COUNT: usize = 256;
/// 总召唤窗口下限（ms）
///
/// 总召唤返回的是**全站**数据，站点越大应答帧越多；总线默认超时（500ms）
/// 对大站点偏紧，调试直读给一个能用的下限。
const MIN_INTERROGATION_TIMEOUT_MS: u64 = 2_000;
/// 总召唤窗口上限（ms）
const MAX_INTERROGATION_TIMEOUT_MS: u64 = 60_000;
/// 调用方显式指定超时时允许的下限（ms）
const MIN_CALLER_TIMEOUT_MS: u64 = 100;

/// 请求体
#[derive(Deserialize)]
pub(crate) struct Iec104IoaReadBody {
    /// 目标总线 `port_name`（必填）
    #[serde(default)]
    pub bus: Option<String>,
    /// 待读取的 IOA 列表（必填）
    #[serde(default)]
    pub ioas: Vec<u32>,
    /// 总召唤窗口（ms）；缺省取总线配置（并抬到下限）
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// 单个 IOA 的读取结果
#[derive(Serialize)]
pub(crate) struct Iec104IoaReadEntry {
    /// 信息体地址（IOA）
    pub address: u32,
    /// 读取到的值；本轮未上报为 null（绝不静默返回 0）
    pub value: Option<f64>,
    /// 质量码（good / uncertain / bad / stale）；本轮未上报为 null
    pub quality: Option<String>,
    /// 源时标（UTC 毫秒）；无时标为 0
    pub source_ts: i64,
}

/// 读取结果
#[derive(Serialize)]
pub(crate) struct Iec104IoaReadResult {
    /// 目标总线
    pub bus: String,
    /// 取数来源：`collector` = 复用已有采集器；`standalone` = 自建连接
    pub source: String,
    /// 与请求 IOA 一一对应的命中结果（保持请求顺序）
    pub entries: Vec<Iec104IoaReadEntry>,
    /// 本轮总召唤返回的全量快照（按 IOA 升序）
    pub snapshot: Vec<Iec104IoaReadEntry>,
}

/// 取数来源的对外标签
fn source_label(source: Iec104ReadSource) -> &'static str {
    match source {
        Iec104ReadSource::Collector => "collector",
        Iec104ReadSource::Standalone => "standalone",
    }
}

/// core 读取结果 → 对外 DTO
fn map_entries(src: Vec<AddressReadEntry>) -> Vec<Iec104IoaReadEntry> {
    src.into_iter()
        .map(|e| Iec104IoaReadEntry {
            address: e.address,
            value: e.value,
            quality: e.quality.map(|q| q.as_str().to_string()),
            source_ts: e.source_ts,
        })
        .collect()
}

/// IOA 列表合法性校验（返回给现场的中文原因）
fn validate_ioas(ioas: &[u32]) -> Result<(), String> {
    if ioas.is_empty() {
        return Err("ioas 不能为空：请至少指定一个 IOA（如 100）".to_string());
    }
    if ioas.len() > MAX_IOA_COUNT {
        return Err(format!(
            "单次最多读取 {MAX_IOA_COUNT} 个 IOA，当前 {} 个",
            ioas.len()
        ));
    }
    if let Some(bad) = ioas.iter().find(|a| **a > IEC104_MAX_IOA) {
        return Err(format!(
            "IOA {bad} 超出 IEC 104 合法域（0..={IEC104_MAX_IOA}，信息体地址恒 3 字节）"
        ));
    }
    Ok(())
}

/// POST /api/protocol-tools/iec104/ioa-read — 按 IOA 直读 IEC104 从站实时值
pub(crate) async fn read_iec104_ioas(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Iec104IoaReadBody>,
) -> Result<Response, ApiError> {
    // ── 参数校验 ──
    let bus_name = body
        .bus
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let Some(bus_name) = bus_name else {
        return Ok(error_response(
            BusErrorKind::MissingBus,
            "缺少必填参数 bus（目标总线 port_name），请指定后再读取",
        ));
    };
    if let Err(msg) = validate_ioas(&body.ioas) {
        return Err(ApiError::bad_request(msg));
    }

    // ── 总线与协议校验 ──
    let cfg = state.config.load_full();
    let Some(bus) = cfg.buses.iter().find(|b| b.port_name == bus_name).cloned() else {
        return Ok(error_response(
            BusErrorKind::BusNotFound,
            format!("总线 {bus_name} 不在当前配置中，无法按 IOA 直读"),
        ));
    };
    if !bus.protocol.eq_ignore_ascii_case("iec104") {
        return Ok(error_response(
            BusErrorKind::UnsupportedProtocol,
            format!(
                "总线 {bus_name} 的协议为 {}，本端点仅支持 IEC 60870-5-104（iec104）总召唤直读",
                bus.protocol
            ),
        ));
    }

    // 总召唤窗口：调用方显式指定则按其值（带上下限），否则取总线配置并抬到下限
    let timeout_ms = match body.timeout_ms {
        Some(v) => Some(v.clamp(MIN_CALLER_TIMEOUT_MS, MAX_INTERROGATION_TIMEOUT_MS)),
        None => Some(bus.timeout_ms.max(MIN_INTERROGATION_TIMEOUT_MS)),
    };

    // ── 取数：复用采集器优先，自建连接兜底 ──
    let collectors = state.south.scheduler.collectors();
    let (source, result) = match collectors.get(&bus_name).cloned() {
        Some(collector) => {
            // 该总线已建点位、采集器在跑：直读会与采集循环争用同一 TCP 链路
            if !state.south.scheduler.is_bus_paused(&bus_name) {
                return Ok(error_response(
                    BusErrorKind::BusBusy,
                    format!(
                        "总线 {bus_name} 正在采集中：按 IOA 直读会与采集循环争用同一链路，\
                         请先 POST /api/buses/{bus_name}/pause 暂停该总线采集后再试"
                    ),
                ));
            }
            (
                Iec104ReadSource::Collector,
                iec104_read_via_collector(&collector, &body.ioas).await,
            )
        }
        None => (
            Iec104ReadSource::Standalone,
            iec104_read_standalone(&bus, &body.ioas, timeout_ms).await,
        ),
    };

    let result: AddressReadResult = match result {
        Ok(r) => r,
        Err(failure) => {
            audit_send(
                &state,
                &bus_name,
                &format!("iec104 ioa-read failed: {}", failure.kind.as_str()),
            );
            return Ok(error_response(failure.kind, failure.detail));
        }
    };

    audit_send(
        &state,
        &bus_name,
        &format!(
            "iec104 ioa-read ioas={} source={}",
            body.ioas.len(),
            source_label(source)
        ),
    );

    Ok(Json(ApiResponse::ok(Iec104IoaReadResult {
        bus: bus_name,
        source: source_label(source).to_string(),
        entries: map_entries(result.entries),
        snapshot: map_entries(result.snapshot),
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioa_列表_空与越界分别给出对应原因() {
        assert!(validate_ioas(&[]).unwrap_err().contains("ioas 不能为空"));
        assert!(validate_ioas(&[0, IEC104_MAX_IOA]).is_ok());
        assert!(validate_ioas(&[IEC104_MAX_IOA + 1])
            .unwrap_err()
            .contains("超出 IEC 104 合法域"));
    }

    #[test]
    fn ioa_列表_超长被拒() {
        let ioas = vec![1u32; MAX_IOA_COUNT + 1];
        assert!(validate_ioas(&ioas).unwrap_err().contains("单次最多读取"));
    }

    #[test]
    fn 映射_未命中保持_null而非_0() {
        let mapped = map_entries(vec![AddressReadEntry {
            address: 100,
            value: None,
            quality: None,
            source_ts: 0,
        }]);
        assert!(mapped[0].value.is_none());
        assert!(mapped[0].quality.is_none());
    }
}
