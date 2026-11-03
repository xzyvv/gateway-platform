//! 字段级配置校验器。
//!
//! 这些函数签名统一为 `fn(T) -> Result<(), ValidationError>`，由各配置结构体
//! 通过 `#[validate(custom(...))]` 直接引用；`validate_tcp_addr` 额外承担
//! 「端点格式」这一类无法用单一长度/范围表达的规则。

use validator::ValidationError;

use crate::domain::{protocol_error_message, VALID_PROTOCOLS};

/// 合法波特率白名单（覆盖工业常用值）
const VALID_BAUD_RATES: &[u32] = &[
    1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800,
];

/// 校验 topic 不含非法字符
///
/// r25-VLD1（修复）：拒绝 MQTT 通配符与系统保留前缀。
///
/// 旧实现只排除空格 / NUL / `\r` / `\n`，但 MQTT publish topic 还有三类
/// 隐性非法用法会被静默放行：
///   - `+`：单级通配符，仅订阅侧有效；publish 写入 broker 字面后下游
///     订阅者按通配符匹配，导致报警主题被串订阅、报警漏触发或误触发。
///   - `#`：多级通配符，同上问题。
///   - `$` 开头：MQTT broker 系统保留前缀（$SYS/），broker 多半静默丢弃，
///     "发送成功但云端无数据"的根因之一。
///
/// 校验时机：配置加载阶段（fail-fast），运维错误立即可见而非运行期踩坑。
fn is_valid_mqtt_topic(topic: &str) -> bool {
    if topic.is_empty() || topic.len() > 65535 {
        return false;
    }
    if topic.contains([' ', '\0', '\r', '\n']) {
        return false;
    }
    // 通配符禁止：subscribe-only 字面不应出现在 publish topic。
    if topic.contains('+') || topic.contains('#') {
        return false;
    }
    // broker 系统保留前缀：$SYS/ 等。允许内部"$"作为普通字符（非首位）。
    if topic.starts_with('$') {
        return false;
    }
    true
}

/// 校验函数（validator 0.18 要求 fn(T) -> Result<(), ValidationError>）
pub(crate) fn validate_baud_rate(baud: u32) -> Result<(), ValidationError> {
    if VALID_BAUD_RATES.contains(&baud) {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_baud_rate"))
    }
}

pub(crate) fn validate_mqtt_topic(topic: &str) -> Result<(), ValidationError> {
    if is_valid_mqtt_topic(topic) {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_mqtt_topic"))
    }
}

pub(crate) fn validate_qos(qos: u8) -> Result<(), ValidationError> {
    if qos <= 2 {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_qos"))
    }
}

pub(crate) fn validate_non_zero_interval(interval: u64) -> Result<(), ValidationError> {
    if interval >= 1 {
        Ok(())
    } else {
        Err(ValidationError::new("interval_must_be_positive"))
    }
}

/// Bug 228: 显式配置 `scale = 0` 的 fail-fast 校验。
///
/// 读路径 `convert_value` 为 `raw * scale + offset`，scale=0 时结果恒等于 `offset`——
/// 采集明明成功、质量 Good，上报值却被静默拉平；而写路径 `reverse_convert`
/// 又把 0 当作"不做缩放"直接透传，同一份配置读写两套语义、破坏互逆。
/// serde 缺省已是 1.0，故显式 0（以及非有限值）一律视为配置错误。
pub(crate) fn validate_scale(scale: f64) -> Result<(), ValidationError> {
    if scale == 0.0 {
        let mut err = ValidationError::new("scale_must_not_be_zero");
        err.message = Some(
            "scale 不能为 0：会使采集值恒等于 offset（如需 1:1 透传请省略该字段或设为 1.0）".into(),
        );
        Err(err)
    } else if !scale.is_finite() {
        Err(ValidationError::new("scale_must_be_finite"))
    } else {
        Ok(())
    }
}

/// 合法字节序（短四字码 + 历史长写法，单一事实来源）。
///
/// 短四字码按 Modbus 约定大写；长写法（`big-endian` 等）是模板字段的历史取值，
/// 两者都由 [`crate::config::PointConfig::byte_order_enum`] 解析，必须与本表同步。
pub(crate) const VALID_BYTE_ORDERS: &[&str] = &[
    "big-endian",
    "little-endian",
    "big-endian-byte-swapped",
    "little-endian-byte-swapped",
    "ABCD",
    "BADC",
    "CDAB",
    "DCBA",
];

/// 校验字节序字段：先精确匹配（ABCD 等大写四字码），再忽略大小写匹配（兼容长写法）。
pub(crate) fn validate_byte_order(b: &str) -> Result<(), ValidationError> {
    if VALID_BYTE_ORDERS.contains(&b)
        || VALID_BYTE_ORDERS.contains(&b.to_ascii_lowercase().as_str())
    {
        Ok(())
    } else {
        let mut err = ValidationError::new("invalid_byte_order");
        err.message = Some(
            format!(
                "invalid byte_order '{}', must be one of {:?}",
                b, VALID_BYTE_ORDERS
            )
            .into(),
        );
        Err(err)
    }
}

