//! IEC 104 协议参数：时序 / 控制域常量与功能码映射

use std::time::Duration;

use super::frames::{TYPE_C_DC_NA_1, TYPE_C_RC_NA_1, TYPE_C_SC_NA_1};

/// C-12: U-frame TESTFR_ACT 控制域（主动探测）
///
/// IEC 104 U-frame 控制域第 1 字节：b0/b1 恒为 1（与 I/S 格式区分），
/// 功能位 one-hot：b2 = STARTDT act(0x07) / b3 = STARTDT con(0x0B) /
/// b4 = STOPDT act(0x13) / b5 = STOPDT con(0x23) / b6 = TESTFR act(0x43) /
/// b7 = TESTFR con(0x83)。
pub(crate) const TESTFR_ACT_CF: [u8; 4] = [0x43, 0x00, 0x00, 0x00];

/// C-12: U-frame TESTFR_CON 控制域（对 server 探测的确认）
///
/// 注意 act 与 con 是**不同**字节：旧实现把 0x83（con）当探测帧发出，
/// server 不会对 con 回 con，导致每轮探测都超时误判断连。
pub(crate) const TESTFR_CON_CF: [u8; 4] = [0x83, 0x00, 0x00, 0x00];

/// C-12: keepalive 探测周期（IEC 104 t3 默认 20s，取 10s 更早发现链路异常）
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// C-12: 单次 keepalive 探测响应等待上限（小于 IEC 104 t1 = 15s）
pub(crate) const KEEPALIVE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// C-12: 连续探测失败容忍次数（避免网络抖动导致连接抖动）
pub(crate) const KEEPALIVE_MAX_CONSECUTIVE_FAILURES: u32 = 2;

/// 每收到多少个 I-frame 回一次 S-frame（确认 N(R)）
///
/// IEC 104 规定发送方未确认 I-frame 上限 k = 12（默认）、确认超时 t2 = 10s。
/// 取 8 < k，保证在 server 发送窗口打满前就释放，避免 server 停发甚至断链。
pub(crate) const ACK_EVERY_N_FRAMES: u16 = 8;

/// 决策表 #19：对时应答等待上限。响应/无响应均不阻断建链（best-effort）。
pub(crate) const CLOCK_SYNC_TIMEOUT: Duration = Duration::from_secs(2);

/// 决策表 #18：由点位功能码映射控制命令类型
///
/// 46 → 双点命令（C_DC_NA_1）；50 → 调节步命令（C_RC_NA_1）；
/// 其余（含 45 单点命令与未指定）→ 单点命令（C_SC_NA_1）。
pub(crate) fn command_type_of(function_code: Option<u8>) -> u8 {
    match function_code {
        Some(46) => TYPE_C_DC_NA_1,
        Some(50) => TYPE_C_RC_NA_1,
        _ => TYPE_C_SC_NA_1,
    }
}
