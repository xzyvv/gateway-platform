//! 报文编辑器 handler（P0-1：真实帧下发）
//!
//! 历史实现是「静态知识库 + 伪随机派生」——响应按请求 hex 哈希编造，从未触达总线，
//! 现场据此判断设备行为会得到完全错误的结论。本实现改为：
//! 1. 校验请求帧 → 2. 申请总线独占租约（采集占用/他会话占用则拒绝）
//!    → 3. 原样下发并收齐设备真实回帧 → 4. 按协议判定 CRC/异常码 → 5. 写审计日志。
//!
//! 契约 §3 过渡期已结束（2026-10-25）：请求缺 `bus` 不再回落旧 mock 行为，
//! 直接返回 400 + `missing_bus`；前端已全量显式传 `bus`。

use crate::web::api::protocol_tools::{acquire_bus, audit_send, error_response};
use crate::web::debug_bus::{
    clamp_timeout_ms, parse_hex, to_hex, verify_modbus_rtu, BusErrorKind, FrameCheck,
};
use crate::web::types::{ApiResponse, AppState};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::sync::Arc;
use ts_rs::TS;

/// 发送结果
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SendResult {
    pub request_hex: String,
    pub response_hex: String,
    pub success: bool,
    pub latency_ms: i32,
    pub parsed: String,
    /// 失败分类（契约 §5）；成功时为 null
    pub error: Option<String>,
}

/// 预置帧
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct Preset {
    pub name: String,
    pub hex: String,
}

const PRESETS: &[(&str, &str)] = &[
    ("读保持寄存器(0-1)", "01 03 00 00 00 02"),
    ("读输入寄存器(0-3)", "01 04 00 00 00 04"),
    ("读线圈(0-7)", "01 01 00 00 00 08"),
    ("写单寄存器(5=100)", "01 06 00 05 00 64"),
];

