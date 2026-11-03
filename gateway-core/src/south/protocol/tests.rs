//! 修复(审查#B2) 回归：采集组级读超时预算必须随「点位数 × 总线超时」自适应，
//! 而不是硬编码 5s——否则逐点串行读取的 DLT645/CJT188 大组每个采集周期都会被
//! 误判为「整组超时」，而设备与链路其实完全正常。

use super::*;
use crate::config::BusConfig;
use crate::engine::metrics::Metrics;
use crate::south::bus::{BusDeps, BusManager};
// 单协议裁剪构建下这两个模块不存在，按 feature 门控（协议裁剪矩阵 F1）。
#[cfg(all(feature = "dlt645", feature = "cjt188"))]
use crate::south::protocol::cjt188::Cjt188Collector;
#[cfg(all(feature = "dlt645", feature = "cjt188"))]
use crate::south::protocol::dlt645::Dlt645Collector;
use crate::south::protocol::skeleton::SkeletonCollector;
use std::sync::Arc;

/// 挂在不存在的串口上的总线（仅用于取 io_timeout / 构造协议实例）。
fn test_bus(port: &str, timeout_ms: u64) -> Arc<BusManager> {
    let cfg = BusConfig {
        port_name: port.to_string(),
        timeout_ms,
        ..Default::default()
    };
    Arc::new(BusManager::spawn_with(
        &cfg,
        None,
        Arc::new(Metrics::new()),
        BusDeps::production(),
    ))
}

/// N-8：未覆写 `diag` 的协议不得返回假 "ok"，且 `diag_supported()` 必须为 false。
///
/// 假 "ok" 会被运维读成「设备正常」，与真实链路状态无关，属误导性输出。
#[tokio::test]
async fn default_diag_reports_not_implemented() {
    let c = SkeletonCollector::new("n8_diag", test_bus("n8_diag", 500));
    assert!(
        !c.diag_supported(),
        "skeleton 未覆写 diag，能力位必须为 false"
    );
    let d = c.diag().await;
    assert!(!d.contains(": ok"), "默认 diag 不得返回假 ok，实际: {d}");
    assert!(
        d.contains("diag not implemented"),
        "应如实报告未实现，实际: {d}"
    );
}

/// 默认实现（未覆写的协议，如 skeleton）：单点 1s 线性估算，钳制 [5s, 60s]。
#[tokio::test]
async fn default_batch_timeout_budget_scales_and_clamps() {
    let c = SkeletonCollector::new("b2_def", test_bus("b2_def", 500));
    assert_eq!(c.batch_timeout_budget(1), Duration::from_secs(5), "下限 5s");
    assert_eq!(
        c.batch_timeout_budget(10),
        Duration::from_secs(10),
        "10 点 × 1s"
    );
    assert_eq!(
        c.batch_timeout_budget(1000),
        Duration::from_secs(60),
        "上限 60s"
    );
    assert!(
        c.batch_timeout_budget(10) > Duration::from_secs(5),
        "大组预算必须超过旧的硬编码 5s"
    );
}

/// DLT645/CJT188 覆写：按 `总线 io_timeout × 3`（首试 + 2 次重试）× 点数估算，
/// 上限 120s。500ms 总线上的 10 点位组应得 15s，而非硬编码的 5s。
#[cfg(all(feature = "dlt645", feature = "cjt188"))]
#[tokio::test]
async fn per_point_serial_protocols_budget_follows_bus_timeout() {
    let bus = test_bus("b2_dlt", 500);
    let dlt = Dlt645Collector::new("b2_dlt", bus.clone());
    let cjt = Cjt188Collector::new("b2_cjt", bus.clone(), Arc::new(Metrics::new()));

    assert_eq!(
        dlt.batch_timeout_budget(1),
        Duration::from_secs(5),
        "下限 5s"
    );
    assert_eq!(
        dlt.batch_timeout_budget(10),
        Duration::from_secs(15),
        "10 × (500ms × 3)"
    );
    assert_eq!(cjt.batch_timeout_budget(10), Duration::from_secs(15));
    assert_eq!(
        dlt.batch_timeout_budget(100),
        Duration::from_secs(120),
        "上限 120s"
    );

    // 预算随总线 timeout_ms 增长：2s 总线 → 10 × (2000ms × 3) = 60s
    let slow = Dlt645Collector::new("b2_dlt_slow", test_bus("b2_dlt_slow", 2000));
    assert_eq!(slow.batch_timeout_budget(10), Duration::from_secs(60));
    assert!(
        slow.batch_timeout_budget(10) > dlt.batch_timeout_budget(10),
        "总线超时越大，组级预算必须越大"
    );
}

/// 采集错误 → 统计层稳定错误码的映射（每总线「最近错误码 / 超时率」的数据来源）。
///
/// 映射口径一旦漂移，前端展示的错误码与告警规则就全部对不上，故在此逐变体锁定。
#[test]
fn collector_error_maps_to_stable_error_code() {
    assert_eq!(
        CollectorError::BusClosed.error_code(),
        CollectErrorCode::BusClosed
    );
    assert_eq!(
        CollectorError::NoReply.error_code(),
        CollectErrorCode::NoReply
    );
    assert_eq!(
        CollectorError::InvalidFrame("bad".into()).error_code(),
        CollectErrorCode::Decode
    );
    assert_eq!(
        CollectorError::InvalidBcd("bad".into()).error_code(),
        CollectErrorCode::Decode
    );
    assert_eq!(
        CollectorError::ChecksumError.error_code(),
        CollectErrorCode::Checksum
    );
    assert_eq!(
        CollectorError::IoError("io".into()).error_code(),
        CollectErrorCode::Io
    );
    assert_eq!(
        CollectorError::Timeout(Duration::from_millis(500)).error_code(),
        CollectErrorCode::Timeout
    );
    assert_eq!(
        CollectorError::InvalidConfig("cfg".into()).error_code(),
        CollectErrorCode::Config
    );
    assert_eq!(
        CollectorError::Unsupported("x".into()).error_code(),
        CollectErrorCode::Other
    );
}

/// Modbus 异常码必须原样保留（错误码要能定位到具体异常）。
#[cfg(feature = "modbus")]
#[test]
fn modbus_exception_keeps_raw_code() {
    assert_eq!(
        CollectorError::ModbusException(2).error_code(),
        CollectErrorCode::ModbusException(2)
    );
    assert_eq!(
        CollectorError::ModbusException(2).error_code().code(),
        "EXC_02"
    );
}

/// 只有 `Timeout` 计入「超时率」，其余失败不得被算成超时。
#[test]
fn only_timeout_error_maps_to_timeout_code() {
    let timeout = CollectorError::Timeout(Duration::from_secs(1))
        .error_code()
        .code();
    assert_eq!(timeout, "TIMEOUT");

    for e in [
        CollectorError::NoReply,
        CollectorError::ChecksumError,
        CollectorError::IoError("io".into()),
        CollectorError::BusClosed,
    ] {
        assert_ne!(
            e.error_code(),
            CollectErrorCode::Timeout,
            "{:?} 不得被计为超时",
            e
        );
    }
}
