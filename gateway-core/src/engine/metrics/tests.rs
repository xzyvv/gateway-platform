use super::*;
use std::sync::atomic::Ordering;

#[test]
fn test_export_contains_alias_metrics() {
    let m = Metrics::new();
    let out = m.prometheus_export();
    assert!(out.contains("gateway_uplink_total"));
    assert!(out.contains("gateway_queue_length"));
    assert!(out.contains("gateway_devices_online"));
    assert!(out.contains("gateway_errors_total"));
}

/// #5/#6：死信处置链路（重投 / 重投冲突 / 主动清理 / 自动裁剪）必须有指标。
///
/// 这几个数字此前只存在于 handler 的 200/409 响应里，Grafana 上完全不可见：
/// 「有人一直点重投但一直失败」和「后台正在悄悄裁剪数据」都无法被发现。
#[test]
fn test_export_contains_dead_letter_disposal_metrics() {
    let m = Metrics::new();
    m.inc_dead_letter_replay();
    m.inc_dead_letter_replay_conflict();
    m.add_dead_letter_purged(5);
    m.record_dead_letter_trim(9);
    let out = m.prometheus_export();
    assert!(out.contains("gateway_dead_letter_replay_total 1"), "{out}");
    assert!(
        out.contains("gateway_dead_letter_replay_conflict_total 1"),
        "{out}"
    );
    assert!(out.contains("gateway_dead_letter_purged_total 5"), "{out}");
    assert!(out.contains("gateway_dead_letter_trimmed_total 9"), "{out}");
    assert!(
        out.contains("gateway_dead_letter_last_trim_rows 9"),
        "{out}"
    );
    // 时间戳是 gauge，必须落在「当下附近」而不是 0（0 表示从未裁剪）
    let ts = m.dead_letter_trim_stats().1;
    assert!(
        ts > 1_600_000_000_000,
        "last_trim_ms 必须写入真实时间: {ts}"
    );
}

/// 回归：`queue_persist` 子结构的 render 此前从未被 `prometheus_export` 调用，
/// 导致队列持久化 / WAL / outbox 降级指标在 `/api/metrics` 上完全不可见。
#[test]
fn test_export_contains_queue_persist_and_wal_gauges() {
    let m = Metrics::new();
    m.queue_persist.persist_dropped.store(3, Ordering::Relaxed);
    m.queue_persist
        .wal_size_bytes
        .store(1234, Ordering::Relaxed);
    m.queue_persist.db_size_bytes.store(5678, Ordering::Relaxed);
    let out = m.prometheus_export();
    assert!(out.contains("gateway_queue_persist_dropped_total 3"));
    assert!(out.contains("gateway_wal_size_bytes 1234"));
    assert!(out.contains("gateway_sqlite_db_size_bytes 5678"));
}

#[test]
fn test_uplink_total_is_sum_of_successes() {
    let m = Metrics::new();
    m.mqtt.publish_success.store(42, Ordering::Relaxed);
    m.webhook.publish_success.store(8, Ordering::Relaxed);
    let out = m.prometheus_export();
    // uplink_total = 42 + 8 = 50
    assert!(out.contains("gateway_uplink_total 50"));
}

#[test]
fn test_queue_length_alias() {
    let m = Metrics::new();
    m.queue_depth.store(1234, Ordering::Relaxed);
    let out = m.prometheus_export();
    assert!(out.contains("gateway_queue_length 1234"));
}

#[test]
fn test_devices_online_alias() {
    let m = Metrics::new();
    m.points_online.store(56, Ordering::Relaxed);
    let out = m.prometheus_export();
    assert!(out.contains("gateway_devices_online 56"));
}

#[test]
fn test_point_write_counters() {
    let m = Metrics::new();
    assert_eq!(m.point_write_total.load(Ordering::Relaxed), 0);
    assert_eq!(m.point_write_failures.load(Ordering::Relaxed), 0);
    m.record_point_write(true);
    m.record_point_write(true);
    m.record_point_write(false);
    let out = m.prometheus_export();
    assert!(out.contains("gateway_point_write_total 3"));
    assert!(out.contains("gateway_point_write_error_total 1"));
}

