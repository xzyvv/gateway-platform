//! 抓包 / 回发的统一帧解读（DD-29）
//!
//! 背景：报文捕获页过去只认 Modbus，报文编辑页另有一套解读，同一帧在两个页面
//! 可能得到不同结论；而 Modbus 的 MBAP 头靠「第 3/4 字节是否为 0」猜测，8 字节
//! 以上的 RTU 帧（如 FC16 写多寄存器）会被误判成 TCP，从站地址与功能码整体错位。
//!
//! 现在只有这一份实现：
//! - 传输形态由总线配置（`tcp_addr` 是否存在）决定，不嗅探；
//! - 支持 modbus（TCP / RTU）、dlt645、cjt188 的文本解读；
//! - 其余协议明确返回「本协议暂无解析器」，不输出可能误导的猜测。

use crate::web::debug_bus::{exception_text, verify_modbus_rtu, FrameCheck};

/// Modbus 应答判定结果（RTU 看 CRC，TCP 看 MBAP 头）。
pub(crate) enum FrameVerdict {
    Ok,
    Exception { code: u8, text: String },
    Invalid(String),
}

/// 单帧解读（报文捕获：不知道对应请求，只描述这一帧）。
///
/// `protocol` 为总线配置的协议名；`is_tcp` 表示该总线走 TCP（Modbus 才有 MBAP 头）；
/// `variant` 为总线生效变体（DLT645 用 1997 / 2007 区分地址字节序）。
pub(crate) fn describe(
    protocol: &str,
    is_tcp: bool,
    variant: Option<&str>,
    bytes: &[u8],
    is_tx: bool,
) -> String {
    let p = protocol.to_ascii_lowercase();
    if p.starts_with("modbus") {
        modbus_single(is_tcp, bytes, is_tx)
    } else if p.starts_with("dlt645") {
        dlt645(bytes, variant)
    } else if p.starts_with("cjt188") {
        cjt188(bytes)
    } else {
        format!("{protocol} 帧（本协议暂无解析器，见 hex）")
    }
}

/// 请求 + 应答的成对解读（报文编辑：把「发了什么 / 回了什么」放在同一行）。
pub(crate) fn describe_exchange(
    protocol: &str,
    is_tcp: bool,
    variant: Option<&str>,
    request: &[u8],
    response: &[u8],
) -> String {
    let p = protocol.to_ascii_lowercase();
    if !p.starts_with("modbus") {
        return describe(protocol, is_tcp, variant, response, false);
    }
    format!(
        "{} → {}",
        modbus_request(is_tcp, request),
        modbus_single(is_tcp, response, false)
    )
}

/// 请求侧宽容解读：用户可能贴的是不带 CRC 的 PDU（内置预置模板即如此），
/// 不能因为「CRC 缺失」就把整条请求标成失败；带合法 CRC 时才剥掉尾部 2 字节。
fn modbus_request(is_tcp: bool, bytes: &[u8]) -> String {
    if is_tcp {
        if bytes.len() < 8 {
            return "请求帧过短，无法解析".into();
        }
        return modbus_summary(bytes[6], bytes[7], &bytes[8..], true);
    }
    if bytes.len() < 2 {
        return "请求帧过短，无法解析".into();
    }
    let framed = bytes.len() >= 5 && !matches!(verify_modbus_rtu(bytes), FrameCheck::Crc);
    let end = if framed { bytes.len() - 2 } else { bytes.len() };
    modbus_summary(bytes[0], bytes[1], &bytes[2..end], true)
}

/// Modbus 应答是否可用：RTU 先查 CRC 再看异常码；TCP 先查 MBAP 头再看异常码。
pub(crate) fn modbus_verdict(is_tcp: bool, response: &[u8]) -> FrameVerdict {
    if !is_tcp {
        if response.len() < 5 {
            return FrameVerdict::Invalid(format!("帧过短（{} 字节），无法解析", response.len()));
        }
        return match verify_modbus_rtu(response) {
            FrameCheck::Ok => FrameVerdict::Ok,
            FrameCheck::Crc => FrameVerdict::Invalid(format!(
                "CRC 校验失败（收到 {} 字节）：波特率/校验位不匹配、线路干扰或帧被截断",
                response.len()
            )),
            FrameCheck::Exception { code, text } => FrameVerdict::Exception { code, text },
        };
    }

    if response.len() < 8 {
        return FrameVerdict::Invalid(format!("MBAP 帧过短（{} 字节），无法解析", response.len()));
    }
    if response[2] != 0 || response[3] != 0 {
        return FrameVerdict::Invalid(format!(
            "MBAP 协议号非 0（0x{:02X}{:02X}），不是合法的 Modbus TCP 帧",
            response[2], response[3]
        ));
    }
    let declared = u16::from_be_bytes([response[4], response[5]]) as usize;
    if declared + 6 != response.len() {
        return FrameVerdict::Invalid(format!(
            "MBAP 长度字段={} 与实际 {} 字节不符（帧被截断或多帧粘连）",
            declared,
            response.len()
        ));
    }
    if response[7] & 0x80 != 0 {
        let code = response[8];
        return FrameVerdict::Exception {
            code,
            text: exception_text(code),
        };
    }
    FrameVerdict::Ok
}

