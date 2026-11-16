//! 校验报告的问题结构与写入工具（从 `report.rs` 拆出，控制单文件行数）。
//!
//! `report.rs` 只保留规则编排，数据结构与去重/分级写入在这里，行为不变。

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
pub(super) fn field_path_of(message: &str) -> String {
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
pub(super) fn push_error(
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
pub(super) fn push_level(
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
