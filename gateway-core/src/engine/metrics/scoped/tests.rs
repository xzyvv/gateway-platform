//! `scoped` 的单元测试（O-6 从原单文件拆出）。
//!
//! 与被测代码同属 `scoped` 模块，`use super::*` 的可见性语义不变。
use super::*;

const T0: i64 = 1_700_000_000_000; // 固定基准时间，避免依赖真实时钟

/// 简化构造：默认无设备、无错误码（覆盖既有用例）
fn sample<'a>(
    bus: &'a str,
    point: &'a str,
    success: bool,
    us: u64,
    now_ms: i64,
) -> CollectSample<'a> {
    CollectSample {
        bus,
        point,
        device: "",
        success,
        duration_us: us,
        error: None,
        now_ms,
    }
}

/// 带错误码的失败样本
fn fail_sample<'a>(
    bus: &'a str,
    point: &'a str,
    error: CollectErrorCode,
    now_ms: i64,
) -> CollectSample<'a> {
    CollectSample {
        bus,
        point,
        device: "",
        success: false,
        duration_us: 1_000,
        error: Some(error),
        now_ms,
    }
}

/// 带设备归属的样本（在线设备数口径）
fn dev_sample<'a>(
    bus: &'a str,
    point: &'a str,
    device: &'a str,
    success: bool,
    now_ms: i64,
) -> CollectSample<'a> {
    CollectSample {
        bus,
        point,
        device,
        success,
        duration_us: 1_000,
        error: None,
        now_ms,
    }
}

#[test]
fn records_success_and_failure_per_scope() {
    let m = ScopedCollectMetrics::default();
    m.record(sample("bus-a", "p1", true, 2_000, T0));
    m.record(sample("bus-a", "p1", false, 8_000, T0 + 1_000));
    m.record(sample("bus-b", "p2", true, 4_000, T0 + 2_000));

    let buses = m.bus_snapshot(T0);
    assert_eq!(buses["bus-a"].total, 2);
    assert_eq!(buses["bus-a"].success, 1);
    assert_eq!(buses["bus-a"].failed, 1);
    assert_eq!(buses["bus-a"].success_rate(), Some(50.0));
    // (2ms + 8ms) / 2 = 5ms
    assert_eq!(buses["bus-a"].avg_latency_ms(), Some(5.0));
    assert_eq!(buses["bus-b"].success_rate(), Some(100.0));
    assert_eq!(buses["bus-b"].last_success_ms, T0 + 2_000);

    let points = m.point_snapshot(T0);
    assert_eq!(points.len(), 2);
    assert_eq!(points["p1"].total, 2);
}

#[test]
fn empty_scope_is_skipped() {
    let m = ScopedCollectMetrics::default();
    m.record(sample("", "p1", true, 1_000, T0));
    assert!(m.bus_snapshot(T0).is_empty());
    assert_eq!(m.point_snapshot(T0).len(), 1);
}

#[test]
fn window_filters_old_days() {
    let m = ScopedCollectMetrics::default();
    let day_ms = DAY_MS;
    m.record(sample("bus", "p", true, 1_000, T0));
    m.record(sample("bus", "p", false, 1_000, T0 + 3 * day_ms));

    // 只看最近 1 天：仅第二个桶（失败）计入
    let recent = m.bus_snapshot(T0 + 3 * day_ms);
    assert_eq!(recent["bus"].total, 1);
    assert_eq!(recent["bus"].failed, 1);
    // 覆盖 4 天：两次都计入
    let all = m.bus_snapshot(T0);
    assert_eq!(all["bus"].total, 2);
}

#[test]
fn retention_caps_bucket_count() {
    let m = ScopedCollectMetrics::default();
    for d in 0..(RETENTION_DAYS as i64 + 5) {
        m.record(sample("bus", "p", true, 1_000, T0 + d * DAY_MS));
    }
    // 超期桶被裁剪：窗口从 T0 起算只能看到 RETENTION_DAYS 个
    let stats = m.bus_snapshot(T0);
    assert_eq!(stats["bus"].total, RETENTION_DAYS as u64);
}