/// 从帧里取 Modbus 从站地址（设备归属匹配用）。非 Modbus / 帧不完整返回 None。
pub(crate) fn modbus_unit_id(is_tcp: bool, bytes: &[u8]) -> Option<u8> {
    if is_tcp {
        if bytes.len() < 8 || bytes[2] != 0 || bytes[3] != 0 {
            return None;
        }
        Some(bytes[6])
    } else {
        // RTU 最短合法帧 5 字节（异常帧），不足则连从站地址都不可信
        if bytes.len() < 5 {
            return None;
        }
        Some(bytes[0])
    }
}

fn modbus_single(is_tcp: bool, bytes: &[u8], is_tx: bool) -> String {
    match modbus_verdict(is_tcp, bytes) {
        FrameVerdict::Invalid(msg) => msg,
        FrameVerdict::Exception { code, text } => {
            let fc = if is_tcp { bytes.get(7) } else { bytes.get(1) }
                .copied()
                .unwrap_or(0);
            format!(
                "异常响应 功能码=0x{:02X} 异常码=0x{:02X}（{text}）",
                fc & 0x7f,
                code
            )
        }
        FrameVerdict::Ok => {
            let Some((slave, fc, data)) = split_modbus(is_tcp, bytes) else {
                return "帧过短，无法解析".into();
            };
            modbus_summary(slave, fc, data, is_tx)
        }
    }
}

/// 拆出（从站地址, 功能码, 数据区）。调用前 `modbus_verdict` 已保证长度足够。
fn split_modbus(is_tcp: bool, bytes: &[u8]) -> Option<(u8, u8, &[u8])> {
    if is_tcp {
        if bytes.len() < 8 {
            return None;
        }
        Some((bytes[6], bytes[7], &bytes[8..]))
    } else {
        if bytes.len() < 4 {
            return None;
        }
        Some((bytes[0], bytes[1], &bytes[2..bytes.len() - 2]))
    }
}

fn modbus_summary(slave: u8, fc_raw: u8, data: &[u8], is_tx: bool) -> String {
    let name = match fc_raw {
        1 => "读线圈",
        2 => "读离散输入",
        3 => "读保持寄存器",
        4 => "读输入寄存器",
        5 => "写单线圈",
        6 => "写单寄存器",
        15 => "写多线圈",
        16 => "写多寄存器",
        _ => "未知功能码",
    };
    if is_tx {
        match fc_raw {
            1..=4 | 15 | 16 if data.len() >= 4 => format!(
                "{name} 从站={slave} 起始={} 数量={}",
                u16_be(data, 0),
                u16_be(data, 2)
            ),
            5 | 6 if data.len() >= 4 => format!(
                "{name} 从站={slave} 地址={} 值={}",
                u16_be(data, 0),
                u16_be(data, 2)
            ),
            _ => format!("{name} 从站={slave}"),
        }
    } else if matches!(fc_raw, 1..=4) {
        if data.is_empty() {
            return format!("{name} 从站={slave}（响应数据为空）");
        }
        let declared = data[0] as usize;
        if data.len() < 1 + declared {
            return format!(
                "{name} 从站={slave}（声明 {declared} 字节，实际 {} 字节，帧截断）",
                data.len() - 1
            );
        }
        let vals: Vec<String> = data[1..1 + declared]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| format!("0x{:02X}{:02X}", c[0], c[1]))
            .collect();
        format!("{name} 从站={slave} 值=[{}]", vals.join(" "))
    } else {
        format!("{name} 从站={slave}（响应）")
    }
}

