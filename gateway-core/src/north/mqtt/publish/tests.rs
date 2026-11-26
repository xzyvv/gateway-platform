use super::throttle::{
    circuit_cooldown_active, throttle_wait_ms, token_bucket_cap, token_bucket_refill,
};

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::MqttConfig;
use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::north::mqtt::{MqttClient, OutboxCmd};
use crate::queue::DataQueue;

/// r41-MQTT2 回归：冷却判定对时钟回拨（now < opened_at）必须安全。
/// 旧实现 `now - opened_at` 在 u64 下溢（debug panic / release 回绕误判冷却已过）。
#[test]
fn circuit_cooldown_handles_clock_rollback() {
    // 正常：冷却期内 → 仍阻断
    assert!(circuit_cooldown_active(1_000, 1_010, 30));
    // 正常：冷却已过 → 放行（半开）
    assert!(!circuit_cooldown_active(1_000, 1_040, 30));
    // 时钟回拨：now < opened_at → 饱和为 0，保守视为仍在冷却期（不 panic / 不提前半开）
    assert!(circuit_cooldown_active(2_000, 1_990, 30));
    assert!(circuit_cooldown_active(2_000, 0, 30));
}

/// P0-3 契约：首次调用给满桶（cap = rate * 突发系数），额度单位是"条"。
#[test]
fn token_bucket_first_call_fills_to_cap() {
    // rate=10 → cap=20，首次调用直接给 20 条额度
    assert_eq!(token_bucket_refill(0, 0, 10, 1_000), (1_000, 20));
    // 突发系数恒为 2
    assert_eq!(token_bucket_cap(10), 20);
    assert_eq!(token_bucket_cap(1_000), 2_000);
}

/// P0-3 契约：补充量按 rate*elapsed/1000 计算，且令牌上限为 cap。
#[test]
fn token_bucket_refills_and_caps() {
    // 1s 补 1000 令牌（rate=1000），上限 2000
    assert_eq!(token_bucket_refill(1_000, 0, 1_000, 2_000), (2_000, 1_000));
    // 累积补充被上限 2000 截断（1900 + 补 2000 → 封顶 2000）
    assert_eq!(
        token_bucket_refill(1_000, 1_900, 1_000, 3_000),
        (3_000, 2_000)
    );
}

/// 契约：elapsed 的补充量取整为 0 时，不推进时间戳（避免高频调用抹平补充）
#[test]
fn token_bucket_small_elapsed_keeps_timestamp() {
    // rate=1000, elapsed=1ms → refill=1 成立，正常推进
    assert_eq!(token_bucket_refill(1_000, 0, 1_000, 1_001), (1_001, 1));
    // rate=1, elapsed=999ms → refill=0，时间戳保持不回退
    assert_eq!(token_bucket_refill(1_000, 0, 1, 1_999), (1_000, 0));
}

/// P0-3 契约：时钟回拨（now < last_refill）不得下溢，也不得补充令牌。
#[test]
fn token_bucket_handles_clock_rollback() {
    // now=900 < last_refill=1000 → elapsed 饱和为 0，时间戳与令牌都不变
    assert_eq!(token_bucket_refill(1_000, 50, 100, 900), (1_000, 50));
}

/// P0-3 契约：退避时长按"攒够一整批"计算，且单次等待不超过上限。
#[test]
fn throttle_wait_scales_with_batch_target() {
    // rate=1000，攒 500 条 → 500ms
    assert_eq!(throttle_wait_ms(500, 0, 1_000), 500);
    // rate=200，攒 500 条 → 2500ms，被 MAX_THROTTLE_WAIT_MS(1000) 截断
    assert_eq!(throttle_wait_ms(500, 0, 200), 1_000);
    // 已攒 400 条、还差 100 条，rate=1000 → 100ms
    assert_eq!(throttle_wait_ms(500, 400, 1_000), 100);
    // 至少等 1ms（available 已满时也保持最小退避）
    assert_eq!(throttle_wait_ms(0, 0, 1_000), 1);
}

