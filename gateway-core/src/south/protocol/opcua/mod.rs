//! OPC UA Client 采集器
//!
//! 作为 OPC UA Client 连接到第三方 OPC UA Server，读取变量值。
//! 使用 async-opcua 的 client 特性，支持匿名 / 用户名密码 / 客户端证书认证
//! （P0-3），以及 None / Basic256Sha256 安全策略。
//!
//! 配置：
//! - bus.tcp_addr: OPC UA Server 端点，如 "opc.tcp://192.168.1.100:4840"
//! - bus.opcua: 认证与安全策略（缺省 = 匿名 + None 明文，兼容存量配置）
//! - point.address: Node ID，如 "ns=2;s=Temperature" 或 "ns=2;i=1001"
//!
//! 模块结构：
//! - frames.rs: NodeId 解析 / DataValue 值提取等协议辅助函数
//! - tests.rs: 单元测试
//!
//! C-11 修复：长连接 keepalive + 半开连接检测
//!
//! 旧实现：
//! 1. `is_connected` 直接读 `AtomicBool`，但 `read` 失败时不会更新它
//! 2. event_loop_handle 在 disconnect 时直接 abort，**不 await 返回的 StatusCode**
//!    ——server 单方面关闭时 event loop 正常完成（StatusCode=Bad）但我们丢失信号
//! 3. 没有主动健康探测，server 半开（TCP 通但 OPC UA 层死）永远不被发现
//!
//! 修复策略（async-opcua 0.19 库自身不发 keepalive，靠 Subscription 探测
//! ServerStatus 实现 OPC UA 标准 keepalive 协议；本实现选择更轻的"read 失败
//! 累计"机制）：
//! 0. `read` / `read_batch` 入口惰性建链（生产路径不调 `Collector::connect`，
//!    同 mc 模块背景）——既是首连入口，也是失败计数判定断连后的重建入口
//! 1. `read` / `read_batch` 失败时累加 `consecutive_read_failures` 计数
//! 2. 计数达到阈值（READ_FAILURE_THRESHOLD = 3）→ 标记 `connected=false` +
//!    清空 session（让外层 supervisor 触发重连）
//! 3. `read` 成功时计数清零
//!
//! 为什么不监听 event_loop_handle 完成？
//! JoinHandle 不可 Arc 共享（await 需要 &mut），move 进 watchdog 后
//! `self.event_loop_handle` 为 None，disconnect 时 abort 失效。
//! 工程上更稳的做法：让 read 失败计数驱动状态切换——read 本身就是
//! 与 server 真实通信的探测器；event loop 完成导致的下一次 read 失败
//! 会被同一计数器捕获。
//!
//! C-14 修复：会话 keepalive（OPC UA 标准会话保活）
//!
//! 旧实现（C-11）只有"read 失败累计"这种**被动**探测：只要外层调度没有触发
//! read（例如点位表为空、采集周期很长、或本总线暂时没有采集任务），server 端
//! 的会话计时器到期后就会**单方面关闭 Session**，而本地 `connected` 仍为 true，
//! 直到下一次 read 才暴露——这期间的数据全部丢失，最长可达一个采集周期。
//!
//! 修复策略（镜像 IEC 104 C-12）：
//! 1. `connect` 成功后启动独立 keepalive supervisor（spawn_catch_unwind 包装）
//! 2. supervisor 每 KEEPALIVE_INTERVAL 主动 Read 标准节点 ServerStatus
//!    （ns=0;i=2256）——一次轻量往返即重置 server 端会话计时器，实现 OPC UA
//!    标准的"会话保活"
//! 3. 与 read 共享同一把 session Mutex，用 `try_lock` 抢锁（抢不到说明 read
//!    正在使用 session、链路活跃，跳过本周期），避免阻塞采集路径
//! 4. 单次探测加超时上限；连续失败达 KEEPALIVE_MAX_CONSECUTIVE_FAILURES
//!    （容忍网络抖动）→ 清空 session + 标记 `connected=false`
//! 5. `read` / `read_batch` 入口惰性建链（C-11 已具备）：判定断连后的下一次
//!    调用自动重建 session 并重启 supervisor
//! 6. `disconnect` 先停 supervisor 再拆 session

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use opcua::client::{ClientBuilder, IdentityToken, Password, Session};
use opcua::crypto::SecurityPolicy;
use opcua::types::{
    AttributeId, EndpointDescription, MessageSecurityMode, NodeId, NumericRange, QualifiedName,
    ReadValueId, TimestampsToReturn, UAString, UserTokenPolicy, UserTokenType,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::{OpcUaClientConfig, PointConfig};
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::south::protocol::{Collector, CollectorError, CollectorResult};

mod frames;
use frames::{extract_value, parse_node_id};

#[cfg(test)]
mod tests;

/// C-11: 连续 read 失败阈值（达到后标记 connected=false 让外层重连）
pub(crate) const READ_FAILURE_THRESHOLD: u32 = 3;

/// C-14: 向 server 请求的 OPC UA Session 超时（毫秒）。
/// server 端在此时间内未收到本端任何请求即单方面关闭会话——
/// keepalive 周期必须显著小于该值。
pub(crate) const SESSION_TIMEOUT_MS: u32 = 10_000;

/// C-14: keepalive 探测周期（必须 < SESSION_TIMEOUT_MS，取 5s 留 2× 余量）
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// C-14: 单次 keepalive 探测响应等待上限（小于探测周期）
pub(crate) const KEEPALIVE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);

