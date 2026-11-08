//! Webhook **客户端级**回归测试
//!
//! 与 `tests.rs`（纯函数级：游标推进 / data_id 唯一）不同，本文件跑真实链路：
//! 真实 SQLite outbox + 本机 TCP 桩端点 + 真实 reqwest 请求。
//!
//! 为什么必须补这一层：D1（4xx / 重试耗尽进死信表）与 D2（死信可观测性）的修复点
//! 都在 `flush_pending` 的分支里。outbox 单测只能证明 `move_to_dead_letter` 这个
//! SQL 操作本身正确，**无法证明收到 404 时 `flush_pending` 真的会调用它**——
//! 若某次重构改错分支（例如把 Permanent 误当 Retriable），outbox 单测全绿，而现场
//! 实际行为退化为「无限重发后静默丢弃」。本文件覆盖三条最关键路径：
//!   1. 永久失败 4xx → outbox 行消失、死信表出现一行且 payload 完整保留
//!   2. 持续 5xx → 重试到上限 → 死信 reason=retry_exhausted、attempt_count=8
//!   3. 死信重投 → 端点修好后投递成功 → 死信与 pending 同时清空
//!
//! 桩服务是几十行 TCP + 手写 HTTP/1.1 应答，不引第三方 mock crate
//! （本 crate 的 dev-dependencies 只有 tempfile / rand）。

use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

/// 测试用固定 data_id（各用例各自独立 outbox，不会互相撞唯一索引）。
const TEST_DATA_ID: i64 = 7_700_001;

/// 在 127.0.0.1:0 起一个最小 HTTP/1.1 桩：对每个请求返回 `status` 指定的状态码。
///
/// 返回 `(url, 可热切换的状态码, 命中计数)`——状态码用 `AtomicU16` 是为了让
/// 「重投」用例能在同一个桩上从 404 切到 200，模拟现场把接收端配置修好。
async fn spawn_stub(status: u16) -> (String, Arc<AtomicU16>, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub listener");
    let addr = listener.local_addr().expect("stub local_addr");
    let now_status = Arc::new(AtomicU16::new(status));
    let hits = Arc::new(AtomicUsize::new(0));
    let (st, hits_in_task) = (now_status.clone(), hits.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let code = st.load(Ordering::SeqCst);
            hits_in_task.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                // 必须把请求体读干净再应答：写完响应就 close，会让 reqwest 在
                // 「写 body」阶段撞 RST，从而被归类为 transport error（Retriable），
                // 而不是本文件要覆盖的 http status 分类。
                if read_full_request(&mut sock).await.is_ok() {
                    let reason = match code {
                        200 => "OK",
                        404 => "Not Found",
                        503 => "Service Unavailable",
                        _ => "Stub",
                    };
                    let resp = format!(
                        "HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/ingest"), now_status, hits)
}

/// 读到「请求头结束 + Content-Length 声明的 body 全部到齐」为止。
async fn read_full_request(sock: &mut tokio::net::TcpStream) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= pos + 4 + len {
                return Ok(());
            }
        }
    }
}

/// 构造一个指向桩端点的客户端（真实 sqlite outbox）。
///
/// 返回的 `TempDir` 必须活到用例结束：outbox 库文件被删除后连接仍在写，
/// 断言会看到「另一张空表」这种难以定位的假失败。
fn new_client(
    url: &str,
    interval_ms: u64,
) -> (
    WebhookClient,
    Arc<crate::engine::metrics::Metrics>,
    tempfile::TempDir,
) {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let store = OutboxStore::open(&dir.path().join("outbox.db")).expect("open outbox db");
    // 降级到内存模式时行为一致，但会让「重投后持久化」的语义失真——直接失败更诚实。
    assert!(!store.is_degraded(), "测试必须跑在 sqlite 文件模式");
    let cfg = WebhookConfig {
        enabled: true,
        url: url.to_string(),
        headers: Default::default(),
        timeout_secs: 5,
        batch_size: 100,
        interval_ms,
        max_retries: 3,
    };
    let metrics = Arc::new(crate::engine::metrics::Metrics::new());
    (
        WebhookClient::new(cfg, store, metrics.clone()),
        metrics,
        dir,
    )
}