// ============================================================================
// I-47（第二部分）/ I-49：发布参数（topic / retain / qos）
// ============================================================================
//
// 生产代码里的 `client.publish(...)` 直接作用于 rumqttc 的 AsyncClient，没有可
// 注入的 mock trait。为了断言**真实下发的报文参数**（而不是读源码字符串），这里
// 起一个最小 MQTT 3.1.1 broker 桩（真 TCP）：处理 CONNECT/CONNACK、PUBLISH/
// PUBACK、PINGREQ/PINGRESP，并记录每条 PUBLISH 的 topic / retain / qos。

/// 一条被 broker 桩收到的 PUBLISH。
#[derive(Clone, Debug)]
struct RecordedPublish {
    topic: String,
    retain: bool,
    qos: u8,
    payload: Vec<u8>,
}

/// 测试用最小 MQTT broker 桩（只接受本测试客户端的连接）。
struct MockBroker {
    addr: String,
    received: Arc<parking_lot::Mutex<Vec<RecordedPublish>>>,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl MockBroker {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock broker 绑定端口失败");
        let addr = listener
            .local_addr()
            .expect("读取 mock broker 本地地址失败")
            .to_string();
        let received: Arc<parking_lot::Mutex<Vec<RecordedPublish>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let received_for_task = received.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        let received = received_for_task.clone();
                        tokio::spawn(async move {
                            let _ = serve_mock_connection(socket, received).await;
                        });
                    }
                }
            }
        });

        Self {
            addr,
            received,
            shutdown,
        }
    }

    fn broker_url(&self) -> String {
        format!("tcp://{}", self.addr)
    }

    fn publishes(&self) -> Vec<RecordedPublish> {
        self.received.lock().clone()
    }

    /// 等待发往 `topic` 的 PUBLISH 达到 `n` 条。
    async fn wait_for_topic(&self, topic: &str, n: usize, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if self
                .received
                .lock()
                .iter()
                .filter(|p| p.topic == topic)
                .count()
                >= n
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }
}

impl Drop for MockBroker {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn serve_mock_connection(
    mut socket: tokio::net::TcpStream,
    received: Arc<parking_lot::Mutex<Vec<RecordedPublish>>>,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut body = Vec::new();
    loop {
        let Some((pkt_type, flags)) = read_mqtt_packet(&mut socket, &mut body).await? else {
            return Ok(());
        };
        match pkt_type {
            // CONNECT → CONNACK（MQTT 3.1.1，返回码 0 = 接受）
            1 => socket.write_all(&[0x20, 0x02, 0x00, 0x00]).await?,
            // PUBLISH：记录 topic/retain/qos，QoS1 回 PUBACK
            3 => {
                let qos = (flags >> 1) & 0x03;
                let retain = (flags & 0x01) != 0;
                let invalid = |msg: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, msg);
                if body.len() < 2 {
                    return Err(invalid("PUBLISH 报文缺少 topic 长度"));
                }
                let topic_len = u16::from_be_bytes([body[0], body[1]]) as usize;
                if body.len() < 2 + topic_len {
                    return Err(invalid("PUBLISH 报文 topic 被截断"));
                }
                let topic = String::from_utf8_lossy(&body[2..2 + topic_len]).into_owned();
                let mut offset = 2 + topic_len;
                let packet_id = if qos > 0 {
                    if body.len() < offset + 2 {
                        return Err(invalid("PUBLISH 报文缺少 packet_id"));
                    }
                    let id = u16::from_be_bytes([body[offset], body[offset + 1]]);
                    offset += 2;
                    id
                } else {
                    0
                };
                received.lock().push(RecordedPublish {
                    topic,
                    retain,
                    qos,
                    payload: body[offset..].to_vec(),
                });
                if qos == 1 {
                    let puback = [0x40, 0x02, (packet_id >> 8) as u8, packet_id as u8];
                    socket.write_all(&puback).await?;
                }
            }
            // PINGREQ → PINGRESP
            12 => socket.write_all(&[0xD0, 0x00]).await?,
            // DISCONNECT：客户端主动断开
            14 => return Ok(()),
            _ => {}
        }
    }
}