/// C-14: 连续探测失败容忍次数（避免网络抖动导致连接抖动）
pub(crate) const KEEPALIVE_MAX_CONSECUTIVE_FAILURES: u32 = 2;

/// C-14: keepalive 探测节点——OPC UA 标准 ServerStatus（ns=0;i=2256）
pub(crate) const SERVER_STATUS_NODE_NUMERIC_ID: u32 = 2256;

/// OPC UA Client 采集器
pub struct OpcUaCollector {
    endpoint: String,
    /// P0-3: 认证/安全配置；`None` = 历史匿名 + None 明文行为
    auth: Option<OpcUaClientConfig>,
    /// C-14: session 用 Arc<Mutex<...>> 包装，让 keepalive supervisor 与 read 共享所有权
    session: Arc<Mutex<Option<Arc<Session>>>>,
    event_loop_handle: Mutex<Option<JoinHandle<opcua::types::StatusCode>>>,
    /// C-14: keepalive supervisor task 句柄
    keepalive_handle: Mutex<Option<JoinHandle<()>>>,
    bus_name: String,
    /// C-14: 用 Arc<AtomicBool> 让 supervisor 能独立观测 / 改写连接状态
    connected: Arc<AtomicBool>,
    /// C-11: 连续 read 失败计数（达到阈值 → 标记断连）
    consecutive_read_failures: AtomicU32,
}

impl OpcUaCollector {
    /// 历史入口：匿名 + None 明文端点（存量配置 / 测试使用）
    pub fn new(endpoint: String, bus_name: String) -> Self {
        Self::with_auth(endpoint, bus_name, None)
    }

    /// P0-3: 带认证配置的构造入口（opcua 采集器工厂使用）
    pub fn with_auth(endpoint: String, bus_name: String, auth: Option<OpcUaClientConfig>) -> Self {
        Self {
            endpoint,
            auth,
            session: Arc::new(Mutex::new(None)),
            event_loop_handle: Mutex::new(None),
            keepalive_handle: Mutex::new(None),
            bus_name,
            connected: Arc::new(AtomicBool::new(false)),
            consecutive_read_failures: AtomicU32::new(0),
        }
    }

    /// C-11: 记录一次 read 成功（清零失败计数）
    fn record_read_ok(&self) {
        self.consecutive_read_failures.store(0, Ordering::Relaxed);
    }

