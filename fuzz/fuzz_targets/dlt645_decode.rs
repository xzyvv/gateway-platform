#![no_main]
//! DLT645 响应解码 fuzz：
//! 1. 随机/畸形字节进解码器（5 个变体全跑）—— 只要求不 panic；
//! 2. 解帧 —— 0x68 起始符缺失、长度域与缓冲区不符时不得死循环；
//! 3. 合法帧 round-trip —— build → parse 必须还原数据区（含 +0x33 变体）。

use gateway_core::fuzz_api as api;
use gateway_core::south::bus::FrameBuffer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 1) 畸形字节：地址固定取前 6 字节（生产侧地址恒为 6 字节 BCD）
    if data.len() > 6 {
        let addr = &data[..6];
        for version in 0..5u8 {
            let _ = api::dlt645_parse_response(&data[6..], addr, version);
        }
    }

    // 2) 解帧：固定上限兜底，防止解帧器不消费导致挂死
    let mut fb = FrameBuffer::new();
    fb.push(data);
    for _ in 0..64 {
        if api::dlt645_extract_frame(&mut fb, None).is_none() {
            break;
        }
    }

    // 3) 合法帧 round-trip：C 位强制为「响应、正常应答、单帧」
    if data.len() >= 6 {
        let addr = &data[..6];
        let ctrl = 0x80 | (data[6 % data.len()] & 0x1F);
        let payload = &data[6..];
        // 协议长度域为 1 字节，超长会被 build 直接丢弃
        let payload = &payload[..payload.len().min(0xFF)];

        for version in 0..5u8 {
            let frame = api::dlt645_build_frame(addr, ctrl, payload, version);
            if frame.is_empty() {
                continue;
            }
            let parsed = api::dlt645_parse_response(&frame, addr, version)
                .expect("自建 DLT645 帧必须可解析");
            let expected: Vec<u8> = if api::dlt645_data_cipher(version) {
                // 变体要求数据区 +0x33，解析时 -0x33，因此期望值需同样换算
                payload.iter().map(|b| b.wrapping_sub(0x33)).collect()
            } else {
                payload.to_vec()
            };
            assert_eq!(parsed, expected, "DLT645 数据区 round-trip 必须逐字节一致");
        }
    }
});
