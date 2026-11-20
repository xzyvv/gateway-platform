//! 跨协议写语义定级测试（判据：能否证明写没有生效）。
//!
//! 这张表是所有南向协议写路径的**唯一口径来源**：写失败分两类——
//! - 明确失败（`DeviceRejected` / `ModbusException` / `BusBusy` / `Unsupported` /
//!   `InvalidConfig`）：能证明写没有生效 → 调用方可安全重试；
//! - 结果未知（`WriteUncertain`）：证明不了 → **禁止**自动重试，
//!   否则累加 / 脉冲类点位会被重复写。
//!
//! 本模块同时锁住两侧：`mark_write_uncertain` 的纯映射语义，以及
//! `mark_write_uncertain_counted` 的计数副作用（走总线总线的协议必须计数）。
use std::time::Duration;

use super::CollectorError;

/// 纯映射：结果未知类错误全部转 `WriteUncertain`。
#[cfg(any(
    feature = "s7comm",
    feature = "fins",
    feature = "mc",
    feature = "iec104",
    feature = "bacnet"
))]
#[test]
fn uncertain_errors_become_write_uncertain() {
    use super::mark_write_uncertain;
    for e in [
        CollectorError::NoReply,
        CollectorError::ChecksumError,
        CollectorError::Timeout(Duration::from_millis(500)),
        CollectorError::IoError("link down".into()),
        CollectorError::InvalidFrame("garbage".into()),
    ] {
        let label = format!("{e:?}");
        let mapped = mark_write_uncertain(e);
        assert!(
            matches!(mapped, CollectorError::WriteUncertain(_)),
            "{label} 必须被标为写结果未知，实际: {mapped:?}"
        );
    }
}

/// 纯映射：明确失败类错误原样保留（可安全重试）。
#[cfg(any(
    feature = "s7comm",
    feature = "fins",
    feature = "mc",
    feature = "iec104",
    feature = "bacnet"
))]
#[test]
fn definite_failures_keep_their_variant() {
    use super::mark_write_uncertain;
    for e in [
        CollectorError::DeviceRejected("device said no".into()),
        CollectorError::BusBusy(Duration::from_millis(50)),
        CollectorError::Unsupported("no write".into()),
        CollectorError::InvalidConfig("bad address".into()),
        CollectorError::BusClosed,
    ] {
        let label = format!("{e:?}");
        let mapped = mark_write_uncertain(e);
        assert!(
            !matches!(mapped, CollectorError::WriteUncertain(_)),
            "{label} 是明确失败，不得被标为写结果未知"
        );
    }
    #[cfg(feature = "modbus")]
    {
        let mapped = mark_write_uncertain(CollectorError::ModbusException(2));
        assert!(matches!(mapped, CollectorError::ModbusException(2)));
    }
}

/// `WriteUncertain` 自映射幂等：已是结果未知的不应被二次包装。
#[cfg(any(
    feature = "s7comm",
    feature = "fins",
    feature = "mc",
    feature = "iec104",
    feature = "bacnet"
))]
#[test]
fn write_uncertain_is_idempotent() {
    use super::mark_write_uncertain;
    let once = mark_write_uncertain(CollectorError::NoReply);
    let twice = mark_write_uncertain(once);
    match twice {
        CollectorError::WriteUncertain(s) => {
            assert!(!s.contains("write outcome uncertain"), "不得重复包装: {s}")
        }
        other => panic!("expected WriteUncertain, got {other:?}"),
    }
}

/// 带计数版本：结果未知时计数器 +1，明确失败不计数。
#[cfg(any(
    feature = "modbus",
    feature = "dlt645",
    feature = "cjt188"
))]
#[tokio::test]
async fn counted_variant_increments_metric_only_on_uncertain() {
    use super::mark_write_uncertain_counted;
    use crate::config::BusConfig;
    use crate::engine::metrics::Metrics;
    use crate::south::bus::{BusDeps, BusManager};
    use std::sync::Arc;

    let metrics = Arc::new(Metrics::new());
    let bus = BusManager::spawn_with(
        &BusConfig {
            port_name: "ws_test".to_string(),
            timeout_ms: 500,
            ..Default::default()
        },
        None,
        metrics.clone(),
        BusDeps::production(),
    );

    let before = metrics.bus.write_uncertain_total.load(std::sync::atomic::Ordering::Relaxed);
    let mapped = mark_write_uncertain_counted(&bus, CollectorError::NoReply);
    assert!(matches!(mapped, CollectorError::WriteUncertain(_)));
    let after = metrics.bus.write_uncertain_total.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(after, before + 1, "写结果未知必须计数");

    // 明确失败不计数
    let mapped = mark_write_uncertain_counted(&bus, CollectorError::DeviceRejected("x".into()));
    assert!(matches!(mapped, CollectorError::DeviceRejected(_)));
    let after2 = metrics
        .bus
        .write_uncertain_total
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(after2, after, "明确失败不得计入写结果未知");
}

