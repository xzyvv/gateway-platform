//! IEC 104 控制方向命令下发（决策表 #18）

use std::sync::atomic::Ordering;

use tokio::io::AsyncWriteExt;

use crate::south::protocol::{CollectorError, CollectorResult};

use super::frames::{
    build_command_asdu, build_iframe, build_uframe, parse_command_confirmation, read_frame,
    COT_ACTIVATION_CONFIRMATION, COT_ACTIVATION_TERMINATION, COT_UNKNOWN_CA, COT_UNKNOWN_COT,
    COT_UNKNOWN_IOA, COT_UNKNOWN_TYPE_ID,
};
use super::{Iec104Collector, TESTFR_ACT_CF, TESTFR_CON_CF};

impl Iec104Collector {
    /// 决策表 #18：控制方向命令下发（C_SC_NA_1 45 / C_DC_NA_1 46 / C_RC_NA_1 50）
    ///
    /// 与对时（best-effort）不同，控制命令必须等到从站肯定确认才算成功：
    /// - 收到**本命令 IOA** 的 COT=激活确认(7)/激活终止(10) → Ok
    /// - COT=未知类型(44) → Unsupported；未知 IOA(47) → InvalidConfig；未知传送原因(45)/
    ///   未知公共地址(46)/其余否定 → InvalidFrame
    /// - 等待窗口内无确认 → NoReply；读帧/写帧失败 → 清空 stream 标记断连（下次 write/read
    ///   惰性重连），避免半开链路上命令被静默吞掉
    ///
    /// 等待期间响应 server 的 TESTFR 探测，避免长时间等确认时被判失联。
    pub(super) async fn send_command(&self, ioa: u32, type_id: u8, obj: u8) -> CollectorResult<()> {
        let mut guard = self.stream.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| CollectorError::IoError("IEC104 not connected".into()))?;

        let ns = self.seq_send.load(Ordering::Relaxed);
        let nr = self.seq_recv.load(Ordering::Relaxed);
        let asdu = build_command_asdu(type_id, 1, self.asdu_addr_len, ioa, obj);
        // C-4：ASDU 超长时显式失败（不再写空帧冒充成功）。此处链路本身完好，
        // 故不清空 stream、不置断连。
        let frame = build_iframe(ns, nr, &asdu)?;

        let mut link_broken = false;
        let outcome: CollectorResult<()> = if let Err(e) = stream.write_all(&frame).await {
            link_broken = true;
            Err(CollectorError::IoError(format!("send command: {}", e)))
        } else {
            // A4: 强制 15-bit 半窗口。N(S) 是模 32768 计数器，32767 后自然回绕到 0，
            // 不能跳过 0（否则对端观察到序号跳变）。& 0x7FFF 保证 cf[1].bit7 恒为 0。
            self.seq_send
                .store(ns.wrapping_add(1) & 0x7FFF, Ordering::Relaxed);
            let deadline = tokio::time::Instant::now() + self.io_timeout;
            loop {
                match tokio::time::timeout_at(deadline, read_frame(stream)).await {
                    Ok(Ok(body)) => {
                        if body.len() < 4 {
                            continue;
                        }
                        // 响应 server 探测，避免等确认期间被判失联
                        if body[0] == TESTFR_ACT_CF[0] {
                            if let Err(e) = stream.write_all(&build_uframe(TESTFR_CON_CF)).await {
                                link_broken = true;
                                break Err(CollectorError::IoError(format!(
                                    "reply TESTFR_CON failed: {}",
                                    e
                                )));
                            }
                            continue;
                        }
                        // S-frame / 其他 U-frame：不携带业务数据，忽略
                        if body[0] & 0x01 == 1 {
                            continue;
                        }
                        self.seq_recv.fetch_add(1, Ordering::Relaxed);
                        let asdu_data = &body[4..];
                        if let Some((_t, resp_ioa, cot)) =
                            parse_command_confirmation(asdu_data, self.asdu_addr_len)
                        {
                            // 串行下发命令，仅接受本命令 IOA 的确认；其他视为噪声继续等待
                            if resp_ioa == ioa {
                                let r =
                                    match cot {
                                        COT_ACTIVATION_CONFIRMATION
                                        | COT_ACTIVATION_TERMINATION => Ok(()),
                                        COT_UNKNOWN_TYPE_ID => Err(CollectorError::Unsupported(
                                            "device does not support this command type".into(),
                                        )),
                                        COT_UNKNOWN_IOA => Err(CollectorError::InvalidConfig(
                                            format!("IOA {} not known by device", ioa),
                                        )),
                                        COT_UNKNOWN_COT => Err(CollectorError::InvalidFrame(
                                            "device reports unknown cause of transmission".into(),
                                        )),
                                        COT_UNKNOWN_CA => Err(CollectorError::InvalidConfig(
                                            "device reports unknown common address".into(),
                                        )),
                                        other => Err(CollectorError::InvalidFrame(format!(
                                            "command rejected by device, COT={}",
                                            other
                                        ))),
                                    };
                                break r;
                            }
                        }
                        // 非命令确认（如总召唤数据、无关 IOA 的确认）：忽略继续等
                    }
                    Ok(Err(e)) => {
                        link_broken = true;
                        break Err(CollectorError::IoError(format!(
                            "command read failed: {}",
                            e
                        )));
                    }
                    Err(_) => break Err(CollectorError::NoReply),
                }
            }
        };

        // `stream` 借用已结束：此时才能安全清空 stream 并置断连，供下次惰性重连
        if link_broken {
            *guard = None;
            self.connected.store(false, Ordering::Relaxed);
        }
        drop(guard);
        outcome
    }
}
