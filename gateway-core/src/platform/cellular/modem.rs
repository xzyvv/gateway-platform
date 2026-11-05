//! 串口通信/拨号逻辑

use std::process::Command;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio_serial::{DataBits, FlowControl, Parity, SerialPortBuilderExt, StopBits};

use super::at_commands::{parse_csq, parse_network_type, parse_operator};
use super::{now_millis, CellularManager, CellularState, LinkState};
use crate::engine::error::{GatewayError, GatewayResult};

/// r35-MDM2: 串口打开超时上限。设备忙或被其他进程占用时，`open_native_async`
/// 可能长时间阻塞，此处设上限避免拨号流程整体挂死。
const AT_PORT_OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// r35-MDM4/5: AT 命令引号参数净化。
/// `AT+CPIN="<pin>"` / `AT+CGDCONT=...,"<apn>"` 中的引号参数若含 `"`、CR/LF
/// 等字符可提前闭合引号并注入额外 AT 命令（AT 命令注入）。采用字符集白名单
/// （字母 / 数字 / `.` / `-` / `_`），可覆盖全部合法 PIN 与 APN 取值，其余一律剔除。
fn sanitize_at_arg(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect()
}

impl CellularManager {
    pub(super) async fn dial_loop(&self) {
        loop {
            let state = self.status.lock().state;
            match state {
                CellularState::Connected => {
                    tokio::time::sleep(Duration::from_secs(self.config.heartbeat_secs)).await;
                }
                CellularState::DialFailed | CellularState::Init | CellularState::NotReady => {
                    if let Err(e) = self.dial().await {
                        tracing::error!("cellular dial failed: {}", e);
                        self.set_state(CellularState::DialFailed);
                        // 退避参数化：避免硬编码导致不同协议行为不一致（retry_backoff_secs）
                        // r35-MDM3: 叠加 ±25% 抖动，避免多设备/多链路同时掉线后退避窗口
                        // 对齐，触发"惊群"式集中重拨（复用 supervision 的抖动实现）。
                        let backoff = crate::engine::supervision::jittered_backoff_secs(
                            self.config.retry_backoff_secs.max(1),
                        )
                        .max(1);
                        tokio::time::sleep(Duration::from_secs(backoff)).await;
                    }
                }
                _ => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn dial(&self) -> GatewayResult<()> {
        self.set_state(CellularState::Dialing);

        // 分阶段超时：各细则以 dial_timeout_secs 为总上限做 clamp（见 clamp_timeout）
        let at_to = self.clamp_timeout(self.config.at_cmd_timeout_secs);

        self.at_command("AT", at_to)
            .await
            .map_err(|e| GatewayError::Io(format!("AT no response: {}", e)))?;
        tracing::info!("cellular: AT OK");

        // ATE0（关闭回显）失败不阻断流程（部分模块不支持），
        // 但必须留痕：后续 AT 响应解析依赖无回显模式。
        if let Err(e) = self.at_command("ATE0", at_to).await {
            tracing::warn!("cellular: ATE0 failed: {}", e);
        }

        let sim_resp = self.at_command("AT+CPIN?", at_to).await.unwrap_or_default();
        if sim_resp.contains("SIM PIN") {
            if let Some(pin) = &self.config.pin {
                // r35-MDM4: PIN 净化，防引号闭合导致的 AT 命令注入
                if let Err(e) = self
                    .at_command(&format!("AT+CPIN=\"{}\"", sanitize_at_arg(pin)), at_to)
                    .await
                {
                    // PIN 写失败后续拨号必然失败，这里留痕便于区分
                    // 「PIN 错」与「模块无响应」。
                    tracing::warn!("cellular: set SIM PIN failed: {}", e);
                }
            } else {
                self.set_state(CellularState::SimNotReady);
                return Err(GatewayError::Io("SIM requires PIN".into()));
            }
        } else if sim_resp.contains("NOT INSERTED") {
            self.set_state(CellularState::SimNotReady);
            return Err(GatewayError::Io("SIM not inserted".into()));
        }
        tracing::info!("cellular: SIM ready");

        // 网络注册：在 register_timeout_secs 内轮询 AT+COPS?（每 2s 一次）
        let register_deadline =
            tokio::time::Instant::now() + self.clamp_timeout(self.config.register_timeout_secs);
        let mut registered = false;
        loop {
            let cops = self.at_command("AT+COPS?", at_to).await.unwrap_or_default();
            if cops.contains(',') {
                let operator = parse_operator(&cops);
                let mut status = self.status.lock();
                status.operator = operator;
                tracing::info!("cellular: operator = {}", status.operator);
                registered = true;
                break;
            }
            if tokio::time::Instant::now() >= register_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        if !registered {
            self.set_state(CellularState::NotReady);
            return Err(GatewayError::Io("not registered to network".into()));
        }

        let csq = self.at_command("AT+CSQ", at_to).await.unwrap_or_default();
        let signal = parse_csq(&csq);
        {
            let mut status = self.status.lock();
            status.signal_strength = signal;
        }
        tracing::info!("cellular: signal = {}", signal);

        // APN 设置失败不阻断流程（部分模块已内置默认 APN），
        // 但必须留痕：APN 写错是「有信号、无 IP」的常见根因，
        // 静默吞掉会让现场无从排查。
        if let Err(e) = self
            .at_command(
                // r35-MDM5: APN 净化，防引号闭合导致的 AT 命令注入
                &format!(
                    "AT+CGDCONT=1,\"IP\",\"{}\"",
                    sanitize_at_arg(&self.config.apn)
                ),
                at_to,
            )
            .await
        {
            tracing::warn!("cellular: set APN ({}) failed: {}", self.config.apn, e);
        }
        let _ = self
            .at_command(
                "ATD*99#",
                self.clamp_timeout(self.config.connect_cmd_timeout_secs),
            )
            .await;

        // 启动 pppd，并在 ppp_bringup_timeout_secs 内等待 PPP 接口拿到 IP；
        // 期间若 pppd 子进程提前退出（chat/鉴权失败等）则立即失败，不再干等满超时。
        let child = self.start_pppd().await?;
        *self.pppd_child.lock() = Some(child);

        let bringup = self.clamp_timeout(self.config.ppp_bringup_timeout_secs);
        let outcome = wait_for_ppp_bringup(
            bringup,
            Duration::from_millis(500),
            || get_iface_ip(&self.config.ppp_iface),
            || {
                let mut guard = self.pppd_child.lock();
                match guard.as_mut() {
                    // try_wait 返回 Some 表示子进程已退出
                    Some(child) => !matches!(child.try_wait(), Ok(Some(_))),
                    None => false,
                }
            },
        )
        .await;

        match outcome {
            PppBringup::Up(ip) => {
                let mut status = self.status.lock();
                status.ip_address = Some(ip);
                status.state = CellularState::Connected;
            }
            PppBringup::ProcessExited => {
                self.stop_pppd().await;
                self.set_state(CellularState::DialFailed);
                return Err(GatewayError::Io("pppd exited before PPP link up".into()));
            }
            PppBringup::Timeout => {
                self.stop_pppd().await;
                self.set_state(CellularState::DialFailed);
                return Err(GatewayError::Io("PPP dial timeout, no IP".into()));
            }
        }

        let cgreg = self
            .at_command("AT+CGREG?", at_to)
            .await
            .unwrap_or_default();
        let network_type = parse_network_type(&cgreg);
        {
            let mut status = self.status.lock();
            status.network_type = network_type;
            status.updated_at = now_millis();
        }

        self.set_state(CellularState::Connected);
        tracing::info!(
            "cellular: connected, ip = {:?}",
            self.status.lock().ip_address
        );
        Ok(())
    }

    async fn at_command(&self, cmd: &str, timeout: Duration) -> GatewayResult<String> {
        // r35-MDM2: `open_native_async()` 是同步系统调用（无 .await），设备忙或驱动卡死时
        // 会阻塞调用它的 tokio worker 线程。改为在 spawn_blocking 中执行并加超时上限，
        // 避免单条 AT 命令把整个运行时线程拖死。
        let port_name = self.config.at_port.clone();
        let baud = self.config.baud_rate;
        let open_fut = tokio::task::spawn_blocking(move || {
            tokio_serial::new(&port_name, baud)
                .data_bits(DataBits::Eight)
                .parity(Parity::None)
                .stop_bits(StopBits::One)
                .flow_control(FlowControl::None)
                .open_native_async()
        });
        let port = match tokio::time::timeout(AT_PORT_OPEN_TIMEOUT, open_fut).await {
            Ok(Ok(Ok(port))) => port,
            Ok(Ok(Err(e))) => return Err(GatewayError::Io(format!("open AT port failed: {}", e))),
            Ok(Err(e)) => {
                return Err(GatewayError::Io(format!(
                    "open AT port task panicked: {}",
                    e
                )))
            }
            Err(_) => {
                return Err(GatewayError::Io(format!(
                    "open AT port timeout after {:?}",
                    AT_PORT_OPEN_TIMEOUT
                )))
            }
        };

        let (reader, mut writer) = tokio::io::split(port);
        let mut reader = BufReader::new(reader);

        let full_cmd = format!("{}\r\n", cmd);
        writer
            .write_all(full_cmd.as_bytes())
            .await
            .map_err(|e| GatewayError::Io(format!("AT send failed: {}", e)))?;
        writer.flush().await.ok();

        let mut response = String::new();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            let mut line = String::new();
            // Bug 36: tokio::time::timeout wraps read_line to prevent AT module hang
            match tokio::time::timeout(remaining, reader.read_line(&mut line)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.contains("OK") {
                        response.push_str(trimmed);
                        break;
                    }
                    if trimmed.contains("ERROR") {
                        response.push_str(trimmed);
                        break;
                    }
                    if !trimmed.is_empty() && trimmed != cmd {
                        response.push_str(trimmed);
                        response.push('\n');
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => break, // timeout elapsed
            }
        }
        Ok(response)
    }

    async fn start_pppd(&self) -> GatewayResult<tokio::process::Child> {
        let mut cmd = TokioCommand::new("pppd");
        cmd.arg(&self.config.data_port)
            .arg(format!("{}", self.config.baud_rate))
            .arg("noauth")
            .arg("defaultroute")
            .arg("usepeerdns")
            .arg("nodeflate")
            .arg("connect")
            .arg("chat -v '' AT OK ATDT*99# CONNECT");
        let child = cmd
            .spawn()
            .map_err(|e| GatewayError::Io(format!("spawn pppd failed: {}", e)))?;
        tracing::info!("cellular: pppd started (pid={:?})", child.id());
        Ok(child)
    }

    /// 异步停止 pppd：SIGTERM 优雅退出 → `ppp_stop_timeout_secs` 内未退出则 SIGKILL，
    /// 最后清理 PPP 接口。全程非阻塞（tokio::process + tokio::time），避免卡住 async worker。
    pub(super) async fn stop_pppd(&self) {
        let taken = { self.pppd_child.lock().take() };
        let mut child = match taken {
            Some(c) => c,
            None => return,
        };
        // SIGTERM 优雅退出（保留 #13/#14 语义：先给 pppd 清理机会再强杀）
        if let Some(pid) = child.id() {
            let _ = TokioCommand::new("kill")
                .args(["-15", &pid.to_string()])
                .status()
                .await;
        }
        let grace = self.clamp_timeout(self.config.ppp_stop_timeout_secs);
        let exited = tokio::time::timeout(grace, async {
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                    Err(_) => break true,
                }
            }
        })
        .await
        .unwrap_or(false);
        if exited {
            tracing::info!("cellular: pppd stopped gracefully");
        } else {
            let _ = child.start_kill(); // SIGKILL
            tracing::warn!(
                "cellular: pppd did not exit on SIGTERM within {:?}, SIGKILL'd",
                grace
            );
        }
        // #13/#14: 清理 ppp 接口（防止残留接口/路由影响下次拨号）
        self.cleanup_ppp_interface();
    }

    /// 同步兜底停止 pppd（仅供 `Drop` 使用，无法 await）：
    /// 直接 SIGKILL + 清理接口，不做优雅等待，保证 Drop 快速返回。
    pub(super) fn stop_pppd_force(&self) {
        let taken = { self.pppd_child.lock().take() };
        if let Some(mut child) = taken {
            let _ = child.start_kill(); // SIGKILL
            tracing::warn!("cellular: pppd force-killed (drop path)");
            self.cleanup_ppp_interface();
        }
    }

    /// 细则阶段超时的总上限：不超过 `dial_timeout_secs`（0 表示不限），且至少 1s。
    fn clamp_timeout(&self, secs: u64) -> Duration {
        let cap = self.config.dial_timeout_secs;
        let effective = if cap == 0 { secs } else { secs.min(cap) };
        Duration::from_secs(effective.max(1))
    }

    /// #13/#14: 清理 ppp 接口——下线 ppp0 + 删除相关默认路由。
    /// 即使 pppd 已退出，内核可能仍残留 ppp0 接口与 default route，会导致：
    /// - 下次拨号前 ip-up 脚本检测到接口已存在而跳过配置
    /// - 路由表残留指向已失效的 ppp0，影响主备切换
    fn cleanup_ppp_interface(&self) {
        let iface = &self.config.ppp_iface;
        // ip link set <iface> down（接口不存在时返回非 0）
        // 失败必须留痕：清理失败会导致内核残留 ppp0 接口与 default route，
        // 影响主备切换（见本函数文档）。此前错误被整个吞掉，只留 debug 日志。
        match Command::new("ip")
            .args(["link", "set", iface, "down"])
            .status()
        {
            Ok(st) if st.success() => {
                tracing::debug!("cellular: cleanup_ppp_interface: {} set down", iface);
            }
            Ok(st) => {
                tracing::warn!(
                    "cellular: cleanup_ppp_interface: 'ip link set {} down' exited with {} \
                     (interface may not exist; residual ppp0 route could break failover)",
                    iface,
                    st
                );
            }
            Err(e) => {
                tracing::warn!(
                    "cellular: cleanup_ppp_interface: 'ip link set {} down' failed to spawn: {} \
                     (residual ppp0 route could break failover)",
                    iface,
                    e
                );
            }
        }
    }

    pub(super) async fn monitor_loop(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(self.config.heartbeat_secs)).await;
            let state = self.status.lock().state;
            if state != CellularState::Connected {
                continue;
            }

            match get_iface_ip(&self.config.ppp_iface) {
                Some(ip) => {
                    let mut status = self.status.lock();
                    status.ip_address = Some(ip);
                    status.updated_at = now_millis();
                }
                None => {
                    tracing::warn!("cellular: ppp0 IP lost, triggering redial");
                    self.stop_pppd().await;
                    self.set_state(CellularState::DialFailed);
                }
            }

            if let Ok(csq) = self.at_command("AT+CSQ", Duration::from_secs(2)).await {
                let signal = parse_csq(&csq);
                let mut status = self.status.lock();
                status.signal_strength = signal;
            }
        }
    }

