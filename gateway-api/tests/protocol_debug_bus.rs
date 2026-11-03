//! P0-1 真实帧下发 / 会话真实回放 —— 接口契约验收（契约 §8）
//!
//! 用注入的假总线替代真实串口/TCP：handler 走完整真实路径（hex 校验 → 总线准入 →
//! 下发 → CRC/异常判定 → 审计），只有物理层被替换，因此能覆盖契约里所有
//! 与硬件无关的分支（超时、CRC 错、异常帧、总线占用、参数错误）。
//!
//! 无法在无硬件环境验证的项：真机下的物理时序、请求取消时的句柄释放（§8 #8）。
//! 这两项依赖真实端口，留给现场联调；其代码路径由「drop 即释放」的 RAII 结构保证。

mod common;

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::mock_app_state_with_debug_bus;
use futures::StreamExt;
use gateway_api::web::debug_bus::{BusErrorKind, DebugBusTrait, ExchangeOutcome};
use gateway_core::config::BusConfig;
use gateway_core::south::protocol::Collector;
use parking_lot::Mutex;
use serde_json::Value;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";
const BUS: &str = "/dev/ttyTEST0";

/// 同一测试二进制内所有用例共用 `BUS` 这一条端口名，而「总线调试锁」是进程级全局表
/// （生产中它保证同总线互斥）。并发执行会让真实下发类用例互相判为 `bus_busy`，
/// 因此这里把会申请租约的用例顺序化；不影响各用例自身的断言强度。
static BUS_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn lock_bus() -> tokio::sync::MutexGuard<'static, ()> {
    BUS_GUARD.lock().await
}

/// 假总线：按队列依次回放预设结果，队列耗尽后统一超时
struct FakeBus {
    queue: Mutex<VecDeque<Result<Vec<u8>, BusErrorKind>>>,
    requests: Mutex<Vec<Vec<u8>>>,
    calls: AtomicUsize,
    delay_ms: u64,
}

impl FakeBus {
    fn new(queue: Vec<Result<Vec<u8>, BusErrorKind>>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(queue.into()),
            requests: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            delay_ms: 0,
        })
    }

    /// 每次收发固定延时（模拟慢总线/慢设备），用于心跳与中断用例
    fn with_delay(queue: Vec<Result<Vec<u8>, BusErrorKind>>, delay_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(queue.into()),
            requests: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            delay_ms,
        })
    }
}

impl DebugBusTrait for FakeBus {
    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
        _timeout_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<ExchangeOutcome, BusErrorKind>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().push(request.to_vec());
        let next = self.queue.lock().pop_front();
        let delay = self.delay_ms;
        Box::pin(async move {
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            match next {
                Some(Ok(bytes)) => Ok(ExchangeOutcome {
                    bytes,
                    elapsed_ms: 7,
                }),
                Some(Err(k)) => Err(k),
                None => Err(BusErrorKind::Timeout),
            }
        })
    }
}

/// 只填必填字段，其余走 serde 默认值（与真实配置文件同一条解析路径）
fn bus_cfg(port: &str) -> BusConfig {
    serde_json::from_value(serde_json::json!({
        "port_name": port,
        "protocol": "modbus",
    }))
    .expect("BusConfig 应能用最少字段反序列化")
}

fn request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn call(state: Arc<gateway_api::web::AppState>, req: Request<Body>) -> (StatusCode, Value) {
    let router = gateway_api::web::build_router(state);
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json)
}

/// 拼一条带正确 CRC16 的 Modbus RTU 帧
fn rtu_frame(prefix: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    let crc = gateway_core::engine::util::crc::crc16(prefix);
    v.extend_from_slice(&crc.to_le_bytes());
    v
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

const SEND_URI: &str = "/api/protocol-tools/packet-editor/send";

#[tokio::test]
async fn 合法帧下发_返回真实响应且无错误码() {
    let _g = lock_bus().await;
    // 从站1 读保持寄存器 0-1 → 正常回 4 字节
    let response = rtu_frame(&[0x01, 0x03, 0x04, 0x00, 0x2A, 0x00, 0x00]);
    let bus = FakeBus::new(vec![Ok(response.clone())]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 03 00 00 00 02 C4 0B", "bus": BUS }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    let r = &body["data"];
    assert_eq!(r["success"], true, "body={body}");
    assert_eq!(r["error"], Value::Null);
    assert_eq!(r["response_hex"], hex(&response));
    assert_eq!(r["request_hex"], "01 03 00 00 00 02 C4 0B");
    assert_eq!(r["latency_ms"], 7);
    // 下发的是解析后的真实字节，未改动
    assert_eq!(
        bus.requests.lock()[0],
        vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x02, 0xC4, 0x0B]
    );
}

#[tokio::test]
async fn 设备不应答_报timeout且响应帧为空() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![Err(BusErrorKind::Timeout)]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 03 00 00 00 02 C4 0B", "bus": BUS, "timeout_ms": 800 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["success"], false);
    assert_eq!(body["data"]["response_hex"], "");
    assert_eq!(body["data"]["error"], "timeout");
}

#[tokio::test]
async fn crc错的响应帧_报crc() {
    let _g = lock_bus().await;
    let mut response = rtu_frame(&[0x01, 0x03, 0x04, 0x00, 0x2A, 0x00, 0x00]);
    let last = response.len() - 1;
    response[last] ^= 0xFF; // 破坏 CRC
    let bus = FakeBus::new(vec![Ok(response)]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 03 00 00 00 02 C4 0B", "bus": BUS }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["success"], false);
    assert_eq!(body["data"]["error"], "crc");
}

#[tokio::test]
async fn 异常响应帧_报exception并给出异常码释义() {
    let _g = lock_bus().await;
    // 01 83 02：功能码 03|0x80，异常码 02（非法数据地址）
    let response = rtu_frame(&[0x01, 0x83, 0x02]);
    let bus = FakeBus::new(vec![Ok(response)]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 03 00 00 00 02 C4 0B", "bus": BUS }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["error"], "exception");
    let parsed = body["data"]["parsed"].as_str().unwrap();
    assert!(
        parsed.contains("非法"),
        "异常码 02 的释义应含「非法」，实际：{parsed}"
    );
}

#[tokio::test]
async fn bus不存在_返回404与bus_not_found() {
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], FakeBus::new(vec![]));

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 03 00 00 00 02", "bus": "/dev/does-not-exist" }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "bus_not_found");
}

#[tokio::test]
async fn hex非法_返回400与invalid_frame() {
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], FakeBus::new(vec![]));

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 0G", "bus": BUS }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_frame");
}

#[tokio::test]
async fn 缺bus_返回400与missing_bus() {
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], FakeBus::new(vec![]));

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 03 00 00 00 02 C4 0B" }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "missing_bus");
}

