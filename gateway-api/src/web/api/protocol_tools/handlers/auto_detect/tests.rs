//! auto_detect 单元测试：探测帧构造 / 单步判定 / 探测引擎中止

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::detect::{spawn_auto_detect, DetectAudit};
use super::judge::{
    judge_byteorder, judge_connectivity, judge_fc, judge_latency, judge_slave_id, read_one_frame,
    read_two_frame, suggest_timeout_ms,
};
use super::{initial_steps, AutoDetectResult};
use crate::web::api::protocol_tools::DebugLease;
use crate::web::debug_bus::{BusErrorKind, DebugBusTrait, ExchangeOutcome};
use gateway_core::engine::util::crc::crc16;

fn with_crc(prefix: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.extend_from_slice(&crc16(prefix).to_le_bytes());
    v
}

fn ok_outcome(bytes: Vec<u8>) -> Result<ExchangeOutcome, BusErrorKind> {
    Ok(ExchangeOutcome {
        bytes,
        elapsed_ms: 12,
    })
}

#[test]
fn 帧构造_探测帧功能码地址与crc正确() {
    for fc in [0x03u8, 0x04] {
        let f = read_one_frame(0x02, fc);
        assert_eq!(f[0], 0x02, "slave id");
        assert_eq!(f[1], fc, "功能码");
        assert_eq!(&f[2..6], &[0x00, 0x00, 0x00, 0x01], "地址0+数量1");
        let (body, tail) = f.split_at(f.len() - 2);
        assert_eq!(
            crc16(body),
            u16::from_le_bytes([tail[0], tail[1]]),
            "CRC 必须正确"
        );
    }
    let f = read_two_frame(0x02);
    assert_eq!(f[1], 0x03);
    assert_eq!(&f[2..6], &[0x00, 0x00, 0x00, 0x02], "读 2 个寄存器");
    let (body, tail) = f.split_at(f.len() - 2);
    assert_eq!(crc16(body), u16::from_le_bytes([tail[0], tail[1]]));
}

#[test]
fn judge_连通性_正常与异常都算在线_crc与超时判fail() {
    let out = judge_connectivity(ok_outcome(with_crc(&[0x02, 0x03, 0x02, 0x00, 0x2A])));
    assert_eq!(out.status, "success");
    assert!(out.value.contains("12ms"), "value 应含实测耗时");
    assert_eq!(out.detail, None);

    let out = judge_connectivity(ok_outcome(with_crc(&[0x02, 0x83, 0x02])));
    assert_eq!(out.status, "success", "异常帧证明设备在线");
    assert!(out
        .detail
        .as_deref()
        .unwrap_or("")
        .starts_with("exception:"));

    let out = judge_connectivity(ok_outcome(with_crc(&[0x02, 0x03])));
    assert_eq!(out.status, "fail");
    assert_eq!(out.detail.as_deref(), Some("crc"));

    let out = judge_connectivity(Err(BusErrorKind::Timeout));
    assert_eq!(out.status, "fail");
    assert_eq!(out.detail.as_deref(), Some("设备无响应"));

    let out = judge_connectivity(Err(BusErrorKind::BusOffline));
    assert_eq!(out.status, "fail");
    assert!(out.offline, "bus_offline 必须中断整场探测");
}

#[test]
fn judge_从站地址_复用连通性结论() {
    let out = judge_slave_id(2, true);
    assert_eq!(out.status, "success");
    assert_eq!(out.value, "2");
    let out = judge_slave_id(2, false);
    assert_eq!(out.status, "fail");
    assert!(out.detail.is_some());
}

