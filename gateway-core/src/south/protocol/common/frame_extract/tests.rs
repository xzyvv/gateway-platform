//! DLT645 / CJT188 帧提取的边界用例。
//!
//! 这些用例盯住 `extract_dlt645_cjt188_frame` 里四处最容易出错的判断：
//! 长度边界（头部 10 字节、帧总长）、第二头字节、校验和 CS、结束字节 0x16，
//! 以及「重同步跳过的前缀垃圾不得混进返回帧」。
//!
//! 旧用例只覆盖了顺利路径：任何一个比较符被反转（`<` 改 `>=`、`!=` 改 `==`），
//! 顺利路径照样通过，只有这些边界用例才能抓到。

use super::*;
use crate::south::bus::FrameBuffer;

/// DLT645 帧：0x68 + 地址(6) + 0x68 + C + L + DATA + CS + 0x16
fn dlt645_frame(data: &[u8]) -> Vec<u8> {
    build_68_frame(
        &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
        true,
        false,
        0x11,
        data,
        "DLT645",
    )
}

/// CJT188 帧：0x68 + 地址(7) + C + L + DATA + CS + 0x16（无第二头字节）
fn cjt188_frame(data: &[u8]) -> Vec<u8> {
    build_68_frame(
        &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
        false,
        false,
        0x11,
        data,
        "CJT188",
    )
}

/// 完整帧必须被原样取出（长度恰好等于帧总长的边界）。
#[test]
fn extracts_complete_dlt645_frame() {
    let f = dlt645_frame(&[0xAA, 0xBB]);
    let mut fb = FrameBuffer::new();
    fb.push(&f);
    let got = extract_dlt645_cjt188_frame(&mut fb, true).expect("应提取到帧");
    assert_eq!(got, f);
}

/// CJT188 无第二头字节，同样要能取出。
#[test]
fn extracts_cjt188_frame_without_second_header() {
    let f = cjt188_frame(&[0xAA]);
    let mut fb = FrameBuffer::new();
    fb.push(&f);
    let got = extract_dlt645_cjt188_frame(&mut fb, false).expect("应提取到帧");
    assert_eq!(got, f);
}

/// 重同步跳过的前缀垃圾必须丢弃，否则 parse_68_response 会因首字节不是
/// 0x68 直接拒绝——等于「重同步到了却用不了」。
#[test]
fn drops_garbage_before_frame() {
    let f = dlt645_frame(&[0xAA]);
    let mut fb = FrameBuffer::new();
    fb.push(&[0x11, 0x22, 0x33]);
    fb.push(&f);
    let got = extract_dlt645_cjt188_frame(&mut fb, true).expect("应提取到帧");
    assert_eq!(got, f, "前缀垃圾不得混进返回帧");
}

/// 头部凑不满 10 字节时必须返回 None（等更多数据）。
#[test]
fn returns_none_when_header_incomplete() {
    let f = dlt645_frame(&[0xAA]);
    let mut fb = FrameBuffer::new();
    fb.push(&f[..9]);
    assert!(
        extract_dlt645_cjt188_frame(&mut fb, true).is_none(),
        "头部不足应等待更多数据"
    );
}

/// 校验和 CS 不对的帧不得被接受。
#[test]
fn rejects_frame_with_bad_checksum() {
    let mut f = dlt645_frame(&[0xAA]);
    let n = f.len();
    f[n - 2] = f[n - 2].wrapping_add(1);
    let mut fb = FrameBuffer::new();
    fb.push(&f);
    let got = extract_dlt645_cjt188_frame(&mut fb, true);
    assert_ne!(got, Some(f.clone()), "CS 不对的帧不得被原样接受");
}

/// 结束字节不是 0x16 的帧不得被接受。
#[test]
fn rejects_frame_with_bad_end_byte() {
    let mut f = dlt645_frame(&[0xAA]);
    let n = f.len();
    f[n - 1] = 0x00;
    let mut fb = FrameBuffer::new();
    fb.push(&f);
    let got = extract_dlt645_cjt188_frame(&mut fb, true);
    assert_ne!(got, Some(f.clone()), "结束字节不对的帧不得被原样接受");
}

/// DLT645 缺第二头字节（start+7 不是 0x68）时应被跳过。
#[test]
fn rejects_dlt645_frame_without_second_header() {
    let mut f = dlt645_frame(&[0xAA]);
    f[7] = 0x00;
    let mut fb = FrameBuffer::new();
    fb.push(&f);
    let got = extract_dlt645_cjt188_frame(&mut fb, true);
    assert_ne!(got, Some(f.clone()), "缺第二头字节的 DLT645 帧应被拒绝");
}
