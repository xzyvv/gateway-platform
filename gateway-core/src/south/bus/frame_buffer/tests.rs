use super::*;

// ===== 基础语义（既有测试，保留）=====

#[test]
fn test_push_and_pop() {
    let mut fb = FrameBuffer::with_capacity(100);
    fb.push(&[0x01, 0x02, 0x03]);
    assert_eq!(fb.len(), 3);
    assert_eq!(fb.pop(2), vec![0x01, 0x02]);
    assert_eq!(fb.len(), 1);
}

#[test]
fn test_no_truncate_under_limit() {
    let mut fb = FrameBuffer::with_capacity(1000);
    fb.push(&[0x68; 50]);
    assert_eq!(fb.len(), 50);
}

#[test]
fn test_pop_clamps_and_clear_semantics() {
    let mut fb = FrameBuffer::new();
    assert!(fb.is_empty());
    fb.push(&[1, 2, 3]);
    assert!(!fb.is_empty());
    assert_eq!(
        fb.pop(10),
        vec![1, 2, 3],
        "pop 长度超过缓冲时按实际长度返回"
    );
    assert!(fb.is_empty());
    fb.push(&[9]);
    fb.clear();
    assert!(fb.is_empty());
}

#[test]
fn test_peek_returns_full_content_after_wrap() {
    let mut fb = FrameBuffer::new();
    fb.push(&[0xAA, 0xBB]);
    assert_eq!(fb.pop(1), vec![0xAA]);
    fb.push(&[0xCC]);
    assert_eq!(fb.peek(), vec![0xBB, 0xCC], "peek 应返回完整连续副本");
    assert_eq!(fb.peek_all(), fb.peek(), "peek 与 peek_all 语义一致");
}

// ===== C-10 修复配套回归测试 =====

/// C-10：单一硬不变量 `len() <= max_len`，无论输入如何构造（多帧头 / 紧邻帧头 / 无帧头）。
/// 旧实现保留字节数在 `[max_len, 2*max_len]` 抖动，与「防止内存膨胀」目标不符。
#[test]
fn c10_invariant_len_le_max_len_for_any_input() {
    // 场景 A：多帧头 + 长尾
    let mut fb = FrameBuffer::with_capacity(100);
    let mut data = vec![0x68; 50];
    data.extend(vec![0xAA; 50]);
    data.push(0x68);
    data.extend(vec![0xBB; 200]); // 总长 301
    fb.push(&data);
    assert!(
        fb.len() <= 100,
        "硬不变量被破坏：len={} max_len=100",
        fb.len()
    );

    // 场景 B：两帧头紧邻（0x68 0x68 ...）
    let mut fb = FrameBuffer::with_capacity(50);
    let data = vec![0x68, 0x68, 0x68, 0x68, 0x68]; // 5 字节帧头
    fb.push(&data);
    assert!(fb.len() <= 50);

    // 场景 C：无帧头纯噪声
    let mut fb = FrameBuffer::with_capacity(30);
    fb.push(&[0x00_u8; 1000]);
    assert_eq!(fb.len(), 30, "无帧头时应硬截到 max_len");
}

/// C-10：截断策略——保留「最近 1 个 0x68 帧头到结尾」。
/// 旧实现保留「最近 2 个 0x68 之间」，两帧头紧邻时退化为「仅最近 1 帧头」，
/// 但中间 1 帧头位置抖动会让保留的字节数在 `[1, 2*max_len]` 之间不稳。
/// 新设计稳定保留单帧头+后续数据，超 max_len 再硬截。
#[test]
fn c10_truncation_preserves_last_header_to_end() {
    let mut fb = FrameBuffer::with_capacity(50);
    // 构造：5 字节帧头 A + 80 字节填充 + 8 字节帧头 B + 100 字节数据
    // 总长 193，超 max_len=50
    let mut data = vec![0x68, 0x10, 0x00, 0x00, 0x00]; // 帧头 A
    data.extend(vec![0xCC; 80]);
    data.extend([0x68, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x68]); // 帧头 B（含 DLT645 双 0x68 帧头）
    data.extend(vec![0xDD; 100]);

    fb.push(&data);

    // 必须包含「最近 1 个 0x68」（即帧头 B 的最后一个 0x68）
    let kept = fb.peek_all();
    assert!(
        kept.contains(&0x68),
        "必须保留至少 1 个 0x68 帧头，实际: {:?}",
        kept
    );
    // 单一不变量
    assert!(fb.len() <= 50, "硬不变量：len={} max_len=50", fb.len());
}

/// C-10：无帧头时硬截到 max_len（保留最新 max_len 字节）。
#[test]
fn c10_no_header_hard_truncates_to_max_len() {
    let mut fb = FrameBuffer::with_capacity(20);
    fb.push(&[0xFF_u8; 100]);
    assert_eq!(fb.len(), 20, "无帧头硬截到 max_len");
    // 保留的是尾部数据（最新字节）
    let kept = fb.peek_all();
    assert!(
        kept.iter().all(|&b| b == 0xFF),
        "无帧头时应保留 max_len 字节的纯噪声"
    );
}

/// C-10：单帧头时从该帧头保留到结尾——保证这帧数据的起始字节不被丢。
#[test]
fn c10_single_header_keeps_from_header() {
    let mut fb = FrameBuffer::with_capacity(10);
    let mut data = vec![0x00_u8; 20];
    data.push(0x68);
    data.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]); // 共 25 字节
    fb.push(&data);
    // 最近 1 个 0x68 在 index 20；保留 index 20..=24 共 5 字节
    assert_eq!(fb.len(), 5, "单帧头时从帧头保留到结尾");
    let kept = fb.peek_all();
    assert_eq!(kept[0], 0x68, "保留内容以该帧头起始");
    assert_eq!(kept, &[0x68, 0x11, 0x22, 0x33, 0x44]);
}

/// C-10：硬不变量「len() <= max_len」在多次连续 push 累积下仍成立。
/// 这是 B-core-007 描述的「反复小段释放导致 buffer 持续膨胀」场景的回归测试。
#[test]
fn c10_invariant_holds_under_repeated_push() {
    let mut fb = FrameBuffer::with_capacity(100);
    // 模拟电磁干扰：每 50ms 推入 50 字节无帧头数据，连续 20 次
    // 旧实现可能累积到 2*100=200 才截断；新实现每次都硬截到 <=100
    for i in 0..20 {
        let mut chunk = vec![0xAA_u8; 50];
        chunk[0] = 0x68; // 每个 chunk 有一个 0x68 帧头
        fb.push(&chunk);
        assert!(
            fb.len() <= 100,
            "iter {} 硬不变量被破坏：len={}",
            i,
            fb.len()
        );
    }
}

/// C-10：连续多帧头（0x55 BACnet MS/TP 前导码）也被识别为帧头。
#[test]
fn c10_bacnet_ms_tp_preamble_recognized() {
    let mut fb = FrameBuffer::with_capacity(30);
    let mut data = vec![0xAA; 40]; // 无帧头填充
    data.push(0x55); // BACnet MS/TP 前导码
    data.extend(vec![0xBB; 20]);
    fb.push(&data);
    // 帧头 0x55 在 index 40；保留 0x55 + 20 = 21 字节 < 30
    let kept = fb.peek_all();
    assert!(kept.contains(&0x55), "必须保留 0x55 BACnet 前导码");
    assert!(fb.len() <= 30);
}
