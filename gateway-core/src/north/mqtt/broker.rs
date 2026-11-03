//! MQTT broker URL 解析

use crate::engine::error::{GatewayError, GatewayResult};

/// broker URL 的协议方案
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerScheme {
    /// tcp:// 或 mqtt:// 或无前缀（默认 port 1883）
    Tcp,
    /// tls:// 或 mqtts://（默认 port 8883，加密）
    Tls,
    /// ws://（默认 port 9001，明文 WebSocket）
    Ws,
    /// wss://（默认 port 443，WebSocket over TLS）
    Wss,
}

/// 解析 broker 地址
pub(crate) fn parse_broker(broker: &str) -> GatewayResult<(String, u16, BrokerScheme)> {
    let (default_port, scheme, s) = if let Some(rest) = broker.strip_prefix("tls://") {
        (8883, BrokerScheme::Tls, rest)
    } else if let Some(rest) = broker.strip_prefix("mqtts://") {
        (8883, BrokerScheme::Tls, rest)
    } else if let Some(rest) = broker.strip_prefix("wss://") {
        (443, BrokerScheme::Wss, rest)
    } else if let Some(rest) = broker.strip_prefix("ws://") {
        (9001, BrokerScheme::Ws, rest)
    } else if let Some(rest) = broker.strip_prefix("tcp://") {
        (1883, BrokerScheme::Tcp, rest)
    } else if let Some(rest) = broker.strip_prefix("mqtt://") {
        (1883, BrokerScheme::Tcp, rest)
    } else {
        (1883, BrokerScheme::Tcp, broker)
    };

    let parts: Vec<&str> = s.splitn(2, ':').collect();
    let (host, port) = match parts.len() {
        1 => (parts[0].to_string(), default_port),
        2 => {
            let port: u16 = parts[1].parse().map_err(|e| {
                GatewayError::Config(format!("invalid broker port '{}': {}", parts[1], e))
            })?;
            (parts[0].to_string(), port)
        }
        _ => {
            return Err(GatewayError::Config(format!(
                "invalid broker address: {}",
                broker
            )))
        }
    };
    if host.is_empty() {
        return Err(GatewayError::Config(format!(
            "empty broker host: {}",
            broker
        )));
    }
    Ok((host, port, scheme))
}

#[cfg(test)]
mod tests;