/// 读取一个完整的 MQTT 报文，返回 (报文类型, 标志位)；EOF 返回 None。
async fn read_mqtt_packet(
    socket: &mut tokio::net::TcpStream,
    body: &mut Vec<u8>,
) -> std::io::Result<Option<(u8, u8)>> {
    use tokio::io::AsyncReadExt;

    let mut first = [0u8; 1];
    match socket.read_exact(&mut first).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let mut remaining = 0usize;
    let mut multiplier = 1usize;
    loop {
        let mut b = [0u8; 1];
        socket.read_exact(&mut b).await?;
        remaining += ((b[0] & 0x7F) as usize) * multiplier;
        if b[0] & 0x80 == 0 {
            break;
        }
        multiplier *= 128;
    }

    body.clear();
    body.resize(remaining, 0);
    if remaining > 0 {
        socket.read_exact(&mut body[..]).await?;
    }
    Ok(Some(((first[0] >> 4) & 0x0F, first[0] & 0x0F)))
}

/// 构造指向 mock broker 的 MQTT 配置。
fn test_config(broker: &MockBroker, topic: &str) -> MqttConfig {
    MqttConfig {
        enabled: true,
        broker: broker.broker_url(),
        topic: topic.to_string(),
        publish_interval: 1,
        qos: 1,
        keep_alive: Some(30),
        ack_timeout_secs: Some(2),
        ..MqttConfig::default()
    }
}

/// I-47（第二部分）：所有设备分组共用同一个 `config.topic`，且 `retain=false`。
///
/// 报文按 device_id 分组嵌套在同一 payload 里，只发一条 PUBLISH；topic 必须
/// 恒等于配置 topic（不得按设备拆分），retain 必须为 false（retain 只用于
/// `<topic>/status` 在线心跳，数据报文置 retain 会让 broker 保留最后一份数据、
/// 新订阅者一上线就收到陈旧值）。
#[tokio::test]
async fn publish_uses_single_topic_without_retain() {
    let broker = MockBroker::start().await;
    let topic = "gw/i47";
    let config = test_config(&broker, topic);

    let queue = Arc::new(DataQueue::new_memory().expect("内存队列初始化失败"));
    let points = vec![
        DataPoint::good("p1", "dev1", 1.0, 1.0, "u16"),
        DataPoint::good("p2", "dev1", 2.0, 2.0, "u16"),
        DataPoint::good("p3", "dev2", 3.0, 3.0, "u16"),
    ];
    queue.enqueue(points).await.expect("入队失败");

    let client = MqttClient::new(config, queue.clone(), None, None, Arc::new(Metrics::new()))
        .expect("MqttClient 构造失败");
    assert!(
        client.wait_connected(Duration::from_secs(5)).await,
        "5s 内应连上 mock broker"
    );

    let _ = client.publish_batch(DataPoint::now()).await;
    assert!(
        broker
            .wait_for_topic(topic, 1, Duration::from_secs(5))
            .await,
        "5s 内应收到发往 config.topic 的数据 PUBLISH"
    );

    let published = broker.publishes();
    let data: Vec<&RecordedPublish> = published.iter().filter(|p| p.topic == topic).collect();
    assert_eq!(
        data.len(),
        1,
        "I-47：一批多设备数据必须只发一条 PUBLISH（同一 config.topic），实际收到 {:?}",
        published
            .iter()
            .map(|p| p.topic.as_str())
            .collect::<Vec<_>>()
    );

    let msg = data[0];
    assert_eq!(
        msg.topic, topic,
        "I-47：数据发布 topic 必须恒等于 config.topic，不得按设备拆分"
    );
    assert!(
        !msg.retain,
        "I-47：数据发布必须 retain=false（retain 仅用于 <topic>/status 心跳）"
    );
    let status_topic = format!("{}/status", topic);
    assert!(
        published
            .iter()
            .all(|p| p.topic == topic || p.topic == status_topic),
        "I-47：不得出现按设备派生的其它 topic，实际 {:?}",
        published
            .iter()
            .map(|p| p.topic.as_str())
            .collect::<Vec<_>>()
    );

    // 默认 JSON 报文首字节是压缩标志（0x00=未压缩），其余是设备报文数组。
    assert_eq!(
        msg.payload.first(),
        Some(&0x00),
        "前置条件：默认未超过压缩阈值，报文首字节应为未压缩标志 0x00"
    );
    let v: serde_json::Value =
        serde_json::from_slice(&msg.payload[1..]).expect("数据报文应为合法 JSON");
    let devices: Vec<&str> = v
        .as_array()
        .expect("顶层应为设备报文数组")
        .iter()
        .map(|r| r["device_id"].as_str().unwrap_or_default())
        .collect();
    assert!(
        devices.contains(&"dev1") && devices.contains(&"dev2"),
        "I-47：同一 topic 的单条报文应包含全部设备分组，实际 {:?}",
        devices
    );

    client.stop().await;
}

