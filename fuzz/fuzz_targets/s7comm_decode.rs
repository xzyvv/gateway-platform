#![no_main]
//! S7comm 响应解码 fuzz（生产解析器直连，见 `fuzz_api`）：
//! 1. 单点 Read Var 响应 + `decode_value` 切片边界（7 种数据类型）；
//! 2. 多 item Read Var 响应（批量路径，item 数 0..=8）；
//! 3. SZL 响应（诊断路径）与 Write Var 响应。
//!
//! 覆盖的是「对端字节不可信」这一面：长度域与真实长度不符、PDU 类型错、
//! Error class/code 异常、多 item 块数与 items 数量不一致等。
//! 只要求不 panic、不越界；语义正确性由 golden 样本与帧级单测兜底。

use gateway_core::fuzz_api as api;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 1) 单点读：同一份字节喂全部数据类型（不同类型走不同字节序/长度分支）
    for dt in 0..7u8 {
        let _ = api::s7_parse_read_response(data, dt);
        let _ = api::s7_decode_value(data, dt);
    }

    // 2) 批量读：item 数包含 0 与多块响应不匹配的情形
    for items in 0..=8u8 {
        let _ = api::s7_parse_read_response_multi(data, items);
    }

    // 3) 诊断与写回
    let _ = api::s7_parse_szl_response(data);
    let _ = api::s7_parse_write_response(data);
});
