//! 决策表 #28：ASDU 公共地址（CA）1/2 字节协商
use super::*;

/// CA 长度判定与编码：`1` → 单字节（取低位）；其余（含 0/2）→ 标准 2 字节小端
#[test]
fn ca_len_and_encode_ca() {
    assert_eq!(ca_len(1), 1);
    assert_eq!(ca_len(2), 2);
    assert_eq!(ca_len(0), 2, "0 视为非 1，按标准 2 字节兜底");
    assert_eq!(ca_len(3), 2, "非标准值一律按 2 字节");
    assert_eq!(encode_ca(0x0102, 1), vec![0x02], "单字节只取低 8 位");
    assert_eq!(encode_ca(0x0102, 2), vec![0x02, 0x01], "2 字节小端");
}

/// 总召唤 ASDU：单字节 CA 时整体比 2 字节短 1 字节，IOA 恒为 3 字节
#[test]
fn interrogation_asdu_ca_len_variants() {
    let one = build_interrogation_asdu(1, 1);
    assert_eq!(
        one,
        vec![100, 0x01, 0x06, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14]
    );
    let two = build_interrogation_asdu(1, 2);
    assert_eq!(two.len(), one.len() + 1, "2 字节 CA 比 1 字节多 1 字节");
    assert_eq!(&two[4..6], &[0x01, 0x00], "2 字节公共地址");
    assert_eq!(&two[6..9], &[0x00, 0x00, 0x00], "IOA 恒 3 字节");
}

/// 对时 ASDU：CA 由 2 字节降为 1 字节后总长 16 → 15，时标位置随之前移
#[test]
fn clock_sync_asdu_one_byte_ca() {
    let dt = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let asdu = build_clock_sync_asdu(1, 1, &dt);
    assert_eq!(asdu.len(), 15, "4(头) + 1(CA) + 3(IOA) + 7(时标)");
    assert_eq!(asdu[4], 0x01, "单字节公共地址");
    assert_eq!(&asdu[5..8], &[0x00, 0x00, 0x00], "IOA = 0");
    assert_eq!(&asdu[8..15], &encode_cp56time2a(&dt));
}

/// 测量 ASDU 解析：单字节 CA 时信息体起点为 5（而非 6），IOA 仍按 3 字节取
#[test]
fn parse_asdu_one_byte_ca() {
    // 类型 14（短浮点）+ 1 个信息体 + COT=20 + 单字节 CA=1 + IOA=100 + 值 + 品质
    let mut asdu = vec![14u8, 0x01, 20, 0x00, 0x01];
    asdu.extend_from_slice(&100u32.to_le_bytes()[..3]);
    asdu.extend_from_slice(&12.5f32.to_le_bytes());
    asdu.push(0x00);
    let mut map = std::collections::HashMap::new();
    parse_asdu(&asdu, &mut map, 1);
    let (v, q, _ts) = map
        .get(&100)
        .copied()
        .expect("单字节 CA 下 IOA 100 应被解析");
    assert!((v - 12.5).abs() < 1e-4, "值解码错误: {v}");
    assert_eq!(q, crate::engine::model::Quality::Good);
}

/// 命令确认解析：单字节 CA 时 IOA 起点为 5；用 2 字节 CA 解析同一帧应因长度不足返回 None
#[test]
fn parse_command_confirmation_one_byte_ca() {
    let asdu = build_command_asdu(TYPE_C_SC_NA_1, 1, 1, 300, 0x81);
    assert_eq!(asdu.len(), 9, "4(头) + 1(CA) + 3(IOA) + 1(命令值)");
    assert_eq!(
        parse_command_confirmation(&asdu, 1),
        Some((TYPE_C_SC_NA_1, 300, 6)),
        "单字节 CA：IOA=300、COT=激活(6)"
    );
    assert_eq!(
        parse_command_confirmation(&asdu, 2),
        None,
        "9 字节帧按 2 字节 CA 解析长度不足（要求 ≥10）"
    );
}
