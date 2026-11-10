//! Web 校验报告（P4）：`POST /api/config/validate` 的唯一规则来源。
//!
//! 与 [`super::gate`] 的区别：门禁（`validate_all`）在启动/保存时 fail-fast，
//! 报告则做**只读**检查并把问题分为 error / warning / info 三级——前者阻断，
//! 后两者只提示。新增交叉规则只加在报告侧时，不会让历史配置无法启动。

use crate::config::point::RatioScope;
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

        // CT/PT 变比与单位不匹配：配了也不会生效。
        //
        // 变比生效范围由单位决定（电压只吃 PT、电流只吃 CT、功率/电量两者都吃、
        // 功率因数都不吃），与解码侧共用 `point::ratio_scope`。错配不报错，
        // 只会让现场「填了变比但读数没放大」，属最典型的静默失效。
        for p in &self.points {
            let scope = p.ratio_scope();
            let unit = p.unit.as_deref().unwrap_or("(空)");
            if let Some(ct) = p.ct_ratio {
                if !matches!(scope, RatioScope::Ct | RatioScope::Both) {
                    push_level(
                        &mut issues,
                        "warning",
                        "ratio_not_applicable",
                        format!("points[{}].ct_ratio", p.id),
                        format!(
                            "点位 {} 单位为 {unit}，不受 CT 变比影响（电压只受 PT 影响、功率因数两者都不受影响），当前 ct_ratio={ct} 不会生效",
                            p.id
                        ),
                    );
                }
            }
            if let Some(pt) = p.pt_ratio {
                if !matches!(scope, RatioScope::Pt | RatioScope::Both) {
                    push_level(
                        &mut issues,
                        "warning",
                        "ratio_not_applicable",
                        format!("points[{}].pt_ratio", p.id),
                        format!(
                            "点位 {} 单位为 {unit}，不受 PT 变比影响（电流只受 CT 影响、功率因数两者都不受影响），当前 pt_ratio={pt} 不会生效",
                            p.id
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
mod report_tests;