#[tokio::test]
async fn 采集引擎占用总线时_报bus_busy() {
    // 用带 collector 的 scheduler 覆盖默认空实现
    let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
    collectors.insert(
        BUS.to_string(),
        Arc::new(common::mock_state::MockCollector::new(BUS)),
    );
    let state = common::mock_state::mock_app_state_with_debug_bus_and_collectors(
        vec![bus_cfg(BUS)],
        FakeBus::new(vec![]),
        collectors,
    );

    let (status, body) = call(
        state,
        request(
            "POST",
            SEND_URI,
            serde_json::json!({ "hex": "01 03 00 00 00 02", "bus": BUS }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"], "bus_busy");
}

const REPLAY_URI: &str = "/api/protocol-tools/packet-replay/sessions/s2/execute";

#[tokio::test]
async fn 回放_录制无响应的会话本次仍无响应记match() {
    let _g = lock_bus().await;
    // s2 是「PLC#4 离线故障」会话：3 条请求，录制时均无响应。
    // 本次设备依然不回 → 行为一致，应全部记 match 而不是 mismatch。
    let bus = FakeBus::new(vec![]); // 队列耗尽 → 统一超时
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (status, body) = call(
        state,
        request(
            "POST",
            REPLAY_URI,
            serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let r = &body["data"];
    assert_eq!(r["total"], 3);
    assert_eq!(r["match"], 3);
    assert_eq!(r["mismatch"], 0);
    assert_eq!(r["failed"], 0);
    // results[] 只含 request 帧，且顺序与会话一致
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0]["packet_id"], "p1");
    assert_eq!(results[1]["packet_id"], "p3");
    assert_eq!(results[2]["packet_id"], "p5");
    assert_eq!(results[0]["status"], "match");
    assert_eq!(results[0]["actual_response_hex"], "");
    assert_eq!(results[0]["error"], Value::Null);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn 回放_期望与实际不一致记mismatch且逐帧对比() {
    let _g = lock_bus().await;
    // s2 录制期望为空；本次设备回了数据 → mismatch
    let bus = FakeBus::new(vec![
        Ok(vec![0x04, 0x03, 0x02, 0x00, 0x01]),
        Ok(vec![0x04, 0x03, 0x02, 0x00, 0x02]),
        Ok(vec![0x04, 0x03, 0x02, 0x00, 0x03]),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (_, body) = call(
        state,
        request(
            "POST",
            REPLAY_URI,
            serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;

    let r = &body["data"];
    assert_eq!(r["total"], 3);
    assert_eq!(r["mismatch"], 3);
    assert_eq!(r["results"][0]["status"], "mismatch");
    assert_eq!(r["results"][0]["actual_response_hex"], "04 03 02 00 01");
}

#[tokio::test]
async fn 回放_stop_on_mismatch首帧即停() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![Ok(vec![0x04, 0x03, 0x02, 0x00, 0x01])]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (_, body) = call(
        state,
        request(
            "POST",
            REPLAY_URI,
            serde_json::json!({
                "bus": BUS, "timeout_ms": 100, "interval_ms": 20, "stop_on_mismatch": true
            }),
        ),
    )
    .await;

    assert_eq!(body["data"]["total"], 1);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn 回放会话不存在返回404() {
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], FakeBus::new(vec![]));

    let (status, _) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/packet-replay/sessions/nope/execute",
            serde_json::json!({ "bus": BUS }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ===== P0-1b：流式回放（SSE）契约验收（契约 §7 #11-16）=====
// 增量送达与中间件穿透已在 spike 中用真实 socket 验证（契约 §8），
// 这里用 oneshot 收集完整 SSE body 后按事件序列/内容断言。

/// 带 `Accept: text/event-stream` 的请求（触发流式模式，契约 §2）
fn sse_request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// 发起流式请求并收集完整 SSE body 文本（流结束时返回响应头 + 全文）
async fn call_sse(
    state: Arc<gateway_api::web::AppState>,
    req: Request<Body>,
) -> (axum::http::HeaderMap, String) {
    let router = gateway_api::web::build_router(state);
    let resp = router.oneshot(req).await.unwrap();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .unwrap();
    (headers, String::from_utf8(body.to_vec()).unwrap())
}

/// 按行提取 SSE 事件名序列（`:hb` 心跳注释天然不含 event: 行，不干扰断言）
fn sse_events(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|l| l.strip_prefix("event: "))
        .collect()
}

#[tokio::test]
async fn 回放流式_事件序列完整且计数与阻塞一致() {
    let _g = lock_bus().await;
    // s2：3 帧录制均无响应；本次设备仍不回（空队列 → 超时）→ 全 match
    let bus = FakeBus::new(vec![]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (headers, text) = call_sse(
        state,
        sse_request(
            "POST",
            REPLAY_URI,
            serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;

    // 契约 §3 响应头
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap()),
        Some("text/event-stream")
    );
    assert_eq!(
        headers
            .get("x-accel-buffering")
            .map(|v| v.to_str().unwrap()),
        Some("no")
    );
    // 契约 §3 事件序列：started → frame×3 → summary
    assert_eq!(
        sse_events(&text),
        ["started", "frame", "frame", "frame", "summary"],
        "body={text}"
    );
    // started 携带计划总帧数，前端据此初始化进度条
    assert!(text.contains("\"total\":3"), "body={text}");
    // frame 顺序与会话一致（s2 的请求帧为 p1/p3/p5）
    let i1 = text.find("\"packet_id\":\"p1\"").expect("缺 p1 帧");
    let i3 = text.find("\"packet_id\":\"p3\"").expect("缺 p3 帧");
    let i5 = text.find("\"packet_id\":\"p5\"").expect("缺 p5 帧");
    assert!(i1 < i3 && i3 < i5, "帧顺序应与会话一致，body={text}");
    // summary 即 v1 §4 ReplayExecuteResult（JSON 字段 match/mismatch）
    assert!(text.contains("\"match\":3"), "body={text}");
    assert!(text.contains("\"mismatch\":0"), "body={text}");
    assert_eq!(bus.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn 回放流式_客户端中断后停止且总线锁释放() {
    let _g = lock_bus().await;
    // 每帧延时 300ms：读完首帧事件即断开，后端应在 ≤(timeout+interval) 内停止
    let bus = FakeBus::with_delay(vec![Ok(vec![0x01]), Ok(vec![0x02]), Ok(vec![0x03])], 300);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let router = gateway_api::web::build_router(state.clone());
    let resp = router
        .oneshot(sse_request(
            "POST",
            REPLAY_URI,
            serde_json::json!({ "bus": BUS, "timeout_ms": 1000, "interval_ms": 20 }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap()),
        Some("text/event-stream")
    );

    // 读到首个 frame 事件即断开（模拟用户点「停止」/关页面）
    let mut stream = resp.into_body().into_data_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        if buf.contains("event: frame") {
            break;
        }
    }
    drop(stream);

    // 租约随任务收尾释放：紧接着的执行不允许一直 bus_busy（契约 §7 #12）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let (status, body) = call(
            state.clone(),
            request(
                "POST",
                REPLAY_URI,
                serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
            ),
        )
        .await;
        if body["error"] != "bus_busy" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["code"], 0, "中断后再次执行应正常返回，body={body}");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "中断后 3s 内总线锁未释放"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn 回放_普通accept头_保持阻塞json行为() {
    let _g = lock_bus().await;
    // 契约 §7 #13：Accept 不含 text/event-stream（如 axios 默认值）→ v1 阻塞行为逐字节不变
    let bus = FakeBus::new(vec![]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let mut req = request(
        "POST",
        REPLAY_URI,
        serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
    );
    req.headers_mut().insert(
        header::ACCEPT,
        "application/json, text/plain, */*".parse().unwrap(),
    );
    let (status, body) = call(state, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    // v1 汇总结构：data.results 全量、计数齐全
    assert_eq!(body["data"]["total"], 3);
    assert_eq!(body["data"]["match"], 3);
    assert_eq!(body["data"]["results"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn 回放流式_总线中途掉线_发failed后正常关流() {
    let _g = lock_bus().await;
    // 帧1 超时（录制无响应 → match），帧2 总线掉线 → error + failed 收尾（契约 §7 #14）
    let bus = FakeBus::new(vec![
        Err(BusErrorKind::Timeout),
        Err(BusErrorKind::BusOffline),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (_, text) = call_sse(
        state,
        sse_request(
            "POST",
            REPLAY_URI,
            serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;

    // to_bytes 能完整返回即证明流正常关闭、无悬挂任务
    assert_eq!(
        sse_events(&text),
        ["started", "frame", "frame", "failed"],
        "body={text}"
    );
    assert!(text.contains("\"error\":\"bus_offline\""), "body={text}");
    assert!(text.contains("\"completed\":2"), "body={text}");
    assert!(text.contains("\"total\":3"), "body={text}");
    assert!(
        !text.contains("event: summary"),
        "failed 收尾不应再有 summary，body={text}"
    );
}

#[tokio::test]
async fn 回放流式_空闲超5秒收到心跳注释() {
    let _g = lock_bus().await;
    // 首帧延时 5.5s > 心跳间隔 5s（契约 §7 #15）；
    // 首帧 mismatch + stop_on_mismatch 让本用例在 ~5.6s 收尾，不拖慢测试套件
    let bus = FakeBus::with_delay(vec![Ok(vec![0x04, 0x03, 0x02, 0x00, 0x01])], 5500);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (_, text) = call_sse(
        state,
        sse_request(
            "POST",
            REPLAY_URI,
            serde_json::json!({
                "bus": BUS, "timeout_ms": 10000, "interval_ms": 20, "stop_on_mismatch": true
            }),
        ),
    )
    .await;

    // axum KeepAlive 的注释行格式为 `: hb`（冒号后带空格，仍是合法 SSE 注释）
    assert!(
        text.lines().any(|l| l.starts_with(':')),
        "5 秒无事件应收到心跳注释行，body={text}"
    );
    assert_eq!(
        sse_events(&text),
        ["started", "frame", "summary"],
        "body={text}"
    );
}

#[tokio::test]
async fn 回放流式_同总线并发第二场返回bus_busy() {
    let _g = lock_bus().await;
    // 契约 v1 §6.1（与扫描 #28 同一互斥约定）：首场回放进行中，第二场立即 bus_busy。
    // 回归历史 bug：async 块只捕获句柄、互斥守卫留在 spawn_replay 栈帧被提前 drop，
    // 导致会话进行中互斥失效（现已由 `SessionBus` 把句柄与守卫绑定为一体，类型层面免疫）。
    let bus = FakeBus::with_delay(
        vec![
            Ok(vec![0x04, 0x03, 0x02, 0x00, 0x01]),
            Ok(vec![0x04, 0x03, 0x02, 0x00, 0x02]),
            Ok(vec![0x04, 0x03, 0x02, 0x00, 0x03]),
        ],
        200,
    );
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let router = gateway_api::web::build_router(state.clone());
    let resp = router
        .oneshot(sse_request(
            "POST",
            REPLAY_URI,
            serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
        ))
        .await
        .unwrap();
    // 读到 started（租约已建立）再发第二场
    let mut stream = resp.into_body().into_data_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        if buf.contains("event: started") {
            break;
        }
    }

    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            REPLAY_URI,
            serde_json::json!({ "bus": BUS, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;
    assert_eq!(body["error"], "bus_busy");
    assert_eq!(body["code"], 4090);

    // 首场跑完 3 帧并正常收尾（租约在测试内释放干净）
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
    }
    assert_eq!(
        sse_events(&buf),
        ["started", "frame", "frame", "frame", "summary"],
        "首场回放应不受干扰跑完，body={buf}"
    );
    assert_eq!(bus.calls.load(Ordering::SeqCst), 3, "第二场未触达总线");
}

// ===== P0-2：从站扫描真实化契约验收（契约 p0-2 §8 #17-28）=====

const SCAN_URI: &str = "/api/protocol-tools/slave-scan";

/// 契约探测帧：`{id} 03 00 00 00 01 + CRC16`
fn probe(id: u8) -> Vec<u8> {
    rtu_frame(&[id, 0x03, 0x00, 0x00, 0x00, 0x01])
}

#[tokio::test]
async fn 扫描_阻塞_只响应的地址在线且探测帧正确() {
    let _g = lock_bus().await;
    // 契约 #17（兼 #25 回归：无 Accept 头走阻塞模式，响应结构与 V1 mock 同形）：
    // 仅 id=2 回正常帧，其余超时
    let bus = FakeBus::new(vec![
        Err(BusErrorKind::Timeout),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
        Err(BusErrorKind::Timeout),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (status, body) = call(
        state,
        request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 3 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    let arr = body["data"].as_array().expect("data 应为结果数组");
    assert_eq!(arr.len(), 3, "每个地址一条结果，body={body}");
    // 结果按地址升序；仅 id=2 在线，func_code="03"，latency 真实（FakeBus 恒 7ms）
    assert_eq!(arr[0]["slave_id"], 1);
    assert_eq!(arr[0]["responded"], false);
    assert_eq!(arr[1]["slave_id"], 2);
    assert_eq!(arr[1]["responded"], true);
    assert_eq!(arr[1]["func_code"], "03");
    assert_eq!(arr[1]["latency_ms"], 7);
    assert_eq!(arr[1]["device_name"], Value::Null, "真实扫描无从得知设备名");
    assert_eq!(arr[2]["slave_id"], 3);
    assert_eq!(arr[2]["responded"], false);
    // 探测帧内容：逐地址 {id} 03 00 00 00 01 + 正确 CRC
    let reqs = bus.requests.lock();
    assert_eq!(reqs.len(), 3);
    for (i, id) in [1u8, 2, 3].iter().enumerate() {
        assert_eq!(
            reqs[i],
            probe(*id),
            "第 {} 次探测帧应为契约探测帧（id={}）",
            i + 1,
            id
        );
    }
}

#[tokio::test]
async fn 扫描_异常帧算在线且功能码取实际应答() {
    let _g = lock_bus().await;
    // 契约 #18：异常帧 83 02（非法地址）→ 协议层有应答即在线
    let bus = FakeBus::new(vec![Ok(rtu_frame(&[0x05, 0x83, 0x02]))]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (status, body) = call(
        state,
        request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 5, "range_end": 5 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let arr = body["data"].as_array().unwrap();
    assert_eq!(arr[0]["responded"], true, "异常帧应判在线，body={body}");
    assert_eq!(arr[0]["func_code"], "83");
}

#[tokio::test]
async fn 扫描流式_crc错帧计入noise且判不在线() {
    let _g = lock_bus().await;
    // 契约 #19：CRC 错帧 → responded=false + summary.noise=1
    let mut bad = rtu_frame(&[0x07, 0x03, 0x02, 0x00, 0x2A]);
    let last = bad.len() - 1;
    bad[last] ^= 0xFF;
    let bus = FakeBus::new(vec![Ok(bad)]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (_, text) = call_sse(
        state,
        sse_request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 7, "range_end": 7 }),
        ),
    )
    .await;

    assert_eq!(
        sse_events(&text),
        ["started", "slave", "summary"],
        "body={text}"
    );
    assert!(text.contains("\"responded\":false"), "body={text}");
    assert!(
        text.contains("\"noise\":1"),
        "CRC 错帧应计入 noise，body={text}"
    );
    assert!(text.contains("\"found\":0"), "body={text}");
}

#[tokio::test]
async fn 扫描_缺bus与bus不存在返回对应机器码() {
    let _g = lock_bus().await;
    // 契约 #20
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], FakeBus::new(vec![]));

    let (status, body) = call(
        state.clone(),
        request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "range_start": 1, "range_end": 3 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "missing_bus");
    assert_eq!(body["code"], 4001);

    let (status, body) = call(
        state,
        request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": "/dev/ttyNOPE", "range_start": 1, "range_end": 3 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "bus_not_found");
}

#[tokio::test]
async fn 扫描_范围非法一律400() {
    let _g = lock_bus().await;
    // 契约 #21：end<start / 越出 1..=247（span>1024 为纵深防御，247 校验先命中，公开路径不可达）
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], FakeBus::new(vec![]));
    for (start, end) in [(5, 3), (0, 5), (1, 248)] {
        let (status, _) = call(
            state.clone(),
            request(
                "POST",
                SCAN_URI,
                serde_json::json!({ "bus": BUS, "range_start": start, "range_end": end }),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "范围 {}..={} 应判 400",
            start,
            end
        );
    }
}

#[tokio::test]
async fn 扫描_非modbus总线返回unsupported_protocol() {
    let _g = lock_bus().await;
    // 契约 #22：探测帧是 Modbus 专属，cjt188 总线明确拒绝而不是扫出假超时
    let cjt_bus: BusConfig = serde_json::from_value(serde_json::json!({
        "port_name": "/dev/ttyCJT",
        "protocol": "cjt188",
    }))
    .unwrap();
    let state = mock_app_state_with_debug_bus(vec![cjt_bus], FakeBus::new(vec![]));

    let (status, body) = call(
        state,
        request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": "/dev/ttyCJT", "range_start": 1, "range_end": 3 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "unsupported_protocol");
    assert_eq!(body["code"], 4002);
}

#[tokio::test]
async fn 扫描流式_事件序列完整且计数一致() {
    let _g = lock_bus().await;
    // 契约 #23：id=2/4 在线，其余超时 → started → slave×5（地址升序）→ summary
    let bus = FakeBus::new(vec![
        Err(BusErrorKind::Timeout),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
        Err(BusErrorKind::Timeout),
        Ok(rtu_frame(&[0x04, 0x83, 0x02])),
        Err(BusErrorKind::Timeout),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (headers, text) = call_sse(
        state,
        sse_request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 5, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;

    // 响应头三件套（与回放同一套共享件）
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap()),
        Some("text/event-stream")
    );
    assert_eq!(
        headers
            .get("x-accel-buffering")
            .map(|v| v.to_str().unwrap()),
        Some("no")
    );
    assert_eq!(
        sse_events(&text),
        ["started", "slave", "slave", "slave", "slave", "slave", "summary"],
        "body={text}"
    );
    assert!(text.contains("\"total\":5"), "body={text}");
    // slave 事件地址升序
    let i1 = text.find("\"slave_id\":1").expect("缺地址1");
    let i2 = text.find("\"slave_id\":2").expect("缺地址2");
    let i5 = text.find("\"slave_id\":5").expect("缺地址5");
    assert!(i1 < i2 && i2 < i5, "slave 事件应按地址升序，body={text}");
    // summary 计数与逐条一致（2 在线：正常帧 + 异常帧），含全量结果
    assert!(text.contains("\"scanned\":5"), "body={text}");
    assert!(text.contains("\"found\":2"), "body={text}");
    assert_eq!(bus.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn 扫描流式_客户端中断后停止且总线锁释放() {
    let _g = lock_bus().await;
    // 契约 #24：每地址延时 300ms，读到首个 slave 事件即断开
    let bus = FakeBus::with_delay(
        vec![
            Ok(rtu_frame(&[0x01, 0x03, 0x02, 0x00, 0x2A])),
            Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
            Ok(rtu_frame(&[0x03, 0x03, 0x02, 0x00, 0x2A])),
        ],
        300,
    );
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let router = gateway_api::web::build_router(state.clone());
    let resp = router
        .oneshot(sse_request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 3, "timeout_ms": 1000, "interval_ms": 20 }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap()),
        Some("text/event-stream")
    );

    let mut stream = resp.into_body().into_data_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        if buf.contains("event: slave") {
            break;
        }
    }
    drop(stream);

    // 租约随任务收尾释放：紧接着的扫描不允许一直 bus_busy
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let (status, body) = call(
            state.clone(),
            request(
                "POST",
                SCAN_URI,
                serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 1, "timeout_ms": 100, "interval_ms": 20 }),
            ),
        )
        .await;
        if body["error"] != "bus_busy" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["code"], 0, "中断后再次扫描应正常返回，body={body}");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "中断后 3s 内总线锁未释放"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn 扫描流式_总线中途掉线发failed_阻塞返回已扫部分() {
    let _g = lock_bus().await;
    // 契约 #26 流式侧：地址1 在线，地址2 掉线 → failed(bus_offline)，无 summary
    let bus = FakeBus::new(vec![
        Ok(rtu_frame(&[0x01, 0x03, 0x02, 0x00, 0x2A])),
        Err(BusErrorKind::BusOffline),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);

    let (_, text) = call_sse(
        state,
        sse_request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 3, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;
    assert_eq!(
        sse_events(&text),
        ["started", "slave", "slave", "failed"],
        "body={text}"
    );
    assert!(text.contains("\"error\":\"bus_offline\""), "body={text}");
    assert!(text.contains("\"scanned\":2"), "body={text}");
    assert!(
        !text.contains("event: summary"),
        "failed 收尾不应再有 summary，body={text}"
    );

    // 契约 #26 阻塞侧：提前收尾返回 200 + 已扫部分（地址连续前缀即「扫到哪个断的」）
    let bus = FakeBus::new(vec![
        Ok(rtu_frame(&[0x01, 0x03, 0x02, 0x00, 0x2A])),
        Err(BusErrorKind::BusOffline),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus);
    let (status, body) = call(
        state,
        request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 3, "timeout_ms": 100, "interval_ms": 20 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let arr = body["data"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "阻塞模式应返回已扫部分结果，body={body}");
    assert_eq!(arr[0]["slave_id"], 1);
    assert_eq!(arr[1]["slave_id"], 2);
}

#[tokio::test]
async fn 扫描_同总线并发第二场返回bus_busy() {
    let _g = lock_bus().await;
    // 契约 #28：首场扫描（每地址 200ms × 5）持有租约期间，第二场立即 bus_busy
    let bus = FakeBus::with_delay(
        (1..=5)
            .map(|id| Ok(rtu_frame(&[id, 0x03, 0x02, 0x00, 0x2A])))
            .collect(),
        200,
    );
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let router = gateway_api::web::build_router(state.clone());
    let resp = router
        .oneshot(sse_request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 5, "timeout_ms": 100, "interval_ms": 20 }),
        ))
        .await
        .unwrap();
    // 读到 started（租约已建立）再发第二场
    let mut stream = resp.into_body().into_data_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        if buf.contains("event: started") {
            break;
        }
    }

    let (status, body) = call(
        state.clone(),
        request(
            "POST",
            SCAN_URI,
            serde_json::json!({ "bus": BUS, "range_start": 1, "range_end": 2, "timeout_ms": 100 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "bus_busy 按契约走 200 + error 字段");
    assert_eq!(body["error"], "bus_busy");
    assert_eq!(body["code"], 4090);

    // 首场不受第二场影响，继续扫完 5 地址并正常收尾（同时保证租约在测试内释放，
    // 不给后续用例留下仍持锁的后台任务）
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
    }
    assert_eq!(
        sse_events(&buf),
        ["started", "slave", "slave", "slave", "slave", "slave", "summary"],
        "首场扫描应不受干扰扫完，body={buf}"
    );
    assert_eq!(bus.calls.load(Ordering::SeqCst), 5, "第二场未触达总线");
}

// ===== P0-3：寄存器扫描真实化契约验收（契约 p0-3 §8 #29-40）=====

const REG_SCAN_URI: &str = "/api/protocol-tools/register-scan";

/// 轮询扫描结果直到 status 非 running 或超时（契约 §2 轮询 GET 模型）
async fn poll_scan_done(state: &Arc<gateway_api::web::AppState>, id: &str) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, body) = call(
            state.clone(),
            request("GET", &format!("{REG_SCAN_URI}/{}", id), Value::Null),
        )
        .await;
        let status = body["data"]["status"].as_str().unwrap_or("");
        if status != "running" {
            return body["data"].clone();
        }
        if std::time::Instant::now() > deadline {
            panic!("扫描 {id} 超时未结束，最后状态：{status}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// 契约 §8 #29：真实扫描 holding 0-3，GET 实时进度 → done，cell 值真实
#[tokio::test]
async fn 寄存器扫描_真实holding扫描结果正确() {
    let _g = lock_bus().await;
    // 4 个地址全部正常响应，值分别为 0x002A, 0x0100, 0x0000, 0xFFFF
    let frames = vec![
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x01, 0x00])),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x00])),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0xFF, 0xFF])),
    ];
    let bus = FakeBus::new(frames);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            REG_SCAN_URI,
            serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 3 }),
        ),
    )
    .await;
    assert_eq!(body["code"], 0);
    let id = body["data"].as_str().unwrap();

    let r = poll_scan_done(&state, id).await;
    assert_eq!(r["status"], "done");
    assert_eq!(r["responded"], 4);
    let cells = r["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 4);
    assert_eq!(cells[0]["value"], 0x2A);
    assert_eq!(cells[1]["value"], 0x100);
    assert_eq!(cells[2]["value"], 0);
    assert_eq!(cells[3]["value"], 65535);
    assert!(cells.iter().all(|c| c["responded"] == true));
}

/// 契约 §8 #30：四种 reg_type 的读帧功能码正确
#[tokio::test]
async fn 寄存器扫描_四种寄存器类型读帧功能码正确() {
    let _g = lock_bus().await;
    let expect: Vec<(&str, u8)> = vec![
        ("holding", 0x03),
        ("input", 0x04),
        ("coils", 0x01),
        ("discrete", 0x02),
    ];
    for (rt, fc) in expect {
        // 单地址，1 个正常响应即可让扫描结束
        let bus = FakeBus::new(vec![Ok(rtu_frame(&[0x01, fc, 0x02, 0x00, 0x01]))]);
        let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
        let (_, body) = call(
            state.clone(),
            request(
                "POST",
                REG_SCAN_URI,
                serde_json::json!({ "bus": BUS, "slave_id": 1, "reg_type": rt, "start_addr": 16, "end_addr": 16 }),
            ),
        )
        .await;
        let id = body["data"].as_str().unwrap();
        poll_scan_done(&state, id).await;
        let req = &bus.requests.lock()[0];
        assert_eq!(req[0], 0x01, "{rt} slave id");
        assert_eq!(req[1], fc, "{rt} 功能码应为 {fc}");
        assert_eq!(&req[2..6], &[0x00, 0x10, 0x00, 0x01], "{rt} 地址+数量");
    }
}

/// 契约 §8 #31：异常帧 → responded=false，error 含 exception
#[tokio::test]
async fn 寄存器扫描_异常帧判不在线且error带exception() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![Ok(rtu_frame(&[0x02, 0x83, 0x02]))]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            REG_SCAN_URI,
            serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_scan_done(&state, id).await;
    let cell = &r["cells"][0];
    assert_eq!(cell["responded"], false, "异常帧不算寄存器存在");
    assert_eq!(cell["func_code"], 0x83);
    assert!(
        cell["error"]
            .as_str()
            .unwrap_or("")
            .starts_with("exception:"),
        "error 应含 exception 释义，实际 {}",
        cell["error"]
    );
}

/// 契约 §8 #32：超时 → responded=false，error=null
#[tokio::test]
async fn 寄存器扫描_超时判不在线且error为null() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![]); // 空队列 → 全部超时
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            REG_SCAN_URI,
            serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 1 }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_scan_done(&state, id).await;
    assert_eq!(r["responded"], 0);
    assert!(r["cells"][0]["responded"] == false);
    assert_eq!(r["cells"][0]["error"], Value::Null);
}

/// 契约 §8 #33：CRC 错 → responded=false，error="crc"
#[tokio::test]
async fn 寄存器扫描_crc错帧error为crc() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![Ok(vec![0x02, 0x03, 0xFF, 0xFF])]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            REG_SCAN_URI,
            serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_scan_done(&state, id).await;
    assert_eq!(r["cells"][0]["responded"], false);
    assert_eq!(r["cells"][0]["error"], "crc");
}

/// 契约 §8 #34：扫描中途总线掉线 → status=failed，已扫 cell 保留
#[tokio::test]
async fn 寄存器扫描_中途掉线status为failed且保留已扫cell() {
    let _g = lock_bus().await;
    // 地址 0 正常，地址 1 掉线
    let bus = FakeBus::new(vec![
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
        Err(BusErrorKind::BusOffline),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            REG_SCAN_URI,
            serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 3 }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_scan_done(&state, id).await;
    assert_eq!(r["status"], "failed");
    let cells = r["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 2, "掉线前已扫 2 个地址");
    assert_eq!(cells[0]["responded"], true);
    assert_eq!(cells[1]["responded"], false, "掉线地址不在线");
}

/// 契约 §8 #35：准入错误矩阵
#[tokio::test]
async fn 寄存器扫描_准入错误返回对应机器码() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    // 缺 bus → missing_bus 4001
    let (s, b) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["error"], "missing_bus");

    // bus 不存在 → bus_not_found 4004
    let (s, b) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": "/dev/no-such", "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["error"], "bus_not_found");

    // slave_id 越界 → 400
    let (s, _) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 999, "reg_type": "holding", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // reg_type 非法 → 400
    let (s, _) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "foobar", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // 范围非法 → 400
    let (s, _) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 10, "end_addr": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// 契约 §8 #35 配套：非 Modbus 总线 → unsupported_protocol 4002
#[tokio::test]
async fn 寄存器扫描_非modbus总线返回unsupported_protocol() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![]);
    let mut cfg = bus_cfg(BUS);
    cfg.protocol = "dlt645".into();
    let state = mock_app_state_with_debug_bus(vec![cfg], bus.clone());
    let (s, b) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["error"], "unsupported_protocol");
    assert_eq!(b["code"], 4002);
}

/// 契约 §8 #36：同总线并发两场扫描 → 第二场 bus_busy
#[tokio::test]
async fn 寄存器扫描_同总线并发第二场返回bus_busy() {
    let _g = lock_bus().await;
    // 首场扫 5 地址，每地址 200ms，持锁约 1s
    let bus = FakeBus::with_delay(
        (0..5)
            .map(|_| Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x01])))
            .collect(),
        200,
    );
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    // 首场启动
    let (_, body) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 4 })),
    )
    .await;
    let id = body["data"].as_str().unwrap();

    // 立即第二场 → bus_busy
    let (s, b) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["error"], "bus_busy");
    assert_eq!(b["code"], 4090);

    // 等首场扫完（释放锁），避免影响后续用例
    poll_scan_done(&state, id).await;
}

