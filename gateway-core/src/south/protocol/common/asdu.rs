//! IEC 60870-5 ASDU（应用服务数据单元）共享编解码
//!
//! ASDU 层与具体传输链路无关：IEC 60870-5-104（TCP/APCI）与 IEC 60870-5-101
//! （串行 FT1.2 链路）使用**完全相同**的 ASDU 结构。故把 ASDU 的构造与解析
//! 上提到中立公共模块，供两个协议复用，避免实现分叉后行为不一致。
//!
//! 本模块在 `iec104` 与 `iec101` 两个 feature 至少启用一个时编译
//! （见 `common/mod.rs` 的 cfg 门控），两者都关闭时不参与编译。

use std::collections::HashMap;

use crate::engine::model::Quality;

/// ASDU 类型标识
pub(crate) const TYPE_M_ME_NC_1: u8 = 14; // 短浮点数测量值
pub(crate) const TYPE_M_ME_NA_1: u8 = 9; // 归一化测量值
pub(crate) const TYPE_M_ME_NB_1: u8 = 13; // 标度化测量值
pub(crate) const TYPE_M_SP_NA_1: u8 = 1; // 单点信息
pub(crate) const TYPE_C_IC_NA_1: u8 = 100; // 总召唤命令
/// 对时命令（时钟同步，决策表 #19）——仅 iec104 实现对时下发，iec101 未使用
#[cfg(feature = "iec104")]
pub(crate) const TYPE_C_CS_NA_1: u8 = 103;

/// 控制方向命令类型（决策表 #18）
#[cfg(feature = "iec104")]
pub(crate) const TYPE_C_SC_NA_1: u8 = 45; // 单点命令
#[cfg(feature = "iec104")]
pub(crate) const TYPE_C_DC_NA_1: u8 = 46; // 双点命令
#[cfg(feature = "iec104")]
pub(crate) const TYPE_C_RC_NA_1: u8 = 50; // 调节步命令

/// 带 CP56Time2a 时标的测量/状态类型（决策表 #17：电力 SOE 必需）
pub(crate) const TYPE_M_SP_TB_1: u8 = 30; // 单点信息 + 7 字节时标
pub(crate) const TYPE_M_ME_TD_1: u8 = 32; // 归一化值 + 7 字节时标
pub(crate) const TYPE_M_ME_TE_1: u8 = 33; // 标度化值 + 7 字节时标
pub(crate) const TYPE_M_ME_TF_1: u8 = 34; // 短浮点值 + 7 字节时标

/// 总召唤命令限定词（QOI）：站召唤
pub(crate) const QOI_STATION: u8 = 0x14;

/// 传送原因（COT）：激活终止
pub(crate) const COT_ACTIVATION_TERMINATION: u16 = 10;

/// 传送原因（COT）：激活——仅 iec104 的控制/对时命令使用
#[cfg(feature = "iec104")]
pub(crate) const COT_ACTIVATION: u16 = 6;

/// 传送原因（COT）：激活确认（决策表 #18 控制命令的肯定确认）
#[cfg(feature = "iec104")]
pub(crate) const COT_ACTIVATION_CONFIRMATION: u16 = 7;

/// 传送原因（COT）：未知类型标识 / 未知传送原因 / 未知公共地址 / 未知信息对象地址
///
/// 决策表 #18：控制命令被从站拒绝（否定确认/未知原因）时返回这些 COT。
#[cfg(feature = "iec104")]
pub(crate) const COT_UNKNOWN_TYPE_ID: u16 = 44;
#[cfg(feature = "iec104")]
pub(crate) const COT_UNKNOWN_COT: u16 = 45;
#[cfg(feature = "iec104")]
pub(crate) const COT_UNKNOWN_CA: u16 = 46;
#[cfg(feature = "iec104")]
pub(crate) const COT_UNKNOWN_IOA: u16 = 47;

/// CP56Time2a 时标长度（字节）
pub(crate) const CP56TIME2A_LEN: usize = 7;

/// 公共地址（CA）字段字节数（决策表 #28）
///
/// IEC 60870-5 的公共地址长度由规约参数协商：`addr_len == 1` 用单字节，
/// 其余（含默认 2）按标准 2 字节处理。
pub(crate) fn ca_len(addr_len: u8) -> usize {
    if addr_len == 1 {
        1
    } else {
        2
    }
}