#[test]
fn test_errors_total_is_sum() {
    let m = Metrics::new();
    m.collect.serial_errors.store(3, Ordering::Relaxed);
    m.collect.collect_failures.store(5, Ordering::Relaxed);
    m.lifecyle.task_panics.store(2, Ordering::Relaxed);
    let out = m.prometheus_export();
    // errors_total = 3 + 5 + 2 = 10
    assert!(out.contains("gateway_errors_total 10"));
}

#[test]
fn test_sink_send_total_increments() {
    let m = Metrics::new();
    m.inc_sink_send("opcua");
    m.inc_sink_send("opcua");
    m.inc_sink_send("modbus");
    assert_eq!(m.sink_send_total("opcua"), 2);
    assert_eq!(m.sink_send_total("modbus"), 1);
    assert_eq!(m.sink_send_total("unknown"), 0);
}

#[test]
fn test_sink_dropped_total_increments() {
    let m = Metrics::new();
    m.inc_sink_dropped("opcua");
    m.inc_sink_dropped("opcua");
    m.inc_sink_dropped("opcua");
    assert_eq!(m.sink_dropped_total("opcua"), 3);
}

#[test]
fn test_sink_channel_depth_set() {
    let m = Metrics::new();
    m.set_sink_channel_depth("modbus", 42);
    assert_eq!(m.sink_channel_depth("modbus"), 42);
    // 负值被 clamp 到 0
    m.set_sink_channel_depth("modbus", -5);
    assert_eq!(m.sink_channel_depth("modbus"), 0);
}

#[test]
fn test_sink_names_lists_sorted() {
    let m = Metrics::new();
    m.inc_sink_send("modbus");
    m.inc_sink_send("opcua");
    let names = m.sink_names();
    assert_eq!(names, vec!["modbus", "opcua"]);
}

#[test]
fn test_prometheus_export_includes_sink_metrics() {
    let m = Metrics::new();
    m.inc_sink_send("opcua");
    m.inc_sink_send("opcua");
    m.inc_sink_dropped("opcua");
    m.set_sink_channel_depth("opcua", 7);
    let out = m.prometheus_export();
    assert!(out.contains("gateway_sink_send_total"));
    assert!(out.contains("sink_dropped_total"));
    assert!(out.contains("sink_channel_depth"));
    // 也验证实际数值（避免硬编码 "opcua" 转义）
    assert!(out.contains("opcua"));
}

/// r31-PM1 (回归)：prometheus_export 的 sink 段输出顺序按 sink 名字典序，
/// 且三个 map（send/dropped/channel_depth）的 key 合并后都被输出。
/// 旧实现用 `Vec + sort()`，新实现用 `BTreeSet`（天然排序）。验证 BTreeSet 行为。
#[test]
fn test_prometheus_export_sink_names_sorted_and_unioned() {
    let m = Metrics::new();
    // 故意按非字典序插入（modbus > opcua > bacnet）
    m.inc_sink_send("modbus");
    m.inc_sink_send("opcua");
    m.inc_sink_send("bacnet");
    m.inc_sink_dropped("opcua");
    m.set_sink_channel_depth("bacnet", 1);
    m.set_sink_channel_depth("modbus", 2);
    // 故意让"send"和"channel_depth"都有"opcua"，确保 union 后只出现一次
    let out = m.prometheus_export();

    // 三个 sink 名都应出现在 dropped 段和 channel_depth 段
    for name in &["bacnet", "opcua", "modbus"] {
        assert!(
            out.contains(&format!("gateway_sink_dropped_total{{sink=\"{}\"}}", name)),
            "dropped 段缺 {}：\n{}",
            name,
            out
        );
        assert!(
            out.contains(&format!("gateway_sink_channel_depth{{sink=\"{}\"}}", name)),
            "depth 段缺 {}：\n{}",
            name,
            out
        );
    }
    // 字典序：bacnet < modbus < opcua
    let pos_b = out.find("bacnet").expect("bacnet");
    let pos_m = out.find("modbus").expect("modbus");
    let pos_o = out.find("opcua").expect("opcua");
    assert!(pos_b < pos_m, "bacnet 应在 modbus 之前");
    assert!(pos_m < pos_o, "modbus 应在 opcua 之前");
}

