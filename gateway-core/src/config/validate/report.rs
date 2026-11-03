//! Web 校验报告（P4）：`POST /api/config/validate` 的唯一规则来源。
//!
//! 与 [`super::gate`] 的区别：门禁（`validate_all`）在启动/保存时 fail-fast，
//! 报告则做**只读**检查并把问题分为 error / warning / info 三级——前者阻断，
//! 后两者只提示。新增交叉规则只加在报告侧时，不会让历史配置无法启动。

use crate::config::GatewayConfig;

/// 配置校验报告中的单条问题。
///
/// `level = "error"` 阻断保存；`level = "warning"` 允许保存但需提示；
/// `level = "info"` 为改进建议（只影响体检评分，不阻断保存）。
/// `code` 是稳定的规则标识，前端据此映射中文类别与修复建议（展示层职责），
/// 规则本身仍只在本模块维护一份。
/// `field` 为可定位的字段路径（如 `buses[0].port_name`），无法定位时为空串；
/// 前端据此推导配置页签实现「定位」跳转（UI 概念不进入核心层）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigValidationIssue {
    pub level: &'static str,
    pub code: &'static str,
    pub field: String,
    pub message: String,
}

/// 从 `validate_all` 的错误文案中提取字段路径。
///
/// 两种既有文案格式：
/// - validator 聚合：`buses[0].baud_rate: invalid_baud_rate`
/// - 扩展校验：`mqtt.broker 协议非法（必须为 mqtt/mqtts/ws/wss）: ...`
///
/// 提取不到时返回空串（前端仅失去「定位」按钮，不影响问题展示）。
fn field_path_of(message: &str) -> String {
    if let Some((path, _)) = message.split_once(": ") {
        if path.contains('.') || path.contains('[') {
            return path.trim().to_string();
        }
    }
    if let Some(first) = message.split_whitespace().next() {
        if first.contains('.') && !first.contains('\'') {
            return first.trim_end_matches(':').to_string();
        }
    }
    String::new()
}

/// 追加一条错误；同一字段已有错误时跳过，避免同一问题重复刷屏。
fn push_error(
    issues: &mut Vec<ConfigValidationIssue>,
    seen: &mut std::collections::HashSet<String>,
    code: &'static str,
    field: String,
    message: String,
) {
    if !field.is_empty() && !seen.insert(field.clone()) {
        return;
    }
    issues.push(ConfigValidationIssue {
        level: "error",
        code,
        field,
        message,
    });
}

/// 追加一条非阻断问题（warning / info）。warning 与 info 天然按对象区分，不做去重。
fn push_level(
    issues: &mut Vec<ConfigValidationIssue>,
    level: &'static str,
    code: &'static str,
    field: String,
    message: String,
) {
    issues.push(ConfigValidationIssue {
        level,
        code,
        field,
        message,
    });
}

impl GatewayConfig {
    /// 生成完整校验报告（Web `POST /api/config/validate` 专用）。
    ///
    /// 规则只在这里维护一份：前端不再自行维护阈值/唯一性/引用规则，避免双源漂移。
    /// 本方法**只读检查**，不改变 `validate_all` 的启动期与保存期语义
    /// （启动期仍以 `validate_all` fail-fast，避免历史配置因新增交叉规则无法启动）。
    ///
    /// 无模板清单时退化为 `validate_report_with(&[])`（跳过模板存在性检查）。
    pub fn validate_report(&self) -> Vec<ConfigValidationIssue> {
        self.validate_report_with(&[])
    }

