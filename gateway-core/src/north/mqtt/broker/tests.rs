use super::*;

#[test]
fn test_parse_broker_tcp() {
    let (host, port, scheme) = parse_broker("tcp://localhost:1883").unwrap();
    assert_eq!(host, "localhost");
    assert_eq!(port, 1883);
    assert_eq!(scheme, BrokerScheme::Tcp);
}

#[test]
fn test_parse_broker_no_scheme() {
    let (host, port, scheme) = parse_broker("localhost:1883").unwrap();
    assert_eq!(host, "localhost");
    assert_eq!(port, 1883);
    assert_eq!(scheme, BrokerScheme::Tcp);
}

#[test]
fn test_parse_broker_tls_default_port() {
    let (host, port, scheme) = parse_broker("tls://broker.example.com").unwrap();
    assert_eq!(host, "broker.example.com");
    assert_eq!(port, 8883);
    assert_eq!(scheme, BrokerScheme::Tls);
}

#[test]
fn test_parse_broker_mqtts() {
    let (host, port, scheme) = parse_broker("mqtts://broker.example.com:8884").unwrap();
    assert_eq!(host, "broker.example.com");
    assert_eq!(port, 8884);
    assert_eq!(scheme, BrokerScheme::Tls);
}

#[test]
fn test_parse_broker_ws() {
    let (host, port, scheme) = parse_broker("ws://broker:9001").unwrap();
    assert_eq!(host, "broker");
    assert_eq!(port, 9001);
    assert_eq!(scheme, BrokerScheme::Ws);
}

#[test]
fn test_parse_broker_wss_default_port() {
    let (host, port, scheme) = parse_broker("wss://broker.example.com").unwrap();
    assert_eq!(host, "broker.example.com");
    assert_eq!(port, 443);
    assert_eq!(scheme, BrokerScheme::Wss);
}

#[test]
fn test_parse_broker_empty_host() {
    assert!(parse_broker("tls://:8883").is_err());
}

#[test]
fn test_parse_broker_invalid_port() {
    assert!(parse_broker("tcp://localhost:abc").is_err());
}
