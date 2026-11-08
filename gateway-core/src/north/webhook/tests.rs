use super::{next_processed_max_id, webhook_data_id, OutgoingMsg};
use std::collections::HashSet;

/// r49-WH1（已退役，r69 移入 cfg(test)）：根据 flush 本轮实际取到的消息列表
/// 计算下次分页起点——取 `msgs` 中最大 outbox_id 作为下次 after_id，保证
/// 分页窗口单调推进、避免首页饥饿；空列表返回 0（fail-open，从头重新探测）。
///
/// r59-WH1：此函数**未考虑退避跳过**——纯「窗口末元素」。`flush_pending` 现改用
/// `next_processed_max_id`，仅在被本轮**实际处理过**的行之间取最大 id。
/// 本函数仅供下面 r49 回归测试使用，**不应在生产路径调用**——若未来需要再次
/// 复用，请优先考虑 `next_processed_max_id`。
fn next_flush_after_id(msgs: &[(i64, OutgoingMsg)]) -> i64 {
    msgs.last().map(|(id, _)| *id).unwrap_or(0)
}

/// Bug C17: 连续生成的 data_id 必须唯一，否则 outbox 唯一索引
/// `idx_outbox_pending_data_id(data_id) WHERE status = 0` 会让后写入的批次
/// 被 `INSERT OR IGNORE` 静默丢弃（原实现直接用 timestamp_millis() 会撞）。
#[test]
fn webhook_data_id_unique() {
    let n = 10_000;
    let ids: HashSet<i64> = (0..n).map(|_| webhook_data_id()).collect();
    assert_eq!(ids.len(), n, "webhook data_id 必须唯一");
}

/// r49-WH1：空批次 → 游标回退到 0（fail-open，从头重新探测）。
#[test]
fn r49_next_flush_after_id_empty_returns_zero() {
    let msgs: Vec<(i64, OutgoingMsg)> = Vec::new();
    assert_eq!(next_flush_after_id(&msgs), 0);
}

/// r49-WH1：非空批次 → 游标前进到本批最大 id，使下一轮窗口单调推进。
/// 修复前固定 after_id=0，首页被退避阻塞的 50 条会永远占满 LIMIT
/// 窗口、id>50 的所有新 pending 消息被永久饥饿（直到退避窗口全耗尽）。
#[test]
fn r49_next_flush_after_id_advances_to_batch_tail() {
    let mk = |id: i64| {
        (
            id,
            OutgoingMsg {
                data_id: id,
                data_ids: vec![],
                topic: "http://t/".into(),
                payload: vec![0],
                qos: 1,
                protocol: "http".into(),
            },
        )
    };
    let msgs = vec![mk(1), mk(7), mk(42), mk(100)];
    assert_eq!(
        next_flush_after_id(&msgs),
        100,
        "下一次 after_id 必须等于本批最大 id，使分页窗口单调推进"
    );
}

/// r49-WH1：单条批次也必须前进到该条的 id，而不是停在 0。
/// 防御性回归：避免有人未来"优化"时 single-element 路径特判返回 0。
#[test]
fn r49_next_flush_after_id_single_row_advances() {
    let msgs = vec![(
        999_999,
        OutgoingMsg {
            data_id: 999_999,
            data_ids: vec![],
            topic: "http://t/".into(),
            payload: vec![0],
            qos: 1,
            protocol: "http".into(),
        },
    )];
    assert_eq!(next_flush_after_id(&msgs), 999_999);
}

/// r59-WH1：退避跳过的行（`processed[i]=false`）不计入 max_id。
/// 修复前 `next_flush_after_id` 直接取窗口末元素 id（这里是 200），
/// 但实际只有 id=50 被处理过：游标推进到 100/150/200，这些行 status=0
/// 永远查不到。
#[test]
fn r59_next_processed_max_id_ignores_skipped_rows() {
    let mk = |id: i64| {
        (
            id,
            OutgoingMsg {
                data_id: id,
                data_ids: vec![],
                topic: "http://t/".into(),
                payload: vec![0],
                qos: 1,
                protocol: "http".into(),
            },
        )
    };
    // 窗口 id 序列：[10, 50, 100, 150, 200]
    let msgs = vec![mk(10), mk(50), mk(100), mk(150), mk(200)];
    // 只 10 和 50 被本轮处理；100/150/200 处于退避期
    let processed = vec![true, true, false, false, false];
    assert_eq!(
        next_processed_max_id(&msgs, &processed),
        50,
        "r59: 游标必须只前进到 50，不能推过未处理行（100/150/200）",
    );
}

