//! 寄存器扫描 handler（P0-3：真实总线逐地址探测）
//!
//! V1 实现：`start_register_scan` 在请求线程内用 `hash(bus, slave, reg_type)`
//! 伪随机派生每个 cell，立即写 `RESULTS` 并返回 id（status 直接 done）。
//! 数据是假的，但同一参数两次扫描结果完全一致，比报错更有误导性。
//!
//! 本实现（契约 p0-3）：`start_register_scan` 只做校验 + 抢总线租约 + spawn
//! 后台任务，立即返回 id；后台任务**逐地址向指定从站发 Modbus 读寄存器帧**，
//! 按真实响应判定每个 `ScanCell`（契约 §3.2）。传输层沿用 V1 的
//! 「start 返回 id + 前端轮询 GET /:id」模型——`status`/`progress`/`cells`
//! 字段天然支持实时展示，前端零改动。
//!
//! 与从站扫描（p0-2）的判定差异：从站扫描看「设备在不在线」，异常帧算在线；
//! 寄存器扫描看「这个地址存不存在」，异常帧（非法数据地址 02）判
//! `responded=false`。
//!
//! 子模块划分：
//! - `judge`：读寄存器帧构造与单地址判定
//! - `scan`：后台逐地址扫描任务

mod judge;
mod scan;
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

use scan::spawn_register_scan;

/// 扫描单元（V1 字段不变，ts-rs 导出不变）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ScanCell {
    pub address: i32,
    pub responded: bool,
    pub value: Option<i32>,
    /// 设备应答功能码（i32：正常 3/4/1/2，异常 0x83=131 等；无应答 null）
    pub func_code: Option<i32>,
    pub error: Option<String>,
}

/// 扫描结果（V1 字段不变；`status` 取值扩展为 running/done/failed/aborted）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ScanResult {
    pub id: String,
    pub bus: String,
    pub slave_id: i32,
    pub reg_type: String,
    pub start_addr: i32,
    pub end_addr: i32,
    pub total: i32,
    pub responded: i32,
    pub cells: Vec<ScanCell>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    pub status: String,
    pub progress: i32,
}

/// 点位建议（V1 不变）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct PointSuggestion {
    pub id: String,
    pub name: String,
    pub address: String,
    pub address_num: i32,
}

#[derive(Deserialize)]
pub struct ScanBody {
    /// 目标总线 `port_name`（必填）；缺省由 handler 返回 400 `missing_bus`
    #[serde(default)]
    pub bus: Option<String>,
    pub slave_id: i32,
    pub reg_type: String,
    pub start_addr: i32,
    pub end_addr: i32,
    /// 单地址读超时（ms），默认 500、上限 10000（复用从站扫描同档）
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// 相邻地址探测间隔（ms），默认 50、下限 20（保护 RTU 半双工）
    #[serde(default)]
    pub interval_ms: Option<u64>,
}

/// 单次寄存器扫描最大地址跨度（纵深防御，沿用 V1）
const MAX_REGISTER_SCAN_RANGE: i64 = 65_536;

/// 扫描任务在注册表中的条目：结果可被后台任务增量写、GET 增量读；
/// `cancel` 由 DELETE `/register-scan/:id` 置位（删除即先中止再移除，P3-4 移除了独立 /stop 路由）。
struct ScanEntry {
    result: Arc<Mutex<ScanResult>>,
    cancel: Arc<AtomicBool>,
}