/// 校验点位功能码：仅接受读写实现实际识别的取值。
///
/// - 读：1/2（线圈、离散输入）、3/4（保持/输入寄存器）
/// - 写：5/6（单线圈/单寄存器）、15/16（多线圈/多寄存器）
///
/// 其它取值（0/7/…）在 `modbus` 的读写路径里都没有分支，配置后只会长期采不到值。
pub(crate) fn validate_function_code(fc: u8) -> Result<(), ValidationError> {
    match fc {
        1..=6 | 15 | 16 => Ok(()),
        _ => {
            let mut err = ValidationError::new("invalid_function_code");
            err.message = Some(format!("function_code {} 非法，应为 1~6 或 15/16", fc).into());
            Err(err)
        }
    }
}

/// 校验网络类协议的目标地址格式 `host:port`，允许尾部携带协议私有查询串。
///
/// 该字段直接交给 TCP/UDP 连接器解析，格式错误（缺端口、端口非数字）只会在建连时
/// 表现为「连不上」，与设备离线无法区分，故在配置阶段 fail-fast。
///
/// 部分协议在端点之后追加查询串表达协议参数，或整串就是参数：
/// - S7comm：`host:102?rack=0&slot=2`
/// - FINS UDP：`host:9600?node=NN`
/// - BACnet MSTP：`master=0&max_master=127`（无 host:port，纯参数串）
///
/// 因此先剥离 `?` 之后的查询串；若剩下一段不含 `:`，按「纯 `key=value` 参数串」放行，
/// 否则按 `host:port` 校验。
pub(crate) fn validate_tcp_addr(addr: &str) -> Result<(), ValidationError> {
    let trimmed = addr.trim();
    let (endpoint, query) = match trimmed.split_once('?') {
        Some((e, q)) => (e, q),
        None => (trimmed, ""),
    };
    // BACnet MSTP 的 tcp_addr 是主站参数串，没有 host:port。
    if !endpoint.contains(':') && query.is_empty() && is_key_value_pairs(endpoint) {
        return Ok(());
    }
    let (host, port) = match endpoint.rsplit_once(':') {
        Some(parts) => parts,
        None => {
            let mut err = ValidationError::new("invalid_tcp_addr");
            err.message = Some(format!("tcp_addr '{}' 缺少端口，应为 host:port", addr).into());
            return Err(err);
        }
    };
    let port_ok = port.parse::<u16>().map(|p| p != 0).unwrap_or(false);
    if host.trim().is_empty() || !port_ok {
        let mut err = ValidationError::new("invalid_tcp_addr");
        err.message =
            Some(format!("tcp_addr '{}' 非法，应为 host:port（端口 1~65535）", addr).into());
        return Err(err);
    }
    Ok(())
}