/// P3-4：DELETE /register-scan/:id 内含「先中止再移除」语义
/// → 结果条目被清理，后台任务在下一地址前停止并释放总线租约
#[tokio::test]
async fn 寄存器扫描_delete中止并移除结果且释放锁() {
    let _g = lock_bus().await;
    // 首场扫 5 地址，每地址 200ms
    let bus = FakeBus::with_delay(
        (0..5)
            .map(|_| Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x01])))
            .collect(),
        200,
    );
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    let (_, body) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 4 })),
    )
    .await;
    let id = body["data"].as_str().unwrap();

    // 等 1 个地址扫完（~200ms）再 DELETE
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let (_, _) = call(
        state.clone(),
        request("DELETE", &format!("{REG_SCAN_URI}/{id}"), Value::Null),
    )
    .await;

    // 契约：删除即先中止再移除 → 后续 GET 拿不到结果
    let (_, g) = call(
        state.clone(),
        request("GET", &format!("{REG_SCAN_URI}/{id}"), Value::Null),
    )
    .await;
    assert!(g["data"].is_null(), "DELETE 后结果应已移除，实际 body={g}");

    // 后台任务收到 cancel 后停止，租约释放
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // 紧接着再发起一场扫描 → 不应是 bus_busy（锁已释放）
    let bus2 = FakeBus::new(vec![Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x01]))]);
    let state2 = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus2.clone());
    let (_, b) = call(
        state2.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    assert_eq!(
        b["error"],
        Value::Null,
        "DELETE 释放锁后新扫描应成功，实际 body={b}"
    );
    let id2 = b["data"].as_str().unwrap();
    poll_scan_done(&state2, id2).await;
}