    /// 带模板库清单的完整校验报告。
    ///
    /// `template_names` 为空表示模板库不可用（拉取失败），此时跳过模板存在性检查——
    /// 与旧前端「模板库为空则不做模板类检查」的降级行为一致。
    ///
    /// 本方法是 Web 体检/保存前置校验的**唯一规则来源**：完整性、模板存在性、
    /// 重名、重复地址、悬空引用、超时越界、性能建议全部在这里判定；
    /// 前端只负责按 `code` 映射中文类别与修复建议（展示层职责）。
    pub fn validate_report_with(&self, template_names: &[String]) -> Vec<ConfigValidationIssue> {
        let mut issues: Vec<ConfigValidationIssue> = Vec::new();
        if let Err(errs) = self.validate_all() {
            for e in errs {
                issues.push(ConfigValidationIssue {
                    level: "error",
                    code: "field_invalid",
                    field: field_path_of(&e),
                    message: e,
                });
            }
        }

        let mut seen: std::collections::HashSet<String> = issues
            .iter()
            .map(|i| i.field.clone())
            .filter(|f| !f.is_empty())
            .collect();

        // ---- 交叉项错误（validator / validate_all 无法表达的「多条之间」约束）----

        // 总线端口重复：运行时按 port_name 作 HashMap key，重复会被静默覆盖。
        let mut first_seen: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (i, b) in self.buses.iter().enumerate() {
            let key = b.port_name.trim();
            if key.is_empty() {
                continue;
            }
            if let Some(&first) = first_seen.get(key) {
                push_error(
                    &mut issues,
                    &mut seen,
                    "bus_port_dup",
                    format!("buses[{i}].port_name"),
                    format!(
                        "总线端口 \"{key}\" 重复（与 buses[{first}] 相同）；运行时后端会按 HashMap 覆盖其中一条，请修改端口名"
                    ),
                );
            } else {
                first_seen.insert(key, i);
            }
        }

        let devices = self.devices.as_deref().unwrap_or(&[]);

        // 设备地址唯一性：同一总线 + 协议下地址重复会命中错误设备。
        let mut dev_seen: std::collections::HashMap<String, &str> =
            std::collections::HashMap::new();
        for d in devices {
            let key = format!("{}|{}|{}", d.bus, d.protocol, d.address);
            if let Some(prev) = dev_seen.get(&key) {
                push_error(
                    &mut issues,
                    &mut seen,
                    "device_addr_dup",
                    format!("devices[{}].address", d.id),
                    format!("设备 {} 与 {prev} 在同一总线+协议下地址重复", d.name),
                );
            } else {
                dev_seen.insert(key, d.name.as_str());
            }
        }

        // 设备重名：点位展开要求设备名全局唯一，重名会让整批展开失败（界面表现为全站 0 点位）。
        let mut name_first: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (i, d) in devices.iter().enumerate() {
            let name = d.name.trim();
            if name.is_empty() {
                continue;
            }
            if let Some(&first) = name_first.get(name) {
                push_error(
                    &mut issues,
                    &mut seen,
                    "device_dup_name",
                    format!("devices[{i}].name"),
                    format!(
                        "设备名「{name}」与 devices[{first}] 重复；设备名参与点位 ID 生成，重名会让整批展开失败并回退 0 点位"
                    ),
                );
            } else {
                name_first.insert(name, i);
            }
        }

        // 设备 ID 重复：删除按 ID 全量匹配，同 ID 的另一台设备会被连带删除。
        let mut id_first: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, d) in devices.iter().enumerate() {
            let id = d.id.trim();
            if id.is_empty() {
                continue;
            }
            if let Some(&first) = id_first.get(id) {
                push_error(
                    &mut issues,
                    &mut seen,
                    "device_dup_id",
                    format!("devices[{i}].id"),
                    format!(
                        "设备 ID「{id}」与 devices[{first}] 重复；删除其中一台会连带删除其余同 ID 设备及其点位"
                    ),
                );
            } else {
                id_first.insert(id, i);
            }
        }

        // 模板存在性：模板名拼错时展开阶段失败并回退 0 点位，界面上却一切正常。
        if !template_names.is_empty() {
            for (i, d) in devices.iter().enumerate() {
                let tpl = d.template.trim();
                if tpl.is_empty() {
                    continue;
                }
                if !template_names.iter().any(|t| t == tpl) {
                    push_error(
                        &mut issues,
                        &mut seen,
                        "tpl_missing",
                        format!("devices[{i}].template"),
                        format!(
                            "设备 {} 引用的模板「{tpl}」在模板库中不存在，展开后将采不到任何点位",
                            d.name
                        ),
                    );
                }
            }
        }

