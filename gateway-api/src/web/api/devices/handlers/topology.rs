//! 设备拓扑图 handler
//!
//! 路由：
//! - GET /api/devices/topology -> device_topology
//!
//! 第 1 期冗余收口：原 `GET /api/devices/topology/stats` 是本端点的聚合派生
//! （同一份配置与调度器快照、同一套 online/offline/warning 判定），
//! 现改为在 `TopologyData` 内附带 `stats` 字段，前端只调一个端点。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use gateway_core::config::{DeviceConfig, PointConfig};

use crate::web::types::{ApiError, ApiResponse, AppState};

use super::super::model::{TopoLink, TopoNode, TopoStats, TopologyData};

// ── 模板展开点位（与 device_ops/health.rs 同源：expand_points + device_point_ids）──
// 模板化配置下点位不在 cfg.points 中，由设备模板运行时展开；只遍历 cfg.points
// 会漏掉展开点位 → 拓扑图少节点、设备状态误判 offline。

fn expand_points(state: &AppState) -> Vec<PointConfig> {
    let cfg = state.config.load_full();
    cfg.devices
        .as_ref()
        .filter(|d| !d.is_empty())
        .and_then(|d| state.config.templates.expand_devices(d).ok())
        .unwrap_or_default()
}

/// 某设备的点位 ID：显式点位按 `device_id` 匹配，模板展开点位按 `"{设备名}_"` 前缀匹配。
fn device_point_ids(
    device: &DeviceConfig,
    cfg_points: &[PointConfig],
    expanded: &[PointConfig],
) -> Vec<String> {
    let prefix = format!("{}_", device.name);
    let mut ids = Vec::new();
    for p in cfg_points.iter().chain(expanded.iter()) {
        let by_id = !device.id.is_empty() && p.device_id.as_deref() == Some(device.id.as_str());
        let by_prefix = !device.name.is_empty() && p.id.starts_with(&prefix);
        if by_id || by_prefix {
            ids.push(p.id.clone());
        }
    }
    ids
}

/// 点位所属设备 id：显式点位取 device_id；模板展开点位按 "{设备名}_" 前缀反查。
fn point_owner_id(p: &PointConfig, devices: &[DeviceConfig]) -> Option<String> {
    if let Some(did) = &p.device_id {
        if !did.is_empty() {
            return Some(did.clone());
        }
    }
    devices
        .iter()
        .find(|d| !d.name.is_empty() && p.id.starts_with(&format!("{}_", d.name)))
        .map(|d| d.id.clone())
}

/// GET /api/devices/topology — 总线→设备→点位 拓扑（含聚合统计）
pub(crate) async fn device_topology(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<TopologyData>>, ApiError> {
    let cfg = state.config.load_full().as_ref().clone();
    let statuses = state.south.scheduler.device_status();
    let expanded = expand_points(&state);

    // 构建 point_id -> quality 映射
    let mut point_quality: HashMap<String, String> = HashMap::new();
    for s in &statuses {
        if let (Some(pid), Some(q)) = (s["point_id"].as_str(), s["quality"].as_str()) {
            point_quality.insert(pid.to_string(), q.to_string());
        }
    }

    let mut nodes: Vec<TopoNode> = Vec::new();
    let mut links: Vec<TopoLink> = Vec::new();

    // 总线节点
    for bus in &cfg.buses {
        nodes.push(TopoNode {
            id: bus.port_name.clone(),
            name: bus.port_name.clone(),
            node_type: "bus".to_string(),
            status: None,
            meta: Some(format!("{} @ {} baud", bus.protocol, bus.baud_rate)),
        });
    }

    // 设备节点（同时统计 online / warning / offline 计数）
    let mut online = 0usize;
    let mut warning = 0usize;
    let mut offline = 0usize;
    let mut device_count = 0usize;
    if let Some(devices) = &cfg.devices {
        for dev in devices {
            // 计算设备状态：如果该设备下有点位 quality=good 则 online，否则 offline
            // （含模板展开点位，否则模板化设备恒误判 offline）
            let dev_points = device_point_ids(dev, &cfg.points, &expanded);
            let mut has_good = false;
            let mut has_warning = false;
            for pid in &dev_points {
                match point_quality.get(pid).map(|s| s.as_str()) {
                    Some("good") => has_good = true,
                    Some("stale") => has_warning = true,
                    _ => {}
                }
            }
            let status = if has_good {
                online += 1;
                "online"
            } else if has_warning {
                warning += 1;
                "warning"
            } else {
                offline += 1;
                "offline"
            }
            .to_string();

            device_count += 1;
            nodes.push(TopoNode {
                id: dev.id.clone(),
                name: dev.name.clone(),
                node_type: "device".to_string(),
                status: Some(status),
                meta: Some(format!("slave: {}", dev.slave_id.unwrap_or(0))),
            });
            links.push(TopoLink {
                source: dev.bus.clone(),
                target: dev.id.clone(),
            });
        }
    }

    // 点位节点（显式点位 + 模板展开点位；展开点位的归属链路按设备名前缀反查）
    let devices = cfg.devices.clone().unwrap_or_default();
    for p in cfg.points.iter().chain(expanded.iter()) {
        let status = point_quality
            .get(&p.id)
            .map(|q| if q == "good" { "online" } else { "offline" }.to_string());
        nodes.push(TopoNode {
            id: p.id.clone(),
            name: p.name.clone().unwrap_or_else(|| p.id.clone()),
            node_type: "point".to_string(),
            status,
            meta: Some(format!("addr: {}", p.address)),
        });
        if let Some(did) = point_owner_id(p, &devices) {
            links.push(TopoLink {
                source: did,
                target: p.id.clone(),
            });
        }
    }

    // 统计与拓扑同源同判据（原先是独立的 /topology/stats 端点）：
    // 点位总数含模板展开点位，与拓扑图节点数一致。
    let stats = TopoStats {
        bus_count: cfg.buses.len(),
        device_count,
        online_count: online,
        offline_count: offline,
        warning_count: warning,
        point_count: cfg.points.len() + expanded.len(),
    };

    Ok(Json(ApiResponse::ok(TopologyData {
        nodes,
        links,
        stats,
    })))
}