/// 契约 §8 #39：suggestions 基于真实 responded cell 生成连续段
#[tokio::test]
async fn 寄存器扫描_suggestions基于真实结果生成连续段() {
    let _g = lock_bus().await;
    // 地址 0,1 响应；2 不响应；3,4 响应 → 两段：[0-1] 和 [3-4]
    let bus = FakeBus::new(vec![
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x01])),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x02])),
        Err(BusErrorKind::Timeout),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x03])),
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x04])),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (_, body) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 4 })),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    poll_scan_done(&state, id).await;

    let (_, b) = call(
        state.clone(),
        request(
            "POST",
            &format!("{REG_SCAN_URI}/{id}/suggestions"),
            Value::Null,
        ),
    )
    .await;
    let suggestions = b["data"].as_array().unwrap();
    assert_eq!(suggestions.len(), 2, "两段连续寄存器");
    assert_eq!(suggestions[0]["address"], "0");
    assert_eq!(suggestions[0]["address_num"], 2);
    assert_eq!(suggestions[1]["address"], "3");
    assert_eq!(suggestions[1]["address_num"], 2);
}

/// 契约 §8 #40：list / delete 仍工作
#[tokio::test]
async fn 寄存器扫描_list与delete正常工作() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x01]))]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (_, body) = call(
        state.clone(),
        request("POST", REG_SCAN_URI, serde_json::json!({ "bus": BUS, "slave_id": 2, "reg_type": "holding", "start_addr": 0, "end_addr": 0 })),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    poll_scan_done(&state, id).await;

    // list 应包含该条
    let (_, b) = call(state.clone(), request("GET", REG_SCAN_URI, Value::Null)).await;
    let list = b["data"].as_array().unwrap();
    assert!(list.iter().any(|r| r["id"] == id), "list 应包含新建扫描");

    // delete → GET 返回 null
    let (_, _) = call(
        state.clone(),
        request("DELETE", &format!("{REG_SCAN_URI}/{}", id), Value::Null),
    )
    .await;
    let (_, b) = call(
        state.clone(),
        request("GET", &format!("{REG_SCAN_URI}/{}", id), Value::Null),
    )
    .await;
    assert_eq!(b["data"], Value::Null, "delete 后 GET 应返回 null");
}

