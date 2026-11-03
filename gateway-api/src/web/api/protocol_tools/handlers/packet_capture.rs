//! 报文捕获 handler（抓包 A 档：真实帧）
//!
//! 数据源：[`gateway_core::south::frame_log`]（采集事务 + 调试直发单点记录）。
//! 与 diagnostics::capture 共用同一帧源，结构不同（本结构含
//! deviceName/matchedId/delayMs/ascii）。设备归属按帧内从站号匹配
//! GatewayConfig::devices（同总线 + address 一致）；匹配不到如实标「未知设备」。
//! 历史 V1 按 hash_id 编造请求/响应配对——误导性假数据，已移除。

use crate::web::types::{ApiResponse, AppState};
use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

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

/// MBAP 头偏移判定：protocol id 字段（bytes[2..4]）为 0x0000 且帧够长 → TCP
fn mbap_offset(bytes: &[u8]) -> usize {
    if bytes.len() > 8 && bytes[2] == 0 && bytes[3] == 0 {
        6
    } else {
        0
    }
}

/// 提取 Modbus 帧的（从站号, 功能码）；非 Modbus/过短帧 None。
fn modbus_head(bytes: &[u8]) -> Option<(u8, u8)> {
    let off = mbap_offset(bytes);
    if bytes.len() < off + 2 {
        return None;
    }
    Some((bytes[off], bytes[off + 1]))
}

/// 轻量文本解析（与 diagnostics::capture 的结构化解析互补，本结构只存 String）
fn describe(bytes: &[u8], is_tx: bool, protocol: &str) -> String {
    if protocol != "modbus" {
        return format!("{} 帧（暂不支持解析，见 hex）", protocol);
    }
    let Some((slave, fc_raw)) = modbus_head(bytes) else {
        return "帧过短，无法解析".into();
    };
    let off = mbap_offset(bytes);
    if fc_raw & 0x80 != 0 {
        let exc = bytes.get(off + 2).copied().unwrap_or(0);
        return format!(
            "异常响应 功能码=0x{:02X} 异常码=0x{:02X}",
            fc_raw & 0x7f,
            exc
        );
    }
    let name = match fc_raw {
        1 => "读线圈",
        2 => "读离散输入",
        3 => "读保持寄存器",
        4 => "读输入寄存器",
        5 => "写单线圈",
        6 => "写单寄存器",
        15 => "写多线圈",
        16 => "写多寄存器",
        _ => "未知功能码",
    };
    if is_tx {
        match fc_raw {
            1..=4 | 15 | 16 if bytes.len() >= off + 6 => {
                let addr = ((bytes[off + 2] as u16) << 8) | bytes[off + 3] as u16;
                let count = ((bytes[off + 4] as u16) << 8) | bytes[off + 5] as u16;
                format!("{} 从站={} 起始={} 数量={}", name, slave, addr, count)
            }
            _ => format!("{} 从站={}", name, slave),
        }
    } else if matches!(fc_raw, 1..=4) && bytes.len() > off + 2 {
        let bc = bytes[off + 2] as usize;
        let start = off + 3;
        if bytes.len() >= start + bc {
            let vals: Vec<String> = bytes[start..start + bc]
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| format!("0x{:02X}{:02X}", c[0], c[1]))
                .collect();
            return format!("{} 从站={} 值=[{}]", name, slave, vals.join(" "));
        }
        format!("{} 从站={}", name, slave)
    } else {
        format!("{} 从站={}（响应）", name, slave)
    }
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

    // 设备归属：帧内从站号 → 同总线 + address 一致的配置设备
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
            let protocol = cfg
                .buses
                .iter()
                .find(|b| b.port_name == f.bus)
                .map(|b| b.protocol.as_str())
                .unwrap_or("modbus");
            let slave = if protocol == "modbus" {
                modbus_head(&f.bytes).map(|h| h.0)
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
                parsed: describe(&f.bytes, f.is_tx, protocol),
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
