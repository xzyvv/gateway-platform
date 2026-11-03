use super::*;

/// SHA256 空字符串已知向量
#[test]
fn test_sha256_empty() {
    let h = sha256(b"");
    let hex: String = h.iter().map(|b| format!("{:02x}", b)).collect();
    assert_eq!(
        hex,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

/// SHA256 "abc" 已知向量
#[test]
fn test_sha256_abc() {
    let h = sha256(b"abc");
    let hex: String = h.iter().map(|b| format!("{:02x}", b)).collect();
    assert_eq!(
        hex,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