/// r32-PM2 (回归)：prometheus_export 走本地 HashMap snapshot 后，输出值与
/// 直接调 sink_*_total() 的返回值一致。验证 snapshot 不会丢数据。
#[test]
fn test_prometheus_export_sink_snapshots_match_direct_reads() {
    let m = Metrics::new();
    // 三个 map 分别灌不同 sink 名 + 数值，确保 union 完整
    m.inc_sink_send("modbus");
    m.inc_sink_send("modbus");
    m.inc_sink_send("opcua");
    m.inc_sink_dropped("opcua");
    m.inc_sink_dropped("opcua");
    m.inc_sink_dropped("opcua");
    m.set_sink_channel_depth("modbus", 42);
    m.set_sink_channel_depth("bacnet", 7); // i64 路径

    let out = m.prometheus_export();

    // 直接调方法拿值（这些是测试白盒接口，不走 snapshot 路径）
    assert_eq!(m.sink_send_total("modbus"), 2);
    assert_eq!(m.sink_send_total("opcua"), 1);
    assert_eq!(m.sink_dropped_total("modbus"), 0); // send 段有，dropped 段无 → 应输出 0
    assert_eq!(m.sink_dropped_total("opcua"), 3);
    assert_eq!(m.sink_channel_depth("modbus"), 42);
    assert_eq!(m.sink_channel_depth("bacnet"), 7);

    // 验证 prometheus_export 输出值与上面一致
    assert!(
        out.contains("gateway_sink_send_total{sink=\"modbus\"} 2"),
        "send modbus=2：\n{}",
        out
    );
    assert!(
        out.contains("gateway_sink_send_total{sink=\"opcua\"} 1"),
        "send opcua=1：\n{}",
        out
    );
    // dropped 段：modbus 不在 dropped map 但在 send → 仍应输出 0
    assert!(
        out.contains("gateway_sink_dropped_total{sink=\"modbus\"} 0"),
        "dropped modbus 应为 0：\n{}",
        out
    );
    assert!(
        out.contains("gateway_sink_dropped_total{sink=\"opcua\"} 3"),
        "dropped opcua=3：\n{}",
        out
    );
    // depth 段：bacnet 在 depth 但不在 send → 应输出 7
    assert!(
        out.contains("gateway_sink_channel_depth{sink=\"bacnet\"} 7"),
        "depth bacnet=7：\n{}",
        out
    );
    assert!(
        out.contains("gateway_sink_channel_depth{sink=\"modbus\"} 42"),
        "depth modbus=42：\n{}",
        out
    );
}

#[test]
fn test_zero_values_when_no_activity() {
    let m = Metrics::new();
    let out = m.prometheus_export();
    assert!(out.contains("gateway_uplink_total 0"));
    assert!(out.contains("gateway_errors_total 0"));
}

/// record_collect 应正确跟踪采集总次数、失败次数、耗时累计
#[test]
fn test_record_collect_tracks_success_and_failure() {
    let m = Metrics::new();
    // 3 次成功 + 2 次失败
    m.record_collect(true, 100);
    m.record_collect(true, 200);
    m.record_collect(true, 300);
    m.record_collect(false, 50);
    m.record_collect(false, 150);
    assert_eq!(
        m.collect.collect_total.load(Ordering::Relaxed),
        5,
        "collect_total 应为 5"
    );
    assert_eq!(
        m.collect.collect_failures.load(Ordering::Relaxed),
        2,
        "collect_failures 应为 2"
    );
    // duration sum = 100+200+300+50+150 = 800
    assert_eq!(m.collect.duration.sum(), 800, "duration_sum 应为 800");
    assert_eq!(m.collect.duration.count(), 5, "duration_count 应为 5");
}

/// 采集失败按 `by_reason` 拆分导出：分桶总和恒等于失败总量。
#[test]
fn test_collect_failures_exported_by_reason() {
    let m = Metrics::new();
    m.record_collect_with_reason(false, 10, Some(CollectErrorCode::Timeout));
    m.record_collect_with_reason(false, 10, Some(CollectErrorCode::Timeout));
    m.record_collect_with_reason(false, 10, Some(CollectErrorCode::Io));
    // 无错误码的失败归入 other 桶
    m.record_collect_with_reason(false, 10, None);
    m.record_collect_with_reason(true, 10, None);

    assert_eq!(m.collect.collect_failures.load(Ordering::Relaxed), 4);
    assert_eq!(
        m.collect.failures_by_reason.load(CollectErrorCode::Timeout),
        2
    );
    assert_eq!(m.collect.failures_by_reason.load(CollectErrorCode::Io), 1);
    assert_eq!(
        m.collect.failures_by_reason.load(CollectErrorCode::Other),
        1
    );

    let out = m.prometheus_export();
    assert!(out.contains("gateway_collect_failures{by_reason=\"timeout\"} 2"));
    assert!(out.contains("gateway_collect_failures{by_reason=\"io\"} 1"));
    assert!(out.contains("gateway_collect_failures{by_reason=\"other\"} 1"));
    // 成功不计入任何失败桶
    assert!(out.contains("gateway_collect_failures{by_reason=\"crc\"} 0"));
}