// ===== 协议工具契约用例共用的设备注入辅助 =====

const DEV_ID: &str = "dev_test_001";
const DEV_NAME: &str = "测试设备";

/// 往 state.config 注入一台 Modbus 设备（bus=BUS, slave_id=2）
fn inject_device(state: &Arc<gateway_api::web::AppState>) {
    let mut cfg = state.config.config.load().as_ref().clone();
    cfg.devices = Some(vec![gateway_core::config::DeviceConfig {
        id: DEV_ID.into(),
        name: DEV_NAME.into(),
        template: "default".into(),
        bus: BUS.into(),
        protocol: "modbus".into(),
        address: "2".into(),
        slave_id: Some(2),
        address_map: None,
        interval: None,
    }]);
    state.config.config.store(std::sync::Arc::new(cfg));
}

// ===== P0-5 通讯参数自动探测（契约 §8 #55-#63）=====

const DETECT_RUN_URI: &str = "/api/protocol-tools/auto-detect/run";

/// 轮询探测结果直到 status 非 running 或超时
async fn poll_detect_done(state: &Arc<gateway_api::web::AppState>, id: &str) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, body) = call(
            state.clone(),
            request("GET", &format!("{DETECT_RUN_URI}/{}", id), Value::Null),
        )
        .await;
        let status = body["data"]["status"].as_str().unwrap_or("");
        if status != "running" {
            return body["data"].clone();
        }
        if std::time::Instant::now() > deadline {
            panic!("探测 {id} 超时未结束，最后状态：{status}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// 注入自定义设备（覆盖协议/总线，用于准入错误用例）
fn inject_device_with(
    state: &Arc<gateway_api::web::AppState>,
    protocol: &str,
    bus: &str,
    slave_id: Option<u8>,
) {
    let mut cfg = state.config.config.load().as_ref().clone();
    cfg.devices = Some(vec![gateway_core::config::DeviceConfig {
        id: DEV_ID.into(),
        name: DEV_NAME.into(),
        template: "default".into(),
        bus: bus.into(),
        protocol: protocol.into(),
        address: "2".into(),
        slave_id,
        address_map: None,
        interval: None,
    }]);
    state.config.config.store(std::sync::Arc::new(cfg));
}

/// 探测全部正常应答的帧队列：连通性 1 帧 + 功能码 2 帧 + 耗时 3 帧 + 字节序 1 帧
fn detect_ok_frames() -> Vec<Result<Vec<u8>, BusErrorKind>> {
    vec![
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])), // 1 连通性
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])), // 3 FC03
        Ok(rtu_frame(&[0x02, 0x04, 0x02, 0x00, 0x2A])), // 3 FC04
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])), // 4 耗时 1
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])), // 4 耗时 2
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])), // 4 耗时 3
        Ok(rtu_frame(&[0x02, 0x03, 0x04, 0x00, 0x01, 0x00, 0x02])), // 5 字节序
    ]
}

