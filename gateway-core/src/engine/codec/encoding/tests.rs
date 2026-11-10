use super::*;

#[test]
fn test_b64_roundtrip() {
    for s in ["", "f", "fo", "foo", "Hello, World!", "工业网关"] {
        let enc = b64_encode(s.as_bytes());
        let dec = b64_decode(&enc).unwrap();
        assert_eq!(dec, s.as_bytes(), "roundtrip failed for {:?}", s);
    }
}

/// r23-ENC1：超过 `B64_MAX_INPUT_BYTES` 的输入必须返回 `None`，
/// 不应在内部分配放大输入尺寸的 Vec。
#[test]
fn test_b64_decode_rejects_oversized_input() {
    // 构造一个明显超过 16 KiB 上限的 base64 字符串（全 'A' 即 0x00，无非法字符）
    let huge = "A".repeat(B64_MAX_INPUT_BYTES + 1);
    assert!(b64_decode(&huge).is_none(), "超过长度上限的输入应返回 None");
}

/// r23-ENC1：刚好等于上限的输入仍可解码（边界不破坏合法 roundtrip）。
#[test]
fn test_b64_decode_accepts_input_at_limit() {
    // 编码一个 12 KiB 的随机 payload（编码后约 16 KiB，恰为上限）
    let payload = vec![0xAB_u8; 12 * 1024];
    let enc = b64_encode(&payload);
    // enc 长度 = ceil(12288 / 3) * 4 = 16384，恰为 B64_MAX_INPUT_BYTES
    assert_eq!(enc.len(), B64_MAX_INPUT_BYTES);
    let dec = b64_decode(&enc).expect("边界长度应可解码");
    assert_eq!(dec, payload);
}
