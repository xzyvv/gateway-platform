//! AT 命令构造/解析

pub(super) fn parse_csq(response: &str) -> u8 {
    for line in response.lines() {
        if line.starts_with("+CSQ:") {
            let parts: Vec<&str> = line.trim_start_matches("+CSQ:").split(',').collect();
            if let Some(rssi) = parts.first() {
                return rssi.trim().parse().unwrap_or(99);
            }
        }
    }
    99
}

pub(super) fn parse_operator(response: &str) -> String {
    for line in response.lines() {
        if line.starts_with("+COPS:") {
            if let Some(start) = line.find('"') {
                if let Some(end) = line[start + 1..].find('"') {
                    return line[start + 1..start + 1 + end].to_string();
                }
            }
        }
    }
    String::new()
}

pub(super) fn parse_network_type(response: &str) -> String {
    // Bug r13-ATC2 (fix): 优先查 +CEREG:（EPS / 4G LTE 注册），再 fallback +CGREG:（GPRS / 2G 3G）。
    //
    // 3GPP TS 27.007 §10.1.19/§10.1.22 规定：
    // - +CGREG: 适用于 GPRS / 2G / 3G 网络注册状态
    // - +CEREG: 适用于 EPS / 4G LTE 网络注册状态（act 取 0-7，7=E-UTRAN）
    // 4G 模块（如 Quectel EC20 / SIM7600）只回 +CEREG:，旧实现查 +CGREG: 找不到匹配
    // → 返回 String::new() → 上报「网络类型=空」，现场 4G 部署可观测性盲区。
    for line in response.lines() {
        // 优先匹配 +CEREG:（4G LTE），跳过 +CGREG: 行
        if line.starts_with("+CEREG:") {
            if let Some(s) = parse_act_from_line(line) {
                return s;
            }
        }
    }
    for line in response.lines() {
        // fallback: +CGREG:（2G/3G）
        if line.starts_with("+CGREG:") {
            if let Some(s) = parse_act_from_line(line) {
                return s;
            }
        }
    }
    String::new()
}

/// 从 +CEREG: / +CGREG: 响应行解析 act 字段（索引 3）映射到网络类型名称。
/// 解析失败返回 None（不映射为 "Unknown"，留给外层 fallback）。
fn parse_act_from_line(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.split(',').collect();
    let act = parts.get(3)?.trim().parse::<u8>().ok()?;
    Some(match act {
        0 => "GSM".to_string(),
        1 => "GSM Compact".to_string(),
        2 => "UTRAN".to_string(),
        3 => "GSM w/EGPRS".to_string(),
        4 => "UTRAN w/HSDPA".to_string(),
        5 => "UTRAN w/HSUPA".to_string(),
        6 => "UTRAN w/HSDPA+HSUPA".to_string(),
        7 => "E-UTRAN".to_string(),
        _ => return None, // 未知 act 不映射，避免误报 "Unknown" 干扰 fallback
    })
}

#[cfg(test)]
mod tests;
