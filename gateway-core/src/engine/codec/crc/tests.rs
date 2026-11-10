use super::*;

/// 表驱动参考实现：在测试内由多项式 0xA001 独立推导。
///
/// 与生产实现（逐位循环）结构不同，用作差分交叉校验——
/// 将来若把 `crc16` 换成查表/切片实现，此测试仍能守住行为等价。
fn crc16_table_reference(data: &[u8]) -> u16 {
    let mut table = [0u16; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u16;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc = (crc >> 8) ^ table[((crc ^ b as u16) & 0xFF) as usize];
    }
    crc
}

/// 确定性 LCG：为随机输入提供可复现的字节流，避免引入外部依赖。
fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// CRC-16/MODBUS 权威测试向量（poly=0x8005 反射、init=0xFFFF、xorout=0x0000）
#[test]
fn test_crc16_modbus_authoritative_vectors() {
    // 业界标准 "check" 向量
    assert_eq!(crc16(b"123456789"), 0x4B37, "\"123456789\" 应为 0x4B37");
    // 空输入：无字节参与迭代，应返回初值
    assert_eq!(crc16(&[]), 0xFFFF, "空输入应返回初值 0xFFFF");
}

/// Modbus RTU 真实帧向量（沿用既有用例，确保行为不回归）
#[test]
fn test_crc16_modbus_rtu_frames() {
    // slave=1, fc=3, addr=0, count=10 → CRC 低/高 = C5 CD
    assert_eq!(crc16(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A]), 0xCDC5);
    // slave=1, fc=3, addr=0, count=1 → CRC 低/高 = 84 0A
    assert_eq!(crc16(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x01]), 0x0A84);
}

/// 差分测试：生产实现 vs 独立表驱动实现，覆盖长度边界与大量伪随机输入
#[test]
fn test_crc16_matches_table_reference() {
    // 长度边界：0..=300
    for len in 0..=300usize {
        let data: Vec<u8> = (0..len)
            .map(|i| (i.wrapping_mul(7).wrapping_add(3)) as u8)
            .collect();
        assert_eq!(
            crc16(&data),
            crc16_table_reference(&data),
            "len={len} 不一致"
        );
    }
    // 伪随机输入（确定性，2000 组）
    let mut state: u64 = 0x1234_5678_9ABC_DEF0;
    for _ in 0..2000 {
        let len = (lcg_next(&mut state) % 512) as usize;
        let data: Vec<u8> = (0..len).map(|_| lcg_next(&mut state) as u8).collect();
        assert_eq!(
            crc16(&data),
            crc16_table_reference(&data),
            "len={len} 随机输入不一致"
        );
    }
}

/// 单比特翻转必须被检出（CRC-16 对任何单比特错误都应能发现）
#[test]
fn test_crc16_detects_single_bit_flip() {
    let base = [0x01u8, 0x03, 0x00, 0x00, 0x00, 0x0A];
    let base_crc = crc16(&base);
    for byte_idx in 0..base.len() {
        for bit in 0..8 {
            let mut flipped = base;
            flipped[byte_idx] ^= 1 << bit;
            assert_ne!(
                crc16(&flipped),
                base_crc,
                "单比特翻转未被检出: byte={byte_idx} bit={bit}"
            );
        }
    }
}