fn dlt645(bytes: &[u8], variant: Option<&str>) -> String {
    if bytes.len() < 12 {
        return format!("DLT645 帧过短（{} 字节），无法解析", bytes.len());
    }
    if bytes[0] != 0x68 {
        return format!("DLT645 帧头错误（应为 0x68，实际 0x{:02X}）", bytes[0]);
    }
    if bytes[7] != 0x68 {
        return "DLT645 第二个帧头错误（应为 0x68）".into();
    }
    // 2007 标准地址低字节在前，展示时反转为高位在前；1997 变体本身按高字节在前传输
    let mut addr = bytes[1..7].to_vec();
    let is_1997 = variant.map(|v| v.contains("1997")).unwrap_or(false);
    if !is_1997 {
        addr.reverse();
    }
    let ctrl = bytes[8];
    let len = bytes[9] as usize;
    let need = 10 + len + 2;
    if bytes.len() < need {
        return format!(
            "DLT645 帧截断：声明数据 {len} 字节，实际仅 {} 字节",
            bytes.len()
        );
    }
    let cs_pos = 10 + len;
    let cs_ok = checksum8(&bytes[..cs_pos]) == bytes[cs_pos];
    let tail_ok = bytes[cs_pos + 1] == 0x16;
    let kind = if ctrl & 0x80 == 0 {
        "主站请求"
    } else if ctrl & 0x40 != 0 {
        "从站异常应答"
    } else {
        "从站正常应答"
    };
    let mut out = format!(
        "DLT645 地址={} 控制码=0x{ctrl:02X}（{kind}） 数据长度={len} 校验={} 帧尾={}",
        hex_compact(&addr),
        if cs_ok { "通过" } else { "失败" },
        if tail_ok { "0x16" } else { "错误" }
    );
    if ctrl & 0x80 != 0 && ctrl & 0x40 != 0 && len == 1 {
        out.push_str(&format!(" 错误信息字 ERR=0x{:02X}", bytes[10]));
    }
    out
}

fn cjt188(bytes: &[u8]) -> String {
    if bytes.len() < 12 {
        return format!("CJT188 帧过短（{} 字节），无法解析", bytes.len());
    }
    if bytes[0] != 0x68 {
        return format!("CJT188 帧头错误（应为 0x68，实际 0x{:02X}）", bytes[0]);
    }
    // 结构：68 + 地址(7) + C + L + DATA + CS + 16，地址低字节在前，展示时反转
    let mut addr = bytes[1..8].to_vec();
    addr.reverse();
    let ctrl = bytes[8];
    let len = bytes[9] as usize;
    let need = 10 + len + 2;
    if bytes.len() < need {
        return format!(
            "CJT188 帧截断：声明数据 {len} 字节，实际仅 {} 字节",
            bytes.len()
        );
    }
    let cs_pos = 10 + len;
    let cs_ok = checksum8(&bytes[..cs_pos]) == bytes[cs_pos];
    let tail_ok = bytes[cs_pos + 1] == 0x16;
    let kind = if ctrl & 0x80 == 0 {
        "主站请求"
    } else if ctrl & 0x40 != 0 {
        "从站异常应答"
    } else {
        "从站正常应答"
    };
    format!(
        "CJT188 地址={} 控制码=0x{ctrl:02X}（{kind}） 数据长度={len} 校验={} 帧尾={}",
        hex_compact(&addr),
        if cs_ok { "通过" } else { "失败" },
        if tail_ok { "0x16" } else { "错误" }
    )
}

fn checksum8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

fn u16_be(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([bytes[at], bytes[at + 1]])
}