        // 点位地址重复：key 与旧前端规则一致（设备/总线/协议/从站号/地址）。
        let mut pt_seen: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
        for p in &self.points {
            let key = format!(
                "{}|{}|{}|{}|{}",
                p.device_id.as_deref().unwrap_or(""),
                p.bus,
                p.protocol,
                p.slave_id.map(|v| v.to_string()).unwrap_or_default(),
                p.address
            );
            if let Some(prev) = pt_seen.get(&key) {
                push_error(
                    &mut issues,
                    &mut seen,
                    "point_addr_dup",
                    format!("points[{}].address", p.id),
                    format!("点位地址与已有点位 {prev} 重复"),
                );
            } else {
                pt_seen.insert(key, p.id.as_str());
            }
        }

        // 引用存在性：悬空引用会让采集器启动期静默跳过该点。
        let bus_names: std::collections::HashSet<&str> =
            self.buses.iter().map(|b| b.port_name.trim()).collect();
        let device_ids: std::collections::HashSet<&str> = devices
            .iter()
            .map(|d| d.id.as_str())
            .filter(|id| !id.is_empty())
            .collect();
        for p in &self.points {
            if let Some(dev) = p.device_id.as_deref().filter(|s| !s.is_empty()) {
                if !device_ids.contains(dev) {
                    push_error(
                        &mut issues,
                        &mut seen,
                        "point_device_missing",
                        format!("points[{}].device_id", p.id),
                        format!("引用的设备 {dev} 不存在"),
                    );
                }
            }
            if !bus_names.contains(p.bus.trim()) {
                push_error(
                    &mut issues,
                    &mut seen,
                    "point_bus_missing",
                    format!("points[{}].bus", p.id),
                    format!("引用的总线 {} 不存在", p.bus),
                );
            }
        }
        for d in devices {
            if !bus_names.contains(d.bus.trim()) {
                push_error(
                    &mut issues,
                    &mut seen,
                    "device_bus_missing",
                    format!("devices[{}].bus", d.id),
                    format!("设备 {} 引用的总线 {} 不存在", d.name, d.bus),
                );
            }
        }

        // ---- 建议性警告（不阻断保存）----

        // 设备协议与总线协议不一致（大小写不敏感：用户可能输入 Modbus/MODBUS）。
        let bus_proto: std::collections::HashMap<&str, &str> = self
            .buses
            .iter()
            .map(|b| (b.port_name.trim(), b.protocol.as_str()))
            .collect();
        for d in devices {
            if let Some(bp) = bus_proto.get(d.bus.trim()) {
                if !bp.is_empty() && !d.protocol.is_empty() && !bp.eq_ignore_ascii_case(&d.protocol)
                {
                    push_level(
                        &mut issues,
                        "warning",
                        "device_protocol_mismatch",
                        format!("devices[{}].protocol", d.id),
                        format!(
                            "设备 {} 协议({})与总线 {} 协议({})不一致",
                            d.name, d.protocol, d.bus, bp
                        ),
                    );
                }
            }
        }

        // 遥测采样率推荐范围（0.0~1.0）。
        if let Some(t) = &self.telemetry {
            if t.enabled && !(0.0..=1.0).contains(&t.sample_ratio) {
                push_level(
                    &mut issues,
                    "warning",
                    "telemetry_sample_ratio",
                    "telemetry.sample_ratio".to_string(),
                    format!("遥测采样率 {} 超出 0.0~1.0 推荐范围", t.sample_ratio),
                );
            }
        }

        // 总线无设备：配了一条总线却没有任何设备，通常意味着接入只做了一半。
        let bus_dev_names: std::collections::HashSet<&str> =
            devices.iter().map(|d| d.bus.trim()).collect();
        for (i, b) in self.buses.iter().enumerate() {
            if !bus_dev_names.contains(b.port_name.trim()) {
                push_level(
                    &mut issues,
                    "warning",
                    "bus_empty",
                    format!("buses[{i}].port_name"),
                    format!("总线 {} 未配置任何设备", b.port_name),
                );
            }
        }