/// R2: ConnAck 被拒计数应出现在 Prometheus 导出中
#[test]
fn test_prometheus_export_includes_connack_refused() {
    let m = Metrics::new();
    m.inc_mqtt_connack_refused();
    m.inc_mqtt_connack_refused();
    let out = m.prometheus_export();
    assert!(out.contains("gateway_mqtt_connack_refused_total 2"));
}

/// X1 (反证闭环)：`pipeline` 子结构的 render 此前从未在 `prometheus_export`
/// 中被调用——`gateway_pipeline_submitted_total` / `gateway_pipeline_dropped_total`
/// 会被无声地吞掉，运维侧只能用 logs 找数据丢失。
///
/// 反证：删掉 [prometheus.rs](`crate::engine::metrics::prometheus`) 里
/// `self.pipeline.render(&mut out);` 这一行，下方 `gateway_pipeline_dropped_total 5`
/// 断言立刻失败。
#[test]
fn test_export_contains_pipeline_metrics() {
    let m = Metrics::new();
    // 直接走子结构字段——保证子结构 API 与 Prometheus 渲染契约一致
    m.pipeline.dropped.store(5, Ordering::Relaxed);
    m.pipeline.submitted.store(8, Ordering::Relaxed);
    let out = m.prometheus_export();
    assert!(
        out.contains("gateway_pipeline_dropped_total 5"),
        "X1: 导出口径必须能透出 dropped 子结构值；实际输出片段:\n{}",
        out
    );
    assert!(
        out.contains("gateway_pipeline_submitted_total 8"),
        "X1: 导出口径必须能透出 submitted 子结构值"
    );
    // 防止重命名 / 把两条合并写漏一个
    let help_lines: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("# HELP gateway_pipeline_"))
        .collect();
    assert_eq!(
        help_lines.len(),
        2,
        "X1: 应恰好有 2 条 pipeline HELP 行，实际 {}",
        help_lines.len()
    );
}

/// X1 (反证闭环)：直接 inc 子结构字段，配合 export 验证渲染路径。
#[test]
fn test_pipeline_metrics_render_handles_large_values() {
    let m = Metrics::new();
    // 大值（Avoid rolling-over 60-year-uptime scenario 仿真）
    m.pipeline.dropped.store(u64::MAX / 2, Ordering::Relaxed);
    let out = m.prometheus_export();
    assert!(out.contains(&format!("gateway_pipeline_dropped_total {}", u64::MAX / 2)));
}

/// X2 (反证闭环)：facade `inc_mqtt_outbox_sync_fallback` 与 Prometheus
/// 导出契约——一旦改 facade 不再 inc，将导致 rate 告警永远为 0。
///
/// 反证：把 facade body 改成 `let _ = self.mqtt.outbox_sync_fallback;`
/// （保留读、移除写）→ 下方 2 个 inc 后导出仍是 0，断言失败。
#[test]
fn test_mqtt_outbox_sync_fallback_counter() {
    let m = Metrics::new();
    m.inc_mqtt_outbox_sync_fallback();
    m.inc_mqtt_outbox_sync_fallback();
    // 子结构字段也按同步 +1，便于直接白盒校验
    assert_eq!(m.mqtt.outbox_sync_fallback.load(Ordering::Relaxed), 2);
    let out = m.prometheus_export();
    assert!(
        out.contains("gateway_mqtt_outbox_sync_fallback_total 2"),
        "X2: 导出口径必须能透出 fallback 计数；实际输出:\n{}",
        out
    );
}