/// 往 outbox 灌一条 `protocol='http'` 的 pending 行（等价 `persist_batch` 的产物）。
fn seed_pending(client: &WebhookClient, payload: &[u8]) {
    let id = client
        .outbox
        .lock()
        .persist(&OutgoingMsg {
            data_id: TEST_DATA_ID,
            data_ids: Vec::new(),
            topic: client.config.url.clone(),
            payload: payload.to_vec(),
            qos: 1,
            protocol: "http".into(),
        })
        .expect("persist pending");
    assert!(id > 0, "pending 行必须真的写入 outbox，实际 rowid={id}");
}

/// 4xx 永久失败：outbox 行消失 + 死信表出现一行（payload 完整保留）。
///
/// 这是 D1 的核心回归点——修复前该分支是 `remove()` 物理删除，
/// 数据在接收端配置修好之后也**没有任何入口**能捞回来。
#[tokio::test]
async fn permanent_4xx_moves_pending_into_dead_letter() {
    let (url, _status, hits) = spawn_stub(404).await;
    let (client, metrics, _dir) = new_client(&url, 1_000);
    let payload = b"[{\"p\":1}]";
    seed_pending(&client, payload);

    client.flush_pending().await;

    assert_eq!(hits.load(Ordering::SeqCst), 1, "应当真的发出过一次请求");
    assert_eq!(
        client.outbox.lock().pending_count(Some("http")).unwrap(),
        0,
        "4xx 之后 pending 行必须搬走，否则会无限重发"
    );
    let dead = client
        .outbox
        .lock()
        .list_dead_letters("http", 10, 0)
        .unwrap();
    assert_eq!(dead.len(), 1, "4xx 必须进死信表，而不是物理删除");
    assert!(
        dead[0].reason.contains("404"),
        "reason 应能直接看出是哪类错误，实际={}",
        dead[0].reason
    );
    assert_eq!(
        dead[0].payload_len,
        payload.len() as i64,
        "死信必须保留完整 payload，否则重投没有意义"
    );
    assert_eq!(dead[0].attempt_count, 0, "永久失败不应消耗重试次数");

    // D2：搬入计数必须递增，否则 Grafana 上「死信」永远是 0，告警不响。
    assert_eq!(
        metrics
            .alarm
            .dead_letter_moved_total
            .load(Ordering::Relaxed),
        1,
        "死信搬入计数必须递增"
    );
}

/// 持续 5xx（可重试）→ 重试到 `MAX_WEBHOOK_ATTEMPTS` → 死信 reason=retry_exhausted。
///
/// `interval_ms = 0` 让指数退避恒为 0，用例无需真等 1s/2s/4s…；
/// 退避闸门 `now < next_at` 在 `next_at == now` 时放行，所以每轮都能立即重试。
#[tokio::test]
async fn retriable_5xx_exhausts_attempts_then_dead_letters() {
    let (url, _status, hits) = spawn_stub(503).await;
    let (client, metrics, _dir) = new_client(&url, 0);
    seed_pending(&client, b"payload");

    // 每次重试需要两轮 flush：第一轮处理该行并把游标推进到它，第二轮查询为空
    // 后游标回退到 0，第三次才能再次取到同一行——因此上限给到 2N 的冗余。
    let mut flushes = 0;
    while client
        .outbox
        .lock()
        .count_dead_letters(Some("http"))
        .unwrap()
        == 0
    {
        client.flush_pending().await;
        flushes += 1;
        assert!(
            flushes < 64,
            "5xx 重试 {flushes} 轮后仍未进死信表，尝试上限失效（会无限重发）"
        );
    }

    let dead = client
        .outbox
        .lock()
        .list_dead_letters("http", 10, 0)
        .unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(
        dead[0].reason, "retry_exhausted",
        "重试用尽与 4xx 必须能区分，运维处置方式不同"
    );
    assert_eq!(
        dead[0].attempt_count, MAX_WEBHOOK_ATTEMPTS as i32,
        "死信应记录实际尝试次数，便于排查是「端点一直挂着」还是「刚失败一次」"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        MAX_WEBHOOK_ATTEMPTS as usize,
        "实际请求次数应等于尝试上限"
    );
    assert_eq!(client.outbox.lock().pending_count(Some("http")).unwrap(), 0);
    assert_eq!(
        metrics
            .alarm
            .dead_letter_moved_total
            .load(Ordering::Relaxed),
        1
    );
}