/// 判断整串是否为 `key=value(&key=value)*` 形式（BACnet MSTP 主站参数）。
fn is_key_value_pairs(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.split('&').all(|seg| {
            seg.split_once('=')
                .map(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
                .unwrap_or(false)
        })
}

pub(crate) fn validate_parity(parity: &str) -> Result<(), ValidationError> {
    match parity {
        "none" | "even" | "odd" => Ok(()),
        _ => Err(ValidationError::new("invalid_parity")),
    }
}

pub(crate) fn validate_data_bits(data_bits: u8) -> Result<(), ValidationError> {
    match data_bits {
        5..=8 => Ok(()),
        _ => Err(ValidationError::new("invalid_data_bits")),
    }
}

pub(crate) fn validate_stop_bits(stop_bits: u8) -> Result<(), ValidationError> {
    match stop_bits {
        1 | 2 => Ok(()),
        _ => Err(ValidationError::new("invalid_stop_bits")),
    }
}

pub(crate) fn validate_modbus_mode(mode: &str) -> Result<(), ValidationError> {
    match mode {
        "rtu" | "ascii" => Ok(()),
        _ => Err(ValidationError::new("invalid_modbus_mode")),
    }
}

pub(crate) fn validate_dlt645_version(version: &str) -> Result<(), ValidationError> {
    // 与 `Dlt645Version::from_config` 的可识别取值保持一致（1997 厂家子变体）
    match version {
        "1997"
        | "1997-addr-be-plain"
        | "1997-addr-le-plain"
        | "1997-addr-be-cipher"
        | "1997-addr-le-cipher"
        | "2007" => Ok(()),
        _ => Err(ValidationError::new("invalid_dlt645_version")),
    }
}

/// OPC UA Client 认证方式：anonymous（默认）/ username / certificate。
pub(crate) fn validate_opcua_auth(auth: &str) -> Result<(), ValidationError> {
    match auth.to_ascii_lowercase().as_str() {
        "anonymous" | "username" | "certificate" => Ok(()),
        _ => {
            let mut err = ValidationError::new("invalid_opcua_auth");
            err.message = Some(
                format!(
                    "invalid opcua auth '{}', must be one of [anonymous, username, certificate]",
                    auth
                )
                .into(),
            );
            Err(err)
        }
    }
}

/// OPC UA Client 安全策略：none（默认）/ basic256sha256。
pub(crate) fn validate_opcua_security_policy(policy: &str) -> Result<(), ValidationError> {
    match policy.to_ascii_lowercase().as_str() {
        "none" | "basic256sha256" => Ok(()),
        _ => {
            let mut err = ValidationError::new("invalid_opcua_security_policy");
            err.message = Some(
                format!(
                    "invalid opcua security_policy '{}', must be one of [none, basic256sha256]",
                    policy
                )
                .into(),
            );
            Err(err)
        }
    }
}

/// OPC UA Client 消息安全模式：none（默认）/ sign / sign_and_encrypt。
pub(crate) fn validate_opcua_security_mode(mode: &str) -> Result<(), ValidationError> {
    match mode.to_ascii_lowercase().as_str() {
        "none" | "sign" | "sign_and_encrypt" => Ok(()),
        _ => {
            let mut err = ValidationError::new("invalid_opcua_security_mode");
            err.message = Some(
                format!(
                    "invalid opcua security_mode '{}', must be one of [none, sign, sign_and_encrypt]",
                    mode
                )
                .into(),
            );
            Err(err)
        }
    }
}

/// 校验重连退避档位表：非空、每档 >=1 秒、且非递减。
pub(crate) fn validate_backoff_seconds(seconds: &[u64]) -> Result<(), ValidationError> {
    if seconds.is_empty() {
        return Err(ValidationError::new("backoff_seconds_empty"));
    }
    if seconds.iter().any(|&s| s < 1) {
        return Err(ValidationError::new("backoff_seconds_too_small"));
    }
    if seconds.windows(2).any(|w| w[1] < w[0]) {
        return Err(ValidationError::new("backoff_seconds_not_ascending"));
    }
    Ok(())
}

/// F2: validate_protocol moved from template.rs to unified validation module
///
/// P0-2：区分「协议拼错」与「协议存在但未编入当前发行 SKU」。后者给出可操作的
/// 提示（重新构建并启用对应 feature / 升级发行包），避免现场把「裁剪」误判为
/// 「配置写错」或「网关不支持该协议」。
pub(crate) fn validate_protocol(p: &str) -> Result<(), validator::ValidationError> {
    if VALID_PROTOCOLS.contains(&p.to_ascii_lowercase().as_str()) {
        Ok(())
    } else {
        let mut err = validator::ValidationError::new("invalid_protocol");
        err.message = Some(protocol_error_message(p).into());
        Err(err)
    }
}

/// 是否使用统一数值解码族（`PointConfig::data_type_enum` / `byte_order_enum` 的消费方）。
///
/// 仅这些协议的 `data_type` 取值是「u16/i16/…」这套统一编码；iec104（float/normalized/…）
/// 与 s7comm（word/int/real/…）自带独立语义，不能套用同一校验。
pub(super) fn uses_numeric_decode(protocol: &str) -> bool {
    matches!(
        protocol.to_ascii_lowercase().as_str(),
        "modbus" | "mc" | "fins" | "dlt645" | "cjt188"
    )
}

#[cfg(test)]
mod tcp_addr_tests {
    use super::validate_tcp_addr;

    #[test]
    fn plain_host_port_is_valid() {
        assert!(validate_tcp_addr("192.168.1.10:502").is_ok());
    }

    /// S7comm 在端点后携带 rack/slot 查询串，配置层不应拒绝。
    #[test]
    fn s7_query_string_is_valid() {
        assert!(validate_tcp_addr("192.168.1.10:102?rack=0&slot=2").is_ok());
    }

    /// FINS UDP 在端点后携带节点号查询串，配置层不应拒绝。
    #[test]
    fn fins_udp_query_string_is_valid() {
        assert!(validate_tcp_addr("192.168.1.10:9600?node=3").is_ok());
    }

    /// BACnet MSTP 的 tcp_addr 是主站参数串，没有 host:port，不能被当作缺端口拒绝。
    #[test]
    fn bacnet_mstp_param_string_is_valid() {
        assert!(validate_tcp_addr("master=0&max_master=127").is_ok());
        assert!(validate_tcp_addr("master=5").is_ok());
    }

    /// 既不是 host:port 也不是 key=value 的串不能被放行。
    #[test]
    fn garbage_is_rejected() {
        assert!(validate_tcp_addr("abc").is_err());
        assert!(validate_tcp_addr("master=").is_err());
        assert!(validate_tcp_addr("=0").is_err());
    }

    #[test]
    fn missing_port_is_rejected() {
        assert!(validate_tcp_addr("192.168.1.10").is_err());
        assert!(validate_tcp_addr("192.168.1.10?rack=0").is_err());
    }

    #[test]
    fn zero_port_is_rejected() {
        assert!(validate_tcp_addr("192.168.1.10:0").is_err());
        assert!(validate_tcp_addr("192.168.1.10:0?rack=0&slot=2").is_err());
    }

    #[test]
    fn non_numeric_port_is_rejected() {
        assert!(validate_tcp_addr("192.168.1.10:abc").is_err());
        assert!(validate_tcp_addr("192.168.1.10:abc?node=1").is_err());
    }

    #[test]
    fn empty_host_is_rejected() {
        assert!(validate_tcp_addr(":502").is_err());
    }
}
