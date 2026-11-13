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
use std::time::Duration;

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

// ============================================================================
// 终止性：重同步分支不得死循环
// ============================================================================
//
// 三个重同步分支（第二头字节不对 / CS 不对 / 结束字节不对）都靠
// `fb.pop(start + 1)` 丢弃「到帧头为止」来保证每轮至少前进 1 字节。
// 一旦这个前进量被改坏（例如 `start + 1` 被改成 `start`），缓冲区里的
// 0x68 永远留在原地，continue 之后 start 不变 → 死循环。
// 现场表现是串口采集任务卡死、采集中断，而不是报错。
//
// 断言式用例抓不到死循环（函数根本不返回，进程不会退出），
// 所以这里显式限时：独立线程跑提取，主线程等结果；超时即判定失败。

/// 帧提取必须在有限时间内返回（超时即视为重同步死循环）。
fn assert_extract_terminates(fb: FrameBuffer, check_second_header: bool) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut fb = fb;
        let _ = tx.send(extract_dlt645_cjt188_frame(&mut fb, check_second_header));
    });
    let got = rx.recv_timeout(Duration::from_secs(5));
    assert!(got.is_ok(), "帧提取未在 5s 内返回：重同步分支疑似死循环");
}

/// 第二头字节不对时，重同步必须能终止（不得原地打转）。
#[test]
fn resync_on_missing_second_header_terminates() {
    let mut f = dlt645_frame(&[0xAA]);
    f[7] = 0x00; // 第二头字节损坏 → 走第二头字节校验分支
    let mut fb = FrameBuffer::new();
    fb.push(&[0x11, 0x22, 0x33]); // 前缀垃圾，保证帧头不在缓冲区首位
    fb.push(&f);
    assert_extract_terminates(fb, true);
}

/// 校验和 CS 不对时，重同步必须能终止（不得原地打转）。
#[test]
fn resync_on_bad_checksum_terminates() {
    let mut f = dlt645_frame(&[0xAA]);
    let n = f.len();
    f[n - 2] = f[n - 2].wrapping_add(1); // CS 错 → 走校验和分支
    let mut fb = FrameBuffer::new();
    fb.push(&[0x11, 0x22, 0x33]);
    fb.push(&f);
    assert_extract_terminates(fb, true);
}

/// 结束字节不对时，重同步必须能终止（不得原地打转）。
#[test]
fn resync_on_bad_end_byte_terminates() {
    let mut f = dlt645_frame(&[0xAA]);
    let n = f.len();
    f[n - 1] = 0x00; // 结束字节错 → 走结束字节分支
    let mut fb = FrameBuffer::new();
    fb.push(&[0x11, 0x22, 0x33]);
    fb.push(&f);
    assert_extract_terminates(fb, true);
}

// ============================================================================
// 缓冲状态不变量：数据不足时不得丢字节
// ============================================================================
//
// 「等更多数据」分支只允许丢弃帧头之前的垃圾前缀，帧头及其后续字节
// 必须原样留在缓冲区里等下一批数据续上。若把丢弃条件写坏
// （例如 `start > 0` 被改成 `start == 0`），垃圾会一直被留在缓冲区，
// 要么把下一帧的起点顶歪，要么让无效残帧冒充下一帧的帧头。

/// 头部不足 10 字节时，已收到的帧字节（含 0x68）必须原样保留。
#[test]
fn keeps_incomplete_frame_bytes_when_header_short() {
    let f = dlt645_frame(&[0xAA]);
    let mut fb = FrameBuffer::new();
    fb.push(&[0x11, 0x22, 0x33]); // 前缀垃圾
    fb.push(&f[..8]); // 头部只到 8 字节
    assert!(
        extract_dlt645_cjt188_frame(&mut fb, true).is_none(),
        "头部不足应等待更多数据"
    );
    assert_eq!(
        fb.peek_all(),
        f[..8].to_vec(),
        "头部不足时不得丢弃已收到的帧字节"
    );
}

/// 帧体不足时，已收到的帧字节必须原样保留。
#[test]
fn keeps_incomplete_frame_bytes_when_body_short() {
    let f = dlt645_frame(&[0xAA, 0xBB]);
    let mut fb = FrameBuffer::new();
    fb.push(&[0x11, 0x22, 0x33]); // 前缀垃圾
    fb.push(&f[..12]); // 头部 10 + 数据 2，帧体尚未收全
    assert!(
        extract_dlt645_cjt188_frame(&mut fb, true).is_none(),
        "帧体不足应等待更多数据"
    );
    assert_eq!(
        fb.peek_all(),
        f[..12].to_vec(),
        "帧体不足时不得丢弃已收到的帧字节"
    );
}

/// 第二头字节损坏的残帧必须被丢弃，不得留在缓冲区里冒充下一帧的帧头。
#[test]
fn drops_residual_frame_with_invalid_second_header() {
    let mut f = dlt645_frame(&[0xAA]);
    f[7] = 0x00; // 第二头字节损坏
    let mut fb = FrameBuffer::new();
    fb.push(&[0x11, 0x22, 0x33]); // 前缀垃圾
    fb.push(&f[..10]); // 恰好凑满头部 10 字节
    assert!(
        extract_dlt645_cjt188_frame(&mut fb, true).is_none(),
        "残帧不完整应等待更多数据"
    );
    assert_ne!(
        fb.peek_all().first(),
        Some(&0x68),
        "第二头字节损坏的残帧必须被丢弃，不得留在缓冲区冒充下一帧帧头"
    );
}
