use super::*;
use crate::engine::model::Quality;
use crate::engine::ports::AlarmPublisher;

fn make_point(id: &str, value: f64) -> DataPoint {
    DataPoint {
        point_id: id.to_string(),
        device_id: String::new(),
        value,
        raw_value: value,
        raw_type: "f64".to_string(),
        quality: Quality::Good,
        quality_reason: String::new(),
        timestamp: 1700000000000,
        source_ts: 1700000000000,
        stale_count: 0,
        value_text: None,
    }
}

fn make_config(id: &str, high: Option<f64>, low: Option<f64>) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: "b".to_string(),
        protocol: "modbus".to_string(),
        address: "1".to_string(),
        interval: 1,
        alarm_high: high,
        alarm_low: low,
        ..Default::default()
    }
}

/// r33-ALM1 测试用：构造 AlarmEngine 时填充 pending_since 字段。
/// 旧测试用 struct literal 缺这一字段，加了字段后必须填。
fn empty_engine(points: HashMap<String, PointConfig>) -> AlarmEngine {
    AlarmEngine {
        points,
        alarm_state: Arc::new(Mutex::new(HashMap::new())),
        pending_since: Arc::new(Mutex::new(HashMap::new())),
        mqtt: None,
        mqtt_topic: "test".to_string(),
        metrics: None,
        // r73-SD1：停机标志 + Notify。
        stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        stop_notify: Arc::new(tokio::sync::Notify::new()),
    }
}

/// 假上报端口收集到的载荷：(topic, JSON 字节)。
type ReceivedAlarms = Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// 假上报端口：记录收到的报警载荷，供"引擎只认契约、不认 MQTT"的行为断言。
///
/// 依赖倒置的测试替身——引擎侧测试不应为此依赖 `north`（那正是本次要消除的反向依赖）。
struct FakeAlarmPublisher {
    received: ReceivedAlarms,
}

impl FakeAlarmPublisher {
    fn new() -> (Arc<Self>, ReceivedAlarms) {
        let received: ReceivedAlarms = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Arc::new(Self {
                received: received.clone(),
            }),
            received,
        )
    }
}

impl AlarmPublisher for FakeAlarmPublisher {
    fn publish_alarm<'a>(
        &'a self,
        topic: &'a str,
        payload: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.received
                .lock()
                .unwrap()
                .push((topic.to_string(), payload));
        })
    }
}

/// 依赖倒置回归：报警事件经 `AlarmPublisher` 契约发出，引擎不感知 MQTT。
///
/// 这是 O-4「engine 不再依赖 north」的**行为等价性证据**：改造前后引擎对外的
/// 可观测行为（发到 `<topic>/alarm`、载荷含 trigger 事件）必须完全一致，
/// 只是发出通道从「直接持有 MqttClient」换成「持有契约」。
#[tokio::test]
async fn alarm_event_published_through_port() {
    let (publisher, received) = FakeAlarmPublisher::new();
    let mut engine = empty_engine(HashMap::from([(
        "t1".to_string(),
        make_config("t1", Some(100.0), None),
    )]));
    engine.mqtt = Some(publisher as Arc<dyn AlarmPublisher>);

    engine.evaluate(&make_point("t1", 101.0));

    // publish 是 spawn 出去的独立任务，等它把载荷投进端口
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while received.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "报警载荷必须在 3s 内投到上报端口"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let got = received.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "只应发出一条报警载荷，实际 {}", got.len());
    assert_eq!(got[0].0, "test/alarm", "topic 必须是 <mqtt_topic>/alarm");
    let payload: serde_json::Value =
        serde_json::from_slice(&got[0].1).expect("报警载荷必须是合法 JSON");
    assert_eq!(payload["event"], "trigger");
    assert_eq!(payload["point_id"], "t1");
}

// ===== r73-SD1：报警引擎纳入优雅停机 =====

/// r73-SD1：`stop()` 置位 stopping，幂等。
#[tokio::test]
async fn r73_alarm_stop_sets_stopping_flag_and_is_idempotent() {
    let engine = Arc::new(empty_engine(HashMap::new()));
    assert!(!engine.is_stopping_set(), "初始不应处于 stopping 状态");
    engine.stop().await;
    assert!(engine.is_stopping_set(), "stop() 必须置位 stopping");
    engine.stop().await;
    engine.stop().await;
    assert!(engine.is_stopping_set());
}