        // 空设备：设备没有任何点位（模板展开的点位不在 `points` 里，故只对显式 ID 判定）。
        let point_dev_ids: std::collections::HashSet<&str> = self
            .points
            .iter()
            .filter_map(|p| p.device_id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        for (i, d) in devices.iter().enumerate() {
            let id = d.id.trim();
            if id.is_empty() {
                continue;
            }
            if !point_dev_ids.contains(id) {
                push_level(
                    &mut issues,
                    "warning",
                    "device_empty",
                    format!("devices[{i}].id"),
                    format!("设备 {} 未配置任何点位", d.name),
                );
            }
        }

        // 总线超时超过 1000ms：慢总线会把整轮轮询拖长，最终表现为数据滞后。
        for (i, b) in self.buses.iter().enumerate() {
            if b.timeout_ms > 1000 {
                push_level(
                    &mut issues,
                    "warning",
                    "bus_timeout",
                    format!("buses[{i}].timeout_ms"),
                    format!(
                        "总线 {} 超时 {}ms 超过建议值 1000ms",
                        b.port_name, b.timeout_ms
                    ),
                );
            }
        }

        // ---- 提示（不阻断保存，仅影响体检评分）----

        // 完整性：原体检只校验「已有条目之间的合理性」，空配置也能拿满分；
        // 这里把关键环节缺失计入，让半空配置同样掉分。
        if self.buses.is_empty() {
            push_level(
                &mut issues,
                "info",
                "cfg_no_bus",
                String::new(),
                "尚未配置任何总线，网关不会采集任何数据".to_string(),
            );
        }
        if devices.is_empty() {
            push_level(
                &mut issues,
                "info",
                "cfg_no_device",
                String::new(),
                "尚未添加任何设备，总线上不会产生任何轮询".to_string(),
            );
        }
        if !devices.is_empty() && self.points.is_empty() {
            push_level(
                &mut issues,
                "info",
                "cfg_no_point",
                String::new(),
                "设备已配置但没有显式点位，若未通过模板展开则不会采集任何数据".to_string(),
            );
        }

        // 性能建议：同一总线上设备过多时轮询会互相排队。
        let mut bus_dev_count: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for d in devices {
            let bus = d.bus.trim();
            if !bus.is_empty() {
                *bus_dev_count.entry(bus).or_insert(0) += 1;
            }
        }
        for (i, b) in self.buses.iter().enumerate() {
            if let Some(&count) = bus_dev_count.get(b.port_name.trim()) {
                if count >= 4 {
                    push_level(
                        &mut issues,
                        "info",
                        "bus_busy",
                        format!("buses[{i}].port_name"),
                        format!(
                            "{} 总线上挂了 {} 台设备，建议采集周期错开",
                            b.port_name, count
                        ),
                    );
                }
            }
        }

        issues
    }
}

#[cfg(test)]
mod report_tests {
    use crate::config::{BusConfig, DeviceConfig, GatewayConfig, PointConfig};

    fn bus(port: &str, proto: &str) -> BusConfig {
        serde_json::from_value(serde_json::json!({
            "port_name": port,
            "protocol": proto,
        }))
        .expect("bus json")
    }

    fn point(id: &str, bus: &str, addr: &str) -> PointConfig {
        PointConfig {
            id: id.to_string(),
            bus: bus.to_string(),
            protocol: "gpio".to_string(),
            address: addr.to_string(),
            slave_id: Some(1),
            interval: 10,
            ..Default::default()
        }
    }

