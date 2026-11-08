use super::*;
use crate::engine::ports::DiskHealthSink;
use tempfile::TempDir;

/// 假磁盘端口：记录最近一次 `set_disk_ok` 的取值与 `flush_fallback` 的调用次数。
///
/// 依赖倒置的测试替身——engine 侧测试不应依赖 `queue`（那正是本次要消除的反向依赖）。
struct FakeDiskSink {
    ok: std::sync::atomic::AtomicBool,
    flushes: std::sync::atomic::AtomicUsize,
}

impl FakeDiskSink {
    fn new(initial_ok: bool) -> Arc<Self> {
        Arc::new(Self {
            ok: std::sync::atomic::AtomicBool::new(initial_ok),
            flushes: std::sync::atomic::AtomicUsize::new(0),
        })
    }
}

impl DiskHealthSink for FakeDiskSink {
    fn set_disk_ok(&self, ok: bool) {
        self.ok.store(ok, std::sync::atomic::Ordering::SeqCst);
    }

    fn flush_fallback<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = GatewayResult<()>> + Send + 'a>> {
        self.flushes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn temp_db_path() -> (TempDir, String) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("disk_history.db");
    (dir, path.to_string_lossy().into_owned())
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

/// r39-VER1 回归：disk_history 与 config 版本 / 上行测试历史共用同一库文件并发写，
/// 连接必须设置 busy_timeout，否则遇 SQLITE_BUSY 立即失败 → 磁盘采样偶发丢失。
#[test]
fn test_conn_sets_busy_timeout() {
    let (_dir, path) = temp_db_path();
    let store = DiskHistoryStore::new(10, &path).unwrap();
    let conn = store.open_db().expect("should succeed");
    let timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("should succeed");
    assert_eq!(timeout, 5000, "busy_timeout 应为 5000ms");
}

#[test]
fn test_push_and_list() {
    let (_dir, path) = temp_db_path();
    let store = DiskHistoryStore::new(100, &path).unwrap();
    store.push(80.5);
    sleep_ms(2);
    store.push(75.0);
    let samples = store.list();
    assert_eq!(samples.len(), 2);
    assert!((samples[0].free_pct - 80.5).abs() < 1e-6);
    assert!((samples[1].free_pct - 75.0).abs() < 1e-6);
}

#[test]
fn test_persistence_across_instances() {
    let (_dir, path) = temp_db_path();
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
}

#[test]
fn test_capacity_ring_buffer() {
    let (_dir, path) = temp_db_path();
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
}

#[test]
fn test_ordering_ascending() {
    let (_dir, path) = temp_db_path();
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
}

#[test]
fn test_min_max_clamped_to_10() {
    let (_dir, path) = temp_db_path();
    // max=3 should be clamped to 10
    let store = DiskHistoryStore::new(3, &path).unwrap();
    for i in 0..15u8 {
        store.push(i as f64);
        sleep_ms(2);
    }
    let samples = store.list();
    assert_eq!(samples.len(), 10);
}

#[test]
fn test_db_recovery_respects_max() {
    let (_dir, path) = temp_db_path();
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

// ============================================================
// P2-9: 磁盘压力判定（阈值边界 + 状态沿）
// ============================================================

/// 阈值边界：10.0 是「刚好还能继续写」的包含边界。
///
/// 这条断言的价值在于不需要真造一个只剩 9% 的分区：监控循环里
/// `free_pct >= 10.0` 的判定抽成纯函数后，边界语义可以被直接钉死。
#[test]
fn p2_disk_ok_threshold_is_inclusive_at_ten_percent() {
    assert!(!disk_ok_from_free_pct(0.0), "0% 必须阻断入队");
    assert!(!disk_ok_from_free_pct(9.99), "9.99% 必须阻断入队");
    assert!(disk_ok_from_free_pct(10.0), "恰好 10% 应放行（包含边界）");
    assert!(disk_ok_from_free_pct(100.0));
    // 采样异常（NaN）时失败必须偏保守：宁可阻断入队，也不能把数据写进快满的盘
    assert!(!disk_ok_from_free_pct(f64::NAN), "NaN 必须视为不 ok");
}

/// `DiskStatus::record` 只在状态翻转时返回 true——否则每分钟刷一条同样的
/// error/info 日志，现场排查时反而会把真正的故障埋在噪声里。
#[test]
fn p2_disk_status_reports_only_on_state_change() {
    let st = DiskStatus::new();
    assert!(!st.is_pressure(), "尚未检测过，不应处于压力态");
    assert!(st.free_pct().is_none(), "未检测过时必须是 None 而不是 0%");

    assert!(
        !st.record(25.0, false),
        "默认压力=false，首次正常检测不算状态变化"
    );
    assert_eq!(st.free_pct(), Some(25.0));

    assert!(st.record(5.0, true), "正常 → 压力 必须报告一次");
    assert!(st.is_pressure());
    assert_eq!(st.free_pct(), Some(5.0));

    assert!(!st.record(3.0, true), "持续压力不得重复报告");
    assert!(!st.record(4.0, true), "压力值继续下降也不得重复报告");

    assert!(st.record(60.0, false), "压力 → 恢复 必须报告一次");
    assert!(!st.is_pressure());
    assert_eq!(st.free_pct(), Some(60.0));
}

/// 监控循环必须**先检测再 sleep**：原实现 sleep 在最前，`disk_ok` 默认 true，
/// 已满盘开机的设备前 60 秒会把数据直接写进 SQLite。
///
/// 这里用真实目录（临时目录所在的磁盘必然 >10% 可用）验证首轮会立即把
/// `disk_ok` 校正为 true，并写入 DiskStatus；若把 sleep 挪回循环开头，本测试
/// 会因 5s 超时而失败。
#[tokio::test]
async fn p2_disk_monitor_checks_immediately_before_first_sleep() {
    use crate::engine::metrics::Metrics;
    use std::sync::Arc;

    // 故意先置为 false（模拟「开机时磁盘是满的」这一最坏状态）
    let sink = FakeDiskSink::new(false);

    let (_dir, path) = temp_db_path();
    let history = Arc::new(DiskHistoryStore::new(10, &path).unwrap());
    let status = Arc::new(DiskStatus::new());
    let metrics = Arc::new(Metrics::new());

    spawn_disk_monitor(sink.clone(), history, status.clone(), metrics, None);

    // 首轮检测应在毫秒级完成；给 5s 冗余足够覆盖慢机
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while status.free_pct().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "磁盘监控 5s 内未完成首轮检测——sleep 很可能又跑到了循环最前面"
        );
        // 必须用 tokio 的 sleep 让出执行权：current-thread 运行时下
        // std::thread::sleep 会阻塞唯一线程，被监管任务永远不会被 poll 到。
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // 断言「首轮检测结果已回写」，而不是假设测试机磁盘一定宽裕。
    // 曾经硬编码 free>=10%：本机 C: 盘曾掉到 9.94%，一个表达
    // 「监控不依赖睡眠即可生效」的用例就变成了环境探测器而误报。
    // 这里按实测值反推期望：healthy 时必须把 disk_ok 校正为 true（覆盖 stale false），
    // 空间确实紧张时必须如实进入压力态（而不是沿用旧的 false 蒙对）。
    let free = status.free_pct().unwrap();
    assert!(free > 0.0, "应读到真实剩余百分比，实际 {free}");
    let expected_ok = free >= 10.0;
    assert_eq!(
        sink.ok.load(std::sync::atomic::Ordering::SeqCst),
        expected_ok,
        "disk_ok 必须与首轮实测一致（free={free}%）"
    );
    assert_eq!(
        status.is_pressure(),
        !expected_ok,
        "压力态必须与首轮实测一致（free={free}%）"
    );
}

/// 磁盘检查路径不存在时必须向上回退到最近的存在目录，而不是直接失败。
///
/// 现场含义：设备上 `/var/lib/gateway` 通常由安装脚本创建，若它尚未存在
/// （首启顺序颠倒 / CI runner 上根本没有该目录），statvfs 会 ENOENT →
/// 磁盘监控循环只打一条 warn、`DiskStatus` 永不记录 →
/// `disk_ok` 永远停在初始值，满盘保护静默失效。
#[test]
fn p2_disk_check_path_missing_falls_back_to_existing_ancestor() {
    // 造一个「父目录存在、目标目录不存在」的路径
    let base = tempfile::tempdir().expect("create temp dir");
    let missing = base.path().join("not-created-yet");
    assert!(
        !missing.is_dir(),
        "前置条件：目标目录必须不存在，否则本用例无效"
    );

    // 回退结果必须是一个真实存在的目录
    let resolved = nearest_existing_dir(missing.to_string_lossy().as_ref())
        .expect("必须回退到某个存在的祖先目录");
    assert!(
        std::path::Path::new(&resolved).is_dir(),
        "回退结果必须存在，实际 {resolved}"
    );

    // 关键断言：对不存在的路径做磁盘检查也必须成功（而不是 Err）
    let free = check_disk_space(missing.to_string_lossy().as_ref())
        .expect("路径不存在时必须回退而不是报错，否则磁盘监控静默失效");
    assert!(free > 0.0, "回退后应读到真实剩余百分比，实际 {free}");

    // 对存在的路径，回退逻辑不应改变结果
    let base_free =
        check_disk_space(base.path().to_string_lossy().as_ref()).expect("存在的目录必须可查");
    assert!(
        (base_free - free).abs() < 1.0,
        "存在目录与不存在的子路径应落在同一分区（{base_free} vs {free}）"
    );
}
