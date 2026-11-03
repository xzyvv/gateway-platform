//! packet_replay 单元测试：请求配对 / 单帧判定 / 回放引擎事件流

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;

use super::engine::{spawn_replay, ReplayAudit, ReplayEvent};
use super::replay_run::{judge_frame, pair_requests};
use super::{builtin_sessions_for, ReplayPacket, ReplaySession};
use crate::web::api::protocol_tools::DebugLease;
use crate::web::debug_bus::{BusErrorKind, DebugBusTrait, ExchangeOutcome};

/// R1 回归：预置演示会话默认不注入，只有 demo 开关开启时才返回。
#[test]
fn 预置演示会话_默认关闭时为空_开启时才返回模板() {
    assert!(
        builtin_sessions_for(false).is_empty(),
        "生产默认不得返回演示会话"
    );
    assert_eq!(
        builtin_sessions_for(true).len(),
        2,
        "demo 模式应返回 s1/s2 两个预置模板"
    );
}

fn packet(id: &str, time_ms: i64, direction: &str, hex: &str) -> ReplayPacket {
    ReplayPacket {
        id: id.into(),
        time_ms,
        direction: direction.into(),
        hex: hex.into(),
        parsed: String::new(),
    }
}

fn session_with(packets: Vec<ReplayPacket>) -> ReplaySession {
    ReplaySession {
        id: "t".into(),
        name: "t".into(),
        device_name: "t".into(),
        duration_ms: 0,
        packet_count: packets.len() as i32,
        created_at_ms: 0,
        packets,
    }
}

#[test]
fn pair_请求与紧随其后的响应配对() {
    let s = session_with(vec![
        packet("p1", 0, "request", "01 03 00 00 00 02"),
        packet("p2", 10, "response", "01 03 04 00 2A 00 00 8A B9"),
        packet("p3", 100, "request", "01 03 00 64 00 01 C5 D5"),
        packet("p4", 110, "response", "01 03 02 00 63 F8 5F"),
    ]);
    let pairs = pair_requests(&s);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].1.as_ref().unwrap().id, "p2");
    assert_eq!(pairs[1].1.as_ref().unwrap().id, "p4");
}

#[test]
fn pair_中间夹其它请求也能配对到最近响应() {
    let s = session_with(vec![
        packet("p1", 0, "request", "AA"),
        packet("p2", 1, "request", "BB"),
        packet("p3", 2, "response", "CC"),
    ]);
    let pairs = pair_requests(&s);
    assert_eq!(pairs.len(), 2);
    // 两条请求都期望同一条响应（同会话视为同设备，时序上紧邻的那条）
    assert_eq!(pairs[0].1.as_ref().unwrap().id, "p3");
    assert_eq!(pairs[1].1.as_ref().unwrap().id, "p3");
}

#[test]
fn pair_末尾请求无响应时期望为空() {
    let s = session_with(vec![packet("p1", 0, "request", "AA")]);
    let pairs = pair_requests(&s);
    assert!(pairs[0].1.is_none());
}

#[test]
fn judge_十六进制空格与大小写不影响比较() {
    assert_eq!(judge_frame("01 03 ab", "0103AB", None).0, "match");
    assert_eq!(judge_frame("01 03", "01 04", None).0, "mismatch");
}

#[test]
fn judge_录制无响应时本次仍无响应记match_收到响应记mismatch() {
    assert_eq!(judge_frame("", "", None).0, "match");
    assert_eq!(judge_frame("", "01 03", None).0, "mismatch");
}

#[test]
fn judge_录制无响应且本次仍超时记match() {
    assert_eq!(judge_frame("", "", Some("timeout")), ("match", None));
}

#[test]
fn judge_超时与发送失败区分状态() {
    assert_eq!(
        judge_frame("x", "", Some("timeout")),
        ("timeout", Some("timeout".into()))
    );
    assert_eq!(
        judge_frame("x", "", Some("bus_offline")),
        ("error", Some("bus_offline".into()))
    );
}

// ===== P0-1b：回放引擎事件流（契约 §7 #16）=====

/// 可控延时的桩总线：恒超时（只关心时序，不关心帧内容）
struct StubBus {
    delay_ms: u64,
}

