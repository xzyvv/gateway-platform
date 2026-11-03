//! 网关自诊断 handler（真实主机指标）
//!
//! 指标来自 [`gateway_core::engine::monitor::host`]：Linux 读 `/proc`、`/sys`，
//! Windows 走系统 API。读不到或不支持的项返回 `null`，前端渲染为「—」。
//!
//! 历史实现用伪随机数 + 时间片生成 CPU/内存/磁盘/负载/进程数，页面数字与设备真实
//! 状态无关；运维据此判断过热与容量会得到错误结论，故整体替换为真实采集。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::Json;
use futures::future::join_all;
use gateway_core::config::BusConfig;
use gateway_core::engine::monitor::host;
use serde::Serialize;
use ts_rs::TS;

use crate::web::types::{ApiError, ApiResponse, AppState};

/// 网络接口状态
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct NetInterface {
    pub name: String,
    pub ip: String,
    /// 状态：up / down
    pub status: String,
    /// 接收字节数
    #[ts(type = "number")]
    pub rx: i64,
    /// 发送字节数
    #[ts(type = "number")]
    pub tx: i64,
}

/// 串口状态
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SerialPort {
    pub name: String,
    /// 状态：ok / error
    pub status: String,
    pub desc: String,
}

/// 网关自诊断报告
///
/// 数值字段为 `null` 表示「该平台不支持或读取失败」，不是 0。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GatewayDiag {
    /// CPU 温度（°C）
    pub cpu_temp: Option<f64>,
    /// CPU 使用率（%）
    pub cpu_usage: Option<f64>,
    /// 内存使用率（%）
    pub mem_usage: Option<f64>,
    /// 磁盘使用率（%）
    pub disk_usage: Option<f64>,
    /// 主机开机时长（秒；无法获取时为 null）
    #[ts(type = "number | null")]
    pub host_uptime_secs: Option<u64>,
    /// 主机开机时长（人类可读字符串；无法获取时为 "-"）
    pub host_uptime_str: String,
    /// gatewayd 进程运行时长（秒）；以 bootstrap 记录的启动时刻为基准
    #[ts(type = "number")]
    pub process_uptime_secs: u64,
    /// gatewayd 进程运行时长（人类可读字符串）
    pub process_uptime_str: String,
    pub net_interfaces: Vec<NetInterface>,
    pub serial_ports: Vec<SerialPort>,
    /// 系统平均负载（1 分钟；Windows 无此概念）
    pub load_avg: Option<f64>,
    /// 进程数
    pub processes: Option<u32>,
}

