//! IEC 104 总召唤采集与建链对时（决策表 #17 / #19）

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::south::protocol::common::asdu::IoaDataMap;
use crate::south::protocol::{CollectorError, CollectorResult};

use super::frames::{
    build_clock_sync_asdu, build_iframe, build_interrogation_asdu, build_sframe, build_uframe,
    is_interrogation_termination, parse_asdu, read_frame,
};
use super::{
    Iec104Collector, ACK_EVERY_N_FRAMES, CLOCK_SYNC_TIMEOUT, TESTFR_ACT_CF, TESTFR_CON_CF,
};

impl Iec104Collector {
    /// 决策表 #19：对时（C_CS_NA_1, 103）
    ///
    /// 发送携带本机 UTC 时间的时钟同步命令，短暂等待从站应答（任一 I-frame，
    /// 通常为 ASDU 103 的激活确认/终止）。best-effort：任何失败仅告警，
    /// 不返回错误、不打断建链——个别从站不支持对时不应导致整链不可用。
    pub(super) async fn send_clock_sync(&self, stream: &mut TcpStream) {
        let now = chrono::Utc::now();
        let asdu = build_clock_sync_asdu(1, self.asdu_addr_len, &now);
        let ns = self.seq_send.load(Ordering::Relaxed);
        let nr = self.seq_recv.load(Ordering::Relaxed);
        // C-4：帧构造失败必须显式退出。对时是 best-effort 路径，仅告警、不打断建链，
        // 但绝不能像旧实现那样写一个空帧出去冒充"发送成功"。
        let frame = match build_iframe(ns, nr, &asdu) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("IEC104 clock sync frame build failed: {}", e);
                return;
            }
        };
        if let Err(e) = stream.write_all(&frame).await {
            tracing::warn!("IEC104 clock sync send failed: {}", e);
            return;
        }
        // A4: 强制 15-bit 半窗口。N(S) 是模 32768 计数器，32767 后自然回绕到 0，
        // 不能跳过 0（否则对端观察到序号跳变）。& 0x7FFF 保证 cf[1].bit7 恒为 0。
        self.seq_send
            .store(ns.wrapping_add(1) & 0x7FFF, Ordering::Relaxed);

        let deadline = tokio::time::Instant::now() + CLOCK_SYNC_TIMEOUT;
        loop {
            match tokio::time::timeout_at(deadline, read_frame(stream)).await {
                Ok(Ok(body)) => {
                    if body.len() < 4 {
                        continue;
                    }
                    // 应答 server 探测，避免建链后立即被判定失联
                    if body[0] == TESTFR_ACT_CF[0] {
                        let _ = stream.write_all(&build_uframe(TESTFR_CON_CF)).await;
                        continue;
                    }
                    if body[0] & 0x01 == 1 {
                        continue; // S/U frame：不携带数据
                    }
                    self.seq_recv.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(
                        "IEC104 clock sync confirmed (asdu type 0x{:02X})",
                        body.get(4).copied().unwrap_or(0)
                    );
                    return;
                }
                Ok(Err(e)) => {
                    tracing::warn!("IEC104 clock sync read failed: {}", e);
                    return;
                }
                Err(_) => {
                    tracing::warn!(
                        "IEC104 clock sync no response within {:?}",
                        CLOCK_SYNC_TIMEOUT
                    );
                    return;
                }
            }
        }
    }

    /// 执行一次总召唤，并用本轮快照**整体替换**数据缓存
    ///
    /// 流程：
    /// 1. 发总召唤 I-frame（携带 N(S) = 发送序号、N(R) = 已收 I-frame 数）
    /// 2. 循环收帧：识别并应答 server 的 TESTFR 探测；I-frame 累加 N(R) 并解析 ASDU
    /// 3. 每 `ACK_EVERY_N_FRAMES` 个 I-frame 回一次 S-frame，避免 server 发送窗口
    ///    （k ≤ 12 未确认 I-frame）打满而停发
    /// 4. 收到总召唤激活终止（类型 100 / COT=10）提前结束，不再干等满 io_timeout
    /// 5. 收尾补一次 S-frame 确认；用本轮快照替换缓存
    ///
    /// 返回 `Ok(())` 表示本轮收到数据且快照已刷新；一帧数据都没收到返回
    /// `Err(NoReply)`（上层按坏质量处理，绝不返回陈旧值）。
    pub(super) async fn interrogate(&self) -> CollectorResult<()> {
        let mut guard = self.stream.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| CollectorError::IoError("IEC104 not connected".into()))?;

        // 发总召唤 I-frame：N(S) 取下一个发送序号，N(R) 捎带确认已收的响应帧
        let ns = self.seq_send.load(Ordering::Relaxed);
        let nr = self.seq_recv.load(Ordering::Relaxed);
        // C-4：帧构造失败直接返回错误，不再写空帧
        let frame = build_iframe(ns, nr, &build_interrogation_asdu(1, self.asdu_addr_len))?;
        if let Err(e) = stream.write_all(&frame).await {
            *guard = None;
            self.connected.store(false, Ordering::Relaxed);
            return Err(CollectorError::IoError(format!(
                "send interrogation: {}",
                e
            )));
        }
        // A4: 强制 15-bit 半窗口。N(S) 是模 32768 计数器，32767 后自然回绕到 0，
        // 不能跳过 0（否则对端观察到序号跳变）。& 0x7FFF 保证 cf[1].bit7 恒为 0。
        self.seq_send
            .store(ns.wrapping_add(1) & 0x7FFF, Ordering::Relaxed);

        let mut data_map: IoaDataMap = HashMap::new();
        let mut unacked: u16 = 0;
        let mut last_acked = nr;
        // 本轮读取中链路是否损坏（读帧错误 / 对端关连接 → early EOF）
        let mut link_broken = false;
        let deadline = tokio::time::Instant::now() + self.io_timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            match tokio::time::timeout_at(deadline, read_frame(stream)).await {
                Ok(Ok(body)) => {
                    if body.len() < 4 {
                        continue;
                    }
                    let control = &body[0..4];
                    if control[0] == TESTFR_ACT_CF[0] {
                        // C-12: server 主动探测本端 → 必须回 TESTFR_CON
                        if let Err(e) = stream.write_all(&build_uframe(TESTFR_CON_CF)).await {
                            tracing::warn!("IEC104 reply TESTFR_CON failed: {}", e);
                            *guard = None;
                            self.connected.store(false, Ordering::Relaxed);
                            return Err(CollectorError::IoError("reply TESTFR_CON failed".into()));
                        }
                        continue;
                    }
                    if control[0] & 0x01 == 1 {
                        // S-frame / 其他 U-frame：不携带数据，忽略
                        continue;
                    }
                    // I-frame：累加 N(R)，按需回 S-frame 释放 server 发送窗口
                    let recv = self
                        .seq_recv
                        .fetch_add(1, Ordering::Relaxed)
                        .wrapping_add(1);
                    unacked += 1;
                    let asdu_data = &body[4..];
                    let terminated = is_interrogation_termination(asdu_data);
                    parse_asdu(asdu_data, &mut data_map, self.asdu_addr_len);
                    if unacked >= ACK_EVERY_N_FRAMES {
                        if let Err(e) = stream.write_all(&build_sframe(recv)).await {
                            tracing::warn!("IEC104 send S-frame failed: {}", e);
                            *guard = None;
                            self.connected.store(false, Ordering::Relaxed);
                            return Err(CollectorError::IoError("send S-frame failed".into()));
                        }
                        last_acked = recv;
                        unacked = 0;
                    }
                    if terminated {
                        break;
                    }
                }
                Ok(Err(e)) => {
                    // 读帧出错（含对端在应答后直接关连接导致的 early EOF）：
                    // 旧实现直接 return IoError，会把本轮**已收到的数据帧**一并丢弃，
                    // 现场表现为「明明读到值却报采集失败」。改为按「本轮窗口提前结束」
                    // 处理——链路已断，仍标记断连供下次 read 惰性重连；已收到的数据由
                    // 收尾的 data_map.is_empty() 决定：有则用本轮数据返回 Ok，无则 NoReply。
                    tracing::warn!("IEC104 read frame error: {}", e);
                    // 此处不能直接 `*guard = None`：`stream` 仍借用自 `guard`，
                    // 待循环退出、`stream` 借用结束后再清空并置断连。
                    link_broken = true;
                    break;
                }
                // 采集窗口到点：用本轮已收到的部分数据
                Err(_) => break,
            }
        }

        // 收尾确认：把尚未确认的接收序号补发给 server。
        // 链路已损坏时不写（对端已关连接，写必然失败且无意义）。
        if !link_broken {
            let nr_now = self.seq_recv.load(Ordering::Relaxed);
            if unacked > 0 && nr_now != last_acked {
                if let Err(e) = stream.write_all(&build_sframe(nr_now)).await {
                    tracing::warn!("IEC104 send final S-frame failed: {}", e);
                }
            }
        }
        // `stream` 借用已结束：此时才能安全清空 stream 并置断连，
        // 供下一次 read 惰性重连。
        if link_broken {
            *guard = None;
            self.connected.store(false, Ordering::Relaxed);
        }
        drop(guard);

        if data_map.is_empty() {
            // 本轮一帧数据都没收到：不覆盖旧快照（保持断连可见，由上层判 stale）
            return Err(CollectorError::NoReply);
        }
        // 整体替换快照：本轮未出现的 IOA 不再返回旧值
        *self.data.lock().await = data_map;
        Ok(())
    }
}
