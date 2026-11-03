//! 回放引擎：事件定义、SSE 转换与后台任务
//!
//! 内容协商 / 响应组装 / 心跳已收敛到 `protocol_tools::sse`（p0-2 起与从站扫描共用）

use crate::web::api::protocol_tools::{DebugLease, IntoSseEvent};
use crate::web::debug_bus::{parse_hex, to_hex, BusErrorKind};
use axum::response::sse::Event;
use futures::channel::mpsc;
use std::sync::Arc;

use super::now_ms;
use super::replay_run::{judge_frame, ReplayExecuteResult, ReplayFrameResult};
use super::ReplayPacket;

/// 回放审计回调：(action, bus, detail)。生产落到平台审计日志。
/// 以回调注入而非直接持 AppState——回放核心循环由此可在无完整应用状态的
/// 单元测试中独立驱动（如 5 分钟硬上限用注入的小上限验证，契约 §7 #16）。
pub(super) type ReplayAudit = Arc<dyn Fn(&'static str, &str, &str) + Send + Sync>;

/// 回放引擎产出的事件：阻塞模式收集为汇总；流式模式逐条转 SSE（契约 §3）
#[derive(Debug)]
pub(super) enum ReplayEvent {
    /// 租约建立后首发：前端据此初始化进度条（total = 计划下发帧数）
    Started {
        session_id: String,
        bus: String,
        total: i32,
        started_at_ms: i64,
    },
    /// 单帧完成：data 即 v1 §4 ReplayFrameResult，字段一字不差
    Frame(ReplayFrameResult),
    /// 正常结束（跑完 / stop_on_mismatch 提前）：data 即 v1 §4 ReplayExecuteResult
    Summary(Box<ReplayExecuteResult>),
    /// 服务端主动停止（5 分钟硬上限）。客户端断开不走这里——连接已断，
    /// 发不出去也没有意义，仅落 debug.replay.abort 审计（契约 §3.4/§4）
    Aborted {
        reason: &'static str,
        completed: i32,
        total: i32,
    },
    /// 总线中途不可用等致命错误（契约 §3.5）
    Failed {
        error: &'static str,
        message: String,
        completed: i32,
        total: i32,
    },
}

impl IntoSseEvent for ReplayEvent {
    fn into_sse(self) -> Event {
        use serde_json::json;
        match self {
            ReplayEvent::Started {
                session_id,
                bus,
                total,
                started_at_ms,
            } => Event::default().event("started").data(
                json!({
                    "session_id": session_id, "bus": bus,
                    "total": total, "started_at_ms": started_at_ms,
                })
                .to_string(),
            ),
            // Frame/Summary 的序列化不可能失败（纯数据结构），失败时给空对象兜底
            ReplayEvent::Frame(f) => Event::default()
                .event("frame")
                .data(serde_json::to_string(&f).unwrap_or_else(|_| "{}".into())),
            ReplayEvent::Summary(r) => Event::default()
                .event("summary")
                .data(serde_json::to_string(&r).unwrap_or_else(|_| "{}".into())),
            ReplayEvent::Aborted {
                reason,
                completed,
                total,
            } => Event::default().event("aborted").data(
                json!({ "reason": reason, "completed": completed, "total": total }).to_string(),
            ),
            ReplayEvent::Failed {
                error,
                message,
                completed,
                total,
            } => Event::default().event("failed").data(
                json!({
                    "error": error, "message": message,
                    "completed": completed, "total": total,
                })
                .to_string(),
            ),
        }
    }
}

/// 启动回放引擎任务：独占租约逐帧下发，事件经 channel 流出，租约随任务结束释放。
///
/// 取消语义（契约 §4）：接收端断开（客户端 abort / 连接超时）后，循环在
/// **下一帧开始前**停止——下发前先查 `is_closed`，发送失败也视为断开；
/// 正在等待的帧按当前 timeout_ms 收尾，不掐断半帧串口事务（保护 RS485 时序）。
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_replay(
    audit: ReplayAudit,
    session_id: String,
    bus_name: String,
    lease: DebugLease,
    pairs: Vec<(ReplayPacket, Option<ReplayPacket>)>,
    timeout_ms: u64,
    interval_ms: u64,
    stop_on_mismatch: bool,
    max_duration: std::time::Duration,
) -> mpsc::UnboundedReceiver<ReplayEvent> {
    let (tx, rx) = mpsc::unbounded();
    tokio::spawn(async move {
        // 租约整体使用：`session` 字段内已绑定互斥守卫，捕获它即连守卫一起进任务
        let started_at_ms = now_ms();
        let started = std::time::Instant::now();
        let planned = pairs.len() as i32;
        if tx
            .unbounded_send(ReplayEvent::Started {
                session_id: session_id.clone(),
                bus: bus_name.clone(),
                total: planned,
                started_at_ms,
            })
            .is_err()
        {
            return; // 流建立前客户端已走：未下发任何帧，无需审计
        }

        // 整个回放会话持有一份租约：既避免逐帧抢锁，也保证会话期间没有第二个
        // 调试者插入，否则「A 发一半、B 插一帧」会让逐帧对比结果无法解释。
        let mut results: Vec<ReplayFrameResult> = Vec::with_capacity(pairs.len());
        let mut match_count: i32 = 0;
        let mut mismatch_count: i32 = 0;
        let mut failed: i32 = 0;
        /// 服务端主动停止原因（客户端断开由 client_aborted 单独表达）
        enum Stop {
            DurationCap,
            BusOffline,
        }
        let mut stop: Option<Stop> = None;
        let mut client_aborted = false;

        for (idx, (req, expected)) in pairs.iter().enumerate() {
            // 总时长硬上限：超限即停止，避免一个异常会话把总线占满 5 分钟以上
            if started.elapsed() >= max_duration {
                tracing::warn!(
                    "replay {} 达到 {}s 上限，剩余帧未执行",
                    session_id,
                    max_duration.as_secs()
                );
                stop = Some(Stop::DurationCap);
                break;
            }
            // 帧间静默：保护 RTU 半双工总线（首个帧前无需等待）
            if idx > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            // 客户端已断开：不再下发新帧（契约 §4「下一帧开始前停止」）
            if tx.is_closed() {
                client_aborted = true;
                break;
            }

            let expected_hex = expected.as_ref().map(|p| p.hex.clone()).unwrap_or_default();
            let (frame, kind) = match parse_hex(&req.hex) {
                Err(_) => (
                    // 录制数据本身损坏：记 error，不中断整场回放
                    ReplayFrameResult {
                        packet_id: req.id.clone(),
                        request_hex: req.hex.clone(),
                        expected_response_hex: expected_hex,
                        actual_response_hex: String::new(),
                        status: "error".into(),
                        latency_ms: 0,
                        error: Some(BusErrorKind::InvalidFrame.as_str().into()),
                    },
                    None,
                ),
                Ok(request) => {
                    let (actual_hex, latency_ms, kind) =
                        match lease.session.exchange(&request, timeout_ms).await {
                            Ok(out) => (to_hex(&out.bytes), out.elapsed_ms as i32, None),
                            Err(k) => (String::new(), timeout_ms as i32, Some(k.as_str())),
                        };
                    let (status, error) = judge_frame(&expected_hex, &actual_hex, kind);
                    (
                        ReplayFrameResult {
                            packet_id: req.id.clone(),
                            request_hex: to_hex(&request),
                            expected_response_hex: expected_hex,
                            actual_response_hex: actual_hex,
                            status: status.into(),
                            latency_ms,
                            error,
                        },
                        kind,
                    )
                }
            };
            match frame.status.as_str() {
                "match" => match_count += 1,
                "mismatch" => mismatch_count += 1,
                _ => failed += 1,
            }
            let bus_offline = kind == Some(BusErrorKind::BusOffline.as_str());
            let status = frame.status.clone();
            results.push(frame.clone());
            if tx.unbounded_send(ReplayEvent::Frame(frame)).is_err() {
                // 发送失败 = 客户端已断开（契约 §4）
                client_aborted = true;
                break;
            }
            // 总线整体不可用（端口掉线/连接失败）时继续跑只会空耗剩余时长，
            // 直接收尾并把已执行结果返回，现场能立刻看到「跑到第几帧断的」
            if bus_offline {
                tracing::warn!("replay {} 总线不可用，提前结束", session_id);
                stop = Some(Stop::BusOffline);
                break;
            }
            if stop_on_mismatch && status != "match" {
                break;
            }
        }

        if client_aborted {
            // 契约 §4：客户端中断仅落审计（连接已断，aborted 事件发不出去）
            audit(
                "debug.replay.abort",
                &bus_name,
                &format!(
                    "replay={} completed={} total={}",
                    session_id,
                    results.len(),
                    planned
                ),
            );
            return;
        }

        let result = ReplayExecuteResult {
            session_id: session_id.clone(),
            bus: bus_name.clone(),
            started_at_ms,
            finished_at_ms: now_ms(),
            total: results.len() as i32,
            match_count,
            mismatch_count,
            failed,
            results,
        };
        audit(
            "debug.frame.send",
            &bus_name,
            &format!(
                "replay={} total={} match={} mismatch={} failed={}",
                session_id, result.total, result.match_count, result.mismatch_count, result.failed
            ),
        );
        let terminal = match stop {
            None => ReplayEvent::Summary(Box::new(result)),
            Some(Stop::DurationCap) => ReplayEvent::Aborted {
                reason: "duration_cap",
                completed: result.total,
                total: planned,
            },
            Some(Stop::BusOffline) => ReplayEvent::Failed {
                error: BusErrorKind::BusOffline.as_str(),
                message: format!("总线 {} 在回放过程中不可用：端口掉线或驱动异常", bus_name),
                completed: result.total,
                total: planned,
            },
        };
        // 此刻客户端断开也允许：任务收尾即释放租约，发送失败无副作用
        let _ = tx.unbounded_send(terminal);
    });
    rx
}