#[test]
fn empty_window_rates_are_none() {
    let m = ScopedCollectMetrics::default();
    m.record(sample("bus", "p", true, 1_000, T0));
    let future = m.bus_snapshot(T0 + 10 * DAY_MS);
    assert_eq!(future["bus"].total, 0);
    assert_eq!(future["bus"].success_rate(), None);
    assert_eq!(future["bus"].avg_latency_ms(), None);
    assert_eq!(future["bus"].timeout_rate(), None);
}

/// 每总线关键指标：在线设备数 / 超时率 / 最近错误码 / 重连次数。
#[test]
fn bus_snapshot_reports_key_bus_metrics() {
    let m = ScopedCollectMetrics::default();
    // dev-1 两个点位：一个成功一个超时 → 设备仍在线
    m.record(CollectSample {
        bus: "bus-a",
        point: "p1",
        device: "dev-1",
        success: true,
        duration_us: 1_000,
        error: None,
        now_ms: T0,
    });
    m.record(CollectSample {
        bus: "bus-a",
        point: "p2",
        device: "dev-1",
        success: false,
        duration_us: 2_000,
        error: Some(CollectErrorCode::Timeout),
        now_ms: T0 + 1_000,
    });
    // dev-2 仅失败 → 离线
    m.record(CollectSample {
        bus: "bus-a",
        point: "p3",
        device: "dev-2",
        success: false,
        duration_us: 3_000,
        error: Some(CollectErrorCode::Io),
        now_ms: T0 + 2_000,
    });
    m.inc_reconnect("bus-a");

    let s = m.bus_snapshot(T0)["bus-a"];
    assert_eq!(s.online_devices, 1, "dev-1 有点位成功即在线，dev-2 离线");
    assert_eq!(s.timeouts, 1);
    // 1 / 3 超时
    assert_eq!(s.timeout_rate(), Some(33.3));
    assert_eq!(s.last_error_code, Some(CollectErrorCode::Io));
    assert_eq!(s.last_error_code.unwrap().code(), "IO");
    assert_eq!(s.reconnects, 1);
    // 点位维度不携带设备/重连信息
    let point = m.point_snapshot(T0)["p1"];
    assert_eq!(point.online_devices, 0);
    assert_eq!(point.reconnects, 0);
}

/// 成功后保留「最近错误码」，便于排查已恢复的抖动。
#[test]
fn last_error_code_survives_recovery() {
    let m = ScopedCollectMetrics::default();
    m.record(CollectSample {
        bus: "bus",
        point: "p",
        device: "",
        success: false,
        duration_us: 1_000,
        error: Some(CollectErrorCode::ModbusException(2)),
        now_ms: T0,
    });
    m.record(sample("bus", "p", true, 1_000, T0 + 1_000));
    let s = m.bus_snapshot(T0)["bus"];
    assert_eq!(
        s.last_error_code,
        Some(CollectErrorCode::ModbusException(2))
    );
    assert_eq!(
        s.last_error_code.unwrap().code(),
        "EXC_02",
        "Modbus 异常码必须保留原始码值"
    );
    assert_eq!(s.timeouts, 0);
}

// ===== 四指标口径锁定 =====

/// 错误码 → 机器可读字符串的映射必须稳定（前端按此展示「最近错误码」）。
///
/// 口径一旦漂移，历史报表与告警规则里的错误码就全部对不上，故在此锁死。
#[test]
fn error_code_strings_are_stable() {
    assert_eq!(CollectErrorCode::Timeout.code(), "TIMEOUT");
    assert_eq!(CollectErrorCode::NoReply.code(), "NO_REPLY");
    assert_eq!(CollectErrorCode::Io.code(), "IO");
    assert_eq!(CollectErrorCode::Checksum.code(), "CRC");
    assert_eq!(CollectErrorCode::Decode.code(), "DECODE");
    assert_eq!(CollectErrorCode::BusClosed.code(), "BUS_CLOSED");
    assert_eq!(CollectErrorCode::Busy.code(), "BUSY");
    assert_eq!(CollectErrorCode::Config.code(), "CONFIG");
    assert_eq!(CollectErrorCode::Other.code(), "OTHER");
    // Modbus 异常码必须保留原始码值（十六进制、大写、两位）
    assert_eq!(CollectErrorCode::ModbusException(2).code(), "EXC_02");
    assert_eq!(CollectErrorCode::ModbusException(0x0B).code(), "EXC_0B");
}

