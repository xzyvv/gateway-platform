//! 统一错误码表（业务码的唯一来源）
//!
//! 此前码值散落在各 handler / 中间件里（`ApiResponse::err(4003, ..)`、
//! `ApiError { code: 8005, .. }`），前端 `errorCodes.ts` 再手工抄一份，
//! 结果是「后端加码、前端没登记」「同一语义两个码」反复发生。本模块把
//! **码值 + 含义 + HTTP 映射**收敛到一处：
//!
//! - 后端新增错误码：先在这里加常量并登记到 [`ALL_CODES`]
//! - 前端照 `ALL_CODES` 补文案（有 `all_literal_codes_in_source_are_registered`
//!   测试兜住「源码里写死但没登记」的漏网之鱼）
//!
//! ## 分段约定（改段前先想清楚 HTTP 语义）
//!
//! | 段 | 语义 | HTTP |
//! |---|---|---|
//! | `0` | 成功 | 200 |
//! | `1xxx` | 配置 / 序列化 | 500 |
//! | `2xxx` | 本地 IO | 500 |
//! | `3xxx` | 协议 / 对端无应答 | 500；3001 → 504 |
//! | `4xxx` | 客户端可纠正 | 400；4290 → 429 |
//! | `5xxx` | 服务端执行失败 | 500；5003 → 503 |
//! | `6xxx` | 业务校验（设备分组） | 400 |
//! | `7xxx` | 授权 | 403 |
//! | `8xxx` | 鉴权 | 8000-8002 → 401；8003-8099 → 403 |
//! | `9xxx` | 历史遗留「不存在」 | 500（仅兼容，新代码用 4004） |
//! | `9999` | 内部错误 | 500 |

use axum::http::StatusCode;

// ===== 0 / 1xxx / 2xxx / 3xxx =====

/// 成功（`code == 0` 是唯一成功码）
pub const OK: i32 = 0;
/// 配置错误 **或** 请求体 JSON 解析失败
pub const CONFIG_OR_JSON: i32 = 1000;
/// 本地 IO 失败
pub const IO: i32 = 2000;
/// 协议层错误（帧非法 / CRC / 编解码）
pub const PROTOCOL: i32 = 3000;
/// 设备或总线在超时内没有应答（HTTP 504）
pub const NO_REPLY: i32 = 3001;

// ===== 4xxx：客户端可纠正 =====

/// 参数非法（`ApiError::bad_request` 的默认码）
pub const BAD_REQUEST: i32 = 4000;
/// 未指定总线 / 设备必填字段为空
pub const BUS_MISSING: i32 = 4001;
/// 参数校验未通过：点位字段、协议不支持等
pub const INVALID_PARAM: i32 = 4002;
/// 配置应用失败（校验通过但落盘/热重载失败）
pub const CONFIG_APPLY_FAILED: i32 = 4003;
/// 资源不存在（设备 / 总线 / 点位 / 版本）
pub const NOT_FOUND: i32 = 4004;
/// 用户域：用户名或显示名不合法（保留，新代码用 4000）
pub const USER_INVALID: i32 = 4005;
/// 用户域：用户不存在（保留，新代码用 4004）
pub const USER_NOT_FOUND: i32 = 4006;
/// 用户域：角色不合法（保留，新代码用 4000）
pub const ROLE_INVALID: i32 = 4007;
/// id 冲突（恢复点位时目标 id 已存在）
pub const DUPLICATE_ID: i32 = 4009;
/// 授权有效但档位配额不足
pub const QUOTA_EXCEEDED: i32 = 4021;
/// 总线忙（协议工具会话租约冲突；HTTP 200 + 业务码）
pub const BUS_BUSY: i32 = 4090;
/// 请求 Content-Type 不受支持（由 envelope 从 HTTP 415 归一）
pub const UNSUPPORTED_MEDIA_TYPE: i32 = 4150;
/// 请求体结构不合法（由 envelope 从 HTTP 422 归一）
pub const UNPROCESSABLE: i32 = 4220;
/// 请求过于频繁（限流中间件；HTTP 429 + `Retry-After`）
pub const RATE_LIMITED: i32 = 4290;
/// 总线离线（协议工具会话；HTTP 200 + 业务码）
pub const BUS_OFFLINE: i32 = 4503;

// ===== 5xxx：服务端执行失败 =====

/// MQTT 北向发布失败
pub const MQTT: i32 = 5000;
/// 上传的证书无法解析
pub const CERT_PARSE_FAILED: i32 = 5001;
/// 依赖能力暂不可用（HTTP 503）
pub const SERVICE_UNAVAILABLE: i32 = 5003;
/// 点位写入**明确失败**（能证明写没有生效，可安全重试）
pub const WRITE_FAILED: i32 = 5101;
/// 点位写入**结果未知**（设备可能已执行，禁止自动重试，先读回核实）
pub const WRITE_UNCERTAIN: i32 = 5102;