fn hex_compact(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与采集侧同算法的 Modbus CRC16（低字节先传）
    fn crc16(data: &[u8]) -> u16 {
        let mut crc = 0xFFFFu16;
        for &b in data {
            crc ^= b as u16;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xA001;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc
    }

    #[test]
    fn modbus_rtu_long_write_multi_is_not_mistaken_for_tcp() {
        // 回归：旧 mbap_offset 看到 bytes[2..4]==0 就按 TCP 跳过 6 字节，
        // 这帧 13 字节会被读成 从站=04 功能码=00。必须仍按 RTU 解析出 从站=1 / FC=16。
        let frame = [
            0x01, 0x10, 0x00, 0x00, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x00, 0x0B, 0x00, 0x00,
        ];
        let ok = crc16(&frame[..11]);
        let mut frame = frame;
        frame[11] = ok as u8;
        frame[12] = (ok >> 8) as u8;
        let text = describe("modbus", false, None, &frame, true);
        assert!(text.contains("从站=1"), "实际：{text}");
        assert!(text.contains("写多寄存器"), "实际：{text}");
        assert!(text.contains("起始=0"), "实际：{text}");
    }

    #[test]
    fn modbus_tcp_uses_mbap_unit_and_fc() {
        // MBAP: txid=0001 proto=0000 len=0005 unit=01 | FC03 字节数=02 数据=000A
        let frame = [
            0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0x01, 0x03, 0x02, 0x00, 0x0A,
        ];
        let text = describe("modbus", true, None, &frame, false);
        assert!(text.contains("从站=1"), "实际：{text}");
        assert!(text.contains("读保持寄存器"), "实际：{text}");
        assert!(text.contains("值=[0x000A]"), "实际：{text}");
    }

    #[test]
    fn modbus_rtu_crc_error_is_reported_not_guessed() {
        let frame = [0x01, 0x03, 0x02, 0x00, 0x0A, 0x00, 0x00];
        let text = describe("modbus", false, None, &frame, false);
        assert!(text.contains("CRC 校验失败"), "实际：{text}");
    }

    #[test]
    fn modbus_tcp_length_mismatch_is_invalid() {
        let frame = [0x00, 0x01, 0x00, 0x00, 0x00, 0x09, 0x01, 0x03];
        let text = describe("modbus", true, None, &frame, false);
        assert!(text.contains("长度字段"), "实际：{text}");
    }

    #[test]
    fn dlt645_frame_is_described() {
        // DD-28 注释里的合法示例：68 + 6×AA + 68 + 11 + 04 + 4×33 + AD + 16
        let frame = [
            0x68, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0x68, 0x11, 0x04, 0x33, 0x33, 0x33, 0x33,
            0xAD, 0x16,
        ];
        let text = describe("dlt645", false, Some("2007"), &frame, true);
        assert!(text.contains("DLT645"), "实际：{text}");
        assert!(text.contains("主站请求"), "实际：{text}");
        assert!(text.contains("校验=通过"), "实际：{text}");
    }

    #[test]
    fn cjt188_frame_is_described() {
        // 68 + 7×AA + 01 + 03 + 00 00 00 + CS + 16（与前端 sampleFrames.cjt188 同源）
        let frame = [
            0x68, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0x01, 0x03, 0x00, 0x00, 0x00, 0x12,
            0x16,
        ];
        let text = describe("cjt188", false, None, &frame, true);
        assert!(text.contains("CJT188"), "实际：{text}");
        assert!(text.contains("主站请求"), "实际：{text}");
        assert!(text.contains("校验=通过"), "实际：{text}");
    }

    #[test]
    fn unsupported_protocol_is_explicit_not_guessed() {
        let text = describe("s7comm", false, None, &[0x03, 0x00, 0x00], true);
        assert!(text.contains("本协议暂无解析器"), "实际：{text}");
    }

    #[test]
    fn exchange_reuses_single_frame_reading() {
        let req = [0x01, 0x03, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00];
        let crc = crc16(&req[..6]);
        let req = [
            req[0],
            req[1],
            req[2],
            req[3],
            req[4],
            req[5],
            crc as u8,
            (crc >> 8) as u8,
        ];
        // 应答：01 03 04 00 0A 00 0B + CRC
        let resp = [0x01, 0x03, 0x04, 0x00, 0x0A, 0x00, 0x0B, 0x00, 0x00];
        let crc = crc16(&resp[..7]);
        let resp = [
            resp[0],
            resp[1],
            resp[2],
            resp[3],
            resp[4],
            resp[5],
            resp[6],
            crc as u8,
            (crc >> 8) as u8,
        ];
        let text = describe_exchange("modbus", false, None, &req, &resp);
        assert!(
            text.contains("读保持寄存器 从站=1 起始=0 数量=2"),
            "实际：{text}"
        );
        assert!(text.contains("值=[0x000A 0x000B]"), "实际：{text}");
    }

    #[test]
    fn exchange_request_without_crc_is_not_reported_as_failure() {
        // 内置预置模板就是不带 CRC 的 PDU：不能把请求侧标成 CRC 失败，
        // 否则整行解读会把「设备正常应答」淹没在假错误里。
        let req = [0x01, 0x03, 0x00, 0x00, 0x00, 0x02];
        let resp = [0x01, 0x03, 0x02, 0x00, 0x0A, 0x00, 0x00];
        let crc = crc16(&resp[..5]);
        let resp = [
            resp[0],
            resp[1],
            resp[2],
            resp[3],
            resp[4],
            crc as u8,
            (crc >> 8) as u8,
        ];
        let text = describe_exchange("modbus", false, None, &req, &resp);
        assert!(!text.contains("CRC 校验失败"), "实际：{text}");
        assert!(
            text.contains("读保持寄存器 从站=1 起始=0 数量=2"),
            "实际：{text}"
        );
    }
}