    /// 协议取 `gpio`：它在任何协议裁剪构建里都合法（见 `domain::VALID_PROTOCOLS`），
    /// 而 `modbus` 只在 `--features modbus` 时存在。点位协议会被 `validate_protocol`
    /// 校验，报告规则（重复地址 / 悬空引用 / 重复端口）本身与协议无关，
    /// 不应因裁剪构建而失去回归。
    fn device(id: &str, name: &str, bus: &str) -> DeviceConfig {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": name,
            "template": "",
            "bus": bus,
            "protocol": "gpio",
            "address": "1",
        }))
        .expect("device json")
    }

    /// 干净配置：一条总线 + 一台挂在其上的设备 + 一个挂在该设备上的点位。
    /// 只配总线不配设备会命中「总线无设备 / 缺少设备」规则，无法代表「无问题」。
    fn base() -> GatewayConfig {
        let mut p = point("p1", "/dev/ttyS1", "40001");
        p.device_id = Some("d1".to_string());
        GatewayConfig {
            buses: vec![bus("/dev/ttyS1", "gpio")],
            devices: Some(vec![device("d1", "设备A", "/dev/ttyS1")]),
            points: vec![p],
            ..Default::default()
        }
    }

    #[test]
    fn clean_config_has_no_issues() {
        assert!(base().validate_report().is_empty());
    }

    #[test]
    fn duplicate_bus_port_is_error() {
        let mut cfg = base();
        cfg.buses.push(bus("/dev/ttyS1", "gpio"));
        let report = cfg.validate_report();
        assert!(
            report
                .iter()
                .any(|i| i.level == "error" && i.field == "buses[1].port_name"),
            "report: {:?}",
            report
        );
    }

    #[test]
    fn duplicate_point_address_is_error() {
        let mut cfg = base();
        // 重复地址的 key 含 device_id：p2 必须挂同一台设备才会被判为重复
        let mut p2 = point("p2", "/dev/ttyS1", "40001");
        p2.device_id = Some("d1".to_string());
        cfg.points.push(p2);
        let report = cfg.validate_report();
        assert!(
            report
                .iter()
                .any(|i| i.level == "error" && i.field == "points[p2].address"),
            "report: {:?}",
            report
        );
    }

    #[test]
    fn dangling_bus_reference_is_error() {
        let mut cfg = base();
        cfg.points[0].bus = "/dev/ttyS9".to_string();
        let report = cfg.validate_report();
        assert!(
            report
                .iter()
                .any(|i| i.level == "error" && i.field == "points[p1].bus"),
            "report: {:?}",
            report
        );
    }

    #[test]
    fn telemetry_ratio_out_of_range_is_warning() {
        let mut cfg = base();
        cfg.telemetry = Some(
            serde_json::from_value(serde_json::json!({
                "enabled": true,
                "sample_ratio": 1.5,
            }))
            .expect("telemetry json"),
        );
        let report = cfg.validate_report();
        assert!(
            report
                .iter()
                .any(|i| i.level == "warning" && i.field == "telemetry.sample_ratio"),
            "report: {:?}",
            report
        );
    }

    /// 模板存在性只在调用方给出模板清单时检查（清单为空代表模板库不可用）。
    #[test]
    fn missing_template_is_error_when_names_provided() {
        let mut cfg = base();
        cfg.devices.as_mut().unwrap()[0].template = "tpl-x".to_string();
        let names = vec!["tpl-a".to_string()];
        let report = cfg.validate_report_with(&names);
        assert!(
            report
                .iter()
                .any(|i| i.level == "error" && i.code == "tpl_missing"),
            "report: {:?}",
            report
        );
        // 不传模板清单时退化为跳过模板检查
        assert!(!cfg
            .validate_report()
            .iter()
            .any(|i| i.code == "tpl_missing"));
    }

    /// 超时越界与完整性都属于非阻断项：warning / info，不影响 valid。
    #[test]
    fn timeout_and_integrity_are_non_blocking() {
        let mut cfg = base();
        cfg.buses[0].timeout_ms = 5000;
        let report = cfg.validate_report();
        assert!(
            report
                .iter()
                .any(|i| i.level == "warning" && i.code == "bus_timeout"),
            "report: {:?}",
            report
        );

        let empty = GatewayConfig::default().validate_report();
        assert!(empty
            .iter()
            .any(|i| i.level == "info" && i.code == "cfg_no_bus"));
        assert!(empty
            .iter()
            .any(|i| i.level == "info" && i.code == "cfg_no_device"));
        assert!(empty.iter().all(|i| i.level != "error"));
    }
}
