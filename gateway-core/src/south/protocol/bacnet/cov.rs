//! COV 推送缓存、订阅状态与后台监听任务。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::task::JoinHandle;

use super::*;

/// 单个对象的 COV 推送缓存项
pub(super) struct CovEntry {
    pub(super) value: f64,
    pub(super) quality: Quality,
    /// 收到通知的时间（ms），作为 DataPoint 时间戳
    pub(super) received_ms: i64,
}

/// COV 订阅与缓存的共享状态（read 路径与后台监听任务共用）
#[derive(Default)]
pub(super) struct CovState {
    /// object id → 最近一次 COV 通知
    pub(super) cache: HashMap<u32, CovEntry>,
    /// object id → 最近一次订阅被设备确认的时间（ms）；缺失即视为订阅不可信
    pub(super) active: HashMap<u32, i64>,
    /// read/read_batch 登记的需要订阅的对象
    pub(super) wanted: HashSet<u32>,
}

/// 后台 COV 监听任务的句柄
pub(super) struct CovLink {
    pub(super) stop: Arc<AtomicBool>,
    pub(super) task: JoinHandle<()>,
}

/// COV 监听 socket 收到的数据报分类
pub(super) enum CovEvent {
    /// 设备推送的最新值
    Notify(frames::CovNotification),
    /// 订阅请求被确认（SimpleACK + 匹配服务号）
    Ack(u8),
    /// 订阅请求被拒绝（Error/Reject/Abort）
    Rejected(u8),
    /// 与 COV 无关的数据报（忽略）
    Ignored,
}

/// 分类一条 COV 监听 socket 上的数据报。
///
/// 只做解析、不改状态，便于用纯数据报单测覆盖通知/ACK/拒绝三种分派。
pub(super) fn classify_cov_datagram(datagram: &[u8]) -> CovEvent {
    if datagram.len() < 6 || datagram[0] != 0x81 || datagram[1] == 0x00 {
        return CovEvent::Ignored;
    }
    let apdu = match BacnetCollector::skip_npdu(&datagram[4..]) {
        Ok(a) => a,
        Err(_) => return CovEvent::Ignored,
    };
    if let Ok(n) = frames::parse_cov_notification_apdu(apdu) {
        return CovEvent::Notify(n);
    }
    match apdu.first() {
        Some(&APDU_SIMPLE_ACK) => match frames::apdu_invoke_id(apdu) {
            Some(id) if frames::parse_subscribe_cov_ack(apdu, id).is_ok() => CovEvent::Ack(id),
            _ => CovEvent::Ignored,
        },
        Some(&APDU_ERROR) | Some(&APDU_REJECT) | Some(&APDU_ABORT) => {
            match frames::apdu_invoke_id(apdu) {
                Some(id) => CovEvent::Rejected(id),
                None => CovEvent::Ignored,
            }
        }
        _ => CovEvent::Ignored,
    }
}

/// 后台 COV 监听循环（Collector 内闭环，不改调度器架构）。
///
/// 独占一条订阅专用 UDP socket，职责三件：
/// 1. 对 `wanted` 中新登记/到期的对象发送 SubscribeCOV（未确认时按固定间隔重试）；
/// 2. 收到 COVNotification 写入推送缓存；
/// 3. 收到 SimpleACK 确认订阅、收到 Error/Reject/Abort 撤销信任。
///
/// 缓存只有在订阅被设备确认且未过期时才被 read 路径采用，因此设备不支持
/// COV 或拒绝订阅时会自动退回主动轮询，不会静默产出陈旧值。
pub(super) async fn cov_listener(
    socket: Arc<UdpSocket>,
    cov: Arc<Mutex<CovState>>,
    stop: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; 1500];
    let mut invoke_id: u8 = 1;
    // invoke id → 对象 ID：订阅请求与 ACK/拒绝之间的归位依据
    let mut pending: HashMap<u8, u32> = HashMap::new();
    // 对象 ID → 下一次发送订阅请求的时间
    let mut next_send: HashMap<u32, tokio::time::Instant> = HashMap::new();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(COV_TICK) => {
                let wanted: Vec<u32> = {
                    let guard = cov.lock().await;
                    guard.wanted.iter().copied().collect()
                };
                let now = tokio::time::Instant::now();
                for oid in wanted {
                    if let Some(t) = next_send.get(&oid) {
                        if *t > now {
                            continue;
                        }
                    }
                    // 先按重试间隔排期；收到 ACK 后改为按续订周期排期
                    next_send.insert(oid, now + COV_RETRY_INTERVAL);
                    let obj_type = ((oid >> 22) & 0x3FF) as u16;
                    let instance = oid & 0x3F_FFFF;
                    let apdu = frames::build_subscribe_cov_apdu(
                        obj_type,
                        instance,
                        invoke_id,
                        COV_SUBSCRIBER_PID,
                        true,
                        Some(COV_LIFETIME_SECS),
                    );
                    let packet = frames::wrap_bvlc_npdu(&apdu);
                    if socket.send(&packet).await.is_ok() {
                        pending.insert(invoke_id, oid);
                    } else {
                        // 发送失败（socket 已坏）：撤销订阅信任，read 回退轮询
                        cov.lock().await.active.remove(&oid);
                    }
                    invoke_id = invoke_id.wrapping_add(1);
                    if invoke_id == 0 {
                        invoke_id = 1;
                    }
                }
            }
            recv = socket.recv(&mut buf) => {
                let n = match recv {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::debug!("BACnet COV recv error: {}", e);
                        continue;
                    }
                };
                match classify_cov_datagram(&buf[..n]) {
                    CovEvent::Notify(n) => {
                        cov.lock().await.cache.insert(
                            n.monitored_object_id,
                            CovEntry {
                                value: n.value,
                                quality: n.quality,
                                received_ms: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                    }
                    CovEvent::Ack(id) => {
                        if let Some(oid) = pending.remove(&id) {
                            cov.lock().await.active.insert(
                                oid,
                                chrono::Utc::now().timestamp_millis(),
                            );
                            next_send.insert(oid, tokio::time::Instant::now() + COV_RENEW_INTERVAL);
                        }
                    }
                    CovEvent::Rejected(id) => {
                        if let Some(oid) = pending.remove(&id) {
                            cov.lock().await.active.remove(&oid);
                        }
                    }
                    CovEvent::Ignored => {}
                }
            }
        }
    }
}