/// 决策表 #25：单事务往返延迟埋点应正确累计、分桶并导出
#[test]
fn test_record_transaction_latency_and_export() {
    let m = Metrics::new();
    m.record_transaction(true, 150); // 落入 <=200 桶
    m.record_transaction(true, 40_000); // 落入 <=50000 桶
    m.record_transaction(false, 6_000_000); // 超出 5s → +Inf 桶，且计入失败
    assert_eq!(m.transaction.duration.count(), 3);
    assert_eq!(m.transaction.duration.sum(), 150 + 40_000 + 6_000_000);
    assert_eq!(m.transaction.failures.load(Ordering::Relaxed), 1);
    // 分桶：<=200 有 1 笔
    assert_eq!(m.transaction.duration.bucket(0), 1);
    // 最后一个桶（+Inf）承接超 5s 的那笔
    assert_eq!(m.transaction.duration.bucket(9), 1);

    let out = m.prometheus_export();
    assert!(out.contains("# TYPE gateway_bus_transaction_duration_us histogram"));
    assert!(out.contains("gateway_bus_transaction_duration_us_count 3"));
    // 事务失败已按原因标签化：未带错误码的失败落 `other` 桶
    assert!(
        out.contains("gateway_bus_transaction_failures_total{by_reason=\"other\"} 1"),
        "事务失败应导出为带 by_reason 标签的序列；实际输出:\n{}",
        out
    );
    // 累积语义：+Inf 桶应等于总笔数
    assert!(out.contains("gateway_bus_transaction_duration_us_bucket{le=\"+Inf\"} 3"));
}

// ===== 每总线四指标：Metrics facade 写入路径 =====

/// `record_collect_scoped` 必须同时写到「全局采集计数」与「总线/点位维度」，
/// 且错误码、设备归属一并透传（否则四指标无从统计）。
#[test]
fn test_record_collect_scoped_fills_both_dimensions() {
    let m = Metrics::new();
    m.record_collect_scoped("bus-a", "p1", "dev1", true, 1_000, None);
    m.record_collect_scoped(
        "bus-a",
        "p2",
        "dev1",
        false,
        2_000,
        Some(CollectErrorCode::Timeout),
    );

    // 全局维度
    assert_eq!(m.collect.collect_total.load(Ordering::Relaxed), 2);
    assert_eq!(m.collect.collect_failures.load(Ordering::Relaxed), 1);

    // 总线维度
    let bus = &m.scoped.bus_snapshot(0)["bus-a"];
    assert_eq!(bus.total, 2);
    assert_eq!(bus.success, 1);
    assert_eq!(bus.failed, 1);
    assert_eq!(bus.timeouts, 1);
    assert_eq!(bus.timeout_rate(), Some(50.0));
    assert_eq!(bus.last_error_code, Some(CollectErrorCode::Timeout));
    assert_eq!(
        bus.online_devices, 1,
        "dev1 的 p1 最近一次成功 → 整台设备仍在线"
    );

    // 点位维度
    let point = &m.scoped.point_snapshot(0)["p1"];
    assert_eq!(point.total, 1);
    assert_eq!(point.online_devices, 0, "点位维度不统计设备");
    assert_eq!(point.reconnects, 0, "点位维度不统计重连");
}

/// `inc_bus_reconnect` 只累计到总线维度，且是累计值（多次调用递增）。
#[test]
fn test_inc_bus_reconnect_accumulates_on_bus_dimension() {
    let m = Metrics::new();
    m.inc_bus_reconnect("bus-a");
    m.inc_bus_reconnect("bus-a");

    assert_eq!(m.scoped.bus_snapshot(0)["bus-a"].reconnects, 2);
    assert!(
        m.scoped.point_snapshot(0).is_empty(),
        "重连不得污染点位维度"
    );
}

/// 空总线名 / 空点位名的写入不应污染任何维度。
#[test]
fn test_record_collect_scoped_skips_empty_scope() {
    let m = Metrics::new();
    m.record_collect_scoped("", "", "", true, 1_000, None);
    assert!(m.scoped.bus_snapshot(0).is_empty());
    assert!(m.scoped.point_snapshot(0).is_empty());
    // 全局采集计数仍应累计（这是全局事实，与维度无关）
    assert_eq!(m.collect.collect_total.load(Ordering::Relaxed), 1);
}
