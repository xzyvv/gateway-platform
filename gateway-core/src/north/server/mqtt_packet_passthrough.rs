//! MQTT 报文透传（透明传输）
//!
//! 命名说明：本模块原名 `passthrough`，是「MQTT 收报文 → 总线转发 → MQTT 回报文」的
//! 纯报文搬运器，不做任何协议解析。为避免与 Modbus 网关桥接（`modbus_gateway_bridge`）
//! 混淆，改名为 `mqtt_packet_passthrough`，类型名 `PassthroughManager` → `MqttPacketPassthrough`。
//! 对外兼容面保持不变：MQTT topic `{base_topic}/passthrough/in|out`、`/api/passthrough/sessions`。
//!
//! 网关作为服务器与设备之间的报文桥梁，不做协议解析：
//! - 服务器通过 MQTT 下发采集报文（hex 格式）
//! - 网关转发到指定串口/TCP 总线
//! - 读取设备响应
//! - 把响应报文（hex）通过 MQTT 发回服务器
//!
//! 指令 topic: {base_topic}/passthrough/in
//! 响应 topic: {base_topic}/passthrough/out
//!
//! 指令 payload (JSON):
//! {
//!   "bus": "com1",                    // 目标总线名
//!   "data": "010300000002C40B",       // hex 格式原始报文
//!   "timeout": 1000,                  // 可选，超时 ms，默认 2000
//!   "id": "req-001"                   // 可选，请求 ID，响应带回
//! }
//!
//! 响应 payload (JSON):
//! {
//!   "id": "req-001",
//!   "bus": "com1",
//!   "data": "010304000A001471D3",     // hex 格式响应报文
//!   "ok": true,
//!   "error": null,
//!   "elapsed_ms": 15
//! }

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Semaphore};

use crate::engine::error::{GatewayError, GatewayResult};
use crate::north::mqtt::MqttClient;
use crate::support::app::GatewayRuntime;

/// r30-PS2: 客户端可指定 `timeout` 字段；硬上限 30 秒防止慢攻击占满总线/任务槽。
pub(crate) const MAX_PASSTHROUGH_TIMEOUT_MS: u64 = 30_000;
/// r30-PS3 + r30b-PS4: passthrough 双向字节数上限。
/// - 入口（PS-3）：单条 passthrough 指令 data 字段解码后字节数。
/// - 响应（PS-4）：设备返回响应字节数。
///
/// 4 KiB 的取值理由：
/// - 串口/Modbus 帧最长通常 < 256B；Modbus RTU 256 字节、DLT645 ≤ 200、CJT188 ≤ 200、
///   BACnet/IP 1500 MSS、IEC 101/104 ≤ 255/253、S7comm 480 PDU、MC 960、FINS 2000。
/// - 4 KiB 留 4x BACnet/IP、2x FINS 余量，覆盖所有透传常见协议。
/// - OPC UA 透传场景理论上 Browse/HistoryRead 可达 8-32 KiB，但当前网关 OPC UA
///   走专用协议路径（`tcp_batch_readonly`，无需透传）；passthrough 入口/响应
///   双限 4 KiB 的限制由"网关生产透传仅用于 Modbus/BCD 计量/IEC 系列"约束支撑。
///
/// 如未来生产透传需支持 OPC UA 大包，应拆分为
/// `MAX_PASSTHROUGH_REQUEST_BYTES` 与 `MAX_PASSTHROUGH_RESPONSE_BYTES` 两个独立常量，
/// 单独放宽响应侧至 32 KiB。
pub(crate) const MAX_PASSTHROUGH_BYTES: usize = 4 * 1024;
/// r30-PS1: 全局并发上限。同号 RS485 总线半双工并发会冲突，过多并发也会耗尽
/// tokio 任务槽。默认 16 够常规场景，又能在洪泛时立刻返回错误而非排队拖死。
pub(crate) const MAX_CONCURRENT_PASSTHROUGH: usize = 16;

/// 透传指令（服务器 → 网关）
#[derive(Debug, Deserialize)]
pub(crate) struct PassthroughRequest {
    /// 目标总线名（对应 BusConfig.port_name）
    pub(crate) bus: String,
    /// hex 格式原始报文
    pub(crate) data: String,
    /// 超时（毫秒），默认 2000
    #[serde(default = "default_timeout")]
    pub(crate) timeout: u64,
    /// 请求 ID（可选，响应时带回）
    #[serde(default)]
    pub(crate) id: String,
}

fn default_timeout() -> u64 {
    2000
}

