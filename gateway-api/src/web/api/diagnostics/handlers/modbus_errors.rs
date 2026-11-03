//! Modbus 异常码速查 handler（**静态知识库**，非运行时错误统计）
//!
//! 返回标准 Modbus 异常码定义和常见故障排查清单，供现场对照排查；
//! **不统计本网关运行期的错误次数/分布**——需要真实运行数据请查审计日志、
//! 采集统计（`/api/diagnostics/stats`）与总线负载（`/api/diagnostics/bus-load`）。
//! 数据完全静态（来自协议规范），未来若有协议扩展（如自定义异常码）可改为配置驱动。
//!
//! 前端已并入「连接测试中心 → 异常码速查」折叠区，并在界面上标注为静态知识库。

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use ts_rs::TS;

use crate::web::types::{ApiError, ApiResponse, AppState};

/// Modbus 异常码定义
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ModbusError {
    /// 异常码（十六进制字符串，如 "01"）
    pub code: String,
    /// 英文名（如 "Illegal Function"）
    pub name: String,
    /// 中文含义
    pub meaning: String,
    /// 常见原因
    pub cause: String,
    /// 处理建议
    pub solution: String,
}

/// 常见故障排查清单
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct CommonIssue {
    /// 故障现象
    pub symptom: String,
    /// 排查项列表
    pub checks: Vec<String>,
}

/// Modbus 标准异常码表
const MODBUS_ERRORS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "01",
        "Illegal Function",
        "非法功能码",
        "从站不支持该功能码，或从站处于错误状态",
        "确认从站型号支持该功能码；检查从站是否处于运行状态",
    ),
    (
        "02",
        "Illegal Data Address",
        "非法数据地址",
        "请求的寄存器地址超出从站允许范围",
        "查阅设备手册确认有效地址范围；检查地址偏移（0-based vs 1-based）",
    ),
    (
        "03",
        "Illegal Data Value",
        "非法数据值",
        "请求的值不在从站允许范围内（如写入值超限）",
        "确认写入值的有效范围；检查字节序/字序是否正确",
    ),
    (
        "04",
        "Slave Device Failure",
        "从站设备故障",
        "从站处理请求时发生不可恢复的错误",
        "重启从站设备；检查从站是否有硬件故障",
    ),
    (
        "05",
        "Acknowledge",
        "确认",
        "从站已接受请求，正在处理（长耗时操作）",
        "稍后轮询结果；增大超时时间",
    ),
    (
        "06",
        "Slave Device Busy",
        "从站忙",
        "从站正在处理其他请求，无法响应",
        "稍后重试；降低请求频率；检查是否有多个主站竞争",
    ),
    (
        "08",
        "Memory Parity Error",
        "存储器奇偶校验错误",
        "从站存储器检测到错误",
        "重新写入数据；若反复出现需更换从站硬件",
    ),
    (
        "0A",
        "Gateway Path Unavailable",
        "网关路径不可用",
        "网关无法将请求路由到目标从站",
        "检查网关配置；确认从站在线",
    ),
    (
        "0B",
        "Gateway Target Device Failed to Respond",
        "网关目标设备无响应",
        "网关未收到目标从站的响应",
        "检查从站电源/通讯线/地址；确认从站协议匹配",
    ),
];

/// 常见故障排查清单
const COMMON_ISSUES: &[(&str, &[&str])] = &[
    (
        "通讯完全无响应",
        &[
            "电源是否正常",
            "通讯线 A/B 是否接反",
            "终端电阻是否安装",
            "从站地址是否正确",
            "波特率/校验位是否匹配",
        ],
    ),
    (
        "偶发性超时",
        &[
            "是否有多个主站竞争总线",
            "总线长度是否超限",
            "波特率是否过高",
            "是否有电磁干扰",
            "从站响应时间是否不稳定",
        ],
    ),
    (
        "数据值异常（非预期）",
        &[
            "字节序/字序是否正确",
            "数据类型是否匹配（float/int16/uint32）",
            "是否需要缩放/偏移转换",
            "寄存器地址是否偏移一位",
        ],
    ),
    (
        "返回异常码 02",
        &[
            "地址是否超出范围",
            "是否混淆了 0-based 和 1-based 地址",
            "功能码与地址是否匹配",
        ],
    ),
];

pub(crate) async fn list_modbus_errors(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ModbusError>>>, ApiError> {
    let list: Vec<ModbusError> = MODBUS_ERRORS
        .iter()
        .map(|(c, n, m, ca, so)| ModbusError {
            code: c.to_string(),
            name: n.to_string(),
            meaning: m.to_string(),
            cause: ca.to_string(),
            solution: so.to_string(),
        })
        .collect();
    Ok(Json(ApiResponse::ok(list)))
}

pub(crate) async fn list_common_issues(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<CommonIssue>>>, ApiError> {
    let list: Vec<CommonIssue> = COMMON_ISSUES
        .iter()
        .map(|(s, checks)| CommonIssue {
            symptom: s.to_string(),
            checks: checks.iter().map(|c| c.to_string()).collect(),
        })
        .collect();
    Ok(Json(ApiResponse::ok(list)))
}
