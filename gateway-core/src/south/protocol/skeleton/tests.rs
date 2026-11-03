use super::*;

#[test]
fn build_request_returns_correct_bytes() {
    let point = PointConfig {
        id: "p1".into(),
        address: "1".into(),
        slave_id: Some(1),
        ..Default::default()
    };
    let req = build_request(&point);
    assert_eq!(req, vec![1, 0x03, 0, 1]);
}

#[test]
fn parse_response_returns_value() {
    let resp = vec![0x01, 0x03, 0x02, 0x00, 0x2A];
    assert_eq!(parse_response(&resp), Some(42.0));
}