/// `DeviceRejected` 的稳定错误码映射：走 Other（不与 Decode/Io 混淆）。
#[test]
fn device_rejected_maps_to_other_error_code() {
    use crate::engine::metrics::CollectErrorCode;
    assert_eq!(
        CollectorError::DeviceRejected("x".into()).error_code(),
        CollectErrorCode::Other
    );
}

/// `DeviceRejected` 与 `InvalidFrame` 必须可区分：前者是「设备说不成功」，
/// 后者是「应答没看懂」——两者的可重试性完全不同。
#[test]
fn device_rejected_is_distinct_from_invalid_frame() {
    let rejected = CollectorError::DeviceRejected("device said no".into());
    let invalid = CollectorError::InvalidFrame("garbage".into());
    assert!(!matches!(rejected, CollectorError::InvalidFrame(_)));
    assert!(!matches!(invalid, CollectorError::DeviceRejected(_)));
}

/// `WriteUncertain` 的稳定错误码映射：走 Other（专用计数器另看）。
#[test]
fn write_uncertain_maps_to_other_error_code() {
    use crate::engine::metrics::CollectErrorCode;
    assert_eq!(
        CollectorError::WriteUncertain("x".into()).error_code(),
        CollectErrorCode::Other
    );
}

/// `clone_collector_error` 必须能复制 `DeviceRejected`（批量写同因回填依赖它）。
#[cfg(any(
    feature = "modbus",
    feature = "mc",
    feature = "s7comm",
    feature = "bacnet",
    feature = "iec101"
))]
#[test]
fn clone_preserves_device_rejected() {
    let e = CollectorError::DeviceRejected("device said no".into());
    match super::clone_collector_error(&e) {
        CollectorError::DeviceRejected(s) => assert_eq!(s, "device said no"),
        other => panic!("expected DeviceRejected, got {other:?}"),
    }
}

/// 写结果未知**不得**被映射成超时（超时率只统计 `Timeout`）。
#[test]
fn write_uncertain_is_not_counted_as_timeout() {
    use crate::engine::metrics::CollectErrorCode;
    assert_ne!(
        CollectorError::WriteUncertain("x".into()).error_code(),
        CollectErrorCode::Timeout
    );
}

/// 三态映射：结果未知类错误一律 → `Uncertain`（禁止自动重试）。
#[test]
fn write_outcome_maps_uncertain_errors() {
    use crate::engine::write::WriteOutcome;
    for e in [
        CollectorError::WriteUncertain("x".into()),
        CollectorError::NoReply,
        CollectorError::Timeout(Duration::from_millis(500)),
        CollectorError::IoError("link down".into()),
        CollectorError::ChecksumError,
        CollectorError::InvalidFrame("garbage".into()),
    ] {
        let label = format!("{e:?}");
        assert_eq!(
            e.write_outcome(),
            WriteOutcome::Uncertain,
            "{label} 必须判为结果未知"
        );
    }
}

/// 三态映射：能证明写没有生效的错误 → `Failed`（可安全重试）。
#[test]
fn write_outcome_maps_definite_failures() {
    use crate::engine::write::WriteOutcome;
    for e in [
        CollectorError::DeviceRejected("device said no".into()),
        CollectorError::BusClosed,
        CollectorError::InvalidBcd("bad".into()),
        CollectorError::BusBusy(Duration::from_millis(50)),
        CollectorError::InvalidConfig("bad address".into()),
        CollectorError::Unsupported("no write".into()),
    ] {
        let label = format!("{e:?}");
        assert_eq!(e.write_outcome(), WriteOutcome::Failed, "{label} 必须判为明确失败");
    }
    #[cfg(feature = "modbus")]
    {
        assert_eq!(
            CollectorError::ModbusException(2).write_outcome(),
            WriteOutcome::Failed
        );
    }
}

/// 三态映射与 `mark_write_uncertain` 必须一致：
/// 被标为 `WriteUncertain` 的错误，三态上一定是 `Uncertain`。
#[cfg(any(
    feature = "s7comm",
    feature = "fins",
    feature = "mc",
    feature = "iec104",
    feature = "bacnet"
))]
#[test]
fn write_outcome_agrees_with_mark_write_uncertain() {
    use crate::engine::write::WriteOutcome;
    use super::mark_write_uncertain;
    for e in [
        CollectorError::NoReply,
        CollectorError::ChecksumError,
        CollectorError::Timeout(Duration::from_millis(500)),
        CollectorError::IoError("link down".into()),
        CollectorError::InvalidFrame("garbage".into()),
    ] {
        let mapped = mark_write_uncertain(e);
        assert_eq!(
            mapped.write_outcome(),
            WriteOutcome::Uncertain,
            "标为 WriteUncertain 后三态必须仍是 Uncertain"
        );
    }
}
