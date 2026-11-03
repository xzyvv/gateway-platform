//! 探测帧构造与 5 项单步判定
//!
//! 统一走 core 的 Modbus 帧构造单一实现，本文件不再本地复制「裸帧 + CRC」。
//! 判定只做只读观测（FC03/FC04），不产生任何写操作。

use crate::web::debug_bus::{verify_modbus_rtu, BusErrorKind, ExchangeOutcome, FrameCheck};
use crate::web::services::modbus_read_request_frame as build_read_request;

// ── 帧构造 ──────────────────────────────────────────────────────────────

/// 读 1 个寄存器的帧（FC03/FC04，目标地址 0）
pub(super) fn read_one_frame(slave: u8, fc: u8) -> Vec<u8> {
    build_read_request(slave, fc, 0, 1)
}

/// 读 2 个保持寄存器的帧（FC03，目标地址 0），用于字节序观察
pub(super) fn read_two_frame(slave: u8) -> Vec<u8> {
    build_read_request(slave, 0x03, 0, 2)
}

// ── 单步判定 ────────────────────────────────────────────────────────────

/// 单步结论（`status` 复用前端已有的 success/fail 着色，fail 不展示 value）
pub(super) struct StepOut {
    pub(super) status: &'static str,
    pub(super) value: String,
    pub(super) detail: Option<String>,
    /// 是否总线掉线（整场探测立即收尾为 failed）
    pub(super) offline: bool,
}

impl StepOut {
    pub(super) fn success(value: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            status: "success",
            value: value.into(),
            detail,
            offline: false,
        }
    }

    pub(super) fn fail(detail: impl Into<String>) -> Self {
        Self {
            status: "fail",
            value: String::new(),
            detail: Some(detail.into()),
            offline: false,
        }
    }

    pub(super) fn offline(detail: impl Into<String>) -> Self {
        Self {
            status: "fail",
            value: String::new(),
            detail: Some(detail.into()),
            offline: true,
        }
    }
}

/// 第 1 项：总线连通性。异常帧也算"在线"（设备解析了帧只是拒绝了内容）。
pub(super) fn judge_connectivity(outcome: Result<ExchangeOutcome, BusErrorKind>) -> StepOut {
    match outcome {
        Ok(out) => match verify_modbus_rtu(&out.bytes) {
            FrameCheck::Ok => StepOut::success(format!("已应答 {}ms", out.elapsed_ms), None),
            FrameCheck::Exception { text, .. } => StepOut::success(
                format!("已应答 {}ms", out.elapsed_ms),
                Some(format!("exception:{text}")),
            ),
            FrameCheck::Crc => StepOut::fail("crc"),
        },
        Err(BusErrorKind::BusOffline) => StepOut::offline("bus_offline"),
        Err(_) => StepOut::fail("设备无响应"),
    }
}

/// 第 2 项：从站地址。复用第 1 项结论，不另发帧。
pub(super) fn judge_slave_id(slave_id: u8, online: bool) -> StepOut {
    if online {
        StepOut::success(slave_id.to_string(), None)
    } else {
        StepOut::fail("从站在当前参数下无应答")
    }
}

/// 单个功能码应答归一化：(是否被设备识别, 诊断备注)
fn fc_note(fc: u8, outcome: &Result<ExchangeOutcome, BusErrorKind>) -> (bool, String) {
    match outcome {
        Ok(out) => match verify_modbus_rtu(&out.bytes) {
            FrameCheck::Ok => (true, format!("{fc:02X}:ok")),
            // 异常帧证明功能码被识别，只是地址/权限被拒
            FrameCheck::Exception { text, .. } => (true, format!("{fc:02X}:exception:{text}")),
            FrameCheck::Crc => (false, format!("{fc:02X}:crc")),
        },
        Err(BusErrorKind::BusOffline) => (false, format!("{fc:02X}:bus_offline")),
        Err(_) => (false, format!("{fc:02X}:timeout")),
    }
}