// ===== 6xxx / 7xxx / 8xxx / 9xxx =====

/// 设备分组名称非法或重复
pub const GROUP_INVALID: i32 = 6001;
/// 设备分组不存在
pub const GROUP_NOT_FOUND: i32 = 6002;
/// 公式求值失败
pub const FORMULA: i32 = 7000;
/// License 未导入
pub const LICENSE_REQUIRED: i32 = 7001;
/// License 失效（过期 / 设备码不符 / 篡改 / 验签失败）
pub const LICENSE_INVALID: i32 = 7003;
/// 未认证 / token 失效
pub const UNAUTHORIZED: i32 = 8000;
/// API Key 缺少写权限
pub const API_KEY_NO_WRITE: i32 = 8003;
/// API Key 缺少所需 scope（如 diagnostics:exec）
pub const API_KEY_SCOPE: i32 = 8004;
/// 拒绝通过普通配置写修改 web 凭据等敏感字段
pub const SENSITIVE_FIELD_DENIED: i32 = 8005;
/// 实时 WebSocket 连接数超限（HTTP 503）
pub const REALTIME_LIMIT: i32 = 8101;
/// 历史遗留的「不存在」码（仅兼容，新代码用 4004）
pub const NOT_FOUND_LEGACY: i32 = 9000;
/// 网关内部错误
pub const INTERNAL: i32 = 9999;

/// 全部已知业务码（不含成功码 0）及其含义。
///
/// 前端 `errorCodes.ts` 与本表保持同步；新增码必须同时登记，否则
/// `all_literal_codes_in_source_are_registered` 测试会失败。
pub const ALL_CODES: &[(i32, &str)] = &[
    (CONFIG_OR_JSON, "配置或请求体解析失败"),
    (IO, "本地 IO 失败"),
    (PROTOCOL, "协议层错误"),
    (NO_REPLY, "设备或总线未应答"),
    (BAD_REQUEST, "参数非法"),
    (BUS_MISSING, "未指定总线或必填字段为空"),
    (INVALID_PARAM, "参数校验未通过"),
    (CONFIG_APPLY_FAILED, "配置应用失败"),
    (NOT_FOUND, "资源不存在"),
    (USER_INVALID, "用户名或显示名不合法"),
    (USER_NOT_FOUND, "用户不存在"),
    (ROLE_INVALID, "角色不合法"),
    (DUPLICATE_ID, "id 已存在"),
    (QUOTA_EXCEEDED, "授权配额不足"),
    (BUS_BUSY, "总线忙"),
    (UNSUPPORTED_MEDIA_TYPE, "Content-Type 不受支持"),
    (UNPROCESSABLE, "请求体结构不合法"),
    (RATE_LIMITED, "请求过于频繁"),
    (BUS_OFFLINE, "总线离线"),
    (MQTT, "MQTT 发布失败"),
    (CERT_PARSE_FAILED, "证书解析失败"),
    (SERVICE_UNAVAILABLE, "依赖能力暂不可用"),
    (WRITE_FAILED, "点位写入失败（可安全重试）"),
    (WRITE_UNCERTAIN, "点位写入结果未知（先读回核实）"),
    (GROUP_INVALID, "设备分组名称不合法"),
    (GROUP_NOT_FOUND, "设备分组不存在"),
    (FORMULA, "公式求值失败"),
    (LICENSE_REQUIRED, "尚未导入授权"),
    (LICENSE_INVALID, "授权已失效"),
    (UNAUTHORIZED, "未认证或登录已失效"),
    (API_KEY_NO_WRITE, "API 密钥缺少写权限"),
    (API_KEY_SCOPE, "API 密钥权限范围不足"),
    (SENSITIVE_FIELD_DENIED, "敏感字段拒绝修改"),
    (REALTIME_LIMIT, "实时连接数超限"),
    (NOT_FOUND_LEGACY, "资源不存在（历史码）"),
    (INTERNAL, "网关内部错误"),
];

/// 是否为已知业务码（成功码 0 不算「错误码」）。
pub fn is_known(code: i32) -> bool {
    ALL_CODES.iter().any(|(c, _)| *c == code)
}

