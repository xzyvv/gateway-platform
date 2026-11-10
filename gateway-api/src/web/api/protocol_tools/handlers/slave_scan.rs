//! 从站扫描 handler（P0-2：真实总线扫描）
//!
//! 历史实现是「静态知识库 + 伪随机派生」——按 `hash(bus) × slave_id` 编造在线列表，
//! 同一总线两次扫描结果一致但全是假的，比报错更有误导性。本实现改为：
//! 逐地址向总线发 Modbus 探测帧（FC03 读 1 个保持寄存器），按真实响应判定在线状态
//! （契约 p0-2 §3）：
//! - 正常帧 / **异常帧**（功能码 | 0x80）→ 在线（协议层有应答即证明设备物理在线，
//!   寄存器 0 是否存在不影响判定）；
//! - CRC 错 / 截断帧 → 不在线，计入 summary.noise（把线路干扰与「无设备」区分开）；
//! - 超时 → 不在线。
//!
//! 传输复用 p0-1b 的 SSE 范式（契约 p0-2 §2）：`Accept: text/event-stream` →
//! 流式逐地址推送（started → slave×N → summary/aborted/failed）；缺省阻塞模式
//! 返回既有 `ApiResponse<Vec<SlaveScanResult>>` 结构（字段不变，数据变真实）。

use crate::web::api::protocol_tools::{
    accepts_sse, acquire_bus, audit_send, error_response, sse_response, DebugLease, IntoSseEvent,
};
use crate::web::debug_bus::{
    clamp_scan_interval_ms, clamp_scan_timeout_ms, verify_modbus_rtu, BusErrorKind, FrameCheck,
    MAX_SCAN_DURATION,
};
use crate::web::services::modbus_read_request_frame as build_read_request;
use crate::web::services::protocol_capabilities_of as protocol_capabilities;
use crate::web::types::{ApiResponse, AppState};
use axum::extract::State;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{channel::mpsc, StreamExt};
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;
use ts_rs::TS;

/// 从站扫描结果
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SlaveScanResult {
    pub slave_id: i32,
    pub responded: bool,
    /// 真实扫描无从得知设备名（归 register_scan / auto_detect 阶段），恒为 null
    pub device_name: Option<String>,
    /// 设备应答的功能码（大写 hex，如 "03" / 异常帧 "83"）；无应答为 null
    pub func_code: Option<String>,
    pub latency_ms: Option<i32>,
}

/// 扫描汇总（流式 `summary` 事件 data，契约 p0-2 §4.3）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SlaveScanSummary {
    pub bus: String,
    /// 已扫描地址数（提前收尾时 < total）
    pub scanned: i32,
    /// 在线地址数（正常帧 + 异常帧）
    pub found: i32,
    /// CRC 错/截断帧次数：>0 提示现场「线路有干扰」，与「设备不在线」区分
    pub noise: i32,
    #[ts(type = "number")]
    pub duration_ms: i64,
    pub results: Vec<SlaveScanResult>,
}

#[derive(Deserialize)]
pub struct ScanBody {
    /// 目标总线 `port_name`（必填）；serde 层宽容反序列化，缺省由 handler 返回 400 `missing_bus`
    #[serde(default)]
    pub bus: Option<String>,
    pub range_start: i32,
    pub range_end: i32,
    /// 单地址探测超时（ms），默认 500、上限 10000
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// 相邻探测间隔（ms），默认 50、下限 20
    #[serde(default)]
    pub interval_ms: Option<u64>,
}

/// Modbus 从站地址合法域（契约 p0-2 §2）
pub(crate) const MODBUS_SLAVE_MIN: i32 = 1;
pub(crate) const MODBUS_SLAVE_MAX: i32 = 247;

/// B-P1 修复：单次从站扫描的最大范围（纵深防御；1..=247 已更严）。
/// 不设上限时可构造约 43 亿次迭代的循环（远程 DoS）。
pub(crate) const MAX_SLAVE_SCAN_SPAN: i64 = 1024;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 探测帧：`{id} 03 00 00 00 01 + CRC16`（FC03 读 1 保持寄存器，只探活不读数）
///
/// 帧构造统一走 core 单一实现，避免本地副本的 CRC 字节序与采集路径不一致。
fn probe_frame(slave_id: i32) -> Vec<u8> {
    build_read_request(slave_id as u8, 0x03, 0, 1)
}

