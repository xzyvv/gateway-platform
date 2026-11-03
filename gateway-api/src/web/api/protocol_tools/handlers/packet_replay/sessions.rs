//! 预置回放会话静态数据
//!
//! 预置会话（s1/s2）由 handler 内置返回，不落盘、不可删/不可覆盖；
//! 用户保存的会话由 `replay_store` 管理并在列表接口中合并。

use super::now_ms;
use super::{ReplayPacket, ReplaySession};

pub(super) fn build_sessions() -> Vec<ReplaySession> {
    let now = now_ms();
    vec![
        ReplaySession {
            id: "s1".into(),
            name: "电力仪表#1 正常采集".into(),
            device_name: "电力仪表#1".into(),
            duration_ms: 5000,
            packet_count: 10,
            created_at_ms: now - 3600000,
            packets: vec![
                ReplayPacket {
                    id: "p1".into(),
                    time_ms: 0,
                    direction: "request".into(),
                    hex: "01 03 00 00 00 02".into(),
                    parsed: "读寄存器 0-1".into(),
                },
                ReplayPacket {
                    id: "p2".into(),
                    time_ms: 25,
                    direction: "response".into(),
                    hex: "01 03 04 00 DC 00 34".into(),
                    parsed: "220, 52".into(),
                },
                ReplayPacket {
                    id: "p3".into(),
                    time_ms: 1000,
                    direction: "request".into(),
                    hex: "01 03 00 00 00 02".into(),
                    parsed: "读寄存器 0-1".into(),
                },
                ReplayPacket {
                    id: "p4".into(),
                    time_ms: 1028,
                    direction: "response".into(),
                    hex: "01 03 04 00 DD 00 35".into(),
                    parsed: "221, 53".into(),
                },
                ReplayPacket {
                    id: "p5".into(),
                    time_ms: 2000,
                    direction: "request".into(),
                    hex: "01 03 00 00 00 02".into(),
                    parsed: "读寄存器 0-1".into(),
                },
                ReplayPacket {
                    id: "p6".into(),
                    time_ms: 2030,
                    direction: "response".into(),
                    hex: "01 03 04 00 DB 00 33".into(),
                    parsed: "219, 51".into(),
                },
                ReplayPacket {
                    id: "p7".into(),
                    time_ms: 3000,
                    direction: "request".into(),
                    hex: "01 03 00 00 00 02".into(),
                    parsed: "读寄存器 0-1".into(),
                },
                ReplayPacket {
                    id: "p8".into(),
                    time_ms: 3022,
                    direction: "response".into(),
                    hex: "01 03 04 00 DC 00 34".into(),
                    parsed: "220, 52".into(),
                },
                ReplayPacket {
                    id: "p9".into(),
                    time_ms: 4000,
                    direction: "request".into(),
                    hex: "01 03 00 00 00 02".into(),
                    parsed: "读寄存器 0-1".into(),
                },
                ReplayPacket {
                    id: "p10".into(),
                    time_ms: 4025,
                    direction: "response".into(),
                    hex: "01 03 04 00 DE 00 35".into(),
                    parsed: "222, 53".into(),
                },
            ],
        },
        ReplaySession {
            id: "s2".into(),
            name: "PLC#4 离线故障".into(),
            device_name: "PLC#4".into(),
            duration_ms: 3000,
            packet_count: 6,
            created_at_ms: now - 7200000,
            packets: vec![
                ReplayPacket {
                    id: "p1".into(),
                    time_ms: 0,
                    direction: "request".into(),
                    hex: "04 03 00 00 00 04".into(),
                    parsed: "读寄存器 0-3".into(),
                },
                ReplayPacket {
                    id: "p2".into(),
                    time_ms: 1000,
                    direction: "response".into(),
                    hex: "".into(),
                    parsed: "超时无响应".into(),
                },
                ReplayPacket {
                    id: "p3".into(),
                    time_ms: 1000,
                    direction: "request".into(),
                    hex: "04 03 00 00 00 04".into(),
                    parsed: "重试1".into(),
                },
                ReplayPacket {
                    id: "p4".into(),
                    time_ms: 2000,
                    direction: "response".into(),
                    hex: "".into(),
                    parsed: "超时无响应".into(),
                },
                ReplayPacket {
                    id: "p5".into(),
                    time_ms: 2000,
                    direction: "request".into(),
                    hex: "04 03 00 00 00 04".into(),
                    parsed: "重试2".into(),
                },
                ReplayPacket {
                    id: "p6".into(),
                    time_ms: 3000,
                    direction: "response".into(),
                    hex: "".into(),
                    parsed: "超时无响应，放弃".into(),
                },
            ],
        },
    ]
}