#[derive(Debug, serde::Deserialize)]
pub struct SendFrameBody {
    pub hex: String,
    /// 目标总线 `port_name`（必填）；serde 层保持宽容反序列化，缺省由 handler 返回 400 `missing_bus`
    #[serde(default)]
    pub bus: Option<String>,
    /// 单帧等待超时（ms），默认 1500、上限 10000
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

pub(crate) async fn list_presets(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<Preset>>>, crate::web::types::ApiError> {
    let list: Vec<Preset> = PRESETS
        .iter()
        .map(|(n, h)| Preset {
            name: n.to_string(),
            hex: h.to_string(),
        })
        .collect();
    Ok(Json(ApiResponse::ok(list)))
}

pub(crate) async fn send_frame(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SendFrameBody>,
) -> Result<Response, crate::web::types::ApiError> {
    let bus_name = body
        .bus
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // ── 缺 bus → 400 missing_bus（契约 §3：mock 回落已移除）
    let Some(bus_name) = bus_name else {
        return Ok(error_response(
            BusErrorKind::MissingBus,
            "缺少必填参数 bus（目标总线 port_name），请指定后再下发",
        ));
    };

    // ── 参数校验（400 invalid_frame）
    let request = match parse_hex(&body.hex) {
        Ok(v) => v,
        Err(detail) => return Ok(error_response(BusErrorKind::InvalidFrame, detail)),
    };
    let timeout_ms = clamp_timeout_ms(body.timeout_ms);

    // ── 总线准入（不存在 404 / 采集占用或他会话占用 bus_busy / 端口不可用 bus_offline）
    let lease = match acquire_bus(&state, &bus_name).await {
        Ok(l) => l,
        Err(e) => {
            audit_send(
                &state,
                &bus_name,
                &format!("result={} detail={}", e.kind.as_str(), e.detail),
            );
            return Ok(error_response(e.kind, e.detail));
        }
    };
    let is_modbus = lease.bus.protocol.starts_with("modbus");

    // ── 真实下发
    match lease.session.exchange(&request, timeout_ms).await {
        Ok(out) => {
            let response_hex = to_hex(&out.bytes);
            let result = if is_modbus {
                judge_modbus(&request, &out.bytes, out.elapsed_ms)
            } else {
                // 非 Modbus 协议目前只做字节级回显：各协议的校验和规则差异大，
                // 在没有对应分帧器的情况下贸然判 CRC 会把正常帧误报为坏帧。
                SendResult {
                    request_hex: to_hex(&request),
                    response_hex,
                    success: true,
                    latency_ms: out.elapsed_ms as i32,
                    parsed: format!(
                        "{} 协议收到 {} 字节响应",
                        lease.bus.protocol,
                        out.bytes.len()
                    ),
                    error: None,
                }
            };
            audit_send(
                &state,
                &bus_name,
                &format!(
                    "request={} response={} success={}",
                    result.request_hex, result.response_hex, result.success
                ),
            );
            Ok(Json(ApiResponse::ok(result)).into_response())
        }
        Err(kind) => {
            let result = SendResult {
                request_hex: to_hex(&request),
                response_hex: String::new(),
                success: false,
                latency_ms: timeout_ms as i32,
                parsed: failure_hint(kind, &bus_name),
                error: Some(kind.as_str().to_string()),
            };
            audit_send(
                &state,
                &bus_name,
                &format!("request={} error={}", result.request_hex, kind.as_str()),
            );
            Ok(Json(ApiResponse::ok(result)).into_response())
        }
    }
}

/// Modbus 响应判定：先 CRC 后异常码（详见 `verify_modbus_rtu` 的注释）
fn judge_modbus(request: &[u8], response: &[u8], elapsed_ms: u64) -> SendResult {
    let base = SendResult {
        request_hex: to_hex(request),
        response_hex: to_hex(response),
        success: false,
        latency_ms: elapsed_ms as i32,
        parsed: String::new(),
        error: None,
    };
    match verify_modbus_rtu(response) {
        FrameCheck::Ok => SendResult {
            success: true,
            parsed: describe_modbus(request, response),
            ..base
        },
        FrameCheck::Crc => SendResult {
            parsed: format!(
                "响应帧校验失败：收到 {} 字节但不满足 CRC16。常见原因：波特率/校验位不匹配、线路干扰、其它从站同时应答",
                response.len()
            ),
            error: Some("crc".into()),
            ..base
        },
        FrameCheck::Exception { code, text } => SendResult {
            parsed: format!("设备返回异常响应：异常码 0x{:02X}（{}）", code, text),
            error: Some("exception".into()),
            ..base
        },
    }
}

/// 面向现场的一行解读：从站号、功能码、地址与数据
fn describe_modbus(request: &[u8], response: &[u8]) -> String {
    let slave = response.first().copied().unwrap_or(0);
    let fc = response.get(1).copied().unwrap_or(0);
    // 数据区 = 去掉 addr/fc 与尾部 CRC16
    let data = if response.len() > 4 {
        &response[2..response.len() - 2]
    } else {
        &[][..]
    };
    match fc {
        // 读保持/输入寄存器：数据为 u16 大端字数组
        0x03 | 0x04 => {
            let addr = request
                .get(2)
                .zip(request.get(3))
                .map(|(h, l)| u16::from_be_bytes([*h, *l]))
                .unwrap_or(0);
            let words: Vec<String> = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c).to_string())
                .collect();
            let kind = if fc == 0x03 {
                "保持寄存器"
            } else {
                "输入寄存器"
            };
            format!(
                "从站{} {} addr={} 返回 {} 个值: [{}]",
                slave,
                kind,
                addr,
                words.len(),
                words.join(", ")
            )
        }
        0x01 | 0x02 => {
            let kind = if fc == 0x01 { "线圈" } else { "离散输入" };
            format!("从站{} {} 返回 {} 字节位图", slave, kind, data.len())
        }
        0x05 | 0x06 | 0x0F | 0x10 => format!("从站{} 写操作已确认（功能码 0x{:02X}）", slave, fc),
        _ => format!("从站{} 功能码 0x{:02X} 返回 {} 字节", slave, fc, data.len()),
    }
}

/// 帧级失败的中文说明（`error` 字段仍是机器码，文案只进 `parsed`）
fn failure_hint(kind: BusErrorKind, bus: &str) -> String {
    match kind {
        BusErrorKind::Timeout => format!(
            "等待 {} 超时仍未收到完整响应。请确认从站号/波特率正确、设备在线，必要时加大超时",
            bus
        ),
        _ => format!("下发失败（{}），请检查总线状态与配置", kind.as_str()),
    }
}