/// 超时率口径：超时是 `failed` 的子集，按**总采集次数**为分母。
///
/// 10 次采集里 3 次超时 → 30%；非超时的失败（IO 错误）不计入超时。
#[test]
fn timeout_rate_counts_only_timeout_failures() {
    let m = ScopedCollectMetrics::default();
    for i in 0..6 {
        m.record(sample("bus", "p", true, 1_000, T0 + i * 10));
    }
    for i in 0..3 {
        m.record(fail_sample(
            "bus",
            "p",
            CollectErrorCode::Timeout,
            T0 + 100 + i * 10,
        ));
    }
    // 1 次非超时失败：不计入超时，但计入 failed
    m.record(fail_sample("bus", "p", CollectErrorCode::Io, T0 + 200));

    let s = m.bus_snapshot(T0)["bus"];
    assert_eq!(s.total, 10);
    assert_eq!(s.failed, 4);
    assert_eq!(s.timeouts, 3, "只有 Timeout 计超时");
    assert_eq!(s.timeout_rate(), Some(30.0));
}

/// 超时率在窗口内无采集时必须为 `None`（不得用 0% 冒充「无超时」）。
#[test]
fn timeout_rate_is_none_without_samples() {
    let m = ScopedCollectMetrics::default();
    m.record(fail_sample("bus", "p", CollectErrorCode::Timeout, T0));
    // 取一个未来窗口：该总线没有落在窗口内的采集
    let future = m.bus_snapshot(T0 + 10 * DAY_MS);
    assert_eq!(future["bus"].total, 0);
    assert_eq!(future["bus"].timeout_rate(), None);
}

/// 在线设备数：设备任一点位最近一次采集成功即视为在线；
/// 该设备所有点位都失败 → 离线。
#[test]
fn online_devices_follow_latest_point_result() {
    let m = ScopedCollectMetrics::default();
    // dev1：p1 成功 → 在线
    m.record(dev_sample("bus", "p1", "dev1", true, T0));
    // dev2：先成功后失败 → 最近一次失败 → 离线
    m.record(dev_sample("bus", "p2", "dev2", true, T0));
    m.record(dev_sample("bus", "p2", "dev2", false, T0 + 1));
    // dev3：从未成功 → 离线
    m.record(dev_sample("bus", "p3", "dev3", false, T0 + 2));

    let s = m.bus_snapshot(T0)["bus"];
    assert_eq!(s.online_devices, 1, "仅 dev1 有最近一次成功的点位");
}

/// 在线设备数按**设备**去重：同一设备的多个点位只计 1 台。
#[test]
fn online_devices_deduplicate_multiple_points() {
    let m = ScopedCollectMetrics::default();
    for i in 0..3 {
        m.record(dev_sample("bus", &format!("p{i}"), "dev1", true, T0 + i));
    }
    let s = m.bus_snapshot(T0)["bus"];
    assert_eq!(s.online_devices, 1, "同一设备的 3 个点位只算 1 台在线");
}

/// 未知设备（device 为空串）不参与在线设备计数。
#[test]
fn samples_without_device_do_not_count_online() {
    let m = ScopedCollectMetrics::default();
    m.record(sample("bus", "p", true, 1_000, T0));
    let s = m.bus_snapshot(T0)["bus"];
    assert_eq!(s.online_devices, 0, "设备未知的采集不得计入在线设备数");
    assert_eq!(s.success, 1, "但成功计数照常");
}