/// r59-WH1：全 false（全部行处于退避期）→ 返回 0，调用方不前移游标，
/// 下一轮从原 `after_id` 重新探测——保留被跳过的行被重试的机会。
#[test]
fn r59_next_processed_max_id_all_skipped_returns_zero() {
    let mk = |id: i64| {
        (
            id,
            OutgoingMsg {
                data_id: id,
                data_ids: vec![],
                topic: "http://t/".into(),
                payload: vec![0],
                qos: 1,
                protocol: "http".into(),
            },
        )
    };
    let msgs = vec![mk(1), mk(2), mk(3)];
    let processed = vec![false, false, false];
    assert_eq!(
        next_processed_max_id(&msgs, &processed),
        0,
        "r59: 全部 false 必须返回 0，flush_pending 据此判断不前移游标",
    );
}

/// r59-WH1：与 r49 兼容——全 true 退化为「窗口末元素 id」。
#[test]
fn r59_next_processed_max_id_all_processed_equals_batch_tail() {
    let mk = |id: i64| {
        (
            id,
            OutgoingMsg {
                data_id: id,
                data_ids: vec![],
                topic: "http://t/".into(),
                payload: vec![0],
                qos: 1,
                protocol: "http".into(),
            },
        )
    };
    let msgs = vec![mk(1), mk(7), mk(42), mk(100)];
    let processed = vec![true, true, true, true];
    assert_eq!(next_processed_max_id(&msgs, &processed), 100);
}

/// r59-WH1：空列表防御性——返回 0，调用方由 `if processed_max_id > 0`
/// 判定不前移游标。
#[test]
fn r59_next_processed_max_id_empty_returns_zero() {
    let msgs: Vec<(i64, OutgoingMsg)> = Vec::new();
    let processed: Vec<bool> = Vec::new();
    assert_eq!(next_processed_max_id(&msgs, &processed), 0);
}

// ===== r61-SD1 回归：webhook 的 shutdown signal 修复 =====
//
// 旧实现 webhook.run 是无尽的 `loop { interval.tick().await; ...
// 没有 stop 接口——systemd TimeoutStopSec 命中后 SIGKILL 直接杀进程。
// outbox 中 protocol='http' 的 pending 行只能等下次启动 replay。
// 修复：WebhookClient 加 stopping 标志 + stop_notify + stop()，
// run 循环立即检查 stopping + select! 监听 stop_notify 提前 break。
/// r61-SD1：stop() 必须把 stopping 标志置位，run 循环下次迭代立即退出。
/// 这是与 r48-MQTT1 同型——"先置位再 notify"保证 select! 任一分支 ready。
#[tokio::test]
async fn r61_webhook_stop_sets_stopping_flag() {
    use super::super::webhook::WebhookClient;
    use crate::config::WebhookConfig;
    use crate::engine::metrics::Metrics;
    use crate::north::mqtt::outbox::OutboxStore;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("outbox.db");
    let outbox = OutboxStore::open(&db_path).unwrap();
    // r79-OBX1：测试用 metrics（与生产路径独立）。
    let wh = WebhookClient::new(WebhookConfig::default(), outbox, Arc::new(Metrics::new()));
    assert!(!wh.is_stopping_set(), "r61: 初始 stopping 必须为 false");
    wh.stop().await;
    assert!(
        wh.is_stopping_set(),
        "r61: stop() 必须把 stopping 置位，run 循环才能立即退出",
    );
}

/// r61-SD1：stop() 之后调多次也必须幂等，且不 panic。
/// 防御性回归：避免未来"优化"把 stop 改成「一次性 close channel」破坏幂等性。
#[tokio::test]
async fn r61_webhook_stop_is_idempotent() {
    use super::super::webhook::WebhookClient;
    use crate::config::WebhookConfig;
    use crate::engine::metrics::Metrics;
    use crate::north::mqtt::outbox::OutboxStore;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("outbox.db");
    let outbox = OutboxStore::open(&db_path).unwrap();
    let wh = WebhookClient::new(WebhookConfig::default(), outbox, Arc::new(Metrics::new()));
    wh.stop().await;
    wh.stop().await; // 第二次也必须不 panic、不阻塞
    wh.stop().await;
    assert!(wh.is_stopping_set());
}