/// 死信重投：接收端修好后重投 → 投递成功 → 死信与 pending 同时清空。
///
/// 这是「进死信 ≠ 数据已丢」这一设计承诺的可执行证据：只要端点恢复，
/// 原件能重新回到上报链路并真正送达。
#[tokio::test]
async fn replayed_dead_letter_is_delivered_and_removed() {
    let (url, status, hits) = spawn_stub(404).await;
    let (client, _metrics, _dir) = new_client(&url, 1_000);
    seed_pending(&client, b"payload");

    client.flush_pending().await;
    assert_eq!(
        client
            .outbox
            .lock()
            .count_dead_letters(Some("http"))
            .unwrap(),
        1,
        "前置条件：第一次投递必须失败并进死信"
    );

    // 现场把接收端修好（路径 / 鉴权 / 白名单）后重投。
    status.store(200, Ordering::SeqCst);
    let dead_id = client
        .outbox
        .lock()
        .list_dead_letters("http", 10, 0)
        .unwrap()[0]
        .id;
    let requeued = client
        .outbox
        .lock()
        .replay_dead_letter(dead_id)
        .expect("replay dead letter");
    assert!(requeued > 0, "重投应生成新的 outbox pending 行");

    client.flush_pending().await;

    assert_eq!(
        client
            .outbox
            .lock()
            .count_dead_letters(Some("http"))
            .unwrap(),
        0,
        "投递成功后死信行应被删除"
    );
    assert_eq!(
        client.outbox.lock().pending_count(Some("http")).unwrap(),
        0,
        "投递成功后 pending 行应被 ack 删除"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "重投后必须真的再发一次请求（1 次失败 + 1 次成功）"
    );
}

/// D6：`send_once` 必须把**本条消息**的失败原因随结果返回。
///
/// 旧实现把原因写进共享槽 `last_error`（`health()` 也读它），再由
/// `flush_pending` 在分类后回读——正确性依赖「发送完立刻读」的隐式时序，
/// 一旦同批次或并发路径插入另一次发送，先失败那条的死信 reason 就会指向别人。
///
/// 这里把该槽预置成无关字符串，再让同一个客户端先后收到 404 与 422：
/// 两次的 reason 必须各自对应，且都不能是预置值。
#[tokio::test]
async fn send_outcome_carries_reason_of_its_own_request() {
    let (url, status, _hits) = spawn_stub(404).await;
    let (client, _metrics, _dir) = new_client(&url, 1_000);
    let msg = OutgoingMsg {
        data_id: TEST_DATA_ID,
        data_ids: Vec::new(),
        topic: url.clone(),
        payload: b"payload".to_vec(),
        qos: 1,
        protocol: "http".into(),
    };
    *client.last_error.write() = Some("stale: http status 500 from another message".into());

    match client.send_once(&msg).await {
        SendOutcome::Permanent(reason) => {
            assert!(
                reason.contains("404"),
                "Permanent 应携带本条消息的 404，实际={reason}"
            );
            assert!(!reason.contains("stale"), "不得回读共享的 last_error");
        }
        _ => panic!("404 应判定为 Permanent"),
    }

    status.store(422, Ordering::SeqCst);
    match client.send_once(&msg).await {
        SendOutcome::Permanent(reason) => {
            assert!(
                reason.contains("422"),
                "状态码变化后原因必须随之变化，实际={reason}"
            );
        }
        _ => panic!("422 应判定为 Permanent"),
    }
}
