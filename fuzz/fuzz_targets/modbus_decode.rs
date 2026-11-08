#![no_main]
//! Modbus RTU 响应解码 fuzz：
//! 1. 随机/畸形字节直接进解码器 —— 只要求不 panic、不越界；
//! 2. 环形缓冲区解帧 —— 残帧、噪声前缀下不得死循环；
//! 3. 合法帧 round-trip —— 真实 CRC 打通被校验挡住的大端寄存器解码路径。

use gateway_core::engine::util::crc::crc16;
use gateway_core::fuzz_api as api;
use gateway_core::south::bus::FrameBuffer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 1) 畸形字节：byte_count / 从站号 / 功能码全部由输入决定
    if data.len() >= 4 {
        let slave = data[0];
        let fc = data[1];
        let expected = u16::from_be_bytes([data[2], data[3]]);
        let _ = api::modbus_parse_read_response(slave, fc, expected, &data[4..]);
        let _ = api::modbus_parse_coils_response(slave, fc, expected, &data[4..]);
    }

    // 2) 解帧：尾部不完整时应返回 None 或推进缓冲；这里用固定上限兜底，
    //    如果解帧器「不消费也不返回」就会撞上限而不是把 fuzz 挂死。
    let mut fb = FrameBuffer::new();
    fb.push(data);
    for _ in 0..64 {
        if api::modbus_extract_frame(&mut fb, None).is_none() {
            break;
        }
    }

    // 3) 合法帧 round-trip：随机字节几乎全被 CRC 拦截，这里补 CRC 后打通深层路径
    if data.len() >= 6 {
        let body = &data[4..];
        let count = (body.len() / 2).min(64) as u16;
        if count > 0 {
            let byte_count = count * 2;
            let mut frame = Vec::with_capacity(5 + byte_count as usize);
            frame.push(data[0]);
            frame.push(if data[1] & 1 == 0 { 0x03 } else { 0x04 });
            frame.push(byte_count as u8);
            frame.extend_from_slice(&body[..byte_count as usize]);
            let crc = crc16(&frame);
            // Modbus 线上 CRC 低字节在前
            frame.push((crc & 0xFF) as u8);
            frame.push((crc >> 8) as u8);

            let regs = api::modbus_parse_read_response(frame[0], frame[1], count, &frame)
                .expect("CRC 正确的读寄存器响应必须可解析");
            assert_eq!(
                regs.len(),
                count as usize,
                "寄存器数量必须与 byte_count 一致"
            );
            for (i, v) in regs.iter().enumerate() {
                let expect = u16::from_be_bytes([body[i * 2], body[i * 2 + 1]]);
                assert_eq!(*v, expect, "寄存器值必须按大端还原");
            }
        }
    }
});