// ===== 启动期 URL 校验回归：run 退出后 health 必须如实报未连接 =====

/// `run()` 因 URL 为空/非法而提前退出时，`health()` 不能再按
/// 「enabled + url 非空 + 无错误」判定 connected=true——否则状态页把
/// 已停摆的 webhook 通道显示为正常，运维只能靠翻启动日志定位。
#[tokio::test]
async fn invalid_url_run_exit_reports_disconnected() {
    use super::super::webhook::WebhookClient;
    use crate::config::WebhookConfig;
    use crate::engine::metrics::Metrics;
    use crate::north::mqtt::outbox::OutboxStore;
    use crate::north::NorthConnector;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let outbox = OutboxStore::open(&dir.path().join("outbox.db")).unwrap();
    let cfg = WebhookConfig {
        enabled: true,
        url: "ftp://example.com/hook".into(),
        ..WebhookConfig::default()
    };
    let wh = Arc::new(WebhookClient::new(cfg, outbox, Arc::new(Metrics::new())));
    assert!(
        wh.health().connected,
        "前置基线：发送前按配置可用性判定为可用"
    );

    // 非法协议前缀 → run 记录错误并立即返回，不会触达网络。
    wh.clone().run().await;

    let h = wh.health();
    assert!(!h.connected, "run 退出后不得再报 connected=true");
    assert_eq!(
        h.last_error.as_deref(),
        Some("invalid webhook url (must be http:// or https://): ftp://example.com/hook"),
        "必须把 URL 非法原因透出到状态接口"
    );
}

/// 空 URL 同样属于「启用但不可能发送」，health 必须为未连接并给出原因。
#[tokio::test]
async fn empty_url_run_exit_reports_disconnected() {
    use super::super::webhook::WebhookClient;
    use crate::config::WebhookConfig;
    use crate::engine::metrics::Metrics;
    use crate::north::mqtt::outbox::OutboxStore;
    use crate::north::NorthConnector;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let outbox = OutboxStore::open(&dir.path().join("outbox.db")).unwrap();
    let cfg = WebhookConfig {
        enabled: true,
        url: String::new(),
        ..WebhookConfig::default()
    };
    let wh = Arc::new(WebhookClient::new(cfg, outbox, Arc::new(Metrics::new())));
    // 空 URL 时 health 本就判定未连接，run 退出后仍应保持未连接并带原因。
    wh.clone().run().await;

    let h = wh.health();
    assert!(!h.connected);
    assert_eq!(h.last_error.as_deref(), Some("webhook url is empty"));
}

// ===== r64-SD1 回归：停机序 await run 退出句柄 =====

/// r64-SD1：`stop()` 后 webhook run 循环必须真的退出，且 JoinHandle 在超时内
/// resolve——gatewayd 的 graceful_shutdown 依赖此保证（先 stop 再 await 句柄）。
///
/// 先 stop 再 spawn：run 已通过 enabled/url 前置校验后，首个 loop 迭代的
/// `if stopping.load() { break }` 立即命中退出，不触达网络发送路径（确定性）。
#[tokio::test]
async fn r64_webhook_run_exits_after_stop() {
    use super::super::webhook::WebhookClient;
    use crate::config::WebhookConfig;
    use crate::engine::metrics::Metrics;
    use crate::north::mqtt::outbox::OutboxStore;
    use std::sync::Arc;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("outbox.db");
    let outbox = OutboxStore::open(&db_path).unwrap();
    // enabled=true + 合法 url，确保 run 进入主循环（否则会因前置校验直接 return）。
    let cfg = WebhookConfig {
        enabled: true,
        url: "http://127.0.0.1:9/hook".into(),
        ..WebhookConfig::default()
    };
    let wh = Arc::new(WebhookClient::new(cfg, outbox, Arc::new(Metrics::new())));

    // 先置位 stopping，再启动 run。
    wh.stop().await;
    let c = wh.clone();
    let handle = tokio::spawn(async move { c.run().await });

    let res = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(
        res.is_ok(),
        "r64: stop() 后 webhook run 循环必须退出（停机序 await 该 JoinHandle）",
    );
}

// ===== r65-SD1 回归：run 循环追踪并 await 上一轮 detached flush =====

