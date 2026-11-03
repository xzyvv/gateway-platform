use super::*;

fn temp_db_path() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}/disk_history_test_{}_{}.db",
        std::env::temp_dir().to_string_lossy(),
        pid,
        nanos
    )
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

/// r39-VER1 回归：disk_history 与 config 版本 / 上行测试历史共用同一库文件并发写，
/// 连接必须设置 busy_timeout，否则遇 SQLITE_BUSY 立即失败 → 磁盘采样偶发丢失。
#[test]
fn test_conn_sets_busy_timeout() {
    let path = temp_db_path();
    let store = DiskHistoryStore::new(10, &path).unwrap();
    let conn = store.open_db().expect("should succeed");
    let timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("should succeed");
    assert_eq!(timeout, 5000, "busy_timeout 应为 5000ms");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_push_and_list() {
    let path = temp_db_path();
    let store = DiskHistoryStore::new(100, &path).unwrap();
    store.push(80.5);
    sleep_ms(2);
    store.push(75.0);
    let samples = store.list();
    assert_eq!(samples.len(), 2);
    assert!((samples[0].free_pct - 80.5).abs() < 1e-6);
    assert!((samples[1].free_pct - 75.0).abs() < 1e-6);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_persistence_across_instances() {
    let path = temp_db_path();
    {
        let store = DiskHistoryStore::new(100, &path).unwrap();
        store.push(60.0);
        sleep_ms(2);
        store.push(55.5);
    }
    let store2 = DiskHistoryStore::new(100, &path).unwrap();
    let samples = store2.list();
    assert_eq!(samples.len(), 2);
    assert!((samples[0].free_pct - 60.0).abs() < 1e-6);
    assert!((samples[1].free_pct - 55.5).abs() < 1e-6);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_capacity_ring_buffer() {
    let path = temp_db_path();
    // max is clamped to min 10
    let store = DiskHistoryStore::new(10, &path).unwrap();
    for i in 0..15u8 {
        store.push(i as f64);
        sleep_ms(2);
    }
    let samples = store.list();
    assert_eq!(samples.len(), 10);
    assert!((samples[0].free_pct - 5.0).abs() < 1e-6);
    assert!((samples[9].free_pct - 14.0).abs() < 1e-6);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM disk_history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 10);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_ordering_ascending() {
    let path = temp_db_path();
    let store = DiskHistoryStore::new(100, &path).unwrap();
    store.push(10.0);
    sleep_ms(5);
    store.push(20.0);
    sleep_ms(5);
    store.push(30.0);
    let samples = store.list();
    assert_eq!(samples.len(), 3);
    assert!(samples[0].ts < samples[1].ts);
    assert!(samples[1].ts < samples[2].ts);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_min_max_clamped_to_10() {
    let path = temp_db_path();
    // max=3 should be clamped to 10
    let store = DiskHistoryStore::new(3, &path).unwrap();
    for i in 0..15u8 {
        store.push(i as f64);
        sleep_ms(2);
    }
    let samples = store.list();
    assert_eq!(samples.len(), 10);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_db_recovery_respects_max() {
    let path = temp_db_path();
    // first instance max=100, write 15 samples
    {
        let store = DiskHistoryStore::new(100, &path).unwrap();
        for i in 0..15u8 {
            store.push(i as f64);
            sleep_ms(2);
        }
    }
    // second instance max=10, only loads latest 10
    let store2 = DiskHistoryStore::new(10, &path).unwrap();
    let samples = store2.list();
    assert_eq!(samples.len(), 10);
    assert!((samples[0].free_pct - 5.0).abs() < 1e-6);
    assert!((samples[9].free_pct - 14.0).abs() < 1e-6);
    let _ = std::fs::remove_file(&path);
}

// ============================================================
// r52-MEM1: 进程 RSS 自监控回归测试
// ============================================================

/// r52-MEM1 契约：read_rss_bytes 必须返回正数（任何活进程都有 RSS > 0）。
/// 这是平台无关契约：Linux / Windows 都必须成功；不支持的平台约定返回 0
/// ——但当前测试目标是「实际能跑」的平台（Linux / Windows），不应在 CI 上
/// 触发 fallback 分支。后续若新增 macOS / BSD 等再放宽为 `>= 0`。
#[cfg(any(target_os = "linux", windows))]
#[test]
fn r52_read_rss_bytes_returns_positive() {
    let bytes = read_rss_bytes().expect("read_rss_bytes must succeed on supported platforms");
    assert!(
        bytes > 0,
        "RSS 必须为正数（实际 {} 字节）；若 0 则走到了 fallback 分支",
        bytes
    );
    // 合理性上界：32 GiB。任何现代进程的 RSS 不应超过此值（保护性 sanity check）。
    // 32 GiB 选为 4× 常见容器内存上限，避免未来硬件扩展误报。
    assert!(
        bytes < 32 * 1024 * 1024 * 1024,
        "RSS {} 字节超出合理上界，疑似采集异常",
        bytes
    );
}

/// r52-MEM1 契约：set_process_rss 写入能被 `Metrics.process_rss_bytes` 读出。
/// 这是 gauge.rs 与 metrics.rs 间的最小契约，回归防止有人重命名字段。
#[test]
fn r52_set_process_rss_roundtrip() {
    use crate::engine::metrics::Metrics;
    let m = Metrics::new();
    // 初始值 0（未采集）
    assert_eq!(
        m.process_rss_bytes
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    m.set_process_rss(123_456_789);
    assert_eq!(
        m.process_rss_bytes
            .load(std::sync::atomic::Ordering::Relaxed),
        123_456_789
    );
    // 0 写入允许（表示采集失败），不应 panic
    m.set_process_rss(0);
    assert_eq!(
        m.process_rss_bytes
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

// ============================================================
// P9：磁盘压力信号回归测试
// ============================================================

/// P9 契约：未检测过时 `free_pct()` 为 `None`（不能把"未检测"说成"0%"）。
#[test]
fn p9_disk_status_free_pct_none_before_first_check() {
    let ds = DiskStatus::new();
    assert!(ds.free_pct().is_none(), "从未检测时不应返回数值");
    assert!(!ds.is_pressure(), "初始状态不应处于磁盘压力");
}

/// P9 契约：`record` 记录真实剩余百分比并刷新压力标志。
#[test]
fn p9_disk_status_record_updates_pressure_and_free_pct() {
    let ds = DiskStatus::new();
    // 磁盘紧张：剩余 5%
    assert!(ds.record(5.0, true), "状态从正常→压力应报告变化");
    assert!(ds.is_pressure());
    assert!((ds.free_pct().unwrap() - 5.0).abs() < 1e-9);
    // 仍在压力区间但数值变化：不应再报告"状态变化"
    assert!(!ds.record(4.5, true), "压力→压力不应报告变化");
    assert!(ds.is_pressure());
    // 恢复
    assert!(ds.record(42.0, false), "压力→正常应报告变化");
    assert!(!ds.is_pressure());
    assert!((ds.free_pct().unwrap() - 42.0).abs() < 1e-9);
}

/// P9 契约：0% 与"未检测"必须可区分——前者是有效值，后者是 None。
#[test]
fn p9_disk_status_zero_percent_is_valid_not_none() {
    let ds = DiskStatus::new();
    ds.record(0.0, true);
    assert_eq!(ds.free_pct(), Some(0.0), "0% 是有效测量值，不能退化成 None");
    assert!(ds.is_pressure());
}

/// P9 契约：`set_disk_pressure` 能被 Prometheus 导出读到。
///
/// 这是 core 内部（磁盘监控）→ 指标层 → 运维看板的完整链路契约：
/// 此前磁盘满只写 tracing 日志，Prometheus 上完全不可见。
#[test]
fn p9_disk_pressure_gauge_roundtrip_and_exported() {
    use crate::engine::metrics::Metrics;
    let m = Metrics::new();
    assert_eq!(
        m.disk_pressure.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    m.set_disk_pressure(1);
    assert_eq!(
        m.disk_pressure.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    let export = m.prometheus_export();
    assert!(
        export.contains("gateway_disk_pressure 1"),
        "Prometheus 导出缺少 disk_pressure=1 样本:\n{export}"
    );
    assert!(export.contains("# TYPE gateway_disk_pressure gauge"));
}