/// 单地址判定（契约 §3.2）：返回 (结果, 是否噪声, 是否总线掉线)
fn judge_probe(
    slave_id: i32,
    outcome: Result<crate::web::debug_bus::ExchangeOutcome, BusErrorKind>,
) -> (SlaveScanResult, bool, bool) {
    let offline = |slave_id: i32| SlaveScanResult {
        slave_id,
        responded: false,
        device_name: None,
        func_code: None,
        latency_ms: None,
    };
    match outcome {
        Ok(out) => {
            // 功能码取实际应答字节（异常帧为 fc|0x80，如 "83"），不硬编码
            let fc = out
                .bytes
                .get(1)
                .map(|b| format!("{:02X}", b))
                .unwrap_or_default();
            match verify_modbus_rtu(&out.bytes) {
                FrameCheck::Ok | FrameCheck::Exception { .. } => (
                    SlaveScanResult {
                        slave_id,
                        responded: true,
                        device_name: None,
                        func_code: Some(fc),
                        latency_ms: Some(out.elapsed_ms as i32),
                    },
                    false,
                    false,
                ),
                // CRC 错/截断：线路噪声，不作为在线证据
                FrameCheck::Crc => (offline(slave_id), true, false),
            }
        }
        // 链路级故障：中断整场扫描（继续扫只会空耗剩余时长）
        Err(BusErrorKind::BusOffline) => (offline(slave_id), false, true),
        // 超时等帧级失败：该地址无设备或不应答 FC03
        Err(_) => (offline(slave_id), false, false),
    }
}

// ===== P0-2：流式扫描（SSE，契约 §4） =====

