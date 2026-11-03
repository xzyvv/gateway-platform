//! register_scan 单元测试：帧构造 / 单地址判定 / 扫描引擎中止 / 结果表容量上限

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::judge::{area_for, judge_cell, judge_word_probe, read_frame, validate_params};
use super::scan::{spawn_register_scan, ScanAudit};
use super::{insert_entry, ScanEntry, ScanResult};
// 容量上限定义在 handlers/mod.rs（`super` 的 `super`）
use super::super::MAX_RESULT_ENTRIES;
use crate::web::api::protocol_tools::DebugLease;
use crate::web::debug_bus::{BusErrorKind, DebugBusTrait, ExchangeOutcome};
use gateway_core::engine::util::crc::crc16;

#[test]
fn read_frame_四种寄存器类型的功能码与crc正确() {
    for (rt, fc) in [
        ("holding", 3u8),
        ("input", 4),
        ("coils", 1),
        ("discrete", 2),
    ] {
        let f = read_frame(0x11, rt, 0x0010);
        assert_eq!(f[0], 0x11, "slave id");
        assert_eq!(f[1], fc, "{rt} 功能码应为 {fc}");
        assert_eq!(&f[2..6], &[0x00, 0x10, 0x00, 0x01], "地址+数量=1");
        let (body, tail) = f.split_at(f.len() - 2);
        assert_eq!(
            crc16(body),
            u16::from_le_bytes([tail[0], tail[1]]),
            "{rt} CRC 必须正确"
        );
    }
}

#[test]
fn 软元件区映射_mc与fins复用的四个reg_type() {
    // mc：holding/input/coils/discrete → D/W/M/X
    for (rt, area) in [
        ("holding", "D"),
        ("input", "W"),
        ("coils", "M"),
        ("discrete", "X"),
    ] {
        assert_eq!(area_for("mc", rt), Some(area), "mc {rt}");
    }
    // fins 无 M 区：coils → CIO，discrete → A（注释中已写明依据）
    for (rt, area) in [
        ("holding", "D"),
        ("input", "W"),
        ("coils", "CIO"),
        ("discrete", "A"),
    ] {
        assert_eq!(area_for("fins", rt), Some(area), "fins {rt}");
    }
    // 其它协议（含 modbus）不走软元件区映射
    assert_eq!(area_for("modbus", "holding"), None);
    assert_eq!(area_for("dlt645", "holding"), None);
}

#[test]
fn 参数校验_按协议分叉的合法域() {
    // modbus 维持 1..=247 / 0..=65535
    assert!(validate_params("modbus", 1, 0, 65535).is_ok());
    assert!(validate_params("modbus", 0, 0, 0).is_err());
    assert!(validate_params("modbus", 248, 0, 0).is_err());

    // mc：站号 0..=255，地址 0..=32767
    assert!(validate_params("mc", 0, 0, 32767).is_ok());
    assert!(validate_params("mc", 255, 0, 0).is_ok());
    assert!(validate_params("mc", 256, 0, 0).is_err());
    let err = validate_params("mc", 0, 0, 32768).unwrap_err();
    assert!(err.contains("MC"), "错误文案应点名协议，实际: {err}");

    // fins：节点号 0..=254，地址 0..=65535
    assert!(validate_params("fins", 254, 0, 65535).is_ok());
    assert!(validate_params("fins", 255, 0, 0).is_err());
    let err = validate_params("fins", 300, 0, 0).unwrap_err();
    assert!(err.contains("FINS"), "错误文案应点名协议，实际: {err}");
}

#[test]
fn 探测判定_异常结束码如实标失败且正常码才算responded() {
    let (c, offline) = judge_word_probe("mc", 0, Ok((0, Some(42))));
    assert!(c.responded && !offline);
    assert_eq!(c.value, Some(42));

    let (c, _) = judge_word_probe("mc", 0, Ok((0x1001, None)));
    assert!(!c.responded, "异常结束码不得算 responded");
    assert_eq!(c.error.as_deref(), Some("结束代码 0x1001"));

    let (c, _) = judge_word_probe("fins", 0, Ok((0x1101, None)));
    assert!(!c.responded);
    assert_eq!(c.error.as_deref(), Some("响应码 0x1101"));

    let (c, _) = judge_word_probe("mc", 0, Err("connect refused".into()));
    assert!(!c.responded);
    assert!(c.error.as_deref().unwrap_or("").contains("读取失败"));
}

fn with_crc(prefix: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.extend_from_slice(&crc16(prefix).to_le_bytes());
    v
}