/// 契约 §8 #55：全部正常响应 → 5 步全 success，value/detail 语义正确
#[tokio::test]
async fn 参数探测_全部正常响应五步全success() {
    let _g = lock_bus().await;
    // 每次收发加 30ms 延时，保证 start 后立即 GET 能看到 running
    let bus = FakeBus::with_delay(detect_ok_frames(), 30);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    inject_device(&state);

    let (s, body) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "start 应成功，body={body}");
    let id = body["data"].as_str().expect("data 应为探测 id").to_string();

    let (_, first) = call(
        state.clone(),
        request("GET", &format!("{DETECT_RUN_URI}/{id}"), Value::Null),
    )
    .await;
    assert_eq!(first["data"]["status"], "running", "初始应 running");
    assert!(
        first["data"]["progress"].as_i64().unwrap_or(100) < 100,
        "初始 progress 应 < 100，body={first}"
    );

    let r = poll_detect_done(&state, &id).await;
    assert_eq!(r["status"], "done");
    assert_eq!(r["progress"], 100);
    assert!(
        r["end_time_ms"].as_i64().is_some(),
        "结束应写入 end_time_ms"
    );
    assert_eq!(r["slave_id"], 2);
    assert_eq!(r["device_name"], DEV_NAME);

    let steps = r["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 5);
    assert!(
        steps.iter().all(|s| s["status"] == "success"),
        "5 步应全 success，实际 {steps:?}"
    );
    assert!(
        steps[0]["value"].as_str().unwrap_or("").contains("ms"),
        "连通性 value 应含实测耗时"
    );
    assert_eq!(steps[1]["value"], "2", "从站地址取设备 slave_id");
    assert_eq!(steps[2]["value"], "03,04");
    assert!(
        steps[3]["value"]
            .as_str()
            .unwrap_or("")
            .starts_with("建议 "),
        "耗时项应给出建议超时"
    );
    assert_eq!(steps[4]["value"], "00 01 00 02", "字节序只报原文 HEX");
    let detail = steps[4]["detail"].as_str().unwrap();
    for key in ["ABCD", "BADC", "CDAB", "DCBA", "需结合点位语义确认"] {
        assert!(detail.contains(key), "detail 缺 {key}，实际 {detail}");
    }
}

