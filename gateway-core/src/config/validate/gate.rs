//! 启停期配置门禁：`GatewayConfig::validate_all` 及其交叉规则。
//!
//! 这里是 fail-fast 的唯一入口——加载配置、保存配置、连接设备前均以「有错误即拒绝」
//! 处理。Web 侧的只读体检报告见 [`super::report`]，两者共享同一批规则。

use validator::ValidationErrors;

use crate::config::GatewayConfig;

use super::field::uses_numeric_decode;

/// 递归展开 validator 的错误树（含嵌套结构体 `Struct` 与列表 `List`），
/// 输出 `路径: 消息` 形式的可读错误列表。
///
/// 需要递归的原因见 [`GatewayConfig::validate_all`]：`field_errors()` 只返回叶子
/// `Field` 错误，会丢弃 `#[validate(nested)]` 产生的嵌套错误。
fn collect_validation_errors(errors: &ValidationErrors, prefix: &str, out: &mut Vec<String>) {
    use validator::ValidationErrorsKind;
    for (field, kind) in errors.errors() {
        let path = if prefix.is_empty() {
            (*field).to_string()
        } else {
            format!("{}.{}", prefix, field)
        };
        match kind {
            ValidationErrorsKind::Field(errs) => {
                for e in errs {
                    let msg = match &e.message {
                        Some(m) => m.to_string(),
                        None => e.code.to_string(),
                    };
                    out.push(format!("{}: {}", path, msg));
                }
            }
            ValidationErrorsKind::Struct(inner) => collect_validation_errors(inner, &path, out),
            ValidationErrorsKind::List(map) => {
                for (idx, inner) in map {
                    collect_validation_errors(inner, &format!("{}[{}]", path, idx), out);
                }
            }
        }
    }
}