#[test]
fn judge_功能码_异常也算识别_全超时才fail() {
    let out = judge_fc(
        ok_outcome(with_crc(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
        ok_outcome(with_crc(&[0x02, 0x84, 0x02])),
    );
    assert_eq!(out.status, "success");
    assert_eq!(out.value, "03,04", "异常帧也算识别 FC04");
    assert!(out.detail.as_deref().unwrap_or("").contains("exception"));

    let out = judge_fc(Err(BusErrorKind::Timeout), Err(BusErrorKind::Timeout));
    assert_eq!(out.status, "fail");
    assert_eq!(out.value, "");
    assert!(out.detail.as_deref().unwrap_or("").contains("timeout"));

    let out = judge_fc(
        ok_outcome(with_crc(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
        Err(BusErrorKind::Timeout),
    );
    assert_eq!(out.status, "success");
    assert_eq!(out.value, "03");
    assert_eq!(out.detail, None, "正常应答不附诊断备注");
}

#[test]
fn 建议超时_ms由实测最大值推导() {
    assert_eq!(suggest_timeout_ms(7), 300, "极快响应取下限 300");
    assert_eq!(suggest_timeout_ms(200), 400);
    assert_eq!(suggest_timeout_ms(2000), 3000, "超上限取 3000");
    assert_eq!(suggest_timeout_ms(100_000), 3000);
    assert_eq!(suggest_timeout_ms(155), 310, "结果取整到 10ms");
}

#[test]
fn judge_耗时_至少一帧应答即success_全超时fail() {
    let out = judge_latency(vec![
        ok_outcome(with_crc(&[0x02, 0x03, 0x02, 0x00, 0x01])),
        Err(BusErrorKind::Timeout),
        ok_outcome(with_crc(&[0x02, 0x83, 0x02])),
    ]);
    assert_eq!(out.status, "success");
    assert_eq!(out.value, "建议 300ms");
    assert!(out.detail.as_deref().unwrap_or("").contains("12ms"));
    assert!(out.detail.as_deref().unwrap_or("").contains("超时"));

    let out = judge_latency(vec![
        Err(BusErrorKind::Timeout),
        Err(BusErrorKind::Timeout),
        Err(BusErrorKind::Timeout),
    ]);
    assert_eq!(out.status, "fail");
}

#[test]
fn judge_字节序_四字节给出四种解释且不下结论() {
    let out = judge_byteorder(ok_outcome(with_crc(&[
        0x02, 0x03, 0x04, 0x00, 0x01, 0x00, 0x02,
    ])));
    assert_eq!(out.status, "success");
    assert_eq!(out.value, "00 01 00 02");
    let detail = out.detail.unwrap();
    assert!(detail.contains("ABCD=65538"), "实际 {detail}");
    assert!(detail.contains("BADC="));
    assert!(detail.contains("CDAB="));
    assert!(detail.contains("DCBA="));
    assert!(detail.contains("需结合点位语义确认"));
}

#[test]
fn judge_字节序_数据不足与crc判fail() {
    let out = judge_byteorder(ok_outcome(with_crc(&[0x02, 0x03, 0x02, 0x00, 0x01])));
    assert_eq!(out.status, "fail");
    assert_eq!(out.detail.as_deref(), Some("数据字节不足"));

    let out = judge_byteorder(ok_outcome(vec![0x02, 0x03, 0x04, 0x00]));
    assert_eq!(out.status, "fail");
    assert_eq!(out.detail.as_deref(), Some("crc"));
}

/// 恒超时的桩总线（测试时序用）
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

/// 契约 §8 #60：stop 中止 → status=aborted，已完成步骤保留
#[tokio::test]
async fn 探测引擎_stop中止后status为aborted且保留已完成步骤() {
    let audit: DetectAudit = Arc::new(|_, _, _| {});
    let result = Arc::new(Mutex::new(AutoDetectResult {
        id: "t".into(),
        device_id: "dev".into(),
        device_name: "设备".into(),
        bus: "b".into(),
        slave_id: 1,
        steps: initial_steps(),
        status: "running".into(),
        progress: 0,
        start_time_ms: 0,
        end_time_ms: None,
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    spawn_auto_detect(
        audit,
        "dev".into(),
        "设备".into(),
        "b".into(),
        DebugLease::for_test(Arc::new(StubBus { delay_ms: 30 })),
        result.clone(),
        cancel.clone(),
        1,
        1000,
        20,
    );
    // 等 1 项完成（~30ms）再 stop
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    cancel.store(true, Ordering::Relaxed);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let r = result.lock();
    assert_eq!(r.status, "aborted");
    assert!(
        r.steps.iter().any(|s| s.status != "pending"),
        "aborted 前已完成的步骤必须保留"
    );
    assert_eq!(r.progress, 100);
    assert!(r.end_time_ms.is_some());
}