/// 重连次数是**累计值**：跨窗口不重置，且只影响总线维度。
#[test]
fn reconnect_count_accumulates_across_windows() {
    let m = ScopedCollectMetrics::default();
    m.inc_reconnect("bus");
    m.inc_reconnect("bus");
    m.inc_reconnect("bus");
    // 空总线名不建条目
    m.inc_reconnect("");

    let now = m.bus_snapshot(T0);
    assert_eq!(now["bus"].reconnects, 3);
    // 取一个没有任何采集的窗口：重连次数仍应保留（累计语义）
    let future = m.bus_snapshot(T0 + 10 * DAY_MS);
    assert_eq!(
        future["bus"].reconnects, 3,
        "重连次数是累计值，不随窗口清零"
    );
    assert_eq!(future["bus"].total, 0);
}

/// 最近错误码只取**最近一次失败**的码：先失败后成功 → 保留失败码（便于排查抖动）。
#[test]
fn last_error_code_keeps_the_latest_failure() {
    let m = ScopedCollectMetrics::default();
    m.record(fail_sample("bus", "p", CollectErrorCode::Checksum, T0));
    m.record(fail_sample("bus", "p", CollectErrorCode::Timeout, T0 + 1));
    let s = m.bus_snapshot(T0)["bus"];
    assert_eq!(
        s.last_error_code,
        Some(CollectErrorCode::Timeout),
        "应取最近一次失败的错误码"
    );
}

/// 失败但没有错误码（如质量码 Bad）时，不得覆盖已有的具体错误码。
#[test]
fn failure_without_code_keeps_previous_error_code() {
    let m = ScopedCollectMetrics::default();
    m.record(fail_sample("bus", "p", CollectErrorCode::Decode, T0));
    // 无错误码的失败（error=None）
    m.record(sample("bus", "p", false, 1_000, T0 + 1));
    let s = m.bus_snapshot(T0)["bus"];
    assert_eq!(
        s.last_error_code,
        Some(CollectErrorCode::Decode),
        "无错误码的失败不得把已知错误码清空"
    );
}

/// 总线之间互不串扰：每条总线的四指标独立统计。
#[test]
fn bus_metrics_are_isolated_per_bus() {
    let m = ScopedCollectMetrics::default();
    m.record(dev_sample("bus-a", "p1", "dev1", true, T0));
    m.record(fail_sample("bus-b", "p1", CollectErrorCode::Timeout, T0));
    m.inc_reconnect("bus-b");

    let snap = m.bus_snapshot(T0);
    assert_eq!(snap["bus-a"].online_devices, 1);
    assert_eq!(snap["bus-a"].timeouts, 0);
    assert_eq!(snap["bus-a"].last_error_code, None);
    assert_eq!(snap["bus-a"].reconnects, 0);

    assert_eq!(snap["bus-b"].online_devices, 0);
    assert_eq!(snap["bus-b"].timeouts, 1);
    assert_eq!(snap["bus-b"].timeout_rate(), Some(100.0));
    assert_eq!(
        snap["bus-b"].last_error_code,
        Some(CollectErrorCode::Timeout)
    );
    assert_eq!(snap["bus-b"].reconnects, 1);
}

// ===== by_reason 标签口径 =====

/// `reason_index` 必须把每个错误码映射到唯一且不越界的桶下标。
///
/// 桶下标一旦漂移，历史时序与 Grafana 图例就对不上，故在此锁死。
#[test]
fn reason_index_covers_unique_buckets() {
    let codes = [
        CollectErrorCode::Timeout,
        CollectErrorCode::NoReply,
        CollectErrorCode::Io,
        CollectErrorCode::Checksum,
        CollectErrorCode::Decode,
        CollectErrorCode::BusClosed,
        CollectErrorCode::Busy,
        CollectErrorCode::ModbusException(2),
        CollectErrorCode::Config,
        CollectErrorCode::Other,
    ];
    let mut indices: Vec<usize> = codes.iter().map(|c| c.reason_index()).collect();
    assert!(
        indices.iter().all(|i| *i < REASON_LABELS.len()),
        "reason_index 越界：{:?}",
        indices
    );
    // 10 个错误码必须恰好铺满 REASON_LABELS 的 10 个桶，不得重复
    indices.sort_unstable();
    let expected: Vec<usize> = (0..REASON_LABELS.len()).collect();
    assert_eq!(indices, expected, "reason_index 未覆盖全部桶或存在重复映射");
    // 标签值必须互不相同（否则不同原因会落到同一时序）
    let mut labels: Vec<&str> = REASON_LABELS.to_vec();
    labels.sort_unstable();
    let before = labels.len();
    labels.dedup();
    assert_eq!(labels.len(), before, "REASON_LABELS 存在重复取值");
}