/// 透传响应（网关 → 服务器）
#[derive(Debug, Serialize)]
pub(crate) struct PassthroughResponse {
    pub(crate) id: String,
    pub(crate) bus: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<String>,
    pub(crate) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) elapsed_ms: u64,
}

/// MQTT 报文透传管理器
/// B11: passthrough session snapshot
#[derive(Debug, Clone, serde::Serialize)]
pub struct PassthroughSession {
    pub id: String,
    pub bus: String,
    pub started_at: u64,
    pub timeout_ms: u64,
    /// r30b-PS5: 唯一 token（全局原子计数器自增），用于 SessionGuard drop 时
    /// 精确匹配删除。旧实现用 (id, bus, started_at) 三元组匹配——同毫秒 +
    /// 同 id/bus 会出现误删（例如客户端用固定 id 反复测试）。
    /// 序列化时 skip（不对外暴露内部计数）。
    #[serde(skip)]
    pub token: u64,
}

/// r30b-PS5: 全局 token 计数器。每构造一个 SessionGuard 自增一次，
/// 保证 token 单调递增，drop 时精确匹配（无碰撞）。
static SESSION_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// B11: RAII guard; removes the session from the list on drop
struct SessionGuard {
    sessions: Arc<std::sync::Mutex<Vec<PassthroughSession>>>,
    token: u64,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.sessions.lock() {
            let token = self.token;
            // r30b-PS5: 改用 token 单值匹配，O(n) 但 n 极小（并发 session ≤ 16），
            // 杜绝 (id, bus, started_at) 三元组碰撞误删。
            guard.retain(|s| s.token != token);
        }
    }
}

pub struct MqttPacketPassthrough {
    mqtt: Arc<MqttClient>,
    gateway: Arc<GatewayRuntime>,
    /// 指令 topic
    in_topic: String,
    /// 响应 topic
    out_topic: String,
    /// B11: active session table
    sessions: Arc<std::sync::Mutex<Vec<PassthroughSession>>>,
    /// r30-PS1: 并发上限 Semaphore。`try_acquire_owned` 在拿不到 permit 时立即返回
    /// `AcquireError`，调用方据此回错误给客户端，不排队、不阻塞。
    concurrency: Arc<Semaphore>,
    /// r73-SD1：与 r61/r62 同型停机标志 + Notify。
    /// 旧实现 run 依赖 incoming channel 的 sender 被 drop 才退出，而 sender 存在
    /// `MqttClient` 内部 → 直到进程结束才 drop。因此整个停机窗口内透传仍在处理
    /// 下行指令、执行设备 I/O 并向已停的 MQTT 发布响应（响应静默丢失）。
    stopping: Arc<std::sync::atomic::AtomicBool>,
    stop_notify: Arc<tokio::sync::Notify>,
}

impl MqttPacketPassthrough {
    /// 创建 MQTT 报文透传管理器
    ///
    /// `base_topic` 为 MQTT 配置中的 topic，透传指令/响应 topic 在此基础上派生。
    pub fn new(mqtt: Arc<MqttClient>, gateway: Arc<GatewayRuntime>, base_topic: &str) -> Arc<Self> {
        Arc::new(Self {
            mqtt,
            gateway,
            in_topic: format!("{}/passthrough/in", base_topic),
            out_topic: format!("{}/passthrough/out", base_topic),
            sessions: Arc::new(std::sync::Mutex::new(Vec::new())),
            concurrency: Arc::new(Semaphore::new(MAX_CONCURRENT_PASSTHROUGH)),
            // r73-SD1：停机标志 + Notify（参考 r61/r62）。
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stop_notify: Arc::new(tokio::sync::Notify::new()),
        })
    }

    /// r73-SD1：触发透传 run 循环退出（设计参考 r48-MQTT1 / r61-SD1 / r72-SD1）。
    /// 先置位 stopping 再 `notify_waiters`，覆盖 `rx.recv().await` 在 wait 与 stop
    /// 之间的丢失唤醒窗口。幂等。
    pub async fn stop(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.stop_notify.notify_waiters();
    }