    pub(super) async fn failover_loop(&self) {
        // 已成功应用的默认路由目标（None = 尚未成功应用过任何默认路由）。
        //
        // 与 `link_state` **分开**记录：`link_state` 描述物理链路通断，
        // 而默认路由是否真的切过去取决于 `ip route replace` 是否成功。
        // 历史上两者混为一谈：切换失败被 `let _ =` 吞掉后，状态已记为
        // 「已切到 4G」且不再重试 → eth 已 down、路由仍指向 eth →
        // **设备实际断网，但状态显示一切正常**（静默断网）。
        let mut applied_route: Option<RouteTarget> = None;

        loop {
            tokio::time::sleep(Duration::from_secs(self.config.heartbeat_secs)).await;
            let eth_up = check_iface_up(&self.config.eth_iface);
            let cell_up = self.status.lock().state == CellularState::Connected
                && get_iface_ip(&self.config.ppp_iface).is_some();

            let new_state = match (eth_up, cell_up) {
                (true, true) => LinkState::BothUp,
                (true, false) => LinkState::EthernetUp,
                (false, true) => LinkState::CellularUp,
                (false, false) => LinkState::Down,
            };

            let old_state = *self.link_state.lock();
            if new_state != old_state {
                tracing::info!("link state changed: {:?} -> {:?}", old_state, new_state);
                *self.link_state.lock() = new_state;
                if new_state == LinkState::Down {
                    tracing::error!("all links down!");
                }
            }

            // 默认路由目标：与物理状态解耦，失败可重试。
            let desired = desired_default_route(new_state);
            if desired == applied_route {
                continue;
            }
            let iface = match desired {
                Some(RouteTarget::Ethernet) => self.config.eth_iface.clone(),
                Some(RouteTarget::Cellular) => self.config.ppp_iface.clone(),
                None => {
                    // 两路都断：不下任何路由（保留现状，等下一轮）。
                    continue;
                }
            };
            if desired == Some(RouteTarget::Cellular) {
                tracing::warn!("ethernet down, switching to 4G");
            }
            match set_default_route(&iface) {
                Ok(()) => {
                    applied_route = desired;
                    tracing::info!("default route now points to {}", iface);
                }
                Err(e) => {
                    // 失败必须显式告警：静默吞掉会让「eth 已 down + 路由未切换」
                    // 变成看不出来的现场断网。
                    tracing::error!(
                        "cellular: switch default route to {} FAILED: {} — \
                         traffic may still use the old link (silent outage risk)",
                        iface,
                        e
                    );
                    // 不更新 applied_route → 下一轮重试。
                }
            }
        }
    }
}