    /// C-14: 拆除当前 session 并标记断连（read 失败累计 / keepalive 探测失败共用）
    ///
    /// 从 `guard` 取出 `Arc<Session>` 后 fire-and-forget 执行 `disconnect`
    /// （不 await——外层会重新 connect，断开是尽力而为）；`connected` 置 false，
    /// 让下一次 `read` / `read_batch` 惰性建链时重建 session。
    fn teardown_session(
        guard: &mut Option<Arc<Session>>,
        connected: &AtomicBool,
        bus_name: &str,
        reason: &str,
    ) {
        if let Some(session) = guard.take() {
            tokio::spawn(async move {
                let _ = session.disconnect().await;
            });
        }
        connected.store(false, Ordering::Relaxed);
        tracing::warn!(
            "opcua bus {}: session torn down ({}), marked disconnected",
            bus_name,
            reason
        );
    }

    /// C-11: 记录一次 read 失败。达到阈值时清空 session + 标记 disconnected
    /// 让外层 scheduler 触发重连。
    async fn record_read_failure(&self) {
        let n = self
            .consecutive_read_failures
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        if n >= READ_FAILURE_THRESHOLD {
            // Bug C3: 本函数由 read()/read_batch() 在「持有 session 锁」期间调用，
            // 若在此直接 lock().await 会因 tokio Mutex 非可重入而自锁死锁
            // （第 3 次连续失败时协程永久挂起、session 永不重建、永不重连）。
            // 改为后台任务 teardown：等调用方释放锁后再 take session（语义不变）。
            let session_slot = self.session.clone();
            let connected = self.connected.clone();
            let bus_name = self.bus_name.clone();
            tokio::spawn(async move {
                let mut session_guard = session_slot.lock().await;
                Self::teardown_session(
                    &mut session_guard,
                    &connected,
                    &bus_name,
                    "consecutive read failures",
                );
            });
        }
    }

    /// C-14: 启动 keepalive supervisor task
    ///
    /// 独立 task 周期性地对 ServerStatus 节点做一次轻量 Read，重置 server 端
    /// 会话计时器（OPC UA 标准会话保活）；与 read 路径共享 `session` 的
    /// Arc<Mutex>（用 try_lock 避免阻塞 read）。连续探测失败 / 超时达阈值 →
    /// 判定会话半开，清空 session 并标记断连。
    fn spawn_keepalive_supervisor(&self) {
        // 丢弃可能存在的旧 supervisor，避免任务泄漏（重连时会被覆盖写入）
        if let Ok(mut old) = self.keepalive_handle.try_lock() {
            if let Some(handle) = old.take() {
                handle.abort();
            }
        }

        let session_slot = self.session.clone();
        let connected = self.connected.clone();
        let bus_name = self.bus_name.clone();

        let handle = crate::engine::supervision::spawn_catch_unwind(
            format!("opcua_keepalive[{}]", self.bus_name),
            async move {
                // 连续失败计数：网络抖动容忍，避免连接抖动
                let mut consecutive_failures: u32 = 0;
                loop {
                    tokio::time::sleep(KEEPALIVE_INTERVAL).await;

                    // try_lock 抢锁：抢不到说明 read / read_batch 正在使用 session，
                    // 链路活跃，本周期跳过（下一周期再尝试）
                    let mut guard = match session_slot.try_lock() {
                        Ok(g) => g,
                        Err(_) => {
                            tracing::trace!(
                                "opcua keepalive: session busy, skip cycle ({})",
                                bus_name
                            );
                            continue;
                        }
                    };

                    let session = match guard.as_ref() {
                        Some(s) => s.clone(),
                        None => {
                            // session 已被清空（disconnect / 断连判定）→ supervisor 退出
                            tracing::debug!(
                                "opcua keepalive: session gone, exiting ({})",
                                bus_name
                            );
                            return;
                        }
                    };

                    // C-14: 轻量 keepalive 探测——读标准节点 ServerStatus
                    // （ns=0;i=2256），触发一次完整 Session + SecureChannel 往返，
                    // 让 server 端会话计时器重置，避免 session_timeout 到期被
                    // server 单方面关闭
                    let probe = tokio::time::timeout(
                        KEEPALIVE_RESPONSE_TIMEOUT,
                        session.read(
                            &[keepalive_read_value_id()],
                            TimestampsToReturn::Neither,
                            0.0,
                        ),
                    )
                    .await;

                    let ok = match probe {
                        Ok(Ok(_)) => {
                            consecutive_failures = 0;
                            tracing::trace!("opcua keepalive: probe ok ({})", bus_name);
                            true
                        }
                        Ok(Err(e)) => {
                            consecutive_failures += 1;
                            tracing::warn!(
                                "opcua keepalive: probe failed ({}/{}): {} ({})",
                                consecutive_failures,
                                KEEPALIVE_MAX_CONSECUTIVE_FAILURES,
                                e,
                                bus_name
                            );
                            false
                        }
                        Err(_timeout) => {
                            consecutive_failures += 1;
                            tracing::warn!(
                                "opcua keepalive: probe timeout ({}/{}): {}",
                                consecutive_failures,
                                KEEPALIVE_MAX_CONSECUTIVE_FAILURES,
                                bus_name
                            );
                            false
                        }
                    };

                    if !ok && consecutive_failures >= KEEPALIVE_MAX_CONSECUTIVE_FAILURES {
                        // 连续多次失败：判定会话半开，清空 session + 标记断连，
                        // 下次 read 惰性建链时重建（supervisor 本轮退出）
                        Self::teardown_session(
                            &mut guard,
                            &connected,
                            &bus_name,
                            "keepalive probe failures",
                        );
                        return;
                    }

                    drop(guard);
                }
            },
        );

        if let Ok(mut h) = self.keepalive_handle.try_lock() {
            *h = Some(handle);
        } else {
            // 极端情况：调用方在持锁时调 spawn（实际不可能）——直接 abort 防泄漏
            handle.abort();
        }
    }
}

