//! 帧构建单元测试：3E/4E/1E 读请求布局、1E 码表映射与不可寻址软元件报错。
use super::*;

#[test]
fn test_build_read_request_layout() {
    let dev = parse_address("D100").expect("D100");
    let req = build_read_request(&dev, 2, 0x0010, McFrameFormat::E3, 0);
    assert_eq!(req.len(), 21);
    assert_eq!(&req[0..2], &[0x50, 0x00]); // 3E 请求子头（标准：0x5000）
    assert_eq!(req[2], 0x00); // 网络号
    assert_eq!(req[3], 0xFF); // PC 号
    assert_eq!(&req[4..6], &[0xFF, 0x03]); // 模块 IO
    assert_eq!(req[6], 0x00); // 站号
    assert_eq!(u16::from_le_bytes([req[7], req[8]]), 12); // 数据长度
    assert_eq!(u16::from_le_bytes([req[9], req[10]]), 0x0010); // 定时器
    assert_eq!(u16::from_le_bytes([req[11], req[12]]), 0x0401); // 读命令
    assert_eq!(u16::from_le_bytes([req[13], req[14]]), 0x0000); // 字子命令
    assert_eq!(req[15], 0xA8); // D 软元件码
    assert_eq!(&req[16..19], &[0x64, 0x00, 0x00]); // 首地址 100 LE
    assert_eq!(u16::from_le_bytes([req[19], req[20]]), 2); // 点数
}

#[test]
fn test_build_read_bit_subcommand() {
    let dev = parse_address("M10").expect("M10");
    let req = build_read_request(&dev, 1, 0x0010, McFrameFormat::E3, 0);
    assert_eq!(u16::from_le_bytes([req[13], req[14]]), 0x0001); // 位子命令
    assert_eq!(req[15], 0x90);
}

#[test]
fn test_frame_format_from_variant() {
    assert_eq!(McFrameFormat::from_variant(None), McFrameFormat::E3);
    assert_eq!(McFrameFormat::from_variant(Some("3e")), McFrameFormat::E3);
    assert_eq!(McFrameFormat::from_variant(Some(" 3E ")), McFrameFormat::E3);
    assert_eq!(McFrameFormat::from_variant(Some("4e")), McFrameFormat::E4);
    assert_eq!(McFrameFormat::from_variant(Some("1e")), McFrameFormat::E1);
    // 未知取值回退 3E
    assert_eq!(McFrameFormat::from_variant(Some("xx")), McFrameFormat::E3);
}

#[test]
fn test_build_4e_read_request_layout() {
    let dev = parse_address("D100").expect("D100");
    let req = build_read_request(&dev, 2, 0x0010, McFrameFormat::E4, 0x1234);
    // 4E = 3E + 2 字节序列号（子头 0x5400）
    assert_eq!(req.len(), 23);
    assert_eq!(&req[0..2], &[0x54, 0x00]); // 4E 请求子头
    assert_eq!(u16::from_le_bytes([req[2], req[3]]), 0x1234); // 序列号
    assert_eq!(req[4], 0x00); // 网络号
    assert_eq!(req[5], 0xFF); // PC 号
    assert_eq!(&req[6..8], &[0xFF, 0x03]); // 模块 IO
    assert_eq!(req[8], 0x00); // 站号
    assert_eq!(u16::from_le_bytes([req[9], req[10]]), 12); // 数据长度（不含序列号）
    assert_eq!(u16::from_le_bytes([req[11], req[12]]), 0x0010); // 定时器
    assert_eq!(u16::from_le_bytes([req[13], req[14]]), 0x0401); // 读命令
    assert_eq!(u16::from_le_bytes([req[15], req[16]]), 0x0000); // 字子命令
    assert_eq!(req[17], 0xA8); // D 软元件码
    assert_eq!(&req[18..21], &[0x64, 0x00, 0x00]); // 首地址
    assert_eq!(u16::from_le_bytes([req[21], req[22]]), 2); // 点数
}

#[test]
fn test_build_1e_read_request_layout() {
    // 对齐官方实例：读 D100 起 2 字 → 01 FF 0A 00 64 00 00 00 20 44 02 00
    let dev = parse_address("D100").expect("D100");
    let req = build_request_1e(FC_BATCH_WORD_READ, &dev, 2, 0x000A, &[]).expect("1E 建帧");
    assert_eq!(
        req,
        vec![0x01, 0xFF, 0x0A, 0x00, 0x64, 0x00, 0x00, 0x00, 0x20, 0x44, 0x02, 0x00]
    );
}

#[test]
fn test_build_1e_bit_read_layout() {
    // 对齐官方实例：读 M16 起 1 位 → 00 FF 0A 00 10 00 00 00 20 4D 01 00
    let dev = parse_address("M16").expect("M16");
    let req = build_request_1e(FC_BATCH_BIT_READ, &dev, 1, 0x000A, &[]).expect("1E 建帧");
    assert_eq!(
        req,
        vec![0x00, 0xFF, 0x0A, 0x00, 0x10, 0x00, 0x00, 0x00, 0x20, 0x4D, 0x01, 0x00]
    );
}

#[test]
fn test_build_1e_word_write_layout() {
    // 对齐官方实例：写 D20/D21 = 34/45 → 03 FF 0A 00 14 00 00 00 20 44 02 00 22 00 2D 00
    let dev = parse_address("D20").expect("D20");
    let req = build_request_1e(
        FC_BATCH_WORD_WRITE,
        &dev,
        2,
        0x000A,
        &[0x22, 0x00, 0x2D, 0x00],
    )
    .expect("1E 建帧");
    assert_eq!(
        req,
        vec![
            0x03, 0xFF, 0x0A, 0x00, 0x14, 0x00, 0x00, 0x00, 0x20, 0x44, 0x02, 0x00, 0x22, 0x00,
            0x2D, 0x00
        ]
    );
}

#[test]
fn test_code_1e_mapping() {
    // 依 ICC 驱动手册 Table 1：3E 单字节码 → 1E 双字节 ASCII 码
    assert_eq!(code_1e_from_code_3e(0xA8), Some(0x4420)); // D
    assert_eq!(code_1e_from_code_3e(0x90), Some(0x4D20)); // M
    assert_eq!(code_1e_from_code_3e(0x9C), Some(0x5820)); // X
    assert_eq!(code_1e_from_code_3e(0xB4), Some(0x5720)); // W
    assert_eq!(code_1e_from_code_3e(0xC2), Some(0x544E)); // TN
    assert_eq!(code_1e_from_code_3e(0xC5), Some(0x434E)); // CN
                                                          // A 系列 1E 不可寻址：SM/SD/ZR/L/F/V/Z 等
    assert_eq!(code_1e_from_code_3e(0x91), None); // SM
    assert_eq!(code_1e_from_code_3e(0xB0), None); // ZR
}

#[test]
fn test_build_1e_unsupported_device_errors() {
    // SM 在 A 系列 1E 帧中不可寻址 → 建帧应报 InvalidConfig
    let dev = parse_address("SM10").expect("SM10");
    let err = build_request_1e(FC_BATCH_BIT_READ, &dev, 1, 0x000A, &[]).expect_err("应报错");
    assert!(
        matches!(
            err,
            crate::south::protocol::CollectorError::InvalidConfig(_)
        ),
        "unexpected: {err:?}"
    );
}