/// 默认路由的目标链路（纯逻辑，便于跨平台单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteTarget {
    Ethernet,
    Cellular,
}

/// 依据物理链路状态决定默认路由应指向哪条链路（纯函数，跨平台可测）。
///
/// - `EthernetUp` / `BothUp` → 以太网优先
/// - `CellularUp` → 4G
/// - `Down` → 无可用目标（不下路由）
fn desired_default_route(state: LinkState) -> Option<RouteTarget> {
    match state {
        LinkState::EthernetUp | LinkState::BothUp => Some(RouteTarget::Ethernet),
        LinkState::CellularUp => Some(RouteTarget::Cellular),
        LinkState::Down => None,
    }
}

/// PPP 建链等待结果（细粒度超时的判定输出）。
#[derive(Debug, PartialEq, Eq)]
enum PppBringup {
    /// 成功拿到 PPP 接口 IP。
    Up(String),
    /// pppd 子进程提前退出（chat/鉴权失败等），无需再等满超时。
    ProcessExited,
    /// 在超时内始终未拿到 IP。
    Timeout,
}

/// 在 `timeout` 内轮询 PPP 接口 IP，同时检测 pppd 子进程是否存活。
///
/// 抽为独立函数并注入闭包，便于在无真实 pppd/串口 的环境下对
/// “拿到 IP / 进程提前退出 / 超时”三条路径做单元测试。
async fn wait_for_ppp_bringup<F, A>(
    timeout: Duration,
    poll_interval: Duration,
    mut poll_ip: F,
    mut is_alive: A,
) -> PppBringup
where
    F: FnMut() -> Option<String>,
    A: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // 先探子进程存活：pppd 已退出则立即失败，不必等满超时
        if !is_alive() {
            return PppBringup::ProcessExited;
        }
        if let Some(ip) = poll_ip() {
            return PppBringup::Up(ip);
        }
        if tokio::time::Instant::now() >= deadline {
            return PppBringup::Timeout;
        }
        tokio::time::sleep(poll_interval).await;
    }
}