/// 修复(审查#B4)：热重载整体替换 `collectors` 时不会调用 `disconnect()`，
/// 若不在此收尾，keepalive supervisor 与 OPC UA event loop 会继续运行、session
/// 一直挂着——旧连接不释放且任务泄漏（每次热重载都堆积一份）。
/// Drop 中不能 await，故把收尾交给后台任务。
///
/// 修复(审查#B4-b)：**不能先 abort event loop 再交后台任务 disconnect**。
/// async-opcua 的请求只有 event loop 被轮询时才会经 transport 写出
/// （session/event_loop.rs：event loop drives the client），event loop 一旦 abort，
/// 后台 `session.disconnect()` 里的 CloseSession 永远到不了服务端，服务端会话
/// 只能等 session_timeout 到期才释放。
/// 故把 event loop 句柄一并交给后台任务：**先 await disconnect（此时 loop 仍存活、
/// CloseSession 可送达），完成后再 abort event loop**；无 session 时直接 abort
/// event loop 防泄漏。Drop 本身仍不阻塞。
impl Drop for OpcUaCollector {
    fn drop(&mut self) {
        // keepalive 只做探测、与通道收发无关，可立即 abort
        if let Ok(mut h) = self.keepalive_handle.try_lock() {
            if let Some(handle) = h.take() {
                handle.abort();
            }
        }
        self.connected.store(false, Ordering::Relaxed);

        // event loop 与 session 一并取出，交给后台任务「先断开、后停 loop」
        let event_loop_handle = self
            .event_loop_handle
            .try_lock()
            .ok()
            .and_then(|mut h| h.take());
        // 尽力断开：若 session 正被 read 占用（try_lock 失败）则跳过——此时
        // Arc<Session> 会随最后一个持有者释放而 drop，底层 TCP 连接随之关闭。
        let session = self
            .session
            .try_lock()
            .ok()
            .and_then(|mut guard| guard.take());

        match (session, event_loop_handle) {
            (Some(session), el) => {
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        // 此刻 event loop 仍在运行，CloseSession 才能送达服务端
                        let _ = session.disconnect().await;
                        // 断开完成后再停 event loop（避免任务泄漏）
                        if let Some(handle) = el {
                            handle.abort();
                        }
                    });
                } else if let Some(handle) = el {
                    // 无运行时无法异步断开：至少停掉 event loop 防泄漏
                    handle.abort();
                }
            }
            (None, Some(handle)) => handle.abort(),
            (None, None) => {}
        }
    }
}