/// `from_code` 必须与 `code()` 互为逆运算（Modbus 异常码除外，其异常码原值不可还原）。
#[test]
fn from_code_reverses_code() {
    for code in [
        CollectErrorCode::Timeout,
        CollectErrorCode::NoReply,
        CollectErrorCode::Io,
        CollectErrorCode::Checksum,
        CollectErrorCode::Decode,
        CollectErrorCode::BusClosed,
        CollectErrorCode::Busy,
        CollectErrorCode::Config,
        CollectErrorCode::Other,
    ] {
        assert_eq!(
            CollectErrorCode::from_code(&code.code()),
            Some(code),
            "from_code 应还原 {}",
            code.code()
        );
    }
    // Modbus 异常码：只还原到「异常」这一类，原始码值不还原
    assert_eq!(
        CollectErrorCode::from_code("EXC_02"),
        Some(CollectErrorCode::ModbusException(2))
    );
    // 未知错误码返回 None，由调用方回退到 Other
    assert_eq!(CollectErrorCode::from_code("NOT_A_CODE"), None);
    assert_eq!(CollectErrorCode::from_code("EXC_ZZ"), None);
}

/// `ReasonCounters` 按原因分桶累加，且导出时标签集合恒定（未发生的原因为 0）。
#[test]
fn reason_counters_bucket_and_render() {
    let c = ReasonCounters::default();
    c.inc(CollectErrorCode::Timeout);
    c.inc(CollectErrorCode::Timeout);
    c.inc(CollectErrorCode::Io);
    c.inc(CollectErrorCode::ModbusException(3));

    assert_eq!(c.load(CollectErrorCode::Timeout), 2);
    assert_eq!(c.load(CollectErrorCode::Io), 1);
    assert_eq!(c.load(CollectErrorCode::ModbusException(3)), 1);
    assert_eq!(c.load(CollectErrorCode::Busy), 0);

    let mut buf = String::new();
    c.render("gateway_collect_failures", &mut buf);
    assert!(buf.contains("gateway_collect_failures{by_reason=\"timeout\"} 2"));
    assert!(buf.contains("gateway_collect_failures{by_reason=\"io\"} 1"));
    assert!(buf.contains("gateway_collect_failures{by_reason=\"protocol_exception\"} 1"));
    // 标签集合恒定：未发生的原因为 0 也必须输出
    assert!(buf.contains("gateway_collect_failures{by_reason=\"busy\"} 0"));
    // 每个标签值恰好一行
    assert_eq!(
        buf.lines()
            .filter(|l| l.starts_with("gateway_collect_failures{"))
            .count(),
        REASON_LABELS.len()
    );
}

/// `ScopedCollectMetrics::render()`：只导出有失败的点位，并给出失败点总数。
#[test]
fn scoped_render_exports_failing_points_only() {
    let m = ScopedCollectMetrics::default();
    m.record(sample("bus", "ok-point", true, 1_000, T0));
    m.record(fail_sample(
        "bus",
        "bad-point",
        CollectErrorCode::Timeout,
        T0,
    ));
    m.record(fail_sample(
        "bus",
        "bad-point",
        CollectErrorCode::Timeout,
        T0 + 1,
    ));

    let mut buf = String::new();
    m.render(&mut buf);
    assert!(buf.contains("gateway_collect_failures_scoped{point=\"bad-point\"} 2"));
    assert!(!buf.contains("ok-point"), "无失败点位不得导出：\n{}", buf);
    assert!(buf.contains("gateway_collect_scoped_points_failing 1"));
}