/// 按 `addr_len` 编码公共地址 CA（小端，低字节在前）。
///
/// 单字节 CA 只取低 8 位；2 字节 CA 按 `u16` 小端输出。
pub(crate) fn encode_ca(asdu_addr: u16, addr_len: u8) -> Vec<u8> {
    if ca_len(addr_len) == 1 {
        vec![(asdu_addr & 0xFF) as u8]
    } else {
        asdu_addr.to_le_bytes().to_vec()
    }
}

/// 构建总召唤 ASDU (Type 100)
///
/// `addr_len` 为公共地址字节数（1 或 2，见 [`ca_len`]）。
pub(crate) fn build_interrogation_asdu(asdu_addr: u16, addr_len: u8) -> Vec<u8> {
    let mut asdu = vec![TYPE_C_IC_NA_1, 0x01, 0x06, 0x00];
    asdu.extend_from_slice(&encode_ca(asdu_addr, addr_len));
    // Bug C6: C_IC_NA_1 的 ASDU 必含 3 字节信息体地址（IOA），站召唤固定为 0。
    // 原实现漏掉 IOA，ASDU 只有 7 字节，真实从站会把 QOI 当作 IOA 首字节而拒收/不响应。
    asdu.extend_from_slice(&[0x00, 0x00, 0x00]); // IOA = 0（站召唤）
    asdu.push(QOI_STATION); // QOI（Qualifier of Interrogation）= 站召唤
    asdu
}

/// CP56Time2a 时标编解码（决策表 #17）
///
/// 7 字节结构（小端）：
/// `[0..2]` 分内毫秒（0..59999，= 秒×1000 + 毫秒）
/// `[2]`    分（低 6 位，bit7 = IV 无效标志）
/// `[3]`    时（低 5 位，bit5/6 = 夏令时标志）
/// `[4]`    日（低 5 位，bit5-7 = 星期）
/// `[5]`    月（低 4 位）
/// `[6]`    年（低 7 位，相对 2000 年）
///
/// 编解码以 UTC 为基准；夏令时位不参与编码（网关统一按 UTC 存储）。
/// 解码对 IV 位采取宽容策略：仍返回时间，避免个别从站置 IV 导致 SOE 时标整体丢失。
///
/// 编码方向仅 iec104 的对时命令使用（iec101 不下发对时命令）。
#[cfg(feature = "iec104")]
pub(crate) fn encode_cp56time2a(dt: &chrono::DateTime<chrono::Utc>) -> [u8; CP56TIME2A_LEN] {
    use chrono::{Datelike, Timelike};
    let ms_in_minute = (dt.second() * 1000 + dt.timestamp_subsec_millis()) as u16;
    let year2 = (dt.year().rem_euclid(100)) as u8;
    [
        (ms_in_minute & 0xFF) as u8,
        (ms_in_minute >> 8) as u8,
        (dt.minute() & 0x3F) as u8,
        (dt.hour() & 0x1F) as u8,
        (dt.day() & 0x1F) as u8,
        (dt.month() & 0x0F) as u8,
        (year2 & 0x7F),
    ]
}

/// 解析 CP56Time2a 时标为 Unix 毫秒；字段越界/非法组合返回 `None`
///
/// 供 time-tagged 测量类型（30/32/33/34）填充 `source_ts` 使用。
pub(crate) fn decode_cp56time2a_millis(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < CP56TIME2A_LEN {
        return None;
    }
    let ms_in_minute = u16::from_le_bytes([bytes[0], bytes[1]]) as u32;
    // 规范：分内毫秒 0..59999（60s × 1000）；超范围视为非法时标
    if ms_in_minute > 59_999 {
        return None;
    }
    let minute = (bytes[2] & 0x3F) as u32;
    let hour = (bytes[3] & 0x1F) as u32;
    let day = (bytes[4] & 0x1F) as u32;
    let month = (bytes[5] & 0x0F) as u32;
    let year = 2000 + (bytes[6] & 0x7F) as i32;
    if minute > 59 || hour > 23 || day == 0 || month == 0 || month > 12 {
        return None;
    }
    let sec = ms_in_minute / 1000;
    let millis = (ms_in_minute % 1000) as i64;
    use chrono::TimeZone;
    let dt = chrono::Utc
        .with_ymd_and_hms(year, month, day, hour, minute, sec)
        .single()?;
    let ts = dt.timestamp_millis() + millis;
    if ts < 0 {
        return None;
    }
    Some(ts as u64)
}