    /// r73-SD1：暴露 stopping 标志供测试断言 stop() 已生效。
    #[cfg(test)]
    pub(crate) fn is_stopping_set(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// B11: query active passthrough sessions
    pub fn list_sessions(&self) -> Vec<PassthroughSession> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 启动透传：订阅指令 topic，循环处理
    pub async fn run(&self) {
        // 创建 incoming 通道并设置到 MqttClient
        let (tx, mut rx) = mpsc::channel::<(String, Vec<u8>)>(256);
        self.mqtt.set_incoming_handler(tx);

        // 订阅指令 topic
        if let Err(e) = self.mqtt.subscribe(&self.in_topic).await {
            tracing::error!("passthrough subscribe {} failed: {}", self.in_topic, e);
            return;
        }
        tracing::info!(
            "passthrough mode started, in={}, out={}",
            self.in_topic,
            self.out_topic
        );

        // 循环处理下行指令
        // r73-SD1：停机标志 + Notify 双保险——避免 rx.recv() 与 stop() 之间的丢失唤醒。
        loop {
            if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let msg = tokio::select! {
                biased;
                _ = self.stop_notify.notified() => break,
                m = rx.recv() => m,
            };
            let Some((topic, payload)) = msg else {
                break;
            };
            if topic != self.in_topic {
                continue;
            }
            let out_topic = self.out_topic.clone();
            let mqtt = self.mqtt.clone();
            let gateway = self.gateway.clone();
            let sessions = self.sessions.clone();
            // r30-PS1: 传 semaphore 进去，handle_request 入口 try_acquire 限流。
            let concurrency = self.concurrency.clone();
            crate::engine::supervision::spawn_catch_unwind("passthrough_request", async move {
                let resp = Self::handle_request(&gateway, &payload, sessions, concurrency).await;
                if let Ok(json) = serde_json::to_vec(&resp) {
                    mqtt.publish_raw(&out_topic, json).await;
                }
            });
        }
        tracing::info!("passthrough manager stopped");
    }

    /// 处理一条透传指令
    pub(crate) async fn handle_request(
        gateway: &GatewayRuntime,
        payload: &[u8],
        sessions: Arc<std::sync::Mutex<Vec<PassthroughSession>>>,
        concurrency: Arc<Semaphore>,
    ) -> PassthroughResponse {
        let start = std::time::Instant::now();
        let err_response = |id: String, bus: String, err: String| PassthroughResponse {
            id,
            bus,
            data: None,
            ok: false,
            error: Some(err),
            elapsed_ms: start.elapsed().as_millis() as u64,
        };

        // r30-PS1: 入口限流——拿不到 permit 立刻返回错误，不排队、不阻塞 tokio 任务。
        // permit 在函数内 RAII，离开 handle_request 时自动释放。
        let permit = match concurrency.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                return err_response(
                    String::new(),
                    String::new(),
                    format!(
                        "too many concurrent passthrough requests (limit={})",
                        MAX_CONCURRENT_PASSTHROUGH
                    ),
                );
            }
        };

        // 1. 解析 JSON 指令
        let req: PassthroughRequest = match serde_json::from_slice(payload) {
            Ok(r) => r,
            Err(e) => {
                return err_response(String::new(), String::new(), format!("invalid json: {}", e));
            }
        };

        // r30-PS3 (修复)：在 hex 解码前先用字符串长度估算上限，避免先分配大 Vec。
        // hex 字符串 = 2 * 字节数 → 字节数 > MAX/2 即拒绝。
        if req.data.len() > MAX_PASSTHROUGH_BYTES * 2 {
            return err_response(
                req.id,
                req.bus,
                format!(
                    "data too long: {} hex chars > max {} ({} bytes)",
                    req.data.len(),
                    MAX_PASSTHROUGH_BYTES * 2,
                    MAX_PASSTHROUGH_BYTES
                ),
            );
        }

        let bus_name = req.bus.clone();
        // r30c-PS7 (修复)：空 id 直接回 `id=""`，多个空 id 请求彼此不可区分。
        // 用 `anon-<started_at>` 补默认 id（仍包含时间戳便于排序），便于排障。
        let raw_id = req.id.clone();
        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let req_id = if raw_id.is_empty() {
            format!("anon-{}", started_at)
        } else {
            raw_id
        };
        // r30b-PS5: 用全局原子计数器发 token，保证唯一。fetch_add(1) 返回旧值，
        // 第一次调用返回 1（自增后变 2），与 SESSION_TOKEN 初值 1 对应。
        let token = SESSION_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _guard = SessionGuard {
            sessions: sessions.clone(),
            token,
        };
        // 让 permit 进入 _permit 命名空间，与 _guard 一起在函数末尾统一释放。
        let _permit = permit;

        // 2. hex 解码
        let request_bytes = match hex_decode(&req.data) {
            Ok(b) => b,
            Err(e) => {
                return err_response(req_id, bus_name, format!("invalid hex: {}", e.message()));
            }
        };

        // 3. 获取目标总线
        let bus = match gateway.bus_manager(&req.bus) {
            Some(b) => b,
            None => {
                return err_response(req_id, bus_name, format!("bus '{}' not found", req.bus));
            }
        };