/// 进程内扫描结果注册表（**临时态**：不入库、不落盘，重启即失，
/// 口径见 `docs/api-protocol-tools.md` §3）。容量上限同探测结果表。
static RESULTS: Lazy<Mutex<HashMap<String, ScanEntry>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// 把扫描条目放入结果表，超限先淘汰**已结束**的旧条目。
///
/// 运行中的 entry 不能淘汰：后台任务持有 `Arc` 克隆，从表里移除会让前端
/// 轮询 `GET /register-scan/:id` 拿不到进度。抽成独立函数仅为便于单元测试
/// （表本身是进程级 static，测试用局部 map 避免与其它用例互相污染）。
fn insert_entry(results: &mut HashMap<String, ScanEntry>, id: String, entry: ScanEntry) {
    super::prune_finished_results(results, super::MAX_RESULT_ENTRIES, |e| {
        let r = e.result.lock();
        (r.status != "running", r.created_at_ms)
    });
    results.insert(id, entry);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) async fn start_register_scan(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ScanBody>,
) -> Result<Response, ApiError> {
    // ── 缺 bus → 400 missing_bus
    let bus_name = body
        .bus
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let Some(bus_name) = bus_name else {
        return Ok(error_response(
            BusErrorKind::MissingBus,
            "缺少必填参数 bus（目标总线 port_name），请指定后再扫描",
        ));
    };

    // ── 与协议无关的结构校验（400 invalid_frame）
    match body.reg_type.as_str() {
        "holding" | "input" | "coils" | "discrete" => {}
        other => {
            return Err(ApiError::bad_request(format!(
                "reg_type 必须为 holding/input/coils/discrete，当前 {}",
                other
            )))
        }
    }
    if body.end_addr < body.start_addr {
        return Err(ApiError::bad_request("end_addr 必须大于等于 start_addr"));
    }
    let span = (body.end_addr as i64) - (body.start_addr as i64) + 1;
    if span > MAX_REGISTER_SCAN_RANGE {
        return Err(ApiError::bad_request(format!(
            "扫描范围过大（{span} > {MAX_REGISTER_SCAN_RANGE}），请缩小范围"
        )));
    }
    let timeout_ms = clamp_scan_timeout_ms(body.timeout_ms);
    let interval_ms = clamp_scan_interval_ms(body.interval_ms);

    // ── 总线准入
    let lease = match acquire_bus(&state, &bus_name).await {
        Ok(l) => l,
        Err(e) => {
            audit_send(
                &state,
                &bus_name,
                &format!("register-scan {}", e.kind.as_str()),
            );
            return Ok(error_response(e.kind, e.detail));
        }
    };

    // ── 协议路由（B3）：按注册表能力位判断；探测帧语义（FC03/FC04）由实现方保证
    match protocol_capabilities(&lease.bus.protocol) {
        Some(caps) if caps.supports_register_scan => {}
        Some(_) => {
            let detail = format!(
                "总线 {} 协议为 {}，未声明支持寄存器扫描；可改用单点读值逐点核对地址",
                bus_name, lease.bus.protocol
            );
            audit_send(&state, &bus_name, "register-scan unsupported_protocol");
            return Ok(error_response(BusErrorKind::UnsupportedProtocol, detail));
        }
        None => {
            let detail = format!(
                "总线 {} 的协议 {} 未在注册表中登记，无法执行寄存器扫描",
                bus_name, lease.bus.protocol
            );
            audit_send(&state, &bus_name, "register-scan unknown_protocol");
            return Ok(error_response(BusErrorKind::UnsupportedProtocol, detail));
        }
    }

    // ── 按协议分叉的合法域校验：mc（站号/软元件地址）与 fins（节点号/地址）
    //    的宽度与 Modbus 不同，必须在拿到总线配置（协议名）之后才能判定。
    let protocol = lease.bus.protocol.to_ascii_lowercase();
    if let Err(msg) =
        judge::validate_params(&protocol, body.slave_id, body.start_addr, body.end_addr)
    {
        audit_send(&state, &bus_name, "register-scan invalid_params");
        return Err(ApiError::bad_request(msg));
    }

    // ── 建结果条目 + spawn 后台任务
    let id = format!("scan_{}", now_ms());
    let total = span as i32;
    let result = ScanResult {
        id: id.clone(),
        bus: bus_name.clone(),
        slave_id: body.slave_id,
        reg_type: body.reg_type.clone(),
        start_addr: body.start_addr,
        end_addr: body.end_addr,
        total,
        responded: 0,
        cells: Vec::with_capacity(total as usize),
        created_at_ms: now_ms(),
        status: "running".into(),
        progress: 0,
    };
    let entry = Arc::new(Mutex::new(result));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        // 容量上限由 insert_entry 统一执行（此前无上限，现场反复扫描会让内存单调增长）
        let mut results = RESULTS.lock();
        insert_entry(
            &mut results,
            id.clone(),
            ScanEntry {
                result: entry.clone(),
                cancel: cancel.clone(),
            },
        );
    }

    spawn_register_scan(
        Arc::new(move |action, bus, detail| {
            state.platform.audit.log(action, bus, detail, "api");
        }),
        bus_name,
        lease,
        entry,
        cancel,
        body.slave_id,
        body.reg_type,
        body.start_addr,
        body.end_addr,
        timeout_ms,
        interval_ms,
    );
    Ok(Json(ApiResponse::ok(id)).into_response())
}

pub(crate) async fn get_register_scan_result(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Option<ScanResult>>>, ApiError> {
    let r = RESULTS.lock().get(&id).map(|e| e.result.lock().clone());
    Ok(Json(ApiResponse::ok(r)))
}

pub(crate) async fn list_register_scan_results(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ScanResult>>>, ApiError> {
    let map = RESULTS.lock();
    let mut list: Vec<(i64, ScanResult)> = map
        .values()
        .map(|e| {
            let r = e.result.lock().clone();
            (r.created_at_ms, r)
        })
        .collect();
    list.sort_by_key(|a| std::cmp::Reverse(a.0));
    Ok(Json(ApiResponse::ok(
        list.into_iter().take(10).map(|(_, r)| r).collect(),
    )))
}

pub(crate) async fn delete_register_scan_result(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    // 删除前先尝试中止（若仍在运行），避免后台任务继续写已删除条目
    if let Some(e) = RESULTS.lock().get(&id) {
        e.cancel.store(true, Ordering::Relaxed);
    }
    RESULTS.lock().remove(&id);
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn generate_register_suggestions(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<PointSuggestion>>>, ApiError> {
    let r = RESULTS.lock().get(&id).map(|e| e.result.lock().clone());
    let r = match r {
        Some(r) => r,
        None => return Err(ApiError::not_found(format!("scan not found: {}", id))),
    };
    let mut cells: Vec<&ScanCell> = r.cells.iter().filter(|c| c.responded).collect();
    cells.sort_by_key(|c| c.address);
    let mut suggestions: Vec<PointSuggestion> = Vec::new();
    let mut iter = cells.iter();
    if let Some(first) = iter.next() {
        let mut start = first.address;
        let mut prev = first.address;
        let mut count = 1;
        for c in iter {
            if c.address == prev + 1 {
                count += 1;
                prev = c.address;
            } else {
                suggestions.push(PointSuggestion {
                    id: format!("suggest_{}", start),
                    name: format!("寄存器_{}", start),
                    address: start.to_string(),
                    address_num: count,
                });
                start = c.address;
                prev = start;
                count = 1;
            }
        }
        suggestions.push(PointSuggestion {
            id: format!("suggest_{}", start),
            name: format!("寄存器_{}", start),
            address: start.to_string(),
            address_num: count,
        });
    }
    Ok(Json(ApiResponse::ok(suggestions)))
}