/// B3 修复：解码带时标类型（30/32/33/34）的 CP56Time2a，失败时记录告警再回退。
///
/// 回退值 `0` 是本模块**文档化的"无时标"哨兵**（见 [`parse_asdu`] 文档），
/// 不带时标的类型同样填 0。旧实现直接 `unwrap_or(0)` 会导致：设备确实发了时标、
/// 但字段非法时，SOE 时间被静默丢弃且无任何报警，事后无法排查。此处补一条 warn
/// 保留可观测性，行为语义不变（仍回退为"无时标"，不产生假的 1970 时标）。
fn decode_ts_or_warn(bytes: &[u8], ioa: u32) -> u64 {
    match decode_cp56time2a_millis(bytes) {
        Some(ms) => ms,
        None => {
            tracing::warn!(
                ioa,
                "invalid CP56Time2a timestamp, falling back to no-timestamp"
            );
            0
        }
    }
}

/// 构建对时 ASDU（Type 103，C_CS_NA_1，决策表 #19）
///
/// 结构：类型(1) + VSQ(1) + COT+OA(2，OA=0) + 公共地址(1/2) + IOA(3) + CP56Time2a(7)。
/// 注：传送原因字段的第二字节即源发地址（OA），不额外占位。
///
/// 仅 iec104 下发对时命令，iec101 链路未实现该命令。
#[cfg(feature = "iec104")]
pub(crate) fn build_clock_sync_asdu(
    asdu_addr: u16,
    addr_len: u8,
    dt: &chrono::DateTime<chrono::Utc>,
) -> Vec<u8> {
    let mut asdu = vec![TYPE_C_CS_NA_1, 0x01];
    asdu.extend_from_slice(&COT_ACTIVATION.to_le_bytes()); // [COT=6, OA=0]
    asdu.extend_from_slice(&encode_ca(asdu_addr, addr_len)); // 公共地址（1/2 字节）
    asdu.extend_from_slice(&[0x00, 0x00, 0x00]); // IOA = 0（时钟同步）
    asdu.extend_from_slice(&encode_cp56time2a(dt));
    asdu
}

/// 构建控制方向命令 ASDU（决策表 #18）
///
/// 结构：类型(1) + VSQ(1) + COT+OA(2，COT=激活/OA=0) + 公共地址(1/2) + IOA(3) + 命令值(1)。
/// `obj` 为单点/双点/调节步命令对象字节（含执行位），由 [`encode_command_object`] 生成。
#[cfg(feature = "iec104")]
pub(crate) fn build_command_asdu(
    type_id: u8,
    asdu_addr: u16,
    addr_len: u8,
    ioa: u32,
    obj: u8,
) -> Vec<u8> {
    let mut asdu = vec![type_id, 0x01];
    asdu.extend_from_slice(&COT_ACTIVATION.to_le_bytes()); // [COT=6, OA=0]
    asdu.extend_from_slice(&encode_ca(asdu_addr, addr_len)); // 公共地址（1/2 字节）
    asdu.extend_from_slice(&ioa.to_le_bytes()[..3]); // IOA（低 3 字节）
    asdu.push(obj);
    asdu
}

/// 编码控制命令对象字节（决策表 #18）
///
/// 三型命令对象均为 1 字节，bit7 = S/E（0=选择 / 1=执行，本实现统一置执行）：
/// - 单点 SCO：bit0 = 状态（0=分 / 1=合）
/// - 双点 DCO：bit0-1 = DCS（1=分 / 2=合）
/// - 调节步 RCO：bit0-1 = RCS（1=降 / 2=升）
///
/// `value` 语义：`0` → 分/降（DCS/RCS=1），非 `0` → 合/升（=2）。
#[cfg(feature = "iec104")]
pub(crate) fn encode_command_object(type_id: u8, value: f64) -> u8 {
    const SE_EXECUTE: u8 = 0x80;
    match type_id {
        TYPE_C_SC_NA_1 => {
            if value != 0.0 {
                SE_EXECUTE | 0x01
            } else {
                SE_EXECUTE
            }
        }
        TYPE_C_DC_NA_1 | TYPE_C_RC_NA_1 => SE_EXECUTE | if value <= 0.0 { 0x01 } else { 0x02 },
        _ => SE_EXECUTE,
    }
}

/// 是否为控制方向命令类型（45/46/50，决策表 #18）
#[cfg(feature = "iec104")]
pub(crate) fn is_command_type(type_id: u8) -> bool {
    matches!(type_id, TYPE_C_SC_NA_1 | TYPE_C_DC_NA_1 | TYPE_C_RC_NA_1)
}