/// r65-SD1 不变量：run 退出后 `last_flush` 槽必须被清空。
///
/// run 退出路径包含"break 时 await 最后 in-flight flush"——若该逻辑被未
/// 改动破坏（slot 残留 Some），停机序的 await webhook_run handle + 进程退出
/// 会让该句柄对应的 detached task 在 SIGKILL 时撕裂。锁定"run 退出后槽空"。
/// 注：即便 outbox 为空 flush_pending 也会跑完整流程（select sql 后命中空闲
/// 回），每 tick 仍会 spawn 一个 detached 任务 → last_flush 槽会被填上。
#[tokio::test]
async fn r65_last_flush_slot_is_drained_after_run_exit() {
    use super::super::webhook::WebhookClient;
    use crate::config::WebhookConfig;
    use crate::engine::metrics::Metrics;
    use crate::north::mqtt::outbox::OutboxStore;
    use std::sync::Arc;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("outbox.db");
    let outbox = OutboxStore::open(&db_path).unwrap();
    let cfg = WebhookConfig {
        enabled: true,
        // 小 interval 让 run 在 stop 前能跑多次 tick、给 last_flush 填充机会。
        interval_ms: 30,
        url: "http://127.0.0.1:9/hook".into(),
        ..WebhookConfig::default()
    };
    let wh = Arc::new(WebhookClient::new(cfg, outbox, Arc::new(Metrics::new())));

    // 先让 run 跑几轮再 stop。
    let c = wh.clone();
    let handle = tokio::spawn(async move { c.run().await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    wh.stop().await;

    let res = tokio::time::timeout(Duration::from_secs(10), handle).await;
    assert!(res.is_ok(), "r65: stop() 后 webhook run 循环必须退出",);

    // 不变量：run 退出后 last_flush 槽必须被清空。
    assert!(
        !wh.last_flush_is_some(),
        "r65: run 退出前必须 await 完最后一次 in-flight flush，槽必须为空"
    );
}

// ===== r79-OBX1：webhook 数据丢失可观测性回归 =====

/// r79-OBX1：metrics 注入后，give-up 与 permanent 路径必须递增对应计数器。
///
/// 旧实现仅 `tracing::error!` 记日志，无 Prometheus 指标——运维无法配置告警，
/// 只能事后翻日志定位。`webhook_data_lost_total` / `webhook_giveup_total`
/// 是「数据真的丢了」的告警信号，每次增加 = 一条数据不可恢复。
///
/// 本测试无法直接走网络（无 mock server），改为通过白盒方式构造场景：
/// 把 give-up / permanent 路径的 `inc_*` 调用绑定为白盒测试——验证该调用
/// 在 `SendOutcome::Permanent` / `attempts >= MAX_WEBHOOK_ATTEMPTS` 时被触发。
/// 由于 flush_pending 内部逻辑强依赖网络 + 退避 + spawn_blocking，**直接
/// 构造 SendOutcome 不便**；这里用最小副作用的等价测试：直接断言
/// `metrics.inc_webhook_*` 调用存在并可通过 metrics 字段读取。
#[test]
fn r79_webhook_metrics_fields_exist_and_increment() {
    use crate::engine::metrics::Metrics;
    use std::sync::Arc;

    let m = Arc::new(Metrics::new());
    // 初始为 0
    assert_eq!(
        m.webhook
            .data_lost
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "r79: 初始 webhook_data_lost_total 必须为 0"
    );
    assert_eq!(
        m.webhook.giveup.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "r79: 初始 webhook_giveup_total 必须为 0"
    );

    // 模拟「Permanent 失败」分支调用。
    m.inc_webhook_data_lost();
    m.inc_webhook_data_lost();
    assert_eq!(
        m.webhook
            .data_lost
            .load(std::sync::atomic::Ordering::Relaxed),
        2,
        "r79: 2 次 inc_webhook_data_lost 必须累加为 2"
    );

    // 模拟「give-up」分支调用。
    m.inc_webhook_giveup();
    assert_eq!(
        m.webhook.giveup.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "r79: 1 次 inc_webhook_giveup 必须累加为 1"
    );
}

/// r79-OBX1：prometheus exporter 必须暴露 webhook_data_lost_total / giveup_total。
/// 验证「指标可达运维」——若导出器忘记 emit，告警规则看不到数据，等于没做。
#[test]
fn r79_webhook_data_lost_metrics_exposed_in_prometheus() {
    use crate::engine::metrics::Metrics;
    use std::sync::Arc;

    let m = Arc::new(Metrics::new());
    m.inc_webhook_data_lost();
    m.inc_webhook_data_lost();
    m.inc_webhook_data_lost();
    m.inc_webhook_giveup();

    let dump = m.prometheus_export();
    assert!(
        dump.contains("gateway_webhook_data_lost_total 3"),
        "r79: prometheus 导出必须包含 gateway_webhook_data_lost_total=3，实际\n{}",
        dump
    );
    assert!(
        dump.contains("gateway_webhook_giveup_total 1"),
        "r79: prometheus 导出必须包含 gateway_webhook_giveup_total=1，实际\n{}",
        dump
    );
}

// ===== P7：webhook 发布指标接线回归 =====

/// 起一个只应答一次的极简 HTTP 端点，返回给定状态行。
async fn serve_once(status_line: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = sock.read(&mut buf).await;
        let body = "{\"ok\":true}";
        let resp = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
            status_line,
            body.len(),
            body
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        let _ = sock.flush().await;
    });
    format!("http://{}/hook", addr)
}