/// r73-SD1（关键）：`stop()` 后 run 必须在数秒内退出。
///
/// 旧实现 run 只在 `RealtimeBus` closed 时退出；bus 的 Arc 被 bootstrap 持有到
/// 进程结束 → run 在停机窗口内不会退出。本测试用 `tokio::spawn(engine.run(bus))`
/// 验证 stop() 能真正唤醒并退出。
#[tokio::test]
async fn r73_alarm_run_exits_after_stop() {
    use crate::engine::supervision::spawn_catch_unwind;
    // RealtimeBus::new(capacity) 自身即返回 Arc<RealtimeBus>。
    let bus = crate::engine::bus::RealtimeBus::new(16);
    let engine = Arc::new(empty_engine(HashMap::new()));

    let engine_for_run = engine.clone();
    let bus_for_run = bus.clone();
    let handle = spawn_catch_unwind("alarm_r73_test", async move {
        engine_for_run.run(bus_for_run).await;
    });
    // 等 run 订阅完成并进入 recv 等待
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!handle.is_finished(), "run 应处于等待 recv 的运行态");

    engine.stop().await;
    let res = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
    assert!(res.is_ok(), "r73: stop() 后 alarm run 必须在 3s 内退出");
}

#[test]
fn test_high_limit_trigger() {
    // 不实际连 MQTT，只验证状态机逻辑
    let engine = empty_engine(HashMap::from([(
        "t1".to_string(),
        make_config("t1", Some(100.0), None),
    )]));

    // 正常值首次评估：state 中无记录（未触发过报警）
    engine.evaluate(&make_point("t1", 50.0));
    assert_eq!(engine.alarm_state.lock().get("t1"), None);

    // 超限：触发报警
    engine.evaluate(&make_point("t1", 101.0));
    assert_eq!(engine.alarm_state.lock().get("t1"), Some(&AlarmType::High));

    // 仍超限：不重复触发（状态保持 High）
    engine.evaluate(&make_point("t1", 102.0));
    assert_eq!(engine.alarm_state.lock().get("t1"), Some(&AlarmType::High));

    // 恢复正常：状态移除
    engine.evaluate(&make_point("t1", 99.0));
    assert_eq!(engine.alarm_state.lock().get("t1"), None);
}

/// r33-ALM1 (回归)：default 0 模式下行为向后兼容——pending_since 字段被正确初始化，
/// evaluate 正常路径下清理去抖候选。
#[test]
fn test_pending_since_cleared_on_normal_value() {
    use std::time::Instant;
    let engine = empty_engine(HashMap::from([(
        "t1".to_string(),
        make_config("t1", Some(100.0), None),
    )]));

    // 模拟去抖候选被异常设置（真实场景下 trigger_or_keep 在 debounce_ms > 0 时填充）
    engine
        .pending_since
        .lock()
        .insert("t1".to_string(), Instant::now());
    assert!(engine.pending_since.lock().contains_key("t1"));

    // 正常值 evaluate 应清理 pending_since（防"瞬时跨阈又回落"残留）
    engine.evaluate(&make_point("t1", 50.0));
    assert!(
        !engine.pending_since.lock().contains_key("t1"),
        "正常值 evaluate 必须清理 pending_since"
    );
}

#[test]
fn test_low_limit_trigger() {
    let engine = empty_engine(HashMap::from([(
        "t1".to_string(),
        make_config("t1", None, Some(10.0)),
    )]));

    engine.evaluate(&make_point("t1", 50.0));
    assert_eq!(engine.alarm_state.lock().get("t1"), None);

    engine.evaluate(&make_point("t1", 9.0));
    assert_eq!(engine.alarm_state.lock().get("t1"), Some(&AlarmType::Low));

    engine.evaluate(&make_point("t1", 11.0));
    assert_eq!(engine.alarm_state.lock().get("t1"), None);
}

#[test]
fn test_no_alarm_config_ignored() {
    let engine = empty_engine(HashMap::from([(
        "t1".to_string(),
        make_config("t1", None, None),
    )]));

    engine.evaluate(&make_point("t1", 999.0));
    // 无阈值配置，不报警
    assert_eq!(engine.alarm_state.lock().get("t1"), None);
}

#[test]
fn test_bad_quality_ignored() {
    let engine = empty_engine(HashMap::from([(
        "t1".to_string(),
        make_config("t1", Some(100.0), None),
    )]));

    let mut p = make_point("t1", 101.0);
    p.quality = Quality::Bad;
    engine.evaluate(&p);
    // Bad 质量不触发报警
    assert_eq!(engine.alarm_state.lock().get("t1"), None);
}