/// I-49：发布与 outbox 记录的 `qos` 恒为 1（配置校验侧已有
/// `roi23_qos_only_accepts_1`，这里补发布侧实现）。
///
/// QoS1 是 at-least-once 语义与 PubAck/重发链路的前提：发布侧一旦降级为 QoS0，
/// 断网瞬间的消息既无 PubAck 也无 outbox 重放（虽然记录仍会落库），队列却会
/// 直接 ack 删除——静默丢数据。
#[tokio::test]
async fn publish_always_uses_qos_one() {
    let broker = MockBroker::start().await;
    let topic = "gw/i49";
    let config = test_config(&broker, topic);

    let queue = Arc::new(DataQueue::new_memory().expect("内存队列初始化失败"));
    queue
        .enqueue(vec![
            DataPoint::good("p1", "dev1", 1.0, 1.0, "u16"),
            DataPoint::good("p2", "dev1", 2.0, 2.0, "u16"),
        ])
        .await
        .expect("入队失败");

    let client = MqttClient::new(config, queue.clone(), None, None, Arc::new(Metrics::new()))
        .expect("MqttClient 构造失败");

    // 接管 outbox 命令通道，捕获「将落库」的那条 OutgoingMsg（qos/topic）
    let (outbox_tx, mut outbox_rx) = mpsc::channel::<OutboxCmd>(8);
    client.outbox_cmd_tx.store(Some(Arc::new(outbox_tx)));

    assert!(
        client.wait_connected(Duration::from_secs(5)).await,
        "5s 内应连上 mock broker"
    );

    let _ = client.publish_batch(DataPoint::now()).await;
    assert!(
        broker
            .wait_for_topic(topic, 1, Duration::from_secs(5))
            .await,
        "5s 内应收到发往 config.topic 的数据 PUBLISH"
    );

    let published = broker.publishes();
    let data: Vec<&RecordedPublish> = published.iter().filter(|p| p.topic == topic).collect();
    assert!(!data.is_empty(), "I-49：应至少收到一条数据 PUBLISH");
    for msg in data {
        assert_eq!(
            msg.qos, 1,
            "I-49：数据发布必须使用 QoS1（topic={}，实际 qos={}）",
            msg.topic, msg.qos
        );
    }

    let mut persisted: Vec<(String, u8)> = Vec::new();
    while let Ok(cmd) = outbox_rx.try_recv() {
        if let OutboxCmd::Persist(msg) = cmd {
            persisted.push((msg.topic, msg.qos));
        }
    }
    assert_eq!(
        persisted.len(),
        1,
        "I-49：一批数据应只产生 1 条 outbox 待发送记录"
    );
    assert_eq!(
        persisted[0].0, topic,
        "I-49：outbox 记录的 topic 必须等于 config.topic"
    );
    assert_eq!(
        persisted[0].1, 1,
        "I-49：outbox 落库的 qos 必须恒为 1（否则重连重放语义与 QoS1 不一致）"
    );

    client.stop().await;
}