fn get_iface_ip(iface: &str) -> Option<String> {
    use std::net::IpAddr;
    let output = Command::new("ip")
        .args(["-4", "addr", "show", iface])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("inet ") {
            if let Some(start) = line.find("inet ") {
                let rest = &line[start + 5..];
                if let Some(end) = rest.find('/') {
                    let ip = rest[..end].trim();
                    if ip.parse::<IpAddr>().is_ok() {
                        return Some(ip.to_string());
                    }
                }
            }
        }
    }
    None
}

fn check_iface_up(iface: &str) -> bool {
    get_iface_ip(iface).is_some()
}

fn set_default_route(iface: &str) -> GatewayResult<()> {
    // Bug r09-MOD3 (fix): 改用 `ip route replace default dev <iface>` 原子替换语义，
    // 替代旧的「del + add」两步操作。
    //
    // 旧实现的隐患：双链路同时在线时（eth + ppp 都有 default route），
    // `ip route del default` 一次只删一条——可能删错要保留的那条，
    // 之后 `ip route add` 添加新路由成功，但内核路由表里仍残留旧路由
    // 指向已 down 的接口 → failover_loop 切换到 cellular 时实际仍走 ethernet
    // → 业务静默断网。
    //
    // `replace` 语义：若已存在 default route 则替换其出接口（不影响 metric），
    // 若不存在则添加。原子操作 + 解决"删错条"问题。
    let status = Command::new("ip")
        .args(["route", "replace", "default", "dev", iface])
        .status()
        .map_err(|e| GatewayError::Io(format!("set default route failed: {}", e)))?;
    if !status.success() {
        // 带上退出码：否则现场只知道「失败」，不知道是权限不足还是接口不存在。
        return Err(GatewayError::Io(format!(
            "set default route failed: 'ip route replace default dev {}' exited with {}",
            iface, status
        )));
    }
    tracing::info!("default route set to {} (replace)", iface);
    Ok(())
}

#[cfg(test)]
mod tests;
