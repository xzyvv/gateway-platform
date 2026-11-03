//! Golden 报文回归（决策表 #10）：请求帧逐字节与 `tests/fixtures/golden/mc/*.hex` 比对。
use super::*;

/// 3E 批量读请求：D100 起读 4 个字（定时器钉死 0x0010，请求子头 0x5000）
#[test]
fn golden_mc_read_request_d100_4words() {
    let dev = parse_address("D100").expect("D100 应可解析");
    let f = build_read_request(&dev, 4, 0x0010, McFrameFormat::E3, 0);
    crate::south::protocol::golden::assert_golden("mc/read_request_d100_4words.hex", &f);
}

/// 4E 批量读请求：D100 起读 4 个字（序列号 0x0001）
#[test]
fn golden_mc_read_request_d100_4words_4e() {
    let dev = parse_address("D100").expect("D100 应可解析");
    let f = build_read_request(&dev, 4, 0x0010, McFrameFormat::E4, 1);
    crate::south::protocol::golden::assert_golden("mc/read_request_d100_4words_4e.hex", &f);
}

/// 1E 批量字读请求：D100 起读 2 个字（对齐官方报文实例）
#[test]
fn golden_mc_read_request_1e_d100_2words() {
    let dev = parse_address("D100").expect("D100 应可解析");
    let f = build_request_1e(FC_BATCH_WORD_READ, &dev, 2, 0x000A, &[]).expect("1E 建帧");
    crate::south::protocol::golden::assert_golden("mc/read_request_1e_d100_2words.hex", &f);
}

/// 1E 批量位读请求：M16 起读 1 位（对齐官方报文实例）
#[test]
fn golden_mc_read_request_1e_m16_1bit() {
    let dev = parse_address("M16").expect("M16 应可解析");
    let f = build_request_1e(FC_BATCH_BIT_READ, &dev, 1, 0x000A, &[]).expect("1E 建帧");
    crate::south::protocol::golden::assert_golden("mc/read_request_1e_m16_1bit.hex", &f);
}