/// 契约 §8 #56：设备全程无响应 → 5 步全 fail，整体仍 done
#[tokio::test]
async fn 参数探测_设备无响应五步全fail() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![]); // 队列耗尽 → 统一超时
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    inject_device(&state);

    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_detect_done(&state, id).await;

    assert_eq!(r["status"], "done", "个别项失败不影响整体 done");
    assert_eq!(r["progress"], 100);
    let steps = r["steps"].as_array().unwrap();
    assert!(
        steps.iter().all(|s| s["status"] == "fail"),
        "全超时应全 fail，实际 {steps:?}"
    );
    assert!(
        steps[0]["detail"].as_str().unwrap_or("").contains("无响应"),
        "连通性 detail 应提示设备无响应"
    );
    assert_eq!(steps[4]["detail"], "设备无应答");
}

/// 契约 §8 #57：异常帧应答 → 连通性/从站/功能码算 success，字节序 fail
#[tokio::test]
async fn 参数探测_异常帧证明在线() {
    let _g = lock_bus().await;
    let frames = vec![Ok(rtu_frame(&[0x02, 0x83, 0x02])); 7];
    let bus = FakeBus::new(frames);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    inject_device(&state);

    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_detect_done(&state, id).await;

    let steps = r["steps"].as_array().unwrap();
    assert_eq!(steps[0]["status"], "success", "异常帧 = 设备在线");
    assert!(steps[0]["detail"]
        .as_str()
        .unwrap_or("")
        .starts_with("exception:"));
    assert_eq!(steps[1]["status"], "success");
    assert_eq!(steps[2]["status"], "success", "异常帧也算识别该功能码");
    assert_eq!(steps[2]["value"], "03,04");
    assert_eq!(steps[3]["status"], "success", "异常帧耗时同样可测");
    assert_eq!(steps[4]["status"], "fail", "字节序拿不到 4 个数据字节");
    assert!(steps[4]["detail"]
        .as_str()
        .unwrap_or("")
        .starts_with("exception:"));
}