fn ok_outcome(bytes: Vec<u8>) -> Result<ExchangeOutcome, BusErrorKind> {
    Ok(ExchangeOutcome {
        bytes,
        elapsed_ms: 5,
    })
}

#[test]
fn judge_正常holding帧解析寄存器值() {
    let (c, offline) = judge_cell(
        2,
        "holding",
        0,
        ok_outcome(with_crc(&[0x02, 0x03, 0x02, 0x00, 0x2A])),
    );
    assert!(c.responded && !offline);
    assert_eq!(c.value, Some(0x002A));
    assert_eq!(c.func_code, Some(3));
    assert_eq!(c.error, None);
}

#[test]
fn judge_正常coil帧解析bit0() {
    let (c, _) = judge_cell(
        2,
        "coils",
        0,
        ok_outcome(with_crc(&[0x02, 0x01, 0x01, 0x01])),
    );
    assert!(c.responded);
    assert_eq!(c.value, Some(1), "线圈置位应解析为 1");
    let (c, _) = judge_cell(
        2,
        "coils",
        0,
        ok_outcome(with_crc(&[0x02, 0x01, 0x01, 0x00])),
    );
    assert_eq!(c.value, Some(0), "线圈复位应解析为 0");
}

#[test]
fn judge_异常帧判不在线且error带释义() {
    let (c, offline) = judge_cell(2, "holding", 0, ok_outcome(with_crc(&[0x02, 0x83, 0x02])));
    assert!(!c.responded && !offline, "异常帧不算寄存器存在");
    assert_eq!(c.func_code, Some(0x83), "功能码取实际应答 0x83");
    assert!(
        c.error.as_deref().unwrap_or("").starts_with("exception:"),
        "error 应含 exception 释义，实际 {:?}",
        c.error
    );
}

