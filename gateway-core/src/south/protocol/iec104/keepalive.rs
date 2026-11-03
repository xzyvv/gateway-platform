//! IEC 104 长连接 keepalive supervisor（C-12）
//!
//! 周期探测、应答 server 探测并在响应窗口内 drain I-frame，与 read 路径共享
//! 同一把 `stream` 锁（用 try_lock 避免阻塞采集）。

use std::sync::atomic::Ordering;

use tokio::io::AsyncWriteExt;

use super::frames::{build_sframe, build_uframe, read_frame};
use super::{
    Iec104Collector, ACK_EVERY_N_FRAMES, KEEPALIVE_INTERVAL, KEEPALIVE_MAX_CONSECUTIVE_FAILURES,
    KEEPALIVE_RESPONSE_TIMEOUT, TESTFR_ACT_CF, TESTFR_CON_CF,
};

impl Iec104Collector {
    /// C-12: 启动 keepalive supervisor task
    ///
    /// 独立 task 周期探测 + 监听连接状态；与 read 路径共享 `stream` 的 Arc<Mutex>
    /// （用 try_lock 避免阻塞 read）。连接失败/读错误 → 标记 connected=false。
    pub(super) fn spawn_keepalive_supervisor(&self) {
        let stream_slot = self.stream.clone();
        let connected = self.connected.clone();
        let addr = self.addr.clone();
        // S09: supervisor 代读 I-frame 时需累加接收序号，与 read 路径共享同一计数器
        let seq_recv = self.seq_recv.clone();

        let handle = crate::engine::supervision::spawn_catch_unwind(
            format!("iec104_keepalive[{}]", self.bus_name),
            async move {
                // 连续失败计数：网络抖动容忍，避免连接抖动
                let mut consecutive_failures: u32 = 0;
                loop {
                    tokio::time::sleep(KEEPALIVE_INTERVAL).await;

                    // try_lock 抢锁：抢不到说明 read 正在用，链路活跃，
                    // 本周期跳过（下一周期再尝试）
                    let mut guard = match stream_slot.try_lock() {
                        Ok(g) => g,
                        Err(_) => {
                            tracing::trace!("iec104 keepalive: stream busy, skip cycle ({})", addr);
                            continue;
                        }
                    };

                    // 拿锁后再次检查连接状态
                    let Some(stream) = guard.as_mut() else {
                        // 连接已断开（disconnect 调用），supervisor 自然退出
                        tracing::debug!("iec104 keepalive: stream gone, exiting ({})", addr);
                        return;
                    };

                    // 主动发 TESTFR_ACT (0x43) 探测
                    if let Err(e) = stream.write_all(&build_uframe(TESTFR_ACT_CF)).await {
                        tracing::warn!(
                            "iec104 keepalive: send TESTFR_ACT failed for {}: {}",
                            addr,
                            e
                        );
                        *guard = None;
                        connected.store(false, Ordering::Relaxed);
                        return;
                    }

                    // Bug 188 / S09: 探测响应等待 + 链路 drain。
                    //
                    // 旧实现只读 1 帧就返回：若这一帧恰是 server 下发的 I-frame
                    // （量测数据），该帧被 supervisor 吞掉却**没有累加 N(R)**，
                    // 采集路径随后发出的 N(R) 就偏小——server 认为未确认帧持续堆积，
                    // 最终停发甚至断链。现改为在响应窗口内持续 drain：
                    //   - I-frame     → 累加 N(R)，记录未确认数（达阈值即回 S-frame）
                    //   - TESTFR_ACT  → 回 TESTFR_CON 后继续（t1 内不回会被关连接）
                    //   - TESTFR_CON  → 探测得到确认，提前结束 drain
                    //   - S/其他 U    → 忽略，继续
                    // 窗口耗尽后补发一次 S-frame，保证 N(R) 与采集路径一致。
                    let drain_deadline = tokio::time::Instant::now() + KEEPALIVE_RESPONSE_TIMEOUT;
                    let mut unacked: u16 = 0;
                    // 窗口内是否读到过任何帧（含 I-frame / 控制帧）→ 链路活跃证据
                    let mut link_alive = false;
                    loop {
                        match tokio::time::timeout_at(drain_deadline, read_frame(stream)).await {
                            Ok(Ok(body)) => {
                                if body.len() < 4 {
                                    continue;
                                }
                                let cf0 = body[0];
                                if cf0 == TESTFR_ACT_CF[0] {
                                    // server 主动探测本端：必须在 t1 内回 TESTFR_CON，
                                    // 否则 server 单方面关连接
                                    if let Err(e) =
                                        stream.write_all(&build_uframe(TESTFR_CON_CF)).await
                                    {
                                        tracing::warn!(
                                            "iec104 keepalive: reply TESTFR_CON failed for {}: {}",
                                            addr,
                                            e
                                        );
                                        *guard = None;
                                        connected.store(false, Ordering::Relaxed);
                                        return;
                                    }
                                    link_alive = true;
                                    continue;
                                }
                                if cf0 == TESTFR_CON_CF[0] {
                                    // 探测已获确认，链路活跃，提前结束 drain
                                    link_alive = true;
                                    break;
                                }
                                if cf0 & 0x01 == 0 {
                                    // I-frame：累加接收序号，避免 supervisor 偷帧导致序号错乱；
                                    // 未确认数达阈值即回 S-frame，防止 server 发送窗口打满
                                    seq_recv.fetch_add(1, Ordering::Relaxed);
                                    unacked += 1;
                                    link_alive = true;
                                    if unacked >= ACK_EVERY_N_FRAMES {
                                        let nr = seq_recv.load(Ordering::Relaxed);
                                        if let Err(e) = stream.write_all(&build_sframe(nr)).await {
                                            tracing::warn!(
                                                "iec104 keepalive: send S-frame failed for {}: {}",
                                                addr,
                                                e
                                            );
                                            *guard = None;
                                            connected.store(false, Ordering::Relaxed);
                                            return;
                                        }
                                        unacked = 0;
                                    }
                                    continue;
                                }
                                // S-frame / 其他控制帧：server 仍在通信，链路活跃
                                tracing::trace!(
                                    "iec104 keepalive: control frame 0x{:02X}, link alive ({})",
                                    cf0,
                                    addr
                                );
                                link_alive = true;
                                continue;
                            }
                            Ok(Err(e)) => {
                                tracing::warn!(
                                    "iec104 keepalive: read frame failed for {}: {}",
                                    addr,
                                    e
                                );
                                *guard = None;
                                connected.store(false, Ordering::Relaxed);
                                return;
                            }
                            Err(_timeout) => break,
                        }
                    }

                    // drain 结束：补发一次 S-frame，确认本轮读到的 I-frame，
                    // 保持 N(R) 与采集路径一致
                    if unacked > 0 {
                        let nr = seq_recv.load(Ordering::Relaxed);
                        if let Err(e) = stream.write_all(&build_sframe(nr)).await {
                            tracing::warn!(
                                "iec104 keepalive: final S-frame failed for {}: {}",
                                addr,
                                e
                            );
                            *guard = None;
                            connected.store(false, Ordering::Relaxed);
                            return;
                        }
                    }

                    if link_alive {
                        consecutive_failures = 0;
                    } else {
                        consecutive_failures += 1;
                        tracing::warn!(
                            "iec104 keepalive: TESTFR_ACT no response ({}/{}) for {}",
                            consecutive_failures,
                            KEEPALIVE_MAX_CONSECUTIVE_FAILURES,
                            addr
                        );
                        if consecutive_failures >= KEEPALIVE_MAX_CONSECUTIVE_FAILURES {
                            // 连续多次无响应：判定链路半开，清空 stream，
                            // 下次 read 惰性建链时重建（supervisor 本轮退出）
                            *guard = None;
                            connected.store(false, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            },
        );

        // 存到 self.keepalive_handle
        // Bug 168: 覆盖前必须 abort 旧 handle。`*h = Some(handle)` 只是 drop 掉旧的
        // JoinHandle，而 drop 是 detach 语义——旧 keepalive 任务会继续跑，多轮重连后
        // 残留多个任务争抢同一条 TCP 流的读端，既泄漏任务又会互相偷帧。
        if let Ok(mut h) = self.keepalive_handle.try_lock() {
            if let Some(old) = h.take() {
                old.abort();
            }
            *h = Some(handle);
        } else {
            // 极端情况：调用方在持锁时调 spawn（实际不可能）——直接 abort 防泄漏
            handle.abort();
        }
    }
}