/// 契约 §8 #58：字节序帧 CRC 错 → 该项 fail detail=crc，其余不受影响
#[tokio::test]
async fn 参数探测_字节序crc错() {
    let _g = lock_bus().await;
    let mut frames = detect_ok_frames();
    let mut bad = rtu_frame(&[0x02, 0x03, 0x04, 0x00, 0x01, 0x00, 0x02]);
    let last = bad.len() - 1;
    bad[last] ^= 0xFF; // 破坏 CRC
    frames[6] = Ok(bad);
    let bus = FakeBus::new(frames);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    inject_device(&state);

    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_detect_done(&state, id).await;

    let steps = r["steps"].as_array().unwrap();
    assert_eq!(steps[4]["status"], "fail");
    assert_eq!(steps[4]["detail"], "crc");
    assert!(steps[..4].iter().all(|s| s["status"] == "success"));
}

/// 契约 §8 #59：准入错误返回对应机器码
#[tokio::test]
async fn 参数探测_准入错误返回对应机器码() {
    let _g = lock_bus().await;

    // a) device_not_found
    let bus = FakeBus::new(vec![]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (s, b) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": "no_such_dev" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["code"], 4004);
    assert_eq!(b["error"], "bus_not_found");

    // b) 非 modbus 设备 → unsupported_protocol
    let bus2 = FakeBus::new(vec![]);
    let st2 = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus2.clone());
    inject_device_with(&st2, "dlt645", BUS, None);
    let (s, b) = call(
        st2.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], 4002);
    assert_eq!(b["error"], "unsupported_protocol");

    // c) 设备未配置总线 → missing_bus
    let bus3 = FakeBus::new(vec![]);
    let st3 = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus3.clone());
    inject_device_with(&st3, "modbus", "", Some(2));
    let (s, b) = call(
        st3.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], 4001);
    assert_eq!(b["error"], "missing_bus");

    // d) 总线被占用 → bus_busy
    let bus4 = FakeBus::with_delay(vec![], 50);
    let st4 = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus4.clone());
    inject_device(&st4);
    let _ = call(
        st4.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    let (_, b) = call(
        st4.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    assert_eq!(b["error"], "bus_busy", "并发应返回 bus_busy，body={b}");
    assert_eq!(b["code"], 4090);
}

/// 契约 §8 #60：探测中 /stop → aborted，已完成步骤保留，租约释放
#[tokio::test]
async fn 参数探测_stop中止后aborted且释放锁() {
    let _g = lock_bus().await;
    let bus = FakeBus::with_delay(vec![], 40);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    inject_device(&state);

    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let (s, _) = call(
        state.clone(),
        request("POST", &format!("{DETECT_RUN_URI}/{id}/stop"), Value::Null),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let r = poll_detect_done(&state, &id).await;
    assert_eq!(r["status"], "aborted");
    let steps = r["steps"].as_array().unwrap();
    assert!(
        steps.iter().any(|s| s["status"] != "pending"),
        "aborted 前已完成的步骤必须保留，实际 {steps:?}"
    );

    // 租约已释放：同总线可立即再次发起
    let (s2, b2) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    assert_eq!(s2, StatusCode::OK);
    assert!(b2["data"].is_string(), "再次发起应拿到新 id，body={b2}");
}

/// 契约 §8 #61：探测中途总线掉线 → failed，已完成步骤保留
#[tokio::test]
async fn 参数探测_中途掉线status为failed() {
    let _g = lock_bus().await;
    // 第 1 帧正常（连通性 success），第 2 帧（功能码 FC03）总线掉线
    let frames = vec![
        Ok(rtu_frame(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
        Err(BusErrorKind::BusOffline),
    ];
    let bus = FakeBus::new(frames);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    inject_device(&state);

    let (_, body) = call(
        state.clone(),
        request(
            "POST",
            DETECT_RUN_URI,
            serde_json::json!({ "device_id": DEV_ID }),
        ),
    )
    .await;
    let id = body["data"].as_str().unwrap();
    let r = poll_detect_done(&state, id).await;

    assert_eq!(r["status"], "failed");
    let steps = r["steps"].as_array().unwrap();
    assert_eq!(steps[0]["status"], "success");
    assert_eq!(steps[1]["status"], "success", "第 2 项复用第 1 项结论");
    assert_eq!(steps[2]["status"], "fail");
    assert!(steps[2]["detail"]
        .as_str()
        .unwrap_or("")
        .contains("bus_offline"));
    assert_eq!(steps[3]["status"], "pending", "掉线后不再执行后续项");
    assert_eq!(steps[4]["status"], "pending");
}

/// 契约 §8 #62：请求体缺 device_id → 422
#[tokio::test]
async fn 参数探测_缺少device_id返回422() {
    let _g = lock_bus().await;
    let bus = FakeBus::new(vec![]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());
    let (s, _) = call(
        state.clone(),
        request("POST", DETECT_RUN_URI, serde_json::json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}
