#![no_main]
//! BACnet MS/TP 帧解码 fuzz：
//! 1. 随机/畸形帧直接进解析器 —— 只要求不 panic、不越界；
//! 2. 解帧 —— 前导码撕裂、长度域超限、CRC 错时不得死循环；
//! 3. 合法帧 round-trip —— build → parse 必须还原 (type, dest, src, data)。
//!
//! 注意：帧类型常量写错（例如把 PollForMaster 的 0x02 写成 0x03）属于语义错误，
//! fuzz 抓不到，cargo-mutants 也不生成 const 变异体——靠 golden 样本与
//! `mstp_frame_type_constants_match_ashrae_135_table_9_1` 的字面量断言兜底；
//! fuzz 负责的是健壮性面（panic / 越界 / 死循环）。

use gateway_core::engine::metrics::Metrics;
use gateway_core::fuzz_api as api;
use gateway_core::south::bus::FrameBuffer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let metrics = Metrics::new();

    // 1) 畸形帧
    let _ = api::mstp_parse_frame(data, &metrics);

    // 2) 解帧：固定上限兜底，防止「不消费也不返回」的循环
    let mut fb = FrameBuffer::new();
    fb.push(data);
    for _ in 0..64 {
        if api::mstp_extract_frame(&mut fb, None).is_none() {
            break;
        }
    }

    // 3) 合法帧 round-trip（数据区截到协议上限，否则解析侧按长度拒绝）
    if data.len() >= 3 {
        let body = &data[3..];
        let body = &body[..body.len().min(api::mstp_max_data_len())];
        let frame = api::mstp_build_frame(data[0], data[1], data[2], body);
        let (ft, dest, src, parsed) =
            api::mstp_parse_frame(&frame, &metrics).expect("自建 MS/TP 帧必须可解析");
        assert_eq!((ft, dest, src), (data[0], data[1], data[2]));
        assert_eq!(parsed, body, "MS/TP 数据区 round-trip 必须逐字节一致");
    }
});
