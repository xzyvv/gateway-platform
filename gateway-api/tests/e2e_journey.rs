//! 端到端交付冒烟：按现场交付顺序走一遍主链路（P1 护栏）
//!
//! 覆盖「开箱 → 认身份 → 看状态 → 读配置 → 校验配置 → 真实扫寄存器 → 看诊断 → 看健康报告」，
//! 每一步都走真实路由、真实中间件、真实 handler，只有物理串口被 `FakeBus` 替换。
//! 目的是用一条命令守住 13 个页面的主干，防止后续改动在「页面还能打开」的表象下悄悄断链。
//!
//! 不覆盖：进程级启动（bootstrap/NTP/看门狗）、真实硬件时序、前端渲染。

mod common;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::mock_app_state_with_debug_bus;
use gateway_api::web::debug_bus::{BusErrorKind, DebugBusTrait, ExchangeOutcome};
use gateway_core::config::BusConfig;
use parking_lot::Mutex;
use serde_json::Value;
use tower::ServiceExt;

const BUS: &str = "/dev/ttySMOKE0";

/// 顺序回放预设应答的假总线；队列耗尽后统一超时
struct FakeBus {
    queue: Mutex<VecDeque<Result<Vec<u8>, BusErrorKind>>>,
    calls: AtomicUsize,
}

impl FakeBus {
    fn new(queue: Vec<Result<Vec<u8>, BusErrorKind>>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(queue.into()),
            calls: AtomicUsize::new(0),
        })
    }
}

impl DebugBusTrait for FakeBus {
    fn exchange<'a>(
        &'a self,
        _request: &'a [u8],
        _timeout_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<ExchangeOutcome, BusErrorKind>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.queue.lock().pop_front();
        Box::pin(async move {
            match next {
                Some(Ok(bytes)) => Ok(ExchangeOutcome {
                    bytes,
                    elapsed_ms: 6,
                }),
                Some(Err(k)) => Err(k),
                None => Err(BusErrorKind::Timeout),
            }
        })
    }
}

fn bus_cfg(port: &str) -> BusConfig {
    serde_json::from_value(serde_json::json!({ "port_name": port, "protocol": "modbus" }))
        .expect("BusConfig 最少字段反序列化")
}

fn rtu_frame(prefix: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.extend_from_slice(&gateway_core::engine::util::crc::crc16(prefix).to_le_bytes());
    v
}

fn request(method: &str, uri: &str, token: Option<&str>, body: Value) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::from(body.to_string())).unwrap()
}

async fn call(state: Arc<gateway_api::web::AppState>, req: Request<Body>) -> (StatusCode, Value) {
    let resp = gateway_api::web::build_router(state)
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 128 * 1024)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// 交付主链路：每一步都断言契约，任一环节断裂即失败并指出环节号。
#[tokio::test]
async fn 交付冒烟_从探活到真实扫描全链路() {
    // 3 个保持寄存器都正常应答（0x0000 / 0x002A / 0x0064），模拟现场设备
    let bus = FakeBus::new(vec![
        Ok(rtu_frame(&[0x01, 0x03, 0x02, 0x00, 0x00])),
        Ok(rtu_frame(&[0x01, 0x03, 0x02, 0x00, 0x2A])),
        Ok(rtu_frame(&[0x01, 0x03, 0x02, 0x00, 0x64])),
    ]);
    let state = mock_app_state_with_debug_bus(vec![bus_cfg(BUS)], bus.clone());

    // ① 探活（容器 / 看门狗依赖，无鉴权）
    let (status, _) = call(
        state.clone(),
        request("GET", "/health/live", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "① /health/live 应返回 200");

    // ② 登录换取 token
    let (status, body) = call(
        state.clone(),
        request(
            "POST",
            "/api/auth/login",
            None,
            serde_json::json!({ "username": "admin", "password": "admin" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "② 登录应成功，body={body}");
    let token = body["token"]
        .as_str()
        .unwrap_or_else(|| panic!("登录响应应携带 token，body={body}"))
        .to_string();

    // ③ 运行状态（总览页首屏）
    let (status, body) = call(
        state.clone(),
        request("GET", "/api/status", Some(&token), Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "③ /api/status 应返回 200");
    assert_eq!(body["code"], 0);
    assert!(!body["data"].is_null(), "状态数据不得为空");

    // ④ 读取当前配置（配置页入口）
    let (status, body) = call(
        state.clone(),
        request("GET", "/api/config", Some(&token), Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "④ /api/config 应返回 200");
    assert_eq!(body["code"], 0);
    let cfg = body["data"].clone();
    assert!(cfg.get("buses").is_some() || cfg.get("devices").is_some());

    // ⑤ 配置校验（保存前的预检，前端「定位」依赖 issues 字段）
    let (status, body) = call(
        state.clone(),
        request(
            "POST",
            "/api/config/validate",
            Some(&token),
            serde_json::json!({ "config": cfg, "templates": [] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "⑤ 配置校验应返回 200，body={body}");
    assert!(body["data"]["valid"].is_boolean());
    assert!(body["data"]["issues"].is_array());

    // ⑥ 真实寄存器扫描：发起 → 轮询 → 完成（现场调试台核心链路）
    let (status, body) = call(
        state.clone(),
        request(
            "POST",
            "/api/protocol-tools/register-scan",
            Some(&token),
            serde_json::json!({
                "bus": BUS, "slave_id": 1, "reg_type": "holding",
                "start_addr": 0, "end_addr": 2
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "⑥ 发起扫描应返回 200，body={body}");
    let scan_id = body["data"].as_str().expect("扫描应返回 id").to_string();

    let mut scanned = Value::Null;
    for _ in 0..40 {
        let (status, body) = call(
            state.clone(),
            request(
                "GET",
                &format!("/api/protocol-tools/register-scan/{scan_id}"),
                Some(&token),
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        scanned = body["data"].clone();
        if scanned["status"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(
        scanned["status"], "done",
        "⑥ 扫描应正常收尾，实际 {scanned}"
    );
    assert_eq!(scanned["total"], 3);
    assert_eq!(scanned["responded"], 3, "⑥ 三个地址都应判为存在");
    assert_eq!(scanned["progress"], 100);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 3);
    let cells = scanned["cells"].as_array().expect("cells 应为数组");
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0]["address"], 0);
    assert_eq!(cells[0]["value"], 0);
    assert!(cells[0]["func_code"].is_number());

    // ⑦ 诊断（运维页入口）
    let (status, body) = call(
        state.clone(),
        request("GET", "/api/diagnostics/gateway", Some(&token), Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "⑦ 网关诊断应返回 200");
    assert_eq!(body["code"], 0);

    // ⑧ 健康报告（交付验收单数据源）
    let (status, body) = call(
        state.clone(),
        request("GET", "/api/health/report", Some(&token), Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "⑧ 健康报告应返回 200");
    assert_eq!(body["code"], 0);
    assert!(!body["data"].is_null());
}
