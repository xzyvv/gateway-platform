//! IEC 104 写语义定级测试（决策表 #18 的控制方向命令）。
//!
//! 判据只有一个：**能否证明命令没有执行**。
//! - 从站回了否定确认（COT=44/45/46/47 等）→ 命令确定未执行 → 明确失败，
//!   调用方可安全重试；
//! - 从站不回（超时 / 断链 / 读帧失败）→ 无法证明 → 结果未知，
//!   **禁止**自动重试（遥控、脉冲类点位重复下发就是事故）。
use super::*;

use crate::config::PointConfig;
use crate::south::protocol::CollectorError;

/// 从站桩：完成 STARTDT 握手 + 对时应答，然后对控制命令按 `cot` 应答。
///
/// `cot = None` 时对命令保持沉默（模拟从站不回）。
async fn command_slave(listener: tokio::net::TcpListener, cot: Option<u8>) {
    let Ok((mut stream, _)) = listener.accept().await else {
        return;
    };
    // 1) STARTDT act → con
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    if stream
        .write_all(&build_uframe([0x0B, 0x00, 0x00, 0x00]))
        .await
        .is_err()
    {
        return;
    }
    // 2) 建链对时 → 回一帧 I 帧确认
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    let ack = build_iframe(0, 1, &[0x64, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00])
        .expect("I-frame 构造失败");
    if stream.write_all(&ack).await.is_err() {
        return;
    }
    // 3) 控制命令 → 按 cot 应答（或沉默）
    let Some(cot) = cot else {
        // 保持连接但不应答命令；等主站超时后退出
        tokio::time::sleep(Duration::from_secs(2)).await;
        return;
    };
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    // 确认 ASDU：类型 45（单点命令）+ COT + CA(2 字节) + IOA(3 字节) + 命令值
    let mut asdu = build_command_asdu(TYPE_C_SC_NA_1, 1, 2, 100, 0x81);
    asdu[2] = cot; // COT 低字节（OA 在 asdu[3]）
    let frame = build_iframe(1, 2, &asdu).expect("命令确认 I-frame 构造失败");
    let _ = stream.write_all(&frame).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// 造一个 IOA=100 的单点控制点位
fn control_point() -> PointConfig {
    serde_json::from_value(serde_json::json!({
        "id": "cmd1",
        "bus": "b1",
        "protocol": "iec104",
        "address": "100",
        "function_code": 45,
        "interval": 1,
    }))
    .expect("point config")
}

/// 从站回了否定确认（未知传送原因 COT=45）→ 命令确定未执行 → `DeviceRejected`
#[tokio::test]
async fn negative_confirmation_is_device_rejected() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假从站端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    let server = tokio::spawn(command_slave(listener, Some(COT_UNKNOWN_COT as u8)));

    let collector = Iec104Collector::new(
        addr,
        "bus_iec104_cmd".to_string(),
        Duration::from_millis(500),
        2,
    );

    let err = collector
        .write(&control_point(), 1.0)
        .await
        .expect_err("否定确认必须判失败");
    assert!(
        matches!(err, CollectorError::DeviceRejected(_)),
        "unexpected: {err:?}"
    );
    server.abort();
}

/// 从站回了未知类型标识（COT=44）→ 明确不支持 → `Unsupported`（明确失败，不重试）
#[tokio::test]
async fn unknown_type_id_is_unsupported() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假从站端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    let server = tokio::spawn(command_slave(listener, Some(COT_UNKNOWN_TYPE_ID as u8)));

    let collector = Iec104Collector::new(
        addr,
        "bus_iec104_cmd2".to_string(),
        Duration::from_millis(500),
        2,
    );

    let err = collector
        .write(&control_point(), 1.0)
        .await
        .expect_err("否定确认必须判失败");
    assert!(
        matches!(err, CollectorError::Unsupported(_)),
        "unexpected: {err:?}"
    );
    server.abort();
}

/// 从站不回 → 无法证明命令未执行 → `WriteUncertain`（禁止自动重试）
#[tokio::test]
async fn no_confirmation_is_write_uncertain() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假从站端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    let server = tokio::spawn(command_slave(listener, None));

    let collector = Iec104Collector::new(
        addr,
        "bus_iec104_cmd3".to_string(),
        Duration::from_millis(300),
        2,
    );

    let err = collector
        .write(&control_point(), 1.0)
        .await
        .expect_err("无确认必须判失败");
    assert!(
        matches!(err, CollectorError::WriteUncertain(_)),
        "unexpected: {err:?}"
    );
    server.abort();
}