/// C-14: 构造 keepalive 探测的 ReadValueId（标准 ServerStatus 节点 ns=0;i=2256）
fn keepalive_read_value_id() -> ReadValueId {
    ReadValueId {
        node_id: NodeId::new(0u16, SERVER_STATUS_NODE_NUMERIC_ID),
        attribute_id: AttributeId::Value as u32,
        index_range: NumericRange::default(),
        data_encoding: QualifiedName::null(),
    }
}

// ===== P0-3: 认证 / 安全策略映射 =====

/// 安全策略：配置校验保证只可能是 `none` / `basic256sha256`，其余值按缺失处理。
fn security_policy_of(auth: Option<&OpcUaClientConfig>) -> SecurityPolicy {
    match auth
        .map(|a| a.security_policy.to_ascii_lowercase())
        .as_deref()
    {
        Some("basic256sha256") => SecurityPolicy::Basic256Sha256,
        _ => SecurityPolicy::None,
    }
}

fn security_mode_of(auth: Option<&OpcUaClientConfig>) -> MessageSecurityMode {
    match auth
        .map(|a| a.security_mode.to_ascii_lowercase())
        .as_deref()
    {
        Some("sign") => MessageSecurityMode::Sign,
        Some("sign_and_encrypt") => MessageSecurityMode::SignAndEncrypt,
        _ => MessageSecurityMode::None,
    }
}

/// 端点描述里的用户令牌策略：仅用于声明期望的认证方式；
/// 实际可用性由 Server 返回的端点列表（`user_identity_tokens`）决定。
fn user_token_policy_of(auth: Option<&OpcUaClientConfig>) -> UserTokenPolicy {
    match auth.map(|a| a.auth.to_ascii_lowercase()).as_deref() {
        Some("username") => UserTokenPolicy {
            policy_id: UAString::from("username"),
            token_type: UserTokenType::UserName,
            ..UserTokenPolicy::anonymous()
        },
        Some("certificate") => UserTokenPolicy {
            policy_id: UAString::from("certificate"),
            token_type: UserTokenType::Certificate,
            ..UserTokenPolicy::anonymous()
        },
        _ => UserTokenPolicy::anonymous(),
    }
}

/// 按配置构造 `IdentityToken`；配置缺失必填字段时返回可读错误（配置校验已兜底，
/// 这里防止运行时构造出空凭据导致 Server 拒绝且原因不明）。
fn build_identity_token(auth: Option<&OpcUaClientConfig>) -> CollectorResult<IdentityToken> {
    let Some(cfg) = auth else {
        return Ok(IdentityToken::Anonymous);
    };
    match cfg.auth.to_ascii_lowercase().as_str() {
        "username" => {
            let username = cfg.username.as_deref().unwrap_or("").trim();
            let password = cfg.password.as_deref().unwrap_or("").trim();
            if username.is_empty() || password.is_empty() {
                return Err(CollectorError::IoError(
                    "OPC UA username auth requires both username and password".into(),
                ));
            }
            Ok(IdentityToken::UserName(
                username.to_string(),
                Password::new(password.to_string()),
            ))
        }
        "certificate" => {
            let cert = cfg.client_cert_path.as_deref().unwrap_or("").trim();
            let key = cfg.client_key_path.as_deref().unwrap_or("").trim();
            if cert.is_empty() || key.is_empty() {
                return Err(CollectorError::IoError(
                    "OPC UA certificate auth requires client_cert_path and client_key_path".into(),
                ));
            }
            IdentityToken::new_x509_path(cert, key).map_err(|e| {
                CollectorError::IoError(format!(
                    "OPC UA load client certificate failed (cert={}, key={}): {}",
                    cert, key, e
                ))
            })
        }
        _ => Ok(IdentityToken::Anonymous),
    }
}