impl DebugBusTrait for StubBus {
    fn exchange<'a>(
        &'a self,
        _request: &'a [u8],
        _timeout_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<ExchangeOutcome, BusErrorKind>> + Send + 'a>> {
        let delay = self.delay_ms;
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            Err(BusErrorKind::Timeout)
        })
    }
}

#[tokio::test]
async fn 回放引擎_达到时长上限_发aborted且已完成帧先送达() {
    // 上限 150ms、单帧 60ms、间隔 40ms：帧1 于 ≥60ms 完成，帧2 于 ≥160ms 完成，
    // 帧3 开始前 elapsed≥160≥150 → duration_cap。
    // tokio sleep 不会提前返回，上述时间下界确定，与机器快慢无关。
    let pairs = vec![
        (packet("p1", 0, "request", "01 03"), None),
        (packet("p2", 0, "request", "01 03"), None),
        (packet("p3", 0, "request", "01 03"), None),
    ];
    let audit: ReplayAudit = Arc::new(|_, _, _| {});
    let lease = DebugLease::for_test(Arc::new(StubBus { delay_ms: 60 }));
    let mut rx = spawn_replay(
        audit,
        "t".into(),
        "b".into(),
        lease,
        pairs,
        1000,
        40,
        false,
        std::time::Duration::from_millis(150),
    );

    let mut seq: Vec<&str> = Vec::new();
    while let Some(ev) = rx.next().await {
        match ev {
            ReplayEvent::Started { total, .. } => {
                assert_eq!(total, 3, "started.total = 计划总帧数");
                seq.push("started");
            }
            ReplayEvent::Frame(_) => seq.push("frame"),
            ReplayEvent::Aborted {
                reason,
                completed,
                total,
            } => {
                assert_eq!(reason, "duration_cap");
                assert_eq!(completed, 2, "已达上限前完成的 2 帧先送达");
                assert_eq!(total, 3);
                seq.push("aborted");
            }
            other => panic!("时长上限场景不应出现 {:?}", other),
        }
    }
    assert_eq!(seq, ["started", "frame", "frame", "aborted"]);
}

#[tokio::test]
async fn 回放引擎_接收端断开_下一帧前停止并落abort审计() {
    // 单帧延时 200ms：帧1 于 ~200ms 送达后即丢弃接收端（模拟客户端 abort）；
    // 任务在帧2 的间隔 sleep 后发现 is_closed → 不再下发帧2，收尾并写
    // debug.replay.abort 审计。时序下界确定，无竞态。
    let pairs = vec![
        (packet("p1", 0, "request", "01 03"), None),
        (packet("p2", 0, "request", "01 03"), None),
        (packet("p3", 0, "request", "01 03"), None),
    ];
    let audits = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let audits2 = audits.clone();
    let audit: ReplayAudit = Arc::new(move |action, _bus, detail| {
        audits2.lock().unwrap().push(format!("{action}|{detail}"));
    });

    let mut rx = spawn_replay(
        audit,
        "t".into(),
        "b".into(),
        DebugLease::for_test(Arc::new(StubBus { delay_ms: 200 })),
        pairs,
        1000,
        20,
        false,
        std::time::Duration::from_secs(60),
    );
    // 收到首帧后立刻断开
    match rx.next().await {
        Some(ReplayEvent::Started { .. }) => {}
        other => panic!("首事件应为 started，实际 {:?}", other),
    }
    match rx.next().await {
        Some(ReplayEvent::Frame(_)) => {}
        other => panic!("次事件应为 frame，实际 {:?}", other),
    }
    drop(rx);

    // 等任务感知断开并收尾（间隔 20ms 量级，留足余量）
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let logs = audits.lock().unwrap();
    assert!(
        logs.iter().any(|l| l.starts_with("debug.replay.abort|")),
        "客户端断开应落 debug.replay.abort 审计，实际：{:?}",
        *logs
    );
    assert!(
        !logs.iter().any(|l| l.starts_with("debug.frame.send|")),
        "中断场景不应写常规回放完成审计，实际：{:?}",
        *logs
    );
}