/// 扫描审计回调：(action, bus, detail)，语义同回放（注入以便单元测试独立驱动引擎）
pub(crate) type ScanAudit = Arc<dyn Fn(&'static str, &str, &str) + Send + Sync>;

/// 扫描引擎产出的事件：阻塞模式收集为结果数组；流式模式逐条转 SSE（契约 §4）
#[derive(Debug)]
pub(crate) enum ScanEvent {
    /// 租约建立后首发：前端据此初始化进度（total = 计划探测地址数）
    Started {
        bus: String,
        range_start: i32,
        range_end: i32,
        total: i32,
        timeout_ms: u64,
        interval_ms: u64,
        started_at_ms: i64,
    },
    /// 单地址探完：data 即 SlaveScanResult，字段一字不差（契约 §4.2）
    Slave(SlaveScanResult),
    /// 正常结束：data 即 SlaveScanSummary（含全量结果）
    Summary(Box<SlaveScanSummary>),
    /// 服务端主动停止（时长硬上限）。客户端断开不走这里——仅落审计（契约 §4.4）
    Aborted {
        reason: &'static str,
        scanned: i32,
        total: i32,
    },
    /// 总线中途不可用等致命错误（契约 §4.5）
    Failed {
        error: &'static str,
        message: String,
        scanned: i32,
        total: i32,
    },
}

impl IntoSseEvent for ScanEvent {
    fn into_sse(self) -> Event {
        use serde_json::json;
        match self {
            ScanEvent::Started {
                bus,
                range_start,
                range_end,
                total,
                timeout_ms,
                interval_ms,
                started_at_ms,
            } => Event::default().event("started").data(
                json!({
                    "bus": bus, "range_start": range_start, "range_end": range_end,
                    "total": total, "timeout_ms": timeout_ms, "interval_ms": interval_ms,
                    "started_at_ms": started_at_ms,
                })
                .to_string(),
            ),
            // Slave/Summary 的序列化不可能失败（纯数据结构），失败时给空对象兜底
            ScanEvent::Slave(r) => Event::default()
                .event("slave")
                .data(serde_json::to_string(&r).unwrap_or_else(|_| "{}".into())),
            ScanEvent::Summary(s) => Event::default()
                .event("summary")
                .data(serde_json::to_string(&s).unwrap_or_else(|_| "{}".into())),
            ScanEvent::Aborted {
                reason,
                scanned,
                total,
            } => Event::default()
                .event("aborted")
                .data(json!({ "reason": reason, "scanned": scanned, "total": total }).to_string()),
            ScanEvent::Failed {
                error,
                message,
                scanned,
                total,
            } => Event::default().event("failed").data(
                json!({
                    "error": error, "message": message,
                    "scanned": scanned, "total": total,
                })
                .to_string(),
            ),
        }
    }
}

/// 启动扫描引擎任务：独占租约逐地址探测，事件经 channel 流出，租约随任务结束释放。
///
/// 取消语义（契约 §6.3）：接收端断开后，循环在**下一个地址开始前**停止；
/// 正在等待的探测按当前 timeout_ms 收尾，不掐断半帧串口事务（保护 RS485 时序）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_scan(
    audit: ScanAudit,
    bus_name: String,
    lease: DebugLease,
    range_start: i32,
    range_end: i32,
    timeout_ms: u64,
    interval_ms: u64,
    max_duration: std::time::Duration,
) -> mpsc::UnboundedReceiver<ScanEvent> {
    let (tx, rx) = mpsc::unbounded();
    tokio::spawn(async move {
        // 租约整体使用：`session` 字段内已绑定互斥守卫，捕获它即连守卫一起进任务
        let started_at_ms = now_ms();
        let started = std::time::Instant::now();
        let planned = (range_end as i64 - range_start as i64 + 1) as i32;
        if tx
            .unbounded_send(ScanEvent::Started {
                bus: bus_name.clone(),
                range_start,
                range_end,
                total: planned,
                timeout_ms,
                interval_ms,
                started_at_ms,
            })
            .is_err()
        {
            return; // 流建立前客户端已走：未探测任何地址，无需审计
        }

        // 整场扫描持有一份租约：避免逐地址抢锁，也保证扫描期间没有第二个
        // 调试者插入（与回放同一理由，契约 v1 §6.1）
        let mut results: Vec<SlaveScanResult> = Vec::with_capacity(planned as usize);
        let mut found: i32 = 0;
        let mut noise: i32 = 0;
        /// 服务端主动停止原因（客户端断开由 client_aborted 单独表达）
        enum Stop {
            DurationCap,
            BusOffline,
        }
        let mut stop: Option<Stop> = None;
        let mut client_aborted = false;

        for (idx, slave_id) in (range_start..=range_end).enumerate() {
            // 总时长硬上限：超限即停止，避免异常场次把总线占满 5 分钟以上
            if started.elapsed() >= max_duration {
                tracing::warn!(
                    "slave-scan {} 达到 {}s 上限，剩余地址未探测",
                    bus_name,
                    max_duration.as_secs()
                );
                stop = Some(Stop::DurationCap);
                break;
            }
            // 探测间隔：保护 RTU 半双工总线（首个地址前无需等待）
            if idx > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            // 客户端已断开：不再探测新地址（契约 §6.3「下一个地址开始前停止」）
            if tx.is_closed() {
                client_aborted = true;
                break;
            }

            let outcome = lease
                .session
                .exchange(&probe_frame(slave_id), timeout_ms)
                .await;
            let (result, is_noise, is_offline) = judge_probe(slave_id, outcome);
            if result.responded {
                found += 1;
            }
            if is_noise {
                noise += 1;
            }
            results.push(result.clone());
            if tx.unbounded_send(ScanEvent::Slave(result)).is_err() {
                // 发送失败 = 客户端已断开（契约 §6.3）
                client_aborted = true;
                break;
            }
            if is_offline {
                tracing::warn!("slave-scan {} 总线不可用，提前结束", bus_name);
                stop = Some(Stop::BusOffline);
                break;
            }
        }

        let scanned = results.len() as i32;
        if client_aborted {
            // 契约 §6.3：客户端中断仅落审计（连接已断，aborted 事件发不出去）
            audit(
                "debug.slave_scan.abort",
                &bus_name,
                &format!("scanned={} total={}", scanned, planned),
            );
            return;
        }

        let summary = SlaveScanSummary {
            bus: bus_name.clone(),
            scanned,
            found,
            noise,
            duration_ms: started.elapsed().as_millis() as i64,
            results,
        };
        audit(
            "debug.slave_scan",
            &bus_name,
            &format!(
                "range={}-{} scanned={} found={} noise={} duration_ms={}",
                range_start,
                range_end,
                summary.scanned,
                summary.found,
                summary.noise,
                summary.duration_ms
            ),
        );
        let terminal = match stop {
            None => ScanEvent::Summary(Box::new(summary)),
            Some(Stop::DurationCap) => ScanEvent::Aborted {
                reason: "duration_cap",
                scanned,
                total: planned,
            },
            Some(Stop::BusOffline) => ScanEvent::Failed {
                error: BusErrorKind::BusOffline.as_str(),
                message: format!("总线 {} 在扫描过程中不可用：端口掉线或驱动异常", bus_name),
                scanned,
                total: planned,
            },
        };
        // 此刻客户端断开也允许：任务收尾即释放租约，发送失败无副作用
        let _ = tx.unbounded_send(terminal);
    });
    rx
}