impl Collector for OpcUaCollector {
    fn name(&self) -> &'static str {
        "opcua"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn connect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut session_guard = self.session.lock().await;
            if session_guard.is_some() {
                return Ok(());
            }

            let auth = self.auth.as_ref();
            let mut builder = ClientBuilder::new()
                .application_name("Gateway OPC UA Client")
                .application_uri("urn:gateway:opcua-client")
                .session_retry_limit(3)
                .session_timeout(SESSION_TIMEOUT_MS);
            // P0-3: 证书与信任策略。缺省配置保持历史行为（自动信任 + 示例密钥对），
            // 避免存量匿名连接因缺证书直接断链。
            match auth {
                Some(cfg) => {
                    builder = builder
                        .trust_server_certs(cfg.trust_server_certs)
                        .create_sample_keypair(true);
                    if let (Some(cert), Some(key)) = (&cfg.client_cert_path, &cfg.client_key_path) {
                        let (cert, key) = (cert.trim(), key.trim());
                        if !cert.is_empty() && !key.is_empty() {
                            builder = builder
                                .certificate_path(cert.to_string())
                                .private_key_path(key.to_string());
                        }
                    }
                }
                None => {
                    builder = builder.trust_server_certs(true).create_sample_keypair(true);
                }
            }
            let mut client = builder.client().map_err(|e: Vec<String>| {
                CollectorError::IoError(format!("OPC UA client build failed: {}", e.join(", ")))
            })?;

            let endpoint: EndpointDescription = (
                self.endpoint.as_str(),
                security_policy_of(auth).to_str(),
                security_mode_of(auth),
                user_token_policy_of(auth),
            )
                .into();

            let identity_token = build_identity_token(auth)?;

            let (session, event_loop) = client
                .connect_to_matching_endpoint(endpoint, identity_token)
                .await
                .map_err(|e| CollectorError::IoError(format!("OPC UA connect failed: {}", e)))?;

            let handle = event_loop.spawn();

            if !session.wait_for_connection().await {
                return Err(CollectorError::IoError(
                    "OPC UA session connection failed".into(),
                ));
            }

            *self.event_loop_handle.lock().await = Some(handle);
            self.connected.store(true, Ordering::Relaxed);
            // C-11: 重置失败计数（新连接）
            self.consecutive_read_failures.store(0, Ordering::Relaxed);
            *session_guard = Some(session);
            drop(session_guard); // C-14: 先释放 session 锁再 spawn supervisor

            // C-14: 连接成功后启动 keepalive supervisor（会话保活）
            self.spawn_keepalive_supervisor();

