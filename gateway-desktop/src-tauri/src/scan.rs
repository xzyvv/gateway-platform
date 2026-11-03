//! LAN 网关扫描
//!
//! 职责：探测本机 /24 子网内 1-254 主机的指定端口，先 TCP 快筛
//! 再 HTTP /health/live 确认，返回可达网关列表（按延迟升序）。

use serde::Serialize;

/// 扫描到的网关信息
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredGateway {
    /// 网关 IP 地址
    pub ip: String,
    /// 端口
    pub port: u16,
    /// 网关名称（从 /api/system/info.hostname 取，取不到则用 IP）
    pub name: String,
    /// 探测延迟（毫秒）
    pub latency_ms: u64,
}

/// 获取本机局域网 IP（非回环）。
/// UDP socket 连接公网地址的技巧获取出口 IP，不实际发包。
fn get_local_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// 探测单个 IP:port 是否是网关：先 TCP 连接快筛，再 HTTP /health/live 确认。
async fn probe_gateway(
    ip: String,
    port: u16,
    client: &reqwest::Client,
) -> Option<DiscoveredGateway> {
    let start = std::time::Instant::now();

    // 第一阶段：TCP 端口探测（300ms 超时快速排除未开放的主机）
    let addr = format!("{}:{}", ip, port);
    let tcp = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        tokio::net::TcpStream::connect(&addr),
    )
    .await;
    if tcp.is_err() {
        return None; // 端口未开放或超时
    }

    // 第二阶段：HTTP 健康确认
    let base = format!("http://{}:{}", ip, port);
    let resp = client
        .get(format!("{}/health/live", base))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }

    // 尝试拿网关名称（失败就用 IP，不阻塞主流程）
    let name = match client.get(format!("{}/api/system/info", base)).send().await {
        Ok(r) if r.status().is_success() => r
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| {
                v.get("hostname")
                    .and_then(|h| h.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| ip.clone()),
        _ => ip.clone(),
    };

    Some(DiscoveredGateway {
        ip,
        port,
        name,
        latency_ms: start.elapsed().as_millis() as u64,
    })
}

/// 扫描局域网内的网关设备。
///
/// 流程：
/// 1. 获取本机局域网 IP → 推导 /24 子网
/// 2. 并行探测 1-254 主机的指定端口（默认 8080）
/// 3. TCP 通过后 HTTP /health/live 确认是网关
/// 4. 返回可达网关列表（按延迟升序）
///
/// 前端调用：invoke('scan_gateway_lan', { port: 8080 })
#[tauri::command]
pub async fn scan_gateway_lan(port: Option<u16>) -> Result<Vec<DiscoveredGateway>, String> {
    let port = port.unwrap_or(8080);

    let local_ip = get_local_ip().ok_or("无法获取本机 IP 地址")?;
    let octets = match local_ip {
        std::net::IpAddr::V4(v4) => v4.octets(),
        _ => return Err("不支持 IPv6 局域网扫描".to_string()),
    };
    if octets[0] == 127 {
        return Err("本机仅有回环地址，无法扫描局域网".to_string());
    }

    let subnet_prefix = format!("{}.{}.{}", octets[0], octets[1], octets[2]);
    tracing::info!("LAN 扫描开始: 子网 {}.*:{}", subnet_prefix, port);

    // 扫描专用 HTTP 客户端：短超时，不校验证书
    let probe_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(500))
        .timeout(std::time::Duration::from_secs(1))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| format!("创建探测客户端失败: {}", e))?;

    // 并行探测 1-254，JoinSet 管理并发任务
    let mut join_set = tokio::task::JoinSet::new();
    for host in 1..=254u8 {
        let ip = format!("{}.{}", subnet_prefix, host);
        let client = probe_client.clone(); // reqwest::Client 内部 Arc，clone 廉价
        join_set.spawn(async move { probe_gateway(ip, port, &client).await });
    }

    let mut found = Vec::new();
    while let Some(res) = join_set.join_next().await {
        if let Ok(Some(gw)) = res {
            found.push(gw);
        }
    }

    found.sort_by_key(|g| g.latency_ms);
    tracing::info!("LAN 扫描完成: 发现 {} 个网关", found.len());
    Ok(found)
}