/// 解析控制命令确认报文，返回 `(类型标识, IOA, COT)`
///
/// 从站对命令的肯定确认为 COT=激活确认(7)/激活终止(10)；否定/未知原因为
/// COT=未知类型(44)/未知传送原因(45)/未知公共地址(46)/未知信息对象地址(47)。
/// 非命令类型或长度不足返回 `None`。
#[cfg(feature = "iec104")]
pub(crate) fn parse_command_confirmation(asdu: &[u8], addr_len: u8) -> Option<(u8, u32, u16)> {
    // 头部：类型(1) + VSQ(1) + COT(1) + OA(1) = 4 字节，其后为 CA(1/2) + IOA(3) + 命令值(1)
    let ioa_start = 4 + ca_len(addr_len);
    if asdu.len() < ioa_start + 4 {
        return None;
    }
    let type_id = asdu[0];
    if !is_command_type(type_id) {
        return None;
    }
    let cot = asdu_cot(asdu)?;
    let ioa = u32::from_le_bytes([asdu[ioa_start], asdu[ioa_start + 1], asdu[ioa_start + 2], 0]);
    Some((type_id, ioa, cot))
}

/// 解析归一化值 (-1..1) 为 f64
pub(crate) fn parse_normalized(bytes: &[u8]) -> f64 {
    let raw = i16::from_le_bytes([bytes[0], bytes[1]]);
    raw as f64 / 32768.0
}

/// 解析标度化值
pub(crate) fn parse_scaled(bytes: &[u8]) -> f64 {
    i16::from_le_bytes([bytes[0], bytes[1]]) as f64
}

/// 解析 IEEE 754 短浮点
pub(crate) fn parse_float(bytes: &[u8]) -> f64 {
    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
}

/// IOA 快照表：信息体地址 -> `(value, quality, source_ts_millis)`。
/// `source_ts_millis` 仅对带 CP56Time2a 时标的类型（30/32/33/34）非 0，其余填 0。
/// （iec101/iec104 的总召唤快照与本解析函数共用此一类型别名）
pub(crate) type IoaDataMap = HashMap<u32, (f64, Quality, u64)>;