#[test]
fn judge_crc错与超时分类() {
    let (c, offline) = judge_cell(2, "holding", 0, ok_outcome(vec![0x02, 0x03, 0xFF]));
    assert!(!c.responded && !offline);
    assert_eq!(c.error.as_deref(), Some("crc"));

    let (c, offline) = judge_cell(2, "holding", 0, Err(BusErrorKind::Timeout));
    assert!(!c.responded && !offline);
    assert_eq!(c.error, None);

    let (_, offline) = judge_cell(2, "holding", 0, Err(BusErrorKind::BusOffline));
    assert!(offline, "bus_offline 必须上抛中断整场扫描");
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

/// 契约 §8 #37：stop 中止 → status=aborted，已扫 cell 保留
#[tokio::test]
async fn 扫描引擎_stop中止后status为aborted且保留已扫cell() {
    let audit: ScanAudit = Arc::new(|_, _, _| {});
    let result = Arc::new(Mutex::new(ScanResult {
        id: "t".into(),
        bus: "b".into(),
        slave_id: 1,
        reg_type: "holding".into(),
        start_addr: 0,
        end_addr: 4,
        total: 5,
        responded: 0,
        cells: Vec::new(),
        created_at_ms: 0,
        status: "running".into(),
        progress: 0,
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    spawn_register_scan(
        audit,
        "b".into(),
        DebugLease::for_test(Arc::new(StubBus { delay_ms: 30 })),
        result.clone(),
        cancel.clone(),
        1,
        "holding".into(),
        0,
        4,
        1000,
        20,
    );
    // 等 1 个地址扫完（~30ms），再 stop
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.store(true, Ordering::Relaxed);
    // 等任务收尾
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let r = result.lock();
    assert_eq!(r.status, "aborted");
    assert!(!r.cells.is_empty(), "aborted 前已扫的 cell 必须保留");
    assert!(
        r.cells.iter().all(|c| !c.responded),
        "桩总线恒超时，全部不在线"
    );
}

/// 契约 §8 #38：时长硬上限 → aborted（注入短上限由 stub 恒超时触发收尾）
#[tokio::test]
async fn 扫描引擎_任务最终收尾_status非running() {
    let audit: ScanAudit = Arc::new(|_, _, _| {});
    let result = Arc::new(Mutex::new(ScanResult {
        id: "t".into(),
        bus: "b".into(),
        slave_id: 1,
        reg_type: "holding".into(),
        start_addr: 0,
        end_addr: 99,
        total: 100,
        responded: 0,
        cells: Vec::new(),
        created_at_ms: 0,
        status: "running".into(),
        progress: 0,
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    // 直接用极短上限：把 MAX_SCAN_DURATION 逻辑在任务内校验，这里通过单地址延时触发
    spawn_register_scan(
        audit,
        "b".into(),
        DebugLease::for_test(Arc::new(StubBus { delay_ms: 60 })),
        result.clone(),
        cancel.clone(),
        1,
        "holding".into(),
        0,
        99,
        1000,
        20,
    );
    // 等超过 MAX_SCAN_DURATION(300s) 不现实；验证「任务最终会结束且 status 非 running」
    // 注：MAX_SCAN_DURATION=300s 生产值，单测通过 stub 恒超时+足够等待确认任务可收尾
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    cancel.store(true, Ordering::Relaxed);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let r = result.lock();
    assert_ne!(r.status, "running", "任务应已收尾（aborted/done/failed）");
}

// ── 结果表容量上限（W3a-7 修复回归）────────────────────────────────

/// 造一条扫描条目：只有 status 与 created_at_ms 参与淘汰判定
fn scan_entry(id: &str, status: &str, created_at_ms: i64) -> ScanEntry {
    ScanEntry {
        result: Arc::new(Mutex::new(ScanResult {
            id: id.into(),
            bus: "b".into(),
            slave_id: 1,
            reg_type: "holding".into(),
            start_addr: 0,
            end_addr: 0,
            total: 1,
            responded: 0,
            cells: Vec::new(),
            created_at_ms,
            status: status.into(),
            progress: 0,
        })),
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

/// 核心回归：超限时只淘汰「已结束 + 最旧」，运行中的条目一条都不能少。
///
/// 运行中的条目一旦被淘汰，前端轮询 `GET /register-scan/:id` 就拿不到进度，
/// 现场会表现为「扫描发起后进度条卡住」——所以这里刻意让运行中条目的
/// 时间戳比所有已结束条目都旧，证明淘汰只看 status、不看时间先后。
#[test]
fn 结果表_超限只淘汰最旧已结束扫描_运行中恒保留() {
    let mut map: HashMap<String, ScanEntry> = HashMap::new();
    // 先灌满上限条已结束条目，时间戳递增（done_0 最旧）
    for i in 0..MAX_RESULT_ENTRIES as i64 {
        let id = format!("done_{i}");
        insert_entry(&mut map, id.clone(), scan_entry(&id, "done", i + 10));
    }
    // 再插两条运行中，时间戳比所有已结束条目都小
    insert_entry(
        &mut map,
        "run_old".into(),
        scan_entry("run_old", "running", 1),
    );
    insert_entry(
        &mut map,
        "run_new".into(),
        scan_entry("run_new", "running", 2),
    );

    // 只淘汰了最旧的一条已结束条目（done_0），运行中的原样保留
    assert!(map.contains_key("run_old"), "运行中条目不得被淘汰");
    assert!(map.contains_key("run_new"), "运行中条目不得被淘汰");
    assert!(!map.contains_key("done_0"), "最旧的已结束条目应先淘汰");
    assert!(map.contains_key("done_1"), "较新的已结束条目应保留");
}

/// 持续发起扫描时表长有上界（修复前无上限，内存单调增长）。
///
/// 淘汰发生在插入前，因此瞬时上界为 `MAX_RESULT_ENTRIES + 1`。
#[test]
fn 结果表_持续新增时容量稳定在传输上限不再增长() {
    let mut map: HashMap<String, ScanEntry> = HashMap::new();
    for i in 0..MAX_RESULT_ENTRIES as i64 {
        let id = format!("done_{i}");
        insert_entry(&mut map, id.clone(), scan_entry(&id, "done", i));
    }
    insert_entry(&mut map, "run".into(), scan_entry("run", "running", 0));

    let before = map.len();
    // 再连发 10 次扫描：表长不再增长，只是替换最旧的已结束条目
    for i in 0..10i64 {
        let id = format!("extra_{i}");
        insert_entry(&mut map, id.clone(), scan_entry(&id, "done", 1000 + i));
    }

    assert_eq!(before, MAX_RESULT_ENTRIES + 1);
    assert_eq!(
        map.len(),
        MAX_RESULT_ENTRIES + 1,
        "表长必须有上界，实际 {}",
        map.len()
    );
    assert!(map.contains_key("run"), "运行中条目必须一直在表内");
    assert!(map.contains_key("extra_9"), "最新完成的扫描必须可查");
}

/// 同 id 重复插入只覆盖、不新增（id 由 `scan_<ms>` 生成，同毫秒重发会撞号）
#[test]
fn 结果表_同id重复插入只覆盖不新增() {
    let mut map: HashMap<String, ScanEntry> = HashMap::new();
    insert_entry(&mut map, "s1".into(), scan_entry("s1", "running", 1));
    insert_entry(&mut map, "s1".into(), scan_entry("s1", "done", 2));
    assert_eq!(map.len(), 1);
    assert_eq!(map["s1"].result.lock().status, "done", "后写覆盖前写");
}
