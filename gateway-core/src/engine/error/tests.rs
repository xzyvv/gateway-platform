use super::*;

#[test]
fn code_for_each_variant() {
    assert_eq!(GatewayError::Config("x".into()).code(), 1000);
    assert_eq!(GatewayError::Io("x".into()).code(), 2000);
    assert_eq!(GatewayError::Protocol("x".into()).code(), 3000);
    assert_eq!(GatewayError::NoReply.code(), 0);
    assert_eq!(GatewayError::Queue("x".into()).code(), 4000);
    assert_eq!(GatewayError::Mqtt("x".into()).code(), 5000);
    assert_eq!(GatewayError::Formula("x".into()).code(), 7000);
    assert_eq!(GatewayError::Auth("x".into()).code(), 8000);
    assert_eq!(GatewayError::NotFound("x".into()).code(), 9000);
    assert_eq!(GatewayError::Message("x".into()).code(), 9999);
}

#[test]
fn code_classified_recovers_round_trip() {
    // round-trip：具体变体 → Display → String → From → Message → code_classified
    let cases: [(GatewayError, u32); 8] = [
        (GatewayError::Config("bad json".into()), 1000),
        (GatewayError::Io("eof".into()), 2000),
        (GatewayError::Protocol("bad crc".into()), 3000),
        (GatewayError::Queue("locked".into()), 4000),
        (GatewayError::Mqtt("timeout".into()), 5000),
        (GatewayError::Formula("div0".into()), 7000),
        (GatewayError::Auth("bad token".into()), 8000),
        (GatewayError::NotFound("device".into()), 9000),
    ];
    for (err, expected) in cases {
        let s = err.to_string();
        let roundtrip: GatewayError = s.into();
        assert_eq!(
            roundtrip.code_classified(),
            expected,
            "round-trip 分类失败: {roundtrip}"
        );
    }
}

#[test]
fn code_classified_unknown_message_stays_9999() {
    let err: GatewayError = "some random string".into();
    assert_eq!(err.code_classified(), 9999);
}

#[test]
fn code_classified_non_message_uses_code() {
    assert_eq!(GatewayError::NoReply.code_classified(), 0);
    assert_eq!(GatewayError::Config("x".into()).code_classified(), 1000);
}

#[test]
fn from_string_preserves_content() {
    let err: GatewayError = String::from("hello").into();
    match err {
        GatewayError::Message(s) => assert_eq!(s, "hello"),
        _ => panic!("From<String> 应产生 Message 变体"),
    }
}

#[test]
fn from_str_preserves_content() {
    let err: GatewayError = "world".into();
    match err {
        GatewayError::Message(s) => assert_eq!(s, "world"),
        _ => panic!("From<&str> 应产生 Message 变体"),
    }
}
