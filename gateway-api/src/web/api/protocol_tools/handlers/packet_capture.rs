//! 报文捕获 handler（抓包 A 档：真实帧）
//!
//! 数据源：[`gateway_core::south::frame_log`]（采集事务 + 调试直发单点记录）。
//! 与 diagnostics::capture 共用同一帧源，结构不同（本结构含
//! deviceName/matchedId/delayMs/ascii）。设备归属按帧内从站地址匹配
//! GatewayConfig::devices（同总线 + address 一致）；匹配不到如实标「未知设备」。
//! 历史 V1 按 hash_id 编造请求/响应配对——误导性假数据，已移除。

use crate::web::types::{ApiResponse, AppState};
use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

use super::frame_desc;

/// 单条报文记录（含 deviceName/matchedId/delayMs/ascii）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct Packet {
    pub id: String,
    #[ts(type = "number")]
    pub time_ms: i64,
    pub direction: String,
    pub device_id: String,
    pub device_name: String,
    pub bus: String,
    pub hex: String,
    pub ascii: String,
    pub parsed: String,
    pub matched_id: Option<String>,
    pub delay_ms: Option<i32>,
}

#[derive(Deserialize)]
pub(crate) struct PacketQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub device_id: Option<String>,
}

fn default_limit() -> usize {
    100
}

fn hex_byte(b: u8) -> String {
    format!("{:02X}", b)
}

/// 帧字节 → ASCII 展示（不可打印字符用 · 占位）
fn ascii_of(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| {
            if (0x20..0x7f).contains(b) {
                *b as char
            } else {
                '·'
            }
        })
        .collect()
}

pub(crate) async fn list_packets(
    State(state): State<Arc<AppState>>,
    Query(q): Query<PacketQuery>,
) -> Result<Json<ApiResponse<Vec<Packet>>>, crate::web::types::ApiError> {
    let cfg = state.config.load_full();
    let limit = q.limit.min(200);
    // 真帧快照（新的在前）；无流量时返回空列表，由前端空态如实呈现
    // 经 web::debug_bus 门面访问（arch_deps 禁止 api 层直引 gateway_core::south）
    let frames = crate::web::debug_bus::frame_snapshot(None, limit * 2);

    // 设备归属：帧内从站地址 → 同总线 + address 一致的配置设备
    let dev_list = cfg.devices.as_deref().unwrap_or(&[]);
    let find_dev = |bus: &str, slave: Option<u8>| -> Option<(String, String)> {
        let s = slave? as i64;
        dev_list
            .iter()
            .find(|d| d.bus == bus && d.address.parse::<i64>().ok() == Some(s))
            .map(|d| (d.id.clone(), d.name.clone()))
    };

    let mut rows: Vec<Packet> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            // 传输形态与协议一律取总线配置，不再从帧字节里嗅探 MBAP 头（DD-29）
            let bus_cfg = cfg.buses.iter().find(|b| b.port_name == f.bus);
            let protocol = bus_cfg.map(|b| b.protocol.as_str()).unwrap_or("modbus");
            let is_tcp = bus_cfg.and_then(|b| b.tcp_addr.as_deref()).is_some();
            let variant = bus_cfg.and_then(|b| b.effective_variant());
            let slave = if protocol.to_ascii_lowercase().starts_with("modbus") {
                frame_desc::modbus_unit_id(is_tcp, &f.bytes)
            } else {
                None
            };
            let (device_id, device_name) = match find_dev(&f.bus, slave) {
                Some((id, name)) => (id, name),
                None => (
                    String::new(),
                    slave
                        .map(|s| format!("未知设备（从站 {}）", s))
                        .unwrap_or_else(|| "未知设备".into()),
                ),
            };
            Packet {
                id: format!("pkt_{}_{}", f.ts_ms, i),
                time_ms: f.ts_ms,
                direction: if f.is_tx { "request" } else { "response" }.into(),
                device_id,
                device_name,
                bus: f.bus.clone(),
                hex: f
                    .bytes
                    .iter()
                    .map(|b| hex_byte(*b))
                    .collect::<Vec<_>>()
                    .join(" "),
                ascii: ascii_of(&f.bytes),
                parsed: frame_desc::describe(
                    protocol,
                    is_tcp,
                    variant.as_deref(),
                    &f.bytes,
                    f.is_tx,
                ),
                matched_id: None,
                delay_ms: f.latency_ms,
            }
        })
        .collect();

    // 配对：帧新的在前，rx 的配对 tx 在其后（同总线相邻的前一条 request）
    for i in 0..rows.len() {
        if rows[i].direction != "response" {
            continue;
        }
        if let Some(j) = (i + 1..rows.len())
            .find(|&j| rows[j].direction == "request" && rows[j].bus == rows[i].bus)
        {
            let (rid, tid) = (rows[i].id.clone(), rows[j].id.clone());
            rows[i].matched_id = Some(tid);
            rows[j].matched_id = Some(rid);
        }
    }

    if let Some(did) = &q.device_id {
        rows.retain(|p| p.device_id == *did);
    }
    Ok(Json(ApiResponse::ok(rows)))
}

pub(crate) async fn clear_packets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<()>>, crate::web::types::ApiError> {
    // 真帧语义下「清空」必须真清（否则按钮是假动作）
    crate::web::debug_bus::frame_clear();
    Ok(Json(ApiResponse::ok(())))
}