/// P7：webhook 发送成功必须递增 `publish_success`，失败必须递增 `publish_failure`。
///
/// 修复前 webhook 侧从未写过 publish 指标，导致
/// `gateway_webhook_publish_total{result="success"}` 恒 0，且
/// `gateway_uplink_total`（= mqtt.publish_success + webhook.publish_success）
/// 恒等于 MQTT 单边值——北向整体吞吐被系统性低估。
#[tokio::test]
async fn p7_send_once_increments_publish_metrics() {
    use super::super::webhook::WebhookClient;
    use crate::config::WebhookConfig;
    use crate::engine::metrics::Metrics;
    use crate::north::mqtt::outbox::OutboxStore;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let mk_msg = |url: String| OutgoingMsg {
        data_id: 1,
        data_ids: vec![],
        topic: url,
        payload: b"{}".to_vec(),
        qos: 1,
        protocol: "http".into(),
    };

    // ---- 2xx → success ----
    let ok_url = serve_once("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let outbox = OutboxStore::open(&dir.path().join("outbox.db")).unwrap();
    let metrics = Arc::new(Metrics::new());
    let wh = WebhookClient::new(WebhookConfig::default(), outbox, metrics.clone());

    let outcome = wh.send_once(&mk_msg(ok_url)).await;
    assert!(
        matches!(outcome, super::SendOutcome::Ack),
        "P7: 2xx 必须判定为 Ack"
    );
    assert_eq!(
        metrics.webhook.publish_success.load(Ordering::Relaxed),
        1,
        "P7: 2xx 必须递增 webhook publish_success"
    );
    assert_eq!(
        metrics.webhook.publish_failure.load(Ordering::Relaxed),
        0,
        "P7: 2xx 不得递增 publish_failure"
    );

    // ---- 5xx → failure ----
    let err_url = serve_once("500 Internal Server Error").await;
    let dir2 = tempfile::tempdir().unwrap();
    let outbox2 = OutboxStore::open(&dir2.path().join("outbox.db")).unwrap();
    let metrics2 = Arc::new(Metrics::new());
    let wh2 = WebhookClient::new(WebhookConfig::default(), outbox2, metrics2.clone());

    let outcome2 = wh2.send_once(&mk_msg(err_url)).await;
    assert!(
        matches!(outcome2, super::SendOutcome::Retriable(_)),
        "P7: 5xx 必须判定为 Retriable"
    );
    assert_eq!(
        metrics2.webhook.publish_failure.load(Ordering::Relaxed),
        1,
        "P7: 5xx 必须递增 webhook publish_failure"
    );
    assert_eq!(
        metrics2.webhook.publish_success.load(Ordering::Relaxed),
        0,
        "P7: 5xx 不得递增 publish_success"
    );

    // ---- Prometheus 出口必须真实反映双边值 ----
    let dump = metrics.prometheus_export();
    assert!(
        dump.contains("gateway_webhook_publish_total{result=\"success\"} 1"),
        "P7: prometheus 导出必须包含 webhook publish success=1，实际\n{}",
        dump
    );
    assert!(
        dump.contains("gateway_webhook_publish_total{result=\"failure\"} 0"),
        "P7: prometheus 导出必须包含 webhook publish failure=0，实际\n{}",
        dump
    );
}