        // 4. 发送报文并等待响应（带超时）
        // r30-PS2 (修复)：把客户端指定的 timeout 钳到硬上限，防止慢攻击
        // `timeout=u64::MAX` 占用任务 + 总线槽位直到永远。
        let effective_timeout_ms = req.timeout.clamp(1, MAX_PASSTHROUGH_TIMEOUT_MS);
        let timeout = Duration::from_millis(effective_timeout_ms);
        // r30b-PS5: 真正用 effective_timeout_ms 注册 session 快照（晚于 hex/bus
        // 校验失败路径不影响：失败时 SessionGuard::drop 会按 token 精确移除自己，
        // 不会误删其他 session）。
        {
            let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
            guard.push(PassthroughSession {
                id: req_id.clone(),
                bus: bus_name.clone(),
                started_at,
                timeout_ms: effective_timeout_ms,
                token,
            });
        }
        let result = tokio::time::timeout(timeout, bus.transaction(request_bytes, None)).await;

        match result {
            Ok(Ok(response)) => {
                // r30b-PS4 (修复)：响应长度无上限时，恶意/异常设备返回 1MB
                // 响应会让 outbox 写 2MB 行（hex 编码后翻倍），单行 SQLite 写入
                // 开销巨大、Outbox 表膨胀、Prometheus 抓取出 OOM。透传语义上
                // 截断会破坏协议（CRC 校验不过），故选择直接拒而非截断。
                if response.len() > MAX_PASSTHROUGH_BYTES {
                    return err_response(
                        req_id,
                        bus_name,
                        format!(
                            "response too long: {} bytes > max {} (refusing to publish \
                             oversized response that would bloat outbox/JSON payload)",
                            response.len(),
                            MAX_PASSTHROUGH_BYTES
                        ),
                    );
                }
                PassthroughResponse {
                    id: req_id,
                    bus: bus_name,
                    data: Some(hex_encode(&response)),
                    ok: true,
                    error: None,
                    elapsed_ms: start.elapsed().as_millis() as u64,
                }
            }
            Ok(Err(e)) => err_response(req_id, bus_name, format!("bus error: {}", e)),
            Err(_) => err_response(
                req_id,
                bus_name,
                format!("timeout after {}ms", effective_timeout_ms),
            ),
        }
    }
}

/// hex 字符串解码为字节数组
pub(crate) fn hex_decode(s: &str) -> GatewayResult<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(GatewayError::Protocol("odd length".into()));
    }
    // 修复(审查#A2)：原实现用 `&s[i..i + 2]` 按**字节下标**做字符串切片，data 里一旦出现
    // 中文等多字节字符，下标就会落在字符边界中间 → `str` 切片 panic（异常被上层
    // catch_unwind 吞掉，主站永远收不到任何响应，现场表现为「下发指令无任何返回」）。
    // 改为按 ASCII 逐字节解析十六进制，非十六进制字符返回 Protocol 错误而非 panic。
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

/// 单个 ASCII 字节转十六进制半字节；非 `[0-9a-fA-F]` 返回 Protocol 错误
fn hex_nibble(b: u8) -> GatewayResult<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(GatewayError::Protocol(format!(
            "invalid hex byte 0x{:02x}",
            b
        ))),
    }
}

