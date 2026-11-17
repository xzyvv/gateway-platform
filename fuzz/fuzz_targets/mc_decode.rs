#![no_main]
//! 三菱 MC 响应解析 fuzz：3E / 4E / 1E 三条响应路径的头部与值解码。
//!
//! 这里调用的是**生产同一个函数**：`read_response_3e/4e/1e` 只多一步 `read_exact`,
//! 头部校验与长度推导全部落在 `parse_response_header_*` 里。因此本目标覆盖的
//! 「长度域越界 / 子头错 / 序列号错 / 1E 数据长度超限」都是生产路径的真实分支。
//!
//! 只要求不 panic、不按对端给的长度生成超限缓冲。

use gateway_core::fuzz_api as api;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 1) 3E / 4E 响应头：长度域（head[7..9] / head[9..11]）完全由对端控制
    let _ = api::mc_parse_response_header_3e(data);
    let serial = if data.len() >= 2 {
        u16::from_le_bytes([data[0], data[1]])
    } else {
        0
    };
    let _ = api::mc_parse_response_header_4e(data, serial);
    // 序列号不匹配分支（真实串包场景）
    let _ = api::mc_parse_response_header_4e(data, serial.wrapping_add(1));

    // 2) 1E 响应头：数据长度由请求点数推导，取边界点数
    if data.len() >= 2 {
        let func = data[0] & 0x7F;
        for points in [0u16, 1, 479, 1916, u16::MAX] {
            for (is_read, is_bit) in [(true, false), (true, true), (false, false)] {
                let _ = api::mc_parse_response_header_1e(data, func, is_read, is_bit, points);
            }
        }
    }

    // 3) 帧体长度夹取：<2 装不下结束代码，>960 是异常长度
    for len in [0usize, 1, 2, 960, 961, usize::MAX] {
        let _ = api::mc_body_len_checked(len);
    }

    // 4) 值解码与结束码（对端字节直接进 decode_words / decode_bit）
    for dt in 0..8u8 {
        let _ = api::mc_decode_words(data, dt);
        // 编码只调用不 round-trip：F32 转换会损精度，语义等价由 codec 单测钉住
        let _ = api::mc_encode_words(1.5, dt);
    }
    for index in [0usize, 1, 7, data.len()] {
        let _ = api::mc_decode_bit(data, index);
    }
    let _ = api::mc_check_end_code(u16::from_le_bytes([
        *data.first().unwrap_or(&0),
        *data.get(1).unwrap_or(&0),
    ]));
});
