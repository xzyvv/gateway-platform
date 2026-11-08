//! 健康报告问题诊断（`build_health_problems`）的回归测试。
//!
//! 单独成文件（而非内联在 `status.rs` 尾部）的原因：这些用例覆盖的是
//! 「什么状态算问题、问题算哪一级」的业务判定，改动频率与 handler 主体不同；
//! 混在一个近千行的文件里既拉高行数预算，也让 review 时看不到边界。

use super::*;

fn bus(name: &str, total: usize, online: usize, reads: u64, rate: Option<f64>) -> BusHealth {
    BusHealth {
        bus: name.to_string(),
        protocol: "modbus".to_string(),
        total_devices: total,
        online_devices: online,
        total_reads: reads,
        success: reads,
        success_rate: rate,
        avg_latency_ms: None,
        timeout_rate: None,
        last_error_code: None,
        reconnect_count: 0,
    }
}

fn codes(input: &HealthProblemInput<'_>) -> Vec<String> {
    build_health_problems(input)
        .iter()
        .map(|p| p["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn healthy_gateway_has_no_problems() {
    let buses = vec![bus("/dev/ttyS1", 2, 2, 100, Some(99.5))];
    let input = HealthProblemInput {
        mqtt_configured: true,
        mqtt_connected: true,
        has_north_consumer: true,
        max_queue_rows: 1_000_000,
        queue_len: 10,
        ota_state: "Idle",
        buses: &buses,
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    assert!(codes(&input).is_empty());
}

#[test]
fn mqtt_disconnected_is_critical() {
    let input = HealthProblemInput {
        mqtt_configured: true,
        mqtt_connected: false,
        has_north_consumer: true,
        max_queue_rows: 1_000_000,
        queue_len: 0,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    assert!(codes(&input).contains(&"MQTT_DISCONNECTED".to_string()));
}

#[test]
fn all_devices_offline_is_critical() {
    let buses = vec![bus("/dev/ttyS1", 3, 0, 50, Some(0.0))];
    let input = HealthProblemInput {
        mqtt_configured: false,
        mqtt_connected: false,
        has_north_consumer: false,
        max_queue_rows: 1_000_000,
        queue_len: 0,
        ota_state: "Idle",
        buses: &buses,
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    assert!(codes(&input).contains(&"BUS_ALL_OFFLINE".to_string()));
}

#[test]
fn partial_offline_is_warning_and_low_rate_skipped_when_sample_small() {
    let buses = vec![bus("/dev/ttyS1", 3, 2, 5, Some(10.0))];
    let input = HealthProblemInput {
        mqtt_configured: false,
        mqtt_connected: false,
        has_north_consumer: false,
        max_queue_rows: 1_000_000,
        queue_len: 0,
        ota_state: "Idle",
        buses: &buses,
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    let got = codes(&input);
    assert!(got.contains(&"BUS_PARTIAL_OFFLINE".to_string()));
    // 只有 5 次采集，成功率再低也不下结论
    assert!(!got.contains(&"BUS_LOW_SUCCESS_RATE".to_string()));
}

#[test]
fn empty_bus_does_not_report_offline() {
    // 配了总线但没有任何设备点位落在上面：不能报"全部离线"
    let buses = vec![bus("/dev/ttyS2", 0, 0, 0, None)];
    let input = HealthProblemInput {
        mqtt_configured: false,
        mqtt_connected: false,
        has_north_consumer: false,
        max_queue_rows: 1_000_000,
        queue_len: 0,
        ota_state: "Idle",
        buses: &buses,
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    assert!(!codes(&input).contains(&"BUS_ALL_OFFLINE".to_string()));
}

/// P0-1 回归：同一队列长度（直连调试的 14.9 万条）在有无北向消费者时
/// 必须给出不同结论——无消费者只是本地历史存量（warning），
/// 有消费者才是背压（critical）。
#[test]
fn queue_problem_uses_single_health_verdict() {
    // 无消费者 + 149_668 条：warning，且不是背压
    let no_consumer = HealthProblemInput {
        mqtt_configured: false,
        mqtt_connected: true,
        has_north_consumer: false,
        max_queue_rows: 1_000_000,
        queue_len: 149_668,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    let got = codes(&no_consumer);
    assert!(got.contains(&"QUEUE_LOCAL_HISTORY".to_string()));
    assert!(!got.contains(&"QUEUE_BACKPRESSURE".to_string()));

    // 有消费者 + 同一长度：critical 背压
    let with_consumer = HealthProblemInput {
        mqtt_configured: true,
        mqtt_connected: true,
        has_north_consumer: true,
        max_queue_rows: 1_000_000,
        queue_len: 149_668,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    let got = codes(&with_consumer);
    assert!(got.contains(&"QUEUE_BACKPRESSURE".to_string()));
}

/// 语义拆分回归：内置模拟消费者**没有 MQTT 客户端**，但它照样 dequeue → ack
/// 队列。因此队列等级判定必须看 `has_north_consumer`，不能看 `mqtt_configured`
/// ——否则「有消费者在消费」会被误判成「本地历史存量」。
#[test]
fn sim_consumer_without_mqtt_still_counts_as_consumer() {
    let input = HealthProblemInput {
        mqtt_configured: false,
        mqtt_connected: false,
        // 模拟消费者在跑，但没配 MQTT
        has_north_consumer: true,
        max_queue_rows: 1_000_000,
        queue_len: 149_668,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    let got = codes(&input);
    assert!(
        got.contains(&"QUEUE_BACKPRESSURE".to_string()),
        "有消费者时必须是背压: {:?}",
        got
    );
    assert!(
        !got.contains(&"QUEUE_LOCAL_HISTORY".to_string()),
        "有消费者时不得报本地历史存量: {:?}",
        got
    );
    // 没配 MQTT 就不该报 MQTT 断开
    assert!(!got.contains(&"MQTT_DISCONNECTED".to_string()));
}

/// 反向回归：MQTT 客户端存在但队列无消费者时（MQTT publish 循环尚未起，
/// 或已被停用），存量仍是本地历史语义——两个语义各自独立生效。
#[test]
fn mqtt_configured_without_consumer_keeps_local_history_semantics() {
    let input = HealthProblemInput {
        mqtt_configured: true,
        mqtt_connected: true,
        has_north_consumer: false,
        max_queue_rows: 1_000_000,
        queue_len: 149_668,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    let got = codes(&input);
    assert!(got.contains(&"QUEUE_LOCAL_HISTORY".to_string()));
    assert!(!got.contains(&"QUEUE_BACKPRESSURE".to_string()));
}

/// 回归：容量淘汰必须独立于队列长度与健康等级被报出来。
///
/// 真实故障形态是「断网 → 队列涨到上限 → 持续淘汰 → 上行恢复 → 队列长度回落」。
/// 此时 `queue_len` 已回到 0（不触发任何长度类问题），但数据**确实丢过**。
/// 若淘汰只看长度/等级（边沿触发），这一整段丢失对用户完全不可见。
#[test]
fn queue_eviction_is_reported_even_when_queue_is_empty() {
    let input = HealthProblemInput {
        mqtt_configured: true,
        mqtt_connected: true,
        has_north_consumer: true,
        max_queue_rows: 1_000_000,
        // 队列已被消化干净——长度维度完全健康
        queue_len: 0,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        // 但历史上淘汰过 1200 行，其中 800 行从未上传
        queue_evicted_rows: 1200,
        queue_evicted_undelivered_rows: 800,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    let got = codes(&input);
    assert!(
        got.contains(&"QUEUE_DATA_EVICTED".to_string()),
        "淘汰过数据就必须报出来: {:?}",
        got
    );
    // 长度维度确实是健康的，不能因此误报积压类问题
    assert!(!got.contains(&"QUEUE_BACKPRESSURE".to_string()));
    assert!(!got.contains(&"QUEUE_LOCAL_HISTORY".to_string()));
}

/// 无淘汰时不得凭空多报一条（避免"每次打开健康页都有红条"的告警疲劳）。
#[test]
fn no_eviction_means_no_data_loss_problem() {
    let input = HealthProblemInput {
        mqtt_configured: true,
        mqtt_connected: true,
        has_north_consumer: true,
        max_queue_rows: 1_000_000,
        queue_len: 10,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: 0,
        dead_letter_available: true,
    };
    assert!(!codes(&input).contains(&"QUEUE_DATA_EVICTED".to_string()));
}

fn dl_input(rows: i64, available: bool) -> HealthProblemInput<'static> {
    HealthProblemInput {
        mqtt_configured: true,
        mqtt_connected: true,
        has_north_consumer: true,
        max_queue_rows: 1_000_000,
        queue_len: 10,
        ota_state: "Idle",
        buses: &[],
        disk_level: DiskLevel::Ok,
        disk_free_pct: Some(50.0),
        queue_evicted_rows: 0,
        queue_evicted_undelivered_rows: 0,
        dead_letter_rows: rows,
        dead_letter_available: available,
    }
}

/// 死信非空必须报，但只能是 warning —— 数据还在，可重投。
///
/// 若误报 critical，健康报告会长期停在 critical，现场会逐渐无视它，
/// 和「退役的告警」没区别。
#[test]
fn dead_letter_rows_are_reported_as_warning() {
    let got = codes(&dl_input(37, true));
    assert!(
        got.contains(&"DEAD_LETTER_PENDING".to_string()),
        "死信非空必须报出来: {:?}",
        got
    );
    let probs = build_health_problems(&dl_input(37, true));
    let p = probs
        .iter()
        .find(|p| p["code"] == "DEAD_LETTER_PENDING")
        .expect("problem 存在");
    assert_eq!(p["level"], "warning");
}

/// 死信库不可用时，必须报「查不到」，不能报成「0 条」。
///
/// 这是本组最容易写错的一条：`dead_letter_rows` 在不可用时被填成 0，
/// 如果只按 `rows > 0` 判定，运维会以为"没有未送达数据"。
#[test]
fn unavailable_dead_letter_store_is_reported() {
    let got = codes(&dl_input(0, false));
    assert!(
        got.contains(&"DEAD_LETTER_UNAVAILABLE".to_string()),
        "不可用必须单独报: {:?}",
        got
    );
    assert!(!got.contains(&"DEAD_LETTER_PENDING".to_string()));
}

/// 正常空死信 + 可用库，不得多报（与其它告警不冲突）。
#[test]
fn healthy_dead_letter_store_reports_nothing() {
    let got = codes(&dl_input(0, true));
    assert!(!got.contains(&"DEAD_LETTER_PENDING".to_string()));
    assert!(!got.contains(&"DEAD_LETTER_UNAVAILABLE".to_string()));
}
