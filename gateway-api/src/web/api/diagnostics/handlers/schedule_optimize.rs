//! 调度优化（只读）
//!
//! DD-11：合并读取计划全部由后端计算，与真实采集路径共用同一份
//! `merge_batches`（从站号 / 功能码 / 寄存器宽度 / 运行时自适应 gap），
//! 前端不再自行估算「地址差 ≤ 5 就合并」这类近似值。
//!
//! 路由：`GET /api/diagnostics/schedule-optimize`

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use gateway_core::config::{DeviceConfig, PointConfig};

use crate::web::services;
use crate::web::types::{ApiError, ApiResponse, AppState};

/// 单个合并读取组
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduleOptimizeGroup {
    pub slave_id: i32,
    pub function_code: i32,
    pub start_addr: i32,
    pub end_addr: i32,
    pub point_count: i32,
}

/// 单设备的调度优化结果
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduleOptimizeDevice {
    pub device_id: String,
    pub device_name: String,
    pub protocol: String,
    /// 设备点位所在总线（多总线时取第一个；仅用于展示）
    pub bus: String,
    pub point_count: i32,
    /// 合并前的读取帧数（= 点位数）
    pub original_reads: i32,
    /// 合并后的读取帧数
    pub merged_reads: i32,
    /// 减少的帧数占比（%）
    pub reduction_percent: f64,
    /// 按 50ms/帧 粗估的耗时节省（仅展示，不代表实测值）
    pub saved_estimate_ms: i64,
    /// 该设备协议是否支持合并读取
    pub merge_supported: bool,
    /// 运行时自适应合并间隔阈值（仅 Modbus 有意义）
    pub gap_threshold: i32,
    pub groups: Vec<ScheduleOptimizeGroup>,
}

/// 调度优化总览
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduleOptimizeReport {
    pub devices: Vec<ScheduleOptimizeDevice>,
    pub total_points: i32,
    pub total_original_reads: i32,
    pub total_merged_reads: i32,
    pub reduction_percent: f64,
    pub saved_estimate_ms: i64,
}

fn device_points<'a>(
    device: &DeviceConfig,
    cfg_points: &'a [PointConfig],
    expanded: &'a [PointConfig],
) -> Vec<&'a PointConfig> {
    let prefix = format!("{}_", device.name);
    cfg_points
        .iter()
        .chain(expanded.iter())
        .filter(|p| {
            let by_id = !device.id.is_empty() && p.device_id.as_deref() == Some(device.id.as_str());
            let by_prefix = !device.name.is_empty() && p.id.starts_with(&prefix);
            by_id || by_prefix
        })
        .collect()
}

fn reduction_percent(original: i32, merged: i32) -> f64 {
    if original <= 0 {
        return 0.0;
    }
    (((original - merged) as f64) / original as f64 * 1000.0).round() / 10.0
}

pub(crate) async fn get_schedule_optimize(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ScheduleOptimizeReport>>, ApiError> {
    let cfg = state.config.load_full();
    // 模板模式：devices 在启动/热重载时展开为点位，cfg.points 不含它们。
    let expanded = cfg
        .devices
        .as_ref()
        .filter(|d| !d.is_empty())
        .and_then(|d| state.config.templates.expand_devices(d).ok())
        .unwrap_or_default();

    let devices_cfg = cfg.devices.clone().unwrap_or_default();
    let mut devices = Vec::with_capacity(devices_cfg.len());
    for device in &devices_cfg {
        let points = device_points(device, &cfg.points, &expanded);
        let bus = points
            .first()
            .map(|p| p.bus.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| device.bus.clone());
        let is_modbus = device.protocol == "modbus";

        let groups = if is_modbus && !points.is_empty() {
            let gap = state.south.scheduler.merge_gap(&bus);
            services::merge_plan(&points, gap)
                .into_iter()
                .map(|g| ScheduleOptimizeGroup {
                    slave_id: i32::from(g.slave_id),
                    function_code: i32::from(g.function_code),
                    start_addr: i32::from(g.start_addr),
                    end_addr: i32::from(g.end_addr),
                    point_count: i32::from(g.point_count),
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let point_count = points.len() as i32;
        let merged_reads = if is_modbus {
            groups.len() as i32
        } else {
            point_count
        };
        let original_reads = point_count;
        let saved = i64::from(original_reads - merged_reads).saturating_mul(50);

        devices.push(ScheduleOptimizeDevice {
            device_id: if device.id.is_empty() {
                device.name.clone()
            } else {
                device.id.clone()
            },
            device_name: device.name.clone(),
            protocol: device.protocol.clone(),
            gap_threshold: if is_modbus {
                i32::from(state.south.scheduler.merge_gap(&bus))
            } else {
                0
            },
            bus,
            point_count,
            original_reads,
            merged_reads,
            reduction_percent: reduction_percent(original_reads, merged_reads),
            saved_estimate_ms: saved,
            merge_supported: is_modbus,
            groups,
        });
    }

    let total_points: i32 = devices.iter().map(|d| d.point_count).sum();
    let total_original_reads: i32 = devices.iter().map(|d| d.original_reads).sum();
    let total_merged_reads: i32 = devices.iter().map(|d| d.merged_reads).sum();
    let report = ScheduleOptimizeReport {
        devices,
        total_points,
        total_original_reads,
        total_merged_reads,
        reduction_percent: reduction_percent(total_original_reads, total_merged_reads),
        saved_estimate_ms: i64::from(total_original_reads - total_merged_reads).saturating_mul(50),
    };

    Ok(Json(ApiResponse::ok(report)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DD-11：把响应体导出为前端类型，随 `npm run types:export` 一起刷新。
    #[test]
    fn export_ts_types_schedule_optimize() {
        let out = std::path::Path::new("../gateway-web/src/api/generated");
        std::fs::create_dir_all(out).expect("create generated dir");
        ScheduleOptimizeReport::export_all_to(out).expect("ScheduleOptimizeReport");
    }
}