pub(crate) async fn get_gateway_diag(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<GatewayDiag>>, ApiError> {
    let cfg = state.config.load_full();

    // CPU 使用率是区间量：首次采样只能建立基线，且 Windows 计数器约 15ms 才推进一次，
    // 并发请求可能落在同一 tick。此时短暂等待后复采，最多 3 轮，避免首屏 CPU 显示「—」。
    let mut metrics = host::collect();
    for _ in 0..3 {
        if metrics.cpu_usage_pct.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
        metrics = host::collect();
    }

    let net_interfaces = metrics
        .net_interfaces
        .into_iter()
        .map(|n| NetInterface {
            name: n.name,
            ip: n.ip,
            status: if n.up { "up" } else { "down" }.to_string(),
            rx: clamp_i64(n.rx_bytes),
            tx: clamp_i64(n.tx_bytes),
        })
        .collect();

    let process_uptime = crate::web::uptime::process_uptime_secs();

    Ok(Json(ApiResponse::ok(GatewayDiag {
        cpu_temp: round1(metrics.cpu_temp_c),
        cpu_usage: round1(metrics.cpu_usage_pct),
        mem_usage: round1(metrics.mem_usage_pct),
        disk_usage: round1(metrics.disk_usage_pct),
        host_uptime_secs: metrics.uptime_secs,
        host_uptime_str: format_uptime(metrics.uptime_secs),
        process_uptime_secs: process_uptime,
        process_uptime_str: format_uptime(Some(process_uptime)),
        net_interfaces,
        serial_ports: collect_serial_ports(&cfg.buses).await,
        load_avg: metrics.load_avg_1m.map(|v| (v * 100.0).round() / 100.0),
        processes: metrics.process_count,
    })))
}

/// 保留一位小数，避免浮点噪声进入 UI
fn round1(value: Option<f64>) -> Option<f64> {
    value.map(|v| (v * 10.0).round() / 10.0)
}

/// 累计字节数可能超过 i64，钳制而非回绕
fn clamp_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

/// 运行时长格式化为「15天 6小时 32分」
fn format_uptime(secs: Option<u64>) -> String {
    let Some(total) = secs else {
        return "-".to_string();
    };
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let minutes = (total % 3_600) / 60;
    if days > 0 {
        format!("{days}天 {hours}小时 {minutes}分")
    } else if hours > 0 {
        format!("{hours}小时 {minutes}分")
    } else {
        format!("{minutes}分")
    }
}

/// 总线端点状态：TCP 端点做一次带超时的连接探测，本地串口查设备节点是否存在。
///
/// 历史实现按「序号 + 时间片」伪随机给出 ok/error，与设备实际状态无关。
/// 探测超时 300ms，探测并发进行，总线数量有限（通常 < 10），不构成开销。
async fn collect_serial_ports(buses: &[BusConfig]) -> Vec<SerialPort> {
    const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

    let probes = buses.iter().map(|bus| {
        let tcp_addr = bus
            .tcp_addr
            .as_ref()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty());
        let port_name = bus.port_name.clone();
        async move {
            match tcp_addr {
                Some(addr) => {
                    let reachable =
                        tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(&addr))
                            .await
                            .map(|result| result.is_ok())
                            .unwrap_or(false);
                    SerialPort {
                        name: addr,
                        status: if reachable { "ok" } else { "error" }.to_string(),
                        desc: if reachable {
                            "连接成功"
                        } else {
                            "连接失败"
                        }
                        .to_string(),
                    }
                }
                None => {
                    // 本地串口：设备节点存在即认为可用（Linux /dev/ttySx）。
                    // Windows 的 COMx 需要 `\\.\` 前缀，此处不做特殊处理，如实报不可用。
                    let exists = std::path::Path::new(&port_name).exists();
                    SerialPort {
                        name: port_name,
                        status: if exists { "ok" } else { "error" }.to_string(),
                        desc: if exists { "正常" } else { "设备不存在" }.to_string(),
                    }
                }
            }
        }
    });
    join_all(probes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus_with_port(port_name: &str) -> BusConfig {
        BusConfig {
            port_name: port_name.to_string(),
            ..BusConfig::default()
        }
    }

    fn bus_with_tcp(addr: &str) -> BusConfig {
        BusConfig {
            tcp_addr: Some(addr.to_string()),
            ..BusConfig::default()
        }
    }

    #[tokio::test]
    async fn missing_local_port_is_reported_as_error() {
        let ports = collect_serial_ports(&[bus_with_port("/dev/definitely-not-a-real-port")]).await;

        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].name, "/dev/definitely-not-a-real-port");
        assert_eq!(ports[0].status, "error");
        assert_eq!(ports[0].desc, "设备不存在");
    }

    #[tokio::test]
    async fn existing_local_path_is_reported_as_ok() {
        // 用当前目录代表「设备节点存在」的路径，避免依赖平台相关的 /dev
        let ports = collect_serial_ports(&[bus_with_port(".")]).await;

        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].status, "ok");
        assert_eq!(ports[0].desc, "正常");
    }

    #[tokio::test]
    async fn refused_tcp_endpoint_is_reported_as_error() {
        let ports = collect_serial_ports(&[bus_with_tcp("127.0.0.1:1")]).await;

        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].status, "error");
        assert_eq!(ports[0].desc, "连接失败");
    }

    #[tokio::test]
    async fn listening_tcp_endpoint_is_reported_as_ok() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let ports = collect_serial_ports(&[bus_with_tcp(&addr)]).await;

        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].name, addr);
        assert_eq!(ports[0].status, "ok");
        assert_eq!(ports[0].desc, "连接成功");
    }

    #[tokio::test]
    async fn empty_bus_list_yields_no_ports() {
        assert!(collect_serial_ports(&[]).await.is_empty());
    }
}
