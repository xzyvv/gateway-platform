//! 采集错误码（`CollectErrorCode`）→ 对外 API 业务码的映射。
//!
//! 网关有两套「错误码」，服务不同场景，此前没有任何对应关系：
//!
//! - 统计层 `CollectErrorCode`：给 Prometheus / Grafana 用的**字符串标签**
//!   （`TIMEOUT`、`CRC`、`NO_REPLY`…），按总线聚合后写入 `last_error_code`；
//! - API 业务码（[`crate::web::error_codes`]）：给客户端分支用的**稳定数字码**。
//!
//! 现场看到 `last_error_code=TIMEOUT` 后，运维翻不到该查哪个错误码文档；
//! 反过来客户端拿到 3001 也不知道它会落进哪个统计桶。本模块固化这层映射，
//! 由测试保证「每个统计标签都有确定的对外码」。

use gateway_core::engine::metrics::CollectErrorCode;

use crate::web::error_codes as codes;

/// 统计标签对应的对外错误码。
///
/// 原则：只回答「这个统计标签在对外契约里等价于哪个码」，不反向定义统计口径。
/// 多个标签合并到同一码是刻意的——客户端处置动作相同（如 CRC 与解码失败都要查现场干扰）。
pub fn api_code(err: CollectErrorCode) -> i32 {
    match err {
        // 设备在超时内没有应答：对外统一 3001（HTTP 504）
        CollectErrorCode::Timeout | CollectErrorCode::NoReply => codes::NO_REPLY,
        CollectErrorCode::Io => codes::IO,
        // 帧校验 / 解码 / 从站异常：都是「收到的东西不能用」→ 协议错误
        CollectErrorCode::Checksum
        | CollectErrorCode::Decode
        | CollectErrorCode::ModbusException(_) => codes::PROTOCOL,
        CollectErrorCode::BusClosed => codes::BUS_OFFLINE,
        CollectErrorCode::Busy => codes::BUS_BUSY,
        CollectErrorCode::Config => codes::CONFIG_OR_JSON,
        CollectErrorCode::Other => codes::INTERNAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::engine::metrics::CollectErrorCode;

    const ALL: [CollectErrorCode; 10] = [
        CollectErrorCode::Timeout,
        CollectErrorCode::NoReply,
        CollectErrorCode::Io,
        CollectErrorCode::Checksum,
        CollectErrorCode::Decode,
        CollectErrorCode::BusClosed,
        CollectErrorCode::Busy,
        CollectErrorCode::ModbusException(2),
        CollectErrorCode::Config,
        CollectErrorCode::Other,
    ];

    /// 每个统计标签都必须映射到已登记码，且分类与「该改什么」一致。
    #[test]
    fn every_collect_error_maps_to_registered_code() {
        for err in ALL {
            let code = api_code(err);
            assert!(codes::is_known(code), "{err:?} → {code} 未登记");
            assert_ne!(codes::category(code), "ok", "{err:?} 不能映射成成功");
        }
    }

    #[test]
    fn device_side_labels_map_to_device_categories() {
        for err in [
            CollectErrorCode::Timeout,
            CollectErrorCode::NoReply,
            CollectErrorCode::Checksum,
            CollectErrorCode::Decode,
            CollectErrorCode::BusClosed,
            CollectErrorCode::Busy,
        ] {
            assert_eq!(
                codes::category(api_code(err)),
                "device",
                "{err:?} 应让运维去看现场"
            );
        }
        assert_eq!(
            codes::category(api_code(CollectErrorCode::Config)),
            "client"
        );
    }
}