impl GatewayConfig {
    /// 校验所有点位/设备的 Modbus slave_id（合法范围 1~247）
    pub fn validate_slave_ids(&self) -> Result<(), validator::ValidationError> {
        for p in &self.points {
            if p.protocol.eq_ignore_ascii_case("modbus") {
                if let Some(id) = p.slave_id {
                    if !(1..=247).contains(&id) {
                        let mut err = validator::ValidationError::new("invalid_modbus_slave_id");
                        err.message = Some(
                            format!(
                                "point '{}': modbus slave_id must be 1..=247, got {}",
                                p.id, id
                            )
                            .into(),
                        );
                        return Err(err);
                    }
                }
            }
        }
        if let Some(devices) = &self.devices {
            for d in devices {
                if d.protocol.eq_ignore_ascii_case("modbus") {
                    if let Some(id) = d.slave_id {
                        if !(1..=247).contains(&id) {
                            let mut err =
                                validator::ValidationError::new("invalid_modbus_slave_id");
                            err.message = Some(
                                format!(
                                    "device '{}': modbus slave_id must be 1..=247, got {}",
                                    d.name, id
                                )
                                .into(),
                            );
                            return Err(err);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// 统一验证入口——聚合 validator 校验与 slave_id 校验，一次性返回所有错误。
    pub fn validate_all(&self) -> Result<(), Vec<String>> {
        use validator::Validate;
        let mut errors: Vec<String> = Vec::new();

        if let Err(ve) = self.validate() {
            // D1: 必须递归展开错误树。validator 的 `field_errors()` 只返回叶子 `Field` 错误，
            // 会丢弃 `#[validate(nested)]` 生成的 `Struct`（单结构体）与 `List`（Vec）错误，
            // 导致 buses/points/mqtt 的嵌套校验被静默忽略（fail-fast 失效）。
            collect_validation_errors(&ve, "", &mut errors);
        }
        // V-1 / V-5：validator 0.18 未给 `Option<T>` 实现 `Validate`，
        // `#[validate(nested)]` 无法用于 `Option<Vec<DeviceConfig>>` / `Option<AggregationConfig>`，
        // 必须手动展开，否则 DeviceConfig.protocol 与聚合窗口校验永不生效。
        if let Some(devices) = &self.devices {
            for (i, d) in devices.iter().enumerate() {
                if let Err(ve) = d.validate() {
                    collect_validation_errors(&ve, &format!("devices[{i}]"), &mut errors);
                }
            }
        }
        for (i, p) in self.points.iter().enumerate() {
            if let Some(agg) = &p.aggregation {
                if let Err(ve) = agg.validate() {
                    collect_validation_errors(
                        &ve,
                        &format!("points[{i}].aggregation"),
                        &mut errors,
                    );
                }
            }
        }
        // P0-3：`BusConfig.opcua` 为 `Option<OpcUaClientConfig>`，validator 0.18 不为
        // `Option<T>` 实现 `Validate`，`#[validate(nested)]` 不生效，必须手动展开；
        // 另补跨字段语义校验（auth 与 username/password、策略与模式/证书的配套关系）。
        for (i, bus) in self.buses.iter().enumerate() {
            if let Some(opcua) = &bus.opcua {
                if let Err(ve) = opcua.validate() {
                    collect_validation_errors(&ve, &format!("buses[{i}].opcua"), &mut errors);
                }
                if let Some(msg) = opcua.semantic_error() {
                    errors.push(format!("buses[{i}] ({}) opcua: {}", bus.port_name, msg));
                }
            }
        }
        // OPC UA Server：`Option<OpcuaConfig>` 无法用 `#[validate(nested)]`，
        // 需手动调用语义校验（证书成对、用户唯一、endpoint 非空）。
        if let Some(opcua) = &self.opcua {
            if let Some(msg) = opcua.semantic_error() {
                errors.push(format!("opcua: {}", msg));
            }
        }
        if let Err(e) = self.validate_slave_ids() {
            errors.push(e.to_string());
        }
        if let Err(msgs) = self.validate_cycle_conflict() {
            errors.extend(msgs);
        }
        if let Err(msgs) = self.validate_extended() {
            errors.extend(msgs);
        }
        errors.extend(self.validate_convert_rules());

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// C1：协议转换规则 fail-closed 校验（规则体校验见 `ConvertRuleConfig::validate_rule`）。
    ///
    /// 这里补充「多条之间」的约束：id 全局唯一（HashMap 运行时按 id 记账，重复会串账）。
    /// 单条规则的 transform/scale/offset/enum_map 语义校验在规则自身完成。
    fn validate_convert_rules(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let rules = self.convert_rules.as_deref().unwrap_or(&[]);
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (i, r) in rules.iter().enumerate() {
            for e in r.validate_rule() {
                errs.push(format!("convert_rules[{i}].{}", e));
            }
            if !r.id.trim().is_empty() && !seen.insert(r.id.as_str()) {
                errs.push(format!("convert_rules[{i}].id \"{}\" 重复", r.id));
            }
        }
        errs
    }

    /// 细粒度扩展校验：覆盖原 validate_* 函数未触及的字段
    ///
    /// 包含：
    /// - MQTT broker URL 合法性（scheme/host 非空，仅 mqtt/mqtts/ws/wss）
    /// - NTP server 非空 + 长度上限
    /// - 采集 interval 合理范围（1..=86400s，避免过载或长期空跑）
    /// - Modbus register_count 上限（单次最多 125 寄存器）
    fn validate_extended(&self) -> Result<(), Vec<String>> {
        let mut errs: Vec<String> = Vec::new();

        // MQTT broker URL 校验
        if self.mqtt.enabled {
            let url = self.mqtt.broker.trim();
            if url.is_empty() {
                errs.push("mqtt.broker 不能为空".to_string());
            } else {
                // 必须与 north/mqtt/broker.rs::parse_broker 支持的 scheme 一致。
                // 曾经的列表只有 mqtt/mqtts/ws/wss，导致运行时明明支持、
                // 出厂 config.json 里也用着的 tcp:// 与 tls:// 在门禁期被误判为非法：
                // mqtt.enabled 一打开就启动失败（示例配置默认关闭，问题被掩盖）。
                let scheme_ok = ["mqtt://", "mqtts://", "tls://", "tcp://", "ws://", "wss://"]
                    .iter()
                    .any(|s| url.starts_with(s))
                    // 兼容历史配置：直接写 host[:port]，parse_broker 按 tcp://1883 处理。
                    // 含 "://" 但不匹配上面白名单的（如 http://）仍要拒绝；空串也拒绝。
                    || (!url.trim().is_empty() && !url.contains("://"));
                if !scheme_ok {
                    errs.push(format!(
                        "mqtt.broker 协议非法（必须为 mqtt/mqtts/tls/tcp/ws/wss）: {}",
                        url
                    ));
                }
                // 长度上限（防止配置错误导致的内存爆炸）
                if url.len() > 512 {
                    errs.push(format!("mqtt.broker 长度 {} 超过 512 字符", url.len()));
                }
            }
            // publish_interval 范围（1..=86400s）
            if self.mqtt.publish_interval == 0 || self.mqtt.publish_interval > 86400 {
                errs.push(format!(
                    "mqtt.publish_interval {} 超出合理范围 [1, 86400]",
                    self.mqtt.publish_interval
                ));
            }
            // TLS 配置诚实性：`insecure_skip_verify` 未实现（rumqttc 的
            // `TlsConfiguration::Simple` 无法关闭服务器证书校验，旧实现只打
            // 一行 error 日志），若静默接受会让现场误以为「已跳过校验」而实际
            // 仍严格校验，排障方向完全错误。故在配置期直接拒绝。
            if self
                .mqtt
                .tls
                .as_ref()
                .map(|t| t.insecure_skip_verify)
                .unwrap_or(false)
            {
                errs.push(
                    "mqtt.tls.insecure_skip_verify=true 不受支持（该字段不会真正跳过 \
                     证书校验，只有日志提示）。请改用 mqtt.tls.ca_cert_path 指定 \
                     broker 的自签/私有 CA。"
                        .to_string(),
                );
            }
            // 自定义 CA / 客户端证书只在加密链路（tls/mqtts/wss）有意义；
            // 明文 scheme 下配置它们会被静默忽略，同样是撒谎。
            if let Some(tls) = &self.mqtt.tls {
                let encrypted = ["tls://", "mqtts://", "wss://"]
                    .iter()
                    .any(|s| url.starts_with(s));
                if !encrypted && tls.ca_cert_path.is_some() {
                    errs.push(format!(
                        "mqtt.broker={} 为明文链路，mqtt.tls 配置不会生效；请改用 \
                         tls/mqtts/wss",
                        url
                    ));
                }
            }
        }

        // NTP 配置校验（self.ntp 为 Option<NtpConfig>）
        if let Some(ntp) = &self.ntp {
            if !ntp.server.is_empty() {
                if ntp.server.len() > 256 {
                    errs.push(format!(
                        "ntp.server 长度 {} 超过 256 字符",
                        ntp.server.len()
                    ));
                }
                if ntp.sync_interval == 0 || ntp.sync_interval > 86400 {
                    errs.push(format!(
                        "ntp.sync_interval {} 超出合理范围 [1, 86400]",
                        ntp.sync_interval
                    ));
                }
            }
        }

        // 设备模板 interval 范围校验
        if let Some(devices) = &self.devices {
            for d in devices {
                if let Some(interval) = d.interval {
                    if interval == 0 || interval > 86400 {
                        errs.push(format!(
                            "device '{}' interval {} 超出合理范围 [1, 86400]",
                            d.name, interval
                        ));
                    }
                }
            }
        }

        // 点位级别校验：interval 范围 + Modbus register_count 上限
        for p in &self.points {
            if p.interval == 0 || p.interval > 86400 {
                errs.push(format!(
                    "point '{}' interval {} 超出合理范围 [1, 86400]",
                    p.id, p.interval
                ));
            }
            if p.protocol.eq_ignore_ascii_case("modbus") {
                // V-4：Modbus 采集器只认 `address_num`（`address` 字符串不参与寻址），
                // 缺失时 `unwrap_or(0)` 会静默读 0 号寄存器——值与设备真实数据无关。
                if p.address_num.is_none() {
                    errs.push(format!(
                        "point '{}' 缺少 address_num：Modbus 寻址只读该字段，缺失会静默读取 0 号地址",
                        p.id
                    ));
                }
                if let Some(count) = p.register_count {
                    // Modbus 单次读取最多 125 寄存器（FC03/FC04 标准限制）
                    if count == 0 || count > 125 {
                        errs.push(format!(
                            "point '{}' register_count {} 超出 Modbus 单次读取范围 [1, 125]",
                            p.id, count
                        ));
                    }
                }
            }
            // V-4：数值解码族（modbus/mc/fins/dlt645/cjt188）的 data_type/byte_order
            // 必须能被解析器识别。旧实现非法值静默落进 None / Abcd —— data_type 走
            // `unwrap_or(U16)`、byte_order 走默认 ABCD，采集值「有数但全错」，且与设备
            // 离线难以区分。其它协议（iec104/s7comm/…）的 data_type 自带语义，不在此校验。
            if uses_numeric_decode(&p.protocol) {
                if let Some(dt) = p
                    .data_type
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    if crate::config::point::parse_data_type(dt).is_none() {
                        errs.push(format!(
                            "point '{}' data_type '{}' 非法，应为 u16/i16/u32/i32/u64/i64/f32/f64（兼容 int16/uint16/float/double 等别名）",
                            p.id, dt
                        ));
                    }
                }
            }
            if let Some(bo) = p
                .byte_order
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if crate::config::point::parse_byte_order(bo).is_none() {
                    errs.push(format!(
                        "point '{}' byte_order '{}' 非法，应为 ABCD/DCBA/CDAB/BADC",
                        p.id, bo
                    ));
                }
            }
        }

        // 总线统一变体字段校验（决策表 #24）：取值按 protocol 限定。
        for b in &self.buses {
            if let Some(v) = b
                .variant
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let v_lc = v.to_ascii_lowercase();
                let allowed: &[&str] = match b.protocol.to_ascii_lowercase().as_str() {
                    "modbus" => &["rtu", "ascii"],
                    "dlt645" => &["1997", "2007"],
                    "cjt188" => &["2004", "2018"],
                    "fins" => &["tcp", "udp"],
                    "mc" => &["3e", "4e", "1e"],
                    _ => &[],
                };
                if !allowed.is_empty() && !allowed.contains(&v_lc.as_str()) {
                    // Modbus 专属提示：实测用户配 Modbus TCP 时自然会写 variant:"tcp"，
                    // 但 TCP 由 bus.tcp_addr 隐式选择，variant 只管串口 rtu/ascii，
                    // 报错里必须指出正路，否则用户无从修正。
                    let hint = if b.protocol.eq_ignore_ascii_case("modbus") {
                        "（TCP 传输由 bus.tcp_addr 字段选择，variant 仅用于串口模式）"
                    } else {
                        ""
                    };
                    errs.push(format!(
                        "bus '{}' protocol '{}' variant '{}' 非法，应为 {:?}{}",
                        b.port_name, b.protocol, v, allowed, hint
                    ));
                }
            }
            // IEC104 公共地址字节数只允许 1 或 2（决策表 #28）。
            if let Some(n) = b.asdu_addr_len {
                if n != 1 && n != 2 {
                    errs.push(format!(
                        "bus '{}' asdu_addr_len {} 非法，应为 1 或 2",
                        b.port_name, n
                    ));
                }
            }
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }

    /// 校验南向采集周期 < 北向上报周期。
    pub fn validate_cycle_conflict(&self) -> Result<(), Vec<String>> {
        if !self.mqtt.enabled {
            return Ok(());
        }
        let north = self.mqtt.publish_interval;
        let mut conflicts: Vec<String> = Vec::new();
        for p in &self.points {
            if p.interval >= north {
                conflicts.push(format!(
                    "point {} (id={}) 采集周期 {}s >= 北向周期 {}s，应 < 北向周期",
                    p.name.as_ref().unwrap_or(&p.id),
                    p.id,
                    p.interval,
                    north
                ));
            }
        }
        if let Some(devices) = &self.devices {
            for dev in devices {
                if let Some(interval) = dev.interval {
                    if interval >= north {
                        conflicts.push(format!(
                            "device {} (id={}) 模板采集周期 {}s >= 北向周期 {}s，应 < 北向周期",
                            dev.name, dev.id, interval, north
                        ));
                    }
                }
            }
        }
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(conflicts)
        }
    }
}

/// F2: Unified collection config validation entry point
/// Consolidates bus/point/validate validation into single module
pub fn validate_collection_config(
    config: &crate::config::GatewayConfig,
) -> Result<(), Vec<String>> {
    config.validate_all()
}

#[cfg(test)]
mod tests {
    use crate::config::mqtt::MqttTlsConfig;
    use crate::config::GatewayConfig;

    fn cfg_with_mqtt(broker: &str, tls: Option<MqttTlsConfig>) -> GatewayConfig {
        let mut cfg = GatewayConfig::default();
        cfg.mqtt.enabled = true;
        cfg.mqtt.broker = broker.to_string();
        cfg.mqtt.tls = tls;
        cfg
    }

    /// `insecure_skip_verify` 未实现，必须在配置期拒绝，而不是只打一行日志。
    #[test]
    fn mqtt_insecure_skip_verify_is_rejected_at_config_time() {
        let cfg = cfg_with_mqtt(
            "mqtts://broker.example:8883",
            Some(MqttTlsConfig {
                ca_cert_path: None,
                insecure_skip_verify: true,
            }),
        );
        let errs = cfg.validate_all().expect_err("应拒绝未实现的开关");
        assert!(
            errs.iter().any(|e| e.contains("insecure_skip_verify")),
            "错误信息应指明具体字段: {:?}",
            errs
        );
    }

    /// 明文链路下配置 TLS 证书材料会被静默忽略，同样属于撒谎，必须拒绝。
    #[test]
    fn mqtt_tls_material_on_plaintext_scheme_is_rejected() {
        let cfg = cfg_with_mqtt(
            "mqtt://broker.example:1883",
            Some(MqttTlsConfig {
                ca_cert_path: Some("/etc/gateway/ca.pem".to_string()),
                insecure_skip_verify: false,
            }),
        );
        let errs = cfg.validate_all().expect_err("明文链路配置 TLS 材料应拒绝");
        assert!(
            errs.iter().any(|e| e.contains("不会生效")),
            "应说明该配置不生效: {:?}",
            errs
        );
    }

    /// 正例：wss + 自定义 CA 是合法组合（此前该组合被静默丢弃）。
    #[test]
    fn mqtt_wss_with_custom_ca_is_accepted() {
        let cfg = cfg_with_mqtt(
            "wss://broker.example:8084/mqtt",
            Some(MqttTlsConfig {
                ca_cert_path: Some("/etc/gateway/ca.pem".to_string()),
                insecure_skip_verify: false,
            }),
        );
        cfg.validate_all().expect("wss + 自定义 CA 应通过校验");
    }

    /// broker scheme 白名单必须与运行时 ``parse_broker`` 支持的一致。
    #[test]
    fn mqtt_broker_scheme_matches_runtime_parser() {
        for broker in [
            "tcp://localhost:1883", // 出厂 config.json / dummy 配置使用的写法
            "tls://broker:8883",    // MQTT over TLS
            "mqtt://broker:1883",
            "mqtts://broker:8883",
            "ws://broker:9001/mqtt",
            "wss://broker:8084/mqtt",
            "localhost:1883", // 裸 host:port，parse_broker 按 tcp://1883 处理
        ] {
            let cfg = cfg_with_mqtt(broker, None);
            cfg.validate_all()
                .unwrap_or_else(|e| panic!("{broker} 应通过配置校验: {e:?}"));
        }

        // 不支持的 scheme 仍要拒绝：否则 parse_broker 会把 "http://x" 当主机名
        for broker in ["http://broker:1883", ""] {
            let cfg = cfg_with_mqtt(broker, None);
            let errs = cfg.validate_all().expect_err("非法 broker 应被拒绝");
            assert!(
                errs.iter().any(|e| e.contains("mqtt.broker")),
                "{broker:?}: {errs:?}"
            );
        }
    }

    // ===== 配置负例：未知协议 / 非法枚举 / 缺失必填，必须在门禁期 fail-fast =====

    fn point_with(protocol: &str) -> crate::config::PointConfig {
        crate::config::PointConfig {
            id: "p1".to_string(),
            bus: "bus1".to_string(),
            protocol: protocol.to_string(),
            address: "1".to_string(),
            interval: 10,
            ..Default::default()
        }
    }

    /// 拼写错误的协议名不能等到运行期建 Collector 时才暴露。
    #[test]
    fn unknown_point_protocol_is_rejected_by_gate() {
        let mut cfg = GatewayConfig::default();
        cfg.points.push(point_with("modbu"));
        let errs = cfg.validate_all().expect_err("未知协议名应被拒绝");
        assert!(
            errs.iter()
                .any(|e| e.contains("points[0].protocol") && e.contains("modbu")),
            "错误应定位到 points[0].protocol 并回显非法值: {:?}",
            errs
        );
    }

    #[test]
    fn unknown_bus_protocol_is_rejected_by_gate() {
        let mut cfg = GatewayConfig::default();
        cfg.buses.push(crate::config::BusConfig {
            port_name: "bus1".to_string(),
            protocol: "not-a-protocol".to_string(),
            ..Default::default()
        });
        let errs = cfg.validate_all().expect_err("未知总线协议应被拒绝");
        assert!(
            errs.iter()
                .any(|e| e.contains("buses[0].protocol") && e.contains("not-a-protocol")),
            "错误应定位到 buses[0].protocol: {:?}",
            errs
        );
    }

    /// 非法枚举字符串必须报错，而不是静默回退到默认值（ABCD / None）。
    #[test]
    fn illegal_enum_strings_are_rejected_not_silently_defaulted() {
        let mut cfg = GatewayConfig::default();
        let mut p = point_with("modbus");
        p.address_num = Some(0);
        p.data_type = Some("float128".to_string());
        p.byte_order = Some("ABCDE".to_string());
        cfg.points.push(p);

        let errs = cfg.validate_all().expect_err("非法枚举值应被拒绝");
        assert!(
            errs.iter().any(|e| e.contains("data_type")),
            "应指出非法 data_type: {:?}",
            errs
        );
        assert!(
            errs.iter().any(|e| e.contains("byte_order")),
            "应指出非法 byte_order: {:?}",
            errs
        );
    }

    /// 缺失必填字段（周期为 0 / Modbus 缺寻址字段 / 总线协议为空）一次性全部报出。
    #[test]
    fn missing_required_fields_are_rejected_by_gate() {
        let mut cfg = GatewayConfig::default();
        cfg.buses.push(crate::config::BusConfig {
            port_name: "bus1".to_string(),
            protocol: String::new(), // 必填但为空
            ..Default::default()
        });
        let mut p = point_with("modbus");
        p.interval = 0; // 必填，且 0 会让采集永不触发
        p.address_num = None; // Modbus 寻址只认该字段
        cfg.points.push(p);

        let errs = cfg.validate_all().expect_err("缺失必填字段应被拒绝");
        assert!(
            errs.iter().any(|e| e.contains("buses[0].protocol")),
            "应指出空协议总线: {:?}",
            errs
        );
        assert!(
            errs.iter().any(|e| e.contains("interval")),
            "应指出 interval 非法: {:?}",
            errs
        );
        assert!(
            errs.iter().any(|e| e.contains("address_num")),
            "应指出 Modbus 缺 address_num: {:?}",
            errs
        );
    }
}
