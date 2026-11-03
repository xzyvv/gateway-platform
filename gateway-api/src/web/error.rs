//! Web API 错误类型
//!
//! 统一 API 错误响应：HTTP 状态码 + 业务错误码 + 消息。
//! 响应体同时包含 `error`（向后兼容）与 `code`/`message`（与 ApiResponse 一致），
//! 前端 axios 拦截器可统一处理。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};

/// API 错误
pub struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: u32,
    pub(crate) message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: 4000,
            message: msg.into(),
        }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: 4004,
            message: msg.into(),
        }
    }
    /// 状态冲突：当前系统状态不允许该操作（如「已在运行」却又要求启动）。
    ///
    /// 与 `bad_request` 的区别：400 表示「参数本身不对」，409 表示「参数没问题，
    /// 但先要改变系统状态（停掉正在跑的实例 / 关掉冲突的通道）才能做」。
    /// 业务码沿用客户端可纠正族 4xxx（4009），message 由后端给出明确原因。
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: 4009,
            message: msg.into(),
        }
    }
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: 8000,
            message: msg.into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: 9999,
            message: msg.into(),
        }
    }
    /// License 未导入（无授权文件时的配额/能力拒绝）
    ///
    /// v2.0 语义：没有 license 文件时按「开源版兜底配额」运行
    /// （`config/license.rs` 的 `DEFAULT_ABSENT_QUOTA`：100 点 / 1 协议 / 1 通道）。
    /// 一旦超出该配额，处置动作是**导入授权文件**（而非升级档位），
    /// 故与「授权有效但配额不够」区分开。
    pub fn license_required() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: 7001,
            message: "license required".into(),
        }
    }
    /// License 失效（Expired / Mismatch / Tampered / Invalid 统一对外码）
    pub fn license_expired() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: 7003,
            message: "license expired or invalid".into(),
        }
    }
    /// v2.0 配额超限：当前档位支持最多 N 个，本操作后将达 M > N
    /// 状态码 400（业务校验失败），code 4021 客户端可识别
    pub fn quota_exceeded(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: 4021,
            message: msg.into(),
        }
    }

    /// 按**授权状态**把一次配额拒绝翻译成可执行的错误码。
    ///
    /// v2.0 只有配额约束（无 feature / 档位中间件，见
    /// `docs/design/license-v2-quota.md` §"删除 require_tier_* 中间件"），
    /// 但「为什么不够」的处置动作完全取决于授权状态，三者必须区分：
    ///
    /// | 授权状态 | 码 | 现场看到的处置 |
    /// |---|---|---|
    /// | 失效（过期/设备码不符/被篡改/验签失败） | 7003 | 重新导入或续期授权 |
    /// | 从未导入 | 7001 | 导入 .lic 文件 |
    /// | 有效（档位配额不够） | 4021 | 联系供应商升级档位 |
    ///
    /// 旧实现一律返回 4021「当前授权配额不足」，于是：
    /// - 没导入授权的用户被引导去「升级档位」（错误方向，且他根本没有档位）；
    /// - 授权已过期的用户被告知「配额不足」（真实原因是文件失效）。
    ///
    /// message 取值规则：只有 4021 分支用 `quota_msg`（"当前档位支持最多 N 采集点数，
    /// 当前已 M"）。7001/7003 的拒绝根因是**授权状态本身**，此时配额数字来自兜底值
    /// （无授权时是 `DEFAULT_ABSENT_QUOTA` 的 100 点），把它写进 message 反而误导
    /// 现场去"加点位"而不是"导授权"。7001/7003 用固定英文短句，前端
    /// `BACKEND_MESSAGE_ZH` 已有对应中文翻译。
    pub fn license_denied(state: &gateway_core::license::LicenseState, quota_msg: String) -> Self {
        if state.is_degraded() {
            Self::license_expired()
        } else if !state.present {
            Self::license_required()
        } else {
            Self::quota_exceeded(quota_msg)
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "code": self.code,
            "message": self.message,
            "error": self.message,
        });
        (self.status, Json(body)).into_response()
    }
}

/// 将 GatewayError 映射为合适的 HTTP 状态码 + 业务码
impl From<gateway_core::engine::error::GatewayError> for ApiError {
    fn from(e: gateway_core::engine::error::GatewayError) -> Self {
        let code = e.code();
        let status = match &e {
            gateway_core::engine::error::GatewayError::Config(_) => StatusCode::BAD_REQUEST,
            gateway_core::engine::error::GatewayError::Auth(_) => StatusCode::UNAUTHORIZED,
            gateway_core::engine::error::GatewayError::NotFound(_) => StatusCode::NOT_FOUND,
            gateway_core::engine::error::GatewayError::Protocol(_)
            | gateway_core::engine::error::GatewayError::NoReply
            | gateway_core::engine::error::GatewayError::Io(_)
            | gateway_core::engine::error::GatewayError::Queue(_)
            | gateway_core::engine::error::GatewayError::Mqtt(_)
            | gateway_core::engine::error::GatewayError::Formula(_)
            | gateway_core::engine::error::GatewayError::Message(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        Self {
            status,
            code,
            message: e.to_string(),
        }
    }
}

// 常见错误类型直接转 ApiError，减少 handler 样板
impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: 1000,
            message: e.to_string(),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: 2000,
            message: e.to_string(),
        }
    }
}