            Ok(())
        })
    }

    fn disconnect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // C-14: 先停 keepalive supervisor（让 supervisor 退出循环），再拆 session
            if let Ok(mut h) = self.keepalive_handle.try_lock() {
                if let Some(handle) = h.take() {
                    handle.abort();
                }
            }
            let mut session_guard = self.session.lock().await;
            if let Some(session) = session_guard.take() {
                let _ = session.disconnect().await;
            }
            let mut handle_guard = self.event_loop_handle.lock().await;
            if let Some(h) = handle_guard.take() {
                h.abort();
            }
            self.connected.store(false, Ordering::Relaxed);
        })
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// B3：建链 + 一次真实交互（读标准节点 ServerStatus）。
    ///
    /// 诊断返回文本而不是错误：现场需要看到「连上了但 ServerStatus 非 Good」与
    /// 「根本没连上」的区别；无数据如实报，不伪造成功。
    fn diag<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            if let Err(e) = self.connect().await {
                return format!("OPC UA 端点 {} 建链失败：{e}", self.endpoint);
            }
            let guard = self.session.lock().await;
            let Some(session) = guard.as_ref() else {
                return format!(
                    "OPC UA 端点 {} 建链后 session 为空（未建立会话）",
                    self.endpoint
                );
            };
            let probe = tokio::time::timeout(
                KEEPALIVE_RESPONSE_TIMEOUT,
                session.read(
                    &[keepalive_read_value_id()],
                    TimestampsToReturn::Neither,
                    0.0,
                ),
            )
            .await;
            match probe {
                Err(_) => format!(
                    "OPC UA 端点 {} 建链成功，但 ServerStatus 读取超时（>{:?}）",
                    self.endpoint, KEEPALIVE_RESPONSE_TIMEOUT
                ),
                Ok(Err(e)) => format!(
                    "OPC UA 端点 {} 建链成功，但 ServerStatus 读取失败：{e}",
                    self.endpoint
                ),
                Ok(Ok(values)) => match values.first().and_then(|dv| dv.status) {
                    Some(code) if code.is_good() => format!(
                        "OPC UA 端点 {} 建链成功，ServerStatus 读取正常（StatusCode=Good）",
                        self.endpoint
                    ),
                    Some(code) => format!(
                        "OPC UA 端点 {} 建链成功，但 ServerStatus 返回 StatusCode={code}（非 Good）",
                        self.endpoint
                    ),
                    None => format!(
                        "OPC UA 端点 {} 建链成功，但 ServerStatus 无状态码（空响应）",
                        self.endpoint
                    ),
                },
            }
        })
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>>
    {
        Box::pin(async move {
            // C-11: 生产路径不调 `Collector::connect`（同 mc 模块背景），
            // read 入口惰性建链：既是首连入口，也是判定断连后的重建入口
            if !self.is_connected() {
                self.connect().await?;
            }

            let session_guard = self.session.lock().await;
            let session = match session_guard.as_ref() {
                Some(s) => s,
                None => {
                    // C-11: 记录失败 + 标记断连（让外层重连）
                    self.record_read_failure().await;
                    return Err(CollectorError::IoError(
                        "OPC UA session not connected".into(),
                    ));
                }
            };

            let node_id = parse_node_id(&point.address)?;
            let read_value_id = ReadValueId {
                node_id: node_id.clone(),
                attribute_id: AttributeId::Value as u32,
                index_range: NumericRange::default(),
                data_encoding: QualifiedName::null(),
            };

            // C-11: 失败计入；session 已经 take 时直接返回错误（不调 read）
            let read_result = session
                .read(&[read_value_id], TimestampsToReturn::Both, 0.0)
                .await;
            let results = match read_result {
                Ok(v) => v,
                Err(e) => {
                    self.record_read_failure().await;
                    return Err(CollectorError::IoError(format!(
                        "OPC UA read failed: {}",
                        e
                    )));
                }
            };

            if results.is_empty() {
                self.record_read_failure().await;
                return Err(CollectorError::InvalidFrame(
                    "OPC UA read returned no results".into(),
                ));
            }

            let dv = &results[0];
            let value = match extract_value(dv) {
                Ok(v) => v,
                Err(e) => {
                    self.record_read_failure().await;
                    return Err(e);
                }
            };

            // C-11: 成功路径清零失败计数
            self.record_read_ok();

            let quality = match &dv.status {
                Some(code) if code.is_good() => Quality::Good,
                Some(code) if code.is_uncertain() => Quality::Uncertain,
                _ => Quality::Bad,
            };

            let source_ts = dv
                .source_timestamp
                .as_ref()
                .map(|t| t.as_chrono().timestamp_millis())
                .unwrap_or(0);

            Ok(DataPoint {
                point_id: point.id.clone(),
                device_id: point.device_id.clone().unwrap_or_default(),
                value,
                raw_value: value,
                raw_type: "opcua".into(),
                quality,
                quality_reason: String::new(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                source_ts,
                stale_count: 0,
                value_text: None,
            })
        })
    }

    /// 批量读取：一次 OPC UA Read 请求读取所有节点，避免 N 次网络往返
    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut results: Vec<CollectorResult<DataPoint>> = Vec::with_capacity(points.len());
            if points.is_empty() {
                return results;
            }

            // C-11: 惰性建链（同 read 入口说明）。read_batch 返回 Vec 而非 Result，
            // 建链失败时逐个点位返回同因错误。
            if !self.is_connected() {
                if let Err(e) = self.connect().await {
                    let msg = e.to_string();
                    return (0..points.len())
                        .map(|_| Err(CollectorError::IoError(msg.clone())))
                        .collect();
                }
            }

            let session_guard = self.session.lock().await;
            let session = match session_guard.as_ref() {
                Some(s) => s,
                None => {
                    // C-11: 整批失败计数 +1（而不是 N 次）
                    self.record_read_failure().await;
                    return (0..points.len())
                        .map(|_| {
                            Err(CollectorError::IoError(
                                "OPC UA session not connected".into(),
                            ))
                        })
                        .collect();
                }
            };

            let mut read_value_ids = Vec::with_capacity(points.len());
            let mut parse_ok = vec![true; points.len()];
            let mut parse_errors: Vec<String> = Vec::with_capacity(points.len());
            for (i, p) in points.iter().enumerate() {
                match parse_node_id(&p.address) {
                    Ok(nid) => {
                        read_value_ids.push(ReadValueId {
                            node_id: nid,
                            attribute_id: AttributeId::Value as u32,
                            index_range: NumericRange::default(),
                            data_encoding: QualifiedName::null(),
                        });
                    }
                    Err(e) => {
                        parse_ok[i] = false;
                        parse_errors.push(e.to_string());
                        read_value_ids.push(ReadValueId {
                            node_id: NodeId::null(),
                            attribute_id: AttributeId::Value as u32,
                            index_range: NumericRange::default(),
                            data_encoding: QualifiedName::null(),
                        });
                    }
                }
            }

            let data_values = match session
                .read(&read_value_ids, TimestampsToReturn::Both, 0.0)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    let msg = format!("OPC UA batch read failed: {}", e);
                    // C-11: 整批失败一次计数
                    self.record_read_failure().await;
                    return (0..points.len())
                        .map(|_| Err(CollectorError::IoError(msg.clone())))
                        .collect();
                }
            };

            // C-11: 整批 read 成功（server 端响应了）→ 清零失败计数
            self.record_read_ok();

            let now = chrono::Utc::now().timestamp_millis();
            let mut err_idx = 0;
            for (i, p) in points.iter().enumerate() {
                if !parse_ok[i] {
                    let msg = parse_errors
                        .get(err_idx)
                        .cloned()
                        .unwrap_or_else(|| "invalid node id".into());
                    err_idx += 1;
                    results.push(Err(CollectorError::InvalidFrame(msg)));
                    continue;
                }

                if i >= data_values.len() {
                    results.push(Err(CollectorError::InvalidFrame(
                        "OPC UA batch read returned fewer results than expected".into(),
                    )));
                    continue;
                }

                let dv = &data_values[i];
                let value = match extract_value(dv) {
                    Ok(v) => v,
                    Err(e) => {
                        results.push(Err(e));
                        continue;
                    }
                };

                let quality = match &dv.status {
                    Some(code) if code.is_good() => Quality::Good,
                    Some(code) if code.is_uncertain() => Quality::Uncertain,
                    _ => Quality::Bad,
                };

                let source_ts = dv
                    .source_timestamp
                    .as_ref()
                    .map(|t| t.as_chrono().timestamp_millis())
                    .unwrap_or(0);

                results.push(Ok(DataPoint {
                    point_id: p.id.clone(),
                    device_id: p.device_id.clone().unwrap_or_default(),
                    value,
                    raw_value: value,
                    raw_type: "opcua".into(),
                    quality,
                    quality_reason: String::new(),
                    timestamp: now,
                    source_ts,
                    stale_count: 0,
                    value_text: None,
                }));
            }

            results
        })
    }
}