/// 阻塞模式：收集事件流汇总为一次性 JSON 响应。
/// 提前收尾（硬上限/总线掉线）时返回 200 + 已扫部分结果——现场能看到
/// 「扫到哪个地址断的」（已扫部分是地址连续前缀），契约 §4.5。
async fn collect_scan_blocking(mut rx: mpsc::UnboundedReceiver<ScanEvent>) -> Response {
    let mut results: Vec<SlaveScanResult> = Vec::new();
    while let Some(ev) = rx.next().await {
        match ev {
            ScanEvent::Started { .. } => {}
            ScanEvent::Slave(r) => results.push(r),
            ScanEvent::Summary(s) => return Json(ApiResponse::ok(s.results)).into_response(),
            ScanEvent::Aborted { .. } | ScanEvent::Failed { .. } => break,
        }
    }
    Json(ApiResponse::ok(results)).into_response()
}

pub(crate) async fn scan_slaves(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ScanBody>,
) -> Result<Response, crate::web::types::ApiError> {
    // ── 缺 bus → 400 missing_bus（与 send_frame 对齐，契约 §2）
    let bus_name = body
        .bus
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let Some(bus_name) = bus_name else {
        return Ok(error_response(
            BusErrorKind::MissingBus,
            "缺少必填参数 bus（目标总线 port_name），请指定后再扫描",
        ));
    };

    // ── 范围校验（400 + 中文 message，沿用既有 bad_request 语义，契约 §8 #21）
    if body.range_end < body.range_start {
        return Err(crate::web::types::ApiError::bad_request(
            "range_end 必须大于等于 range_start",
        ));
    }
    if body.range_start < MODBUS_SLAVE_MIN || body.range_end > MODBUS_SLAVE_MAX {
        return Err(crate::web::types::ApiError::bad_request(format!(
            "从站地址必须在 {}..={} 范围内（Modbus 合法域），当前 {}..={}",
            MODBUS_SLAVE_MIN, MODBUS_SLAVE_MAX, body.range_start, body.range_end
        )));
    }
    // 用 i64 计算跨度，避免 i32 溢出，并对上限做可靠比较
    let span = (body.range_end as i64) - (body.range_start as i64) + 1;
    if span > MAX_SLAVE_SCAN_SPAN {
        return Err(crate::web::types::ApiError::bad_request(format!(
            "扫描范围过大（{span} > {MAX_SLAVE_SCAN_SPAN}），请缩小范围"
        )));
    }
    let timeout_ms = clamp_scan_timeout_ms(body.timeout_ms);
    let interval_ms = clamp_scan_interval_ms(body.interval_ms);

    // ── 总线准入（不存在 404 / 采集占用或他会话占用 bus_busy / 端口不可用 bus_offline）
    let lease = match acquire_bus(&state, &bus_name).await {
        Ok(l) => l,
        Err(e) => {
            audit_send(
                &state,
                &bus_name,
                &format!("slave-scan {}", e.kind.as_str()),
            );
            return Ok(error_response(e.kind, e.detail));
        }
    };

    // ── 协议路由（B3）：按注册表能力位判断，不再按协议名硬编码；
    //    新增协议只要声明 supports_scan_slaves=true 即可用此工具。
    match protocol_capabilities(&lease.bus.protocol) {
        Some(caps) if caps.supports_scan_slaves => {}
        Some(_) => {
            let detail = format!(
                "总线 {} 协议为 {}，未声明支持从站扫描；请改用「设备诊断 + 单点读值」确认链路",
                bus_name, lease.bus.protocol
            );
            audit_send(&state, &bus_name, "slave-scan unsupported_protocol");
            return Ok(error_response(BusErrorKind::UnsupportedProtocol, detail));
        }
        None => {
            let detail = format!(
                "总线 {} 的协议 {} 未在注册表中登记，无法执行从站扫描",
                bus_name, lease.bus.protocol
            );
            audit_send(&state, &bus_name, "slave-scan unknown_protocol");
            return Ok(error_response(BusErrorKind::UnsupportedProtocol, detail));
        }
    }

    let st = state.clone();
    let audit: ScanAudit = Arc::new(move |action, bus, detail| {
        st.platform.audit.log(action, bus, detail, "api");
    });
    let rx = spawn_scan(
        audit,
        bus_name,
        lease,
        body.range_start,
        body.range_end,
        timeout_ms,
        interval_ms,
        MAX_SCAN_DURATION,
    );
    // 契约 §2：同一 URL 按 Accept 内容协商——请求 SSE 则逐地址推送，
    // 缺省保持阻塞 JSON 行为（响应结构与 V1 mock 同形）
    if accepts_sse(&headers) {
        Ok(sse_response(rx))
    } else {
        Ok(collect_scan_blocking(rx).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::debug_bus::{DebugBusTrait, ExchangeOutcome};
    use gateway_core::engine::codec::crc::crc16;
    use std::future::Future;
    use std::pin::Pin;

    #[test]
    fn probe_frame_结构为fc03读1寄存器且crc正确() {
        let frame = probe_frame(0x11);
        assert_eq!(&frame[..6], &[0x11, 0x03, 0x00, 0x00, 0x00, 0x01]);
        let (body, tail) = frame.split_at(frame.len() - 2);
        assert_eq!(
            crc16(body),
            u16::from_le_bytes([tail[0], tail[1]]),
            "探测帧 CRC16 必须正确"
        );
    }

    fn with_crc(prefix: &[u8]) -> Vec<u8> {
        let mut v = prefix.to_vec();
        v.extend_from_slice(&crc16(prefix).to_le_bytes());
        v
    }

    fn ok_outcome(bytes: Vec<u8>) -> Result<ExchangeOutcome, BusErrorKind> {
        Ok(ExchangeOutcome {
            bytes,
            elapsed_ms: 7,
        })
    }

    #[test]
    fn judge_正常帧与异常帧都算在线_功能码取实际应答() {
        let (r, noise, offline) =
            judge_probe(2, ok_outcome(with_crc(&[0x02, 0x03, 0x02, 0x00, 0x2A])));
        assert!(r.responded && !noise && !offline);
        assert_eq!(r.func_code.as_deref(), Some("03"));
        assert_eq!(r.latency_ms, Some(7));
        assert_eq!(r.device_name, None, "真实扫描无从得知设备名");

        // 异常帧 83 02（非法地址）：协议层有应答即在线（契约 §3.2）
        let (r, noise, _) = judge_probe(3, ok_outcome(with_crc(&[0x03, 0x83, 0x02])));
        assert!(r.responded && !noise);
        assert_eq!(r.func_code.as_deref(), Some("83"));
    }

    #[test]
    fn judge_crc错记噪声_超时与掉线分类() {
        let (r, noise, offline) = judge_probe(4, ok_outcome(vec![0x04, 0x03, 0xFF, 0xFF]));
        assert!(!r.responded && noise && !offline);

        let (r, noise, offline) = judge_probe(5, Err(BusErrorKind::Timeout));
        assert!(!r.responded && !noise && !offline);

        let (_, _, offline) = judge_probe(6, Err(BusErrorKind::BusOffline));
        assert!(offline, "bus_offline 必须上抛以中断整场扫描");
    }

    /// 可控延时的桩总线：恒超时（只关心时序，不关心帧内容）
    struct StubBus {
        delay_ms: u64,
    }

    impl DebugBusTrait for StubBus {
        fn exchange<'a>(
            &'a self,
            _request: &'a [u8],
            _timeout_ms: u64,
        ) -> Pin<Box<dyn Future<Output = Result<ExchangeOutcome, BusErrorKind>> + Send + 'a>>
        {
            let delay = self.delay_ms;
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                Err(BusErrorKind::Timeout)
            })
        }
    }

    /// 契约 §8 #27：时长硬上限 → aborted(duration_cap)，已扫地址的 slave 事件先送达
    #[tokio::test]
    async fn 扫描引擎_达到时长上限_发aborted且已扫地址先送达() {
        // 上限 150ms、单地址 60ms、间隔 40ms：地址1 于 ≥60ms 完成，地址2 于 ≥160ms 完成，
        // 地址3 开始前 elapsed≥160≥150 → duration_cap。tokio sleep 不提前返回，下界确定。
        let audit: ScanAudit = Arc::new(|_, _, _| {});
        let mut rx = spawn_scan(
            audit,
            "b".into(),
            DebugLease::for_test(Arc::new(StubBus { delay_ms: 60 })),
            1,
            3,
            1000,
            40,
            std::time::Duration::from_millis(150),
        );

        let mut seq: Vec<&str> = Vec::new();
        while let Some(ev) = rx.next().await {
            match ev {
                ScanEvent::Started { total, .. } => {
                    assert_eq!(total, 3, "started.total = 计划探测地址数");
                    seq.push("started");
                }
                ScanEvent::Slave(_) => seq.push("slave"),
                ScanEvent::Aborted {
                    reason,
                    scanned,
                    total,
                } => {
                    assert_eq!(reason, "duration_cap");
                    assert_eq!(scanned, 2, "已达上限前扫完的 2 个地址先送达");
                    assert_eq!(total, 3);
                    seq.push("aborted");
                }
                other => panic!("时长上限场景不应出现 {:?}", other),
            }
        }
        assert_eq!(seq, ["started", "slave", "slave", "aborted"]);
    }

    /// 契约 §8 #24 配套：客户端断开 → 下一地址前停止并落 abort 审计
    #[tokio::test]
    async fn 扫描引擎_接收端断开_下一地址前停止并落abort审计() {
        // 单地址延时 200ms：地址1 送达后丢弃接收端（模拟 abort）；
        // 任务在地址2 的间隔 sleep 后发现 is_closed → 不再探测，落 debug.slave_scan.abort。
        let audits = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let audits2 = audits.clone();
        let audit: ScanAudit = Arc::new(move |action, _bus, detail| {
            audits2.lock().unwrap().push(format!("{action}|{detail}"));
        });

        let mut rx = spawn_scan(
            audit,
            "b".into(),
            DebugLease::for_test(Arc::new(StubBus { delay_ms: 200 })),
            1,
            3,
            1000,
            20,
            std::time::Duration::from_secs(60),
        );
        match rx.next().await {
            Some(ScanEvent::Started { .. }) => {}
            other => panic!("首事件应为 started，实际 {:?}", other),
        }
        match rx.next().await {
            Some(ScanEvent::Slave(_)) => {}
            other => panic!("次事件应为 slave，实际 {:?}", other),
        }
        drop(rx);

        // 等任务感知断开并收尾（间隔 20ms 量级，留足余量）
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let logs = audits.lock().unwrap();
        assert!(
            logs.iter()
                .any(|l| l.starts_with("debug.slave_scan.abort|")),
            "客户端断开应落 debug.slave_scan.abort 审计，实际：{:?}",
            *logs
        );
        assert!(
            !logs.iter().any(|l| l.starts_with("debug.slave_scan|")),
            "中断场景不应写常规扫描完成审计，实际：{:?}",
            *logs
        );
    }
}