/// 解析 ASDU，提取测量值写入 data_map
///
/// data_map 值三元组为 `(value, quality, source_ts_millis)`；`source_ts_millis`
/// 仅对带 CP56Time2a 时标的类型（30/32/33/34）非 0，其余类型填 0（表示无时标）。
///
/// `addr_len` 为公共地址（CA）字节数（1 或 2，见 [`ca_len`]）；信息体地址（IOA）
/// 恒为 3 字节，不受其影响。
pub(crate) fn parse_asdu(asdu: &[u8], data_map: &mut IoaDataMap, addr_len: u8) {
    let header_len = 4 + ca_len(addr_len);
    if asdu.len() < header_len {
        return;
    }
    let type_id = asdu[0];
    let vsq = asdu[1];
    let num_objects = vsq & 0x7F;
    if num_objects == 0 {
        return;
    }
    // SQ 位（bit7）=1 时，只有第一个信息体包含 IOA（3 字节），
    // 后续信息体的 IOA 递增 1 且不带 IOA 字段。
    let sq = (vsq & 0x80) != 0;
    let _cot = u16::from_le_bytes([asdu[2], asdu[3]]);

    let mut pos = header_len;
    let mut current_ioa: Option<u32> = None;
    for _ in 0..num_objects {
        if sq {
            if current_ioa.is_none() {
                if pos + 3 > asdu.len() {
                    break;
                }
                current_ioa = Some(u32::from_le_bytes([
                    asdu[pos],
                    asdu[pos + 1],
                    asdu[pos + 2],
                    0,
                ]));
                pos += 3;
            } else {
                // A7 修复：旧实现 `current_ioa.map_or(1, |x| x + 1)` 在循环不变式被破坏时
                // （本轮 IOA 截断但 current_ioa 仍为 None）会把 IOA 强设为 1，后续所有对象
                // **错位映射到 IOA 1**——整批数据被错误归位，现场表现为"所有点值相同"。
                // SQ=1 路径下 current_ioa 必为 Some（首轮 IOA 截断已在 if 分支 break），
                // 理论不可达 None；此处防御性处理：None 时直接 break 放弃剩余对象，绝不静默错位。
                match current_ioa {
                    Some(x) => current_ioa = Some(x + 1),
                    None => break,
                }
            }
        } else {
            if pos + 3 > asdu.len() {
                break;
            }
            current_ioa = Some(u32::from_le_bytes([
                asdu[pos],
                asdu[pos + 1],
                asdu[pos + 2],
                0,
            ]));
            pos += 3;
        }
        let ioa = match current_ioa {
            Some(v) => v,
            None => break,
        };

        let obj_len = match type_id {
            TYPE_M_ME_NC_1 => {
                if pos + 5 > asdu.len() {
                    break;
                }
                let v = parse_float(&asdu[pos..pos + 4]);
                let quality = asdu[pos + 4];
                let q = if quality & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                data_map.insert(ioa, (v, q, 0));
                5
            }
            TYPE_M_ME_NA_1 => {
                if pos + 3 > asdu.len() {
                    break;
                }
                let v = parse_normalized(&asdu[pos..pos + 2]);
                let quality = asdu[pos + 2];
                let q = if quality & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                data_map.insert(ioa, (v, q, 0));
                3
            }
            TYPE_M_ME_NB_1 => {
                if pos + 3 > asdu.len() {
                    break;
                }
                let v = parse_scaled(&asdu[pos..pos + 2]);
                let quality = asdu[pos + 2];
                let q = if quality & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                data_map.insert(ioa, (v, q, 0));
                3
            }
            TYPE_M_SP_NA_1 => {
                if pos + 1 > asdu.len() {
                    break;
                }
                let v = (asdu[pos] & 0x01) as f64;
                let q = if asdu[pos] & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                data_map.insert(ioa, (v, q, 0));
                1
            }
            // 带 CP56Time2a 时标的类型（决策表 #17）：值 + 时标，时标写入 source_ts
            TYPE_M_ME_TF_1 => {
                if pos + 12 > asdu.len() {
                    break;
                }
                let v = parse_float(&asdu[pos..pos + 4]);
                let q = if asdu[pos + 4] & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                let ts = decode_ts_or_warn(&asdu[pos + 5..pos + 12], ioa);
                data_map.insert(ioa, (v, q, ts));
                12
            }
            TYPE_M_ME_TD_1 => {
                if pos + 10 > asdu.len() {
                    break;
                }
                let v = parse_normalized(&asdu[pos..pos + 2]);
                let q = if asdu[pos + 2] & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                let ts = decode_ts_or_warn(&asdu[pos + 3..pos + 10], ioa);
                data_map.insert(ioa, (v, q, ts));
                10
            }
            TYPE_M_ME_TE_1 => {
                if pos + 10 > asdu.len() {
                    break;
                }
                let v = parse_scaled(&asdu[pos..pos + 2]);
                let q = if asdu[pos + 2] & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                let ts = decode_ts_or_warn(&asdu[pos + 3..pos + 10], ioa);
                data_map.insert(ioa, (v, q, ts));
                10
            }
            TYPE_M_SP_TB_1 => {
                if pos + 8 > asdu.len() {
                    break;
                }
                let v = (asdu[pos] & 0x01) as f64;
                // 单点带时标：bit7 = IV；质量按 SIQ 的 BL/SB/IV 组合判定
                let q = if asdu[pos] & 0x80 == 0 {
                    Quality::Good
                } else {
                    Quality::Bad
                };
                let ts = decode_ts_or_warn(&asdu[pos + 1..pos + 8], ioa);
                data_map.insert(ioa, (v, q, ts));
                8
            }
            _ => return,
        };
        pos += obj_len;
    }
}

/// 读取 ASDU 的传送原因（COT，低 6 位，低字节在前）
pub(crate) fn asdu_cot(asdu: &[u8]) -> Option<u16> {
    if asdu.len() < 4 {
        return None;
    }
    Some(u16::from_le_bytes([asdu[2], asdu[3]]) & 0x3F)
}

/// 判断 ASDU 是否为总召唤激活终止报文（类型 100 且 COT=激活终止）
///
/// IEC101 主站在轮询二级数据时据此判定本次总召唤数据已全部收齐。
pub(crate) fn is_interrogation_termination(asdu: &[u8]) -> bool {
    matches!(asdu.first().copied(), Some(TYPE_C_IC_NA_1))
        && asdu_cot(asdu) == Some(COT_ACTIVATION_TERMINATION)
}
