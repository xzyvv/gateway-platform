#![no_main]
//! 欧姆龙 FINS 响应解码 fuzz：
//! 1. 存储区读/写应答（命令码回显校验 + MRES/SRES 结束码）；
//! 2. 只读结束码入口（调试面用，命令码不匹配须上抛）；
//! 3. 节点地址应答与字序列解码。
//!
//! 命令码同时喂「匹配」与「不匹配」两组，保证串包/错帧分支被覆盖。

use gateway_core::fuzz_api as api;
use libfuzzer_sys::fuzz_target;

const MRC_READ: u8 = 0x01;
const SRC_READ: u8 = 0x01;
const MRC_WRITE: u8 = 0x01;
const SRC_WRITE: u8 = 0x02;

fuzz_target!(|data: &[u8]| {
    // 1) 读应答：命令码匹配（进入结束码/数据区解析）与不匹配（错帧分支）
    let _ = api::fins_parse_read_response(data, MRC_READ, SRC_READ);
    let _ = api::fins_parse_read_end_code(data);
    let mismatch = data.first().copied().unwrap_or(0);
    let _ = api::fins_parse_read_response(data, mismatch, SRC_READ);

    // 2) 写应答
    let _ = api::fins_parse_write_response(data, MRC_WRITE, SRC_WRITE);
    let _ = api::fins_parse_write_response(data, mismatch, SRC_WRITE);

    // 3) 节点地址应答（握手路径）+ 字序列解码
    let _ = api::fins_parse_node_addr_response(data);
    for dt in 0..8u8 {
        let _ = api::fins_decode_words(data, dt);
    }
});