/// 业务码 → HTTP 状态码。
///
/// 这是 [`crate::web::types::ApiResponse`] 的唯一映射入口：业务码只要走
/// `ApiResponse::err`，HTTP 状态就不必在 handler 里重复判断。
///
/// 与 `ApiError` 手工设置 `status` 的路径相比，本函数保证「同一码永远同一状态」。
pub fn http_status(code: i32) -> StatusCode {
    match code {
        OK => StatusCode::OK,
        // 对端（现场设备/总线）未应答是上游超时，不是网关自身故障
        NO_REPLY => StatusCode::GATEWAY_TIMEOUT,
        // 限流：必须是 429，否则前端「稍后再试」的分支不生效
        RATE_LIMITED => StatusCode::TOO_MANY_REQUESTS,
        // 依赖能力/连接数上限：503 表达「稍后可能恢复」，与 500 区分
        SERVICE_UNAVAILABLE | REALTIME_LIMIT => StatusCode::SERVICE_UNAVAILABLE,
        4000..=4999 => StatusCode::BAD_REQUEST,
        6000..=6999 => StatusCode::BAD_REQUEST,
        7000..=7999 => StatusCode::FORBIDDEN,
        8000..=8002 => StatusCode::UNAUTHORIZED,
        8003..=8099 => StatusCode::FORBIDDEN,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for (code, _) in ALL_CODES {
            assert!(seen.insert(*code), "重复登记的错误码: {code}");
        }
    }

    #[test]
    fn http_status_follows_segment_rules() {
        // 客户端可纠正
        for code in [
            BAD_REQUEST,
            NOT_FOUND,
            BUS_BUSY,
            UNSUPPORTED_MEDIA_TYPE,
            GROUP_NOT_FOUND,
        ] {
            assert_eq!(http_status(code), StatusCode::BAD_REQUEST, "code={code}");
        }
        // 限流必须是 429
        assert_eq!(http_status(RATE_LIMITED), StatusCode::TOO_MANY_REQUESTS);
        // 授权：统一 403，避免走 ApiResponse 时被误映射成 500
        for code in [LICENSE_REQUIRED, LICENSE_INVALID, FORMULA] {
            assert_eq!(http_status(code), StatusCode::FORBIDDEN, "code={code}");
        }
        // 鉴权：未认证 401，权限不足 403
        assert_eq!(http_status(UNAUTHORIZED), StatusCode::UNAUTHORIZED);
        for code in [API_KEY_NO_WRITE, API_KEY_SCOPE, SENSITIVE_FIELD_DENIED] {
            assert_eq!(http_status(code), StatusCode::FORBIDDEN, "code={code}");
        }
        // 依赖能力
        assert_eq!(
            http_status(SERVICE_UNAVAILABLE),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(http_status(REALTIME_LIMIT), StatusCode::SERVICE_UNAVAILABLE);
        // 上游超时
        assert_eq!(http_status(NO_REPLY), StatusCode::GATEWAY_TIMEOUT);
        // 服务端
        for code in [
            CONFIG_OR_JSON,
            IO,
            PROTOCOL,
            WRITE_FAILED,
            WRITE_UNCERTAIN,
            INTERNAL,
        ] {
            assert_eq!(
                http_status(code),
                StatusCode::INTERNAL_SERVER_ERROR,
                "code={code}"
            );
        }
        assert_eq!(http_status(OK), StatusCode::OK);
    }

    /// 防漂移：`src/web` 下直接写字面量码的地方（`::err(4003`、`code: 8005`）
    /// 必须已登记到 [`ALL_CODES`]。新增码忘了登记时这里会失败。
    #[test]
    fn all_literal_codes_in_source_are_registered() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web");
        let mut files = Vec::new();
        collect_rs(&root, &mut files);
        assert!(!files.is_empty(), "没找到源码文件，路径不对？");

        let mut unknown = Vec::new();
        for file in files {
            // 本文件就是码表定义与示例的所在地，扫描它只会命中常量定义本身。
            if file.file_name().and_then(|n| n.to_str()) == Some("error_codes.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&file).expect("read source");
            for code in extract_literal_codes(&src) {
                if !is_known(code) {
                    unknown.push((file.display().to_string(), code));
                }
            }
        }
        assert!(
            unknown.is_empty(),
            "源码中出现了未登记的错误码: {unknown:?}"
        );
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// 提取 `::err(1234` 与 `code: 1234` 两种字面量写法。
    ///
    /// 只看 3 位以上的数字：`code: 0` 是成功码，不属于错误码。
    fn extract_literal_codes(src: &str) -> Vec<i32> {
        let mut out = Vec::new();
        for marker in ["::err(", "code:"] {
            let mut rest = src;
            while let Some(idx) = rest.find(marker) {
                let after = &rest[idx + marker.len()..];
                let digits: String = after
                    .chars()
                    .skip_while(|c| c.is_whitespace())
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if digits.len() >= 3 {
                    if let Ok(v) = digits.parse::<i32>() {
                        out.push(v);
                    }
                }
                rest = &rest[idx + marker.len()..];
            }
        }
        out
    }
}