/// 字节数组编码为 hex 字符串
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod ps5_tests {
    use super::*;

    /// r30b-PS5 (回归)：SessionGuard 现在用全局唯一 token（来自 SESSION_TOKEN 原子
    /// 计数器）替代 (id, bus, started_at) 三元组匹配。旧实现在同毫秒 + 同 id/bus
    /// 场景下会误删其他活跃 session。
    ///
    /// 本测试直接构造两个 SessionGuard 持有不同 token，drop 第一个后 sessions
    /// 列表应只保留第二个。模拟"同 id/bus/stared_at"场景：两个 session 共享
    /// 相同 id/bus/started_at，但 token 不同 → 各自精确删自己。
    #[test]
    fn session_guard_uses_unique_token_for_precise_eviction() {
        let sessions = Arc::new(std::sync::Mutex::new(Vec::new()));
        // 模拟同毫秒同 id/bus：两个 session 字段全相同，唯一区别是 token
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let token1 = SESSION_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let token2 = SESSION_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_ne!(token1, token2, "连续 fetch_add 必须给不同 token");

        sessions.lock().unwrap().push(PassthroughSession {
            id: "test".into(),
            bus: "com1".into(),
            started_at: now_ms,
            timeout_ms: 1000,
            token: token1,
        });
        sessions.lock().unwrap().push(PassthroughSession {
            id: "test".into(),  // 同 id
            bus: "com1".into(), // 同 bus
            started_at: now_ms, // 同 started_at
            timeout_ms: 1000,
            token: token2,
        });
        assert_eq!(sessions.lock().unwrap().len(), 2);

        // drop 第一个 guard → 应只删 token1 那个，另一个保留
        {
            let _g1 = SessionGuard {
                sessions: sessions.clone(),
                token: token1,
            };
            // _g1 drop on scope end
        }
        let remaining = sessions.lock().unwrap();
        assert_eq!(remaining.len(), 1, "drop 第一个 guard 后应剩 1 个 session");
        assert_eq!(remaining[0].token, token2, "保留的应是 token2");
    }

    /// r30b-PS5 补充：连续 fetch_add 返回值单调递增（fetch_add 语义：返回旧值然后 +1，
    /// 故两次连续调用必然 next2 > next1）。跨多线程也无碰撞（不做并发测，但
    /// 至少保证序列化调用下单调）。
    #[test]
    fn session_token_is_monotonic() {
        // 拿两个连续 token，next2 必须 > next1（fetch_add 内部对所有线程原子）。
        let next1 = SESSION_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let next2 = SESSION_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(
            next2 > next1,
            "连续 fetch_add 第二次必须返回更大值：next1={} next2={}",
            next1,
            next2
        );
    }
}

// ===== r73-SD1：透传管理器纳入优雅停机 =====

#[cfg(test)]
mod r73_tests {
    use super::*;

    /// r73-SD1：`stop()` 置位 stopping，幂等。
    #[tokio::test]
    async fn r73_passthrough_stop_sets_stopping_flag_and_is_idempotent() {
        let pt = match build_pt() {
            Some(p) => p,
            None => return, // 环境无法构建 MqttClient（例如 broker 解析失败）→ 跳过
        };
        assert!(!pt.is_stopping_set(), "初始不应处于 stopping 状态");
        pt.stop().await;
        assert!(pt.is_stopping_set(), "stop() 必须置位 stopping");
        pt.stop().await;
        pt.stop().await;
        assert!(pt.is_stopping_set());
    }

    /// r73-SD1（关键）：`stop()` 后 run 必须在数秒内退出。
    ///
    /// 旧实现 run 依赖 incoming channel 的 sender 被 drop 才退出，而 sender 存在
    /// `MqttClient` 内部（直到进程结束才 drop）→ run 在停机窗口内不会退出。
    #[tokio::test]
    async fn r73_passthrough_run_exits_after_stop() {
        let pt = match build_pt() {
            Some(p) => p,
            None => return,
        };
        let pt_for_run = pt.clone();
        let handle =
            crate::engine::supervision::spawn_catch_unwind("passthrough_r73_test", async move {
                pt_for_run.run().await;
            });
        // 等 run 完成 set_incoming_handler + subscribe 并进入 rx.recv() 等待
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !handle.is_finished(),
            "run 应处于等待 rx.recv() 的运行态（若已提前返回，说明 subscribe 失败，本测试无效）"
        );

        pt.stop().await;
        let res = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
        assert!(
            res.is_ok(),
            "r73: stop() 后 passthrough run 必须在 3s 内退出"
        );
    }

    /// r73 测试用：构建一个可用的 MqttPacketPassthrough。
    ///
    /// 不需要 broker 真实可达——rumqttc 的 `subscribe`/`set_incoming_handler`
    /// 只入队，run 随后阻塞在 `rx.recv()`。broker 地址用不可达端口避免真实建连。
    fn build_pt() -> Option<Arc<MqttPacketPassthrough>> {
        use crate::config::MqttConfig;
        use crate::queue::{DataQueue, MemoryStorage, QueueStorage};

        let queue = Arc::new(
            DataQueue::with_storage(Arc::new(MemoryStorage::new()) as Arc<dyn QueueStorage>)
                .expect("build queue"),
        );
        let metrics = Arc::new(crate::engine::metrics::Metrics::new());
        let cfg = MqttConfig {
            enabled: true,
            broker: "mqtt://127.0.0.1:1".to_string(),
            topic: "gw/r73".to_string(),
            ..MqttConfig::default()
        };
        let mqtt = match MqttClient::new(cfg, queue, None, None, metrics) {
            Ok(c) => Arc::new(c),
            Err(_) => return None,
        };
        let gateway = crate::support::app::gateway_runtime_without_buses_for_test();
        Some(MqttPacketPassthrough::new(mqtt, gateway, "gw/r73"))
    }
}