/// 第 3 项：功能码支持（FC03 / FC04）。
pub(super) fn judge_fc(
    out03: Result<ExchangeOutcome, BusErrorKind>,
    out04: Result<ExchangeOutcome, BusErrorKind>,
) -> StepOut {
    let (ok03, note03) = fc_note(3, &out03);
    let (ok04, note04) = fc_note(4, &out04);
    let offline = matches!(&out03, Err(BusErrorKind::BusOffline))
        || matches!(&out04, Err(BusErrorKind::BusOffline));
    let notes = format!("{note03} {note04}");

    if ok03 || ok04 {
        let mut fcs: Vec<&str> = Vec::new();
        if ok03 {
            fcs.push("03");
        }
        if ok04 {
            fcs.push("04");
        }
        // 仅在出现异常帧时附诊断备注，正常应答不打扰前端
        let detail = if notes.contains("exception") {
            Some(notes)
        } else {
            None
        };
        StepOut {
            status: "success",
            value: fcs.join(","),
            detail,
            offline,
        }
    } else {
        StepOut {
            status: "fail",
            value: String::new(),
            detail: Some(notes),
            offline,
        }
    }
}

/// 由三次实测耗时的最大值推导建议超时：`clamp(m×2, 300, 3000)` 向下取整到 10ms。
pub(super) fn suggest_timeout_ms(max_elapsed_ms: u64) -> u64 {
    let doubled = max_elapsed_ms.saturating_mul(2);
    (doubled.clamp(300, 3000) / 10) * 10
}

/// 第 4 项：响应耗时 → 建议超时。异常帧的耗时同样计入（设备确实回了帧）。
pub(super) fn judge_latency(outcomes: Vec<Result<ExchangeOutcome, BusErrorKind>>) -> StepOut {
    let mut observed: Vec<String> = Vec::with_capacity(outcomes.len());
    let mut answered = false;
    let mut max_ms = 0u64;
    let mut offline = false;
    for outcome in &outcomes {
        match outcome {
            Ok(out) => {
                answered = true;
                observed.push(format!("{}ms", out.elapsed_ms));
                if out.elapsed_ms > max_ms {
                    max_ms = out.elapsed_ms;
                }
            }
            Err(BusErrorKind::BusOffline) => {
                observed.push("bus_offline".into());
                offline = true;
            }
            Err(_) => observed.push("超时".into()),
        }
    }
    let detail = Some(format!("三次实测: {}", observed.join(", ")));
    if answered {
        StepOut {
            status: "success",
            value: format!("建议 {}ms", suggest_timeout_ms(max_ms)),
            detail,
            offline,
        }
    } else {
        StepOut {
            status: "fail",
            value: String::new(),
            detail,
            offline,
        }
    }
}

/// 第 5 项：字节序。只报 4 字节原文与四种解释，不做自动断言
/// （仅凭数据字节无法反推设备/点位的真实字节序，硬下结论会误导现场）。
pub(super) fn judge_byteorder(outcome: Result<ExchangeOutcome, BusErrorKind>) -> StepOut {
    match outcome {
        Ok(out) => match verify_modbus_rtu(&out.bytes) {
            FrameCheck::Ok => {
                let data_len = out.bytes[2] as usize;
                if data_len >= 4 && out.bytes.len() >= 3 + data_len + 2 {
                    let b0 = out.bytes[3];
                    let b1 = out.bytes[4];
                    let b2 = out.bytes[5];
                    let b3 = out.bytes[6];
                    let abcd = ((b0 as u32) << 24)
                        | ((b1 as u32) << 16)
                        | ((b2 as u32) << 8)
                        | (b3 as u32);
                    let badc = ((b1 as u32) << 24)
                        | ((b0 as u32) << 16)
                        | ((b3 as u32) << 8)
                        | (b2 as u32);
                    let cdab = ((b2 as u32) << 24)
                        | ((b3 as u32) << 16)
                        | ((b0 as u32) << 8)
                        | (b1 as u32);
                    let dcba = ((b3 as u32) << 24)
                        | ((b2 as u32) << 16)
                        | ((b1 as u32) << 8)
                        | (b0 as u32);
                    let hex = [b0, b1, b2, b3]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    StepOut::success(
                        hex,
                        Some(format!(
                            "ABCD={abcd} BADC={badc} CDAB={cdab} DCBA={dcba}（需结合点位语义确认）"
                        )),
                    )
                } else {
                    StepOut::fail("数据字节不足")
                }
            }
            FrameCheck::Exception { text, .. } => StepOut::fail(format!("exception:{text}")),
            FrameCheck::Crc => StepOut::fail("crc"),
        },
        Err(BusErrorKind::BusOffline) => StepOut::offline("bus_offline"),
        Err(_) => StepOut::fail("设备无应答"),
    }
}
