//! APDU / 值解码与 I-frame / S-frame 序号编码契约
use super::*;

#[test]
fn test_parse_float() {
    let bytes = 0.5f32.to_le_bytes();
    let v = parse_float(&bytes);
    assert!((v - 0.5).abs() < 1e-6);
}

#[test]
fn test_parse_normalized() {
    let bytes = 32767i16.to_le_bytes();
    let v = parse_normalized(&bytes);
    assert!((v - 1.0).abs() < 1e-4);
}

#[test]
fn test_build_uframe() {
    let f = build_uframe(STARTDT_ACT);
    assert_eq!(f, vec![0x68, 0x04, 0x07, 0x00, 0x00, 0x00]);
}

// ============================================================
// 数据缓存失效修复：I-frame N(S)/N(R) + S-frame 确认契约
// ============================================================

/// I-frame 控制域必须编码 N(S)（发送序号）与 N(R)（接收序号），低 2 位为 0
/// 旧实现把 N(S)/N(R) 硬编码为 0——重复总召唤会被 server 判为序号错乱
#[test]
fn seq_iframe_encodes_ns_and_nr() {
    let asdu = [0xDE, 0xAD, 0xBE]; // 任意 ASDU 载荷
    let f = build_iframe(1, 2, &asdu).expect("I-frame 构造失败");

    assert_eq!(f[0], 0x68, "起始字节");
    assert_eq!(
        f[1] as usize,
        asdu.len() + 4,
        "长度字段 = ASDU + 4 字节控制域"
    );
    // 低 2 位为 0 → I 格式（不是 U/S 帧）
    assert_eq!(f[2] & 0x01, 0x00, "I-frame bit0 必须为 0");
    assert_eq!(f[3] & 0x01, 0x00, "I-frame bit0 必须为 0");
    assert_eq!(decode_seq(f[2], f[3]), 1, "N(S) 必须还原为入参");
    assert_eq!(decode_seq(f[4], f[5]), 2, "N(R) 必须还原为入参");
    assert_eq!(&f[6..], &asdu, "ASDU 载荷原样附加");

    // 跨字节（值 > 127）时高位字节必须被正确填充
    let big = build_iframe(200, 1000, &asdu).expect("I-frame 构造失败");
    assert_eq!(decode_seq(big[2], big[3]), 200);
    assert_eq!(decode_seq(big[4], big[5]), 1000);
}

/// C-4 回归：超长 ASDU 必须显式报错，绝不能返回空帧冒充"发送成功"。
///
/// 旧实现 `return Vec::new()` → 调用方 `stream.write_all(&[])` 得到 `Ok(0)` 并
/// 判定发送成功，报文实际未上线（大点表总召唤 / 突发 SOE 场景静默丢数据）。
#[test]
fn iframe_rejects_over_long_asdu() {
    // 上限内（249）必须正常构造，长度域 = ASDU + 4 且不溢出 u8
    let ok_asdu = vec![0xABu8; 249];
    let f = build_iframe(0, 0, &ok_asdu).expect("249 字节 ASDU 必须可构造");
    assert_eq!(f[1] as usize, 249 + 4, "长度域 = ASDU + 4 字节控制域");

    // 超限必须返回 InvalidFrame（而不是空帧）
    let too_long = vec![0xABu8; 250];
    match build_iframe(0, 0, &too_long) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("too long"), "错误信息应说明原因，实际: {msg}");
        }
        other => panic!("超长 ASDU 必须返回 InvalidFrame，实际: {other:?}"),
    }
}

/// S-frame 控制域：cf[0]=0x01（bit0=1, bit1=0 → S 格式），cf[1]=0x00，
/// cf[2]/cf[3] 编码确认序号 N(R)
#[test]
fn seq_sframe_ack_encoding() {
    let f = build_sframe(0);
    assert_eq!(f, vec![0x68, 0x04, 0x01, 0x00, 0x00, 0x00]);

    let f8 = build_sframe(8);
    assert_eq!(f8[2], 0x01, "S-frame bit0 必须为 1、bit1 必须为 0");
    assert_eq!(f8[3], 0x00);
    assert_eq!(decode_seq(f8[4], f8[5]), 8, "N(R) 必须还原为入参");

    let big = build_sframe(1000);
    assert_eq!(decode_seq(big[4], big[5]), 1000);
}

/// 确认阈值必须 < k（IEC 104 未确认 I-frame 上限 k = 12），
/// 否则 server 发送窗口打满会停发/断链
// 契约守门：断言对象就是常量取值范围本身，常量断言属预期，不简化
#[allow(clippy::assertions_on_constants)]
#[test]
fn seq_ack_every_n_frames_within_send_window() {
    assert!(
        ACK_EVERY_N_FRAMES >= 1,
        "至少每 1 帧确认一次（0 会导致永不确认）"
    );
    assert!(
        (ACK_EVERY_N_FRAMES as u32) < 12,
        "确认阈值必须 < k(12)，实际 = {}",
        ACK_EVERY_N_FRAMES
    );
}
