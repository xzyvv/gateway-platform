//! OPC UA 协议辅助函数
//!
//! OPC UA 使用 opcua crate 处理底层通信，无原始帧编解码。
//! 本模块提供 NodeId 字符串解析与 DataValue 值提取等辅助函数。

use opcua::types::{DataValue, NodeId};

use crate::south::protocol::{CollectorError, CollectorResult};

/// 解析 Node ID 字符串
///
/// 支持格式：
/// - "ns=2;s=Temperature" -> 字符串节点
/// - "ns=2;i=1001" -> 数字节点
/// - "2,Temperature" -> 简写（ns=2, s=Temperature）
pub(crate) fn parse_node_id(s: &str) -> CollectorResult<NodeId> {
    let s = s.trim();
    if s.starts_with("ns=") {
        let parts: Vec<&str> = s.splitn(2, ';').collect();
        if parts.len() != 2 {
            return Err(CollectorError::InvalidConfig(format!(
                "invalid OPC UA node id '{}', expected ns=N;s=X or ns=N;i=X",
                s
            )));
        }
        let ns: u16 = parts[0]
            .strip_prefix("ns=")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| {
                CollectorError::InvalidConfig(format!("invalid namespace in '{}'", s))
            })?;
        let id_part = parts[1];
        if let Some(v) = id_part.strip_prefix("s=") {
            Ok(NodeId::new(ns, v))
        } else if let Some(v) = id_part.strip_prefix("i=") {
            let id: u32 = v.parse().map_err(|_| {
                CollectorError::InvalidConfig(format!("invalid numeric id in '{}'", s))
            })?;
            Ok(NodeId::new(ns, id))
        } else if let Some(v) = id_part.strip_prefix("g=") {
            Ok(NodeId::new(ns, v))
        } else {
            Err(CollectorError::InvalidConfig(format!(
                "invalid node id format in '{}', expected s=, i=, or g=",
                s
            )))
        }
    } else {
        let parts: Vec<&str> = s.splitn(2, ',').collect();
        if parts.len() != 2 {
            return Err(CollectorError::InvalidConfig(format!(
                "invalid OPC UA node id '{}', use ns=N;s=X or ns=2,Temperature",
                s
            )));
        }
        let ns: u16 = parts[0]
            .parse()
            .map_err(|_| CollectorError::InvalidConfig(format!("invalid namespace in '{}'", s)))?;
        let id = parts[1].trim();
        if let Ok(num) = id.parse::<u32>() {
            Ok(NodeId::new(ns, num))
        } else {
            Ok(NodeId::new(ns, id))
        }
    }
}

/// 从 DataValue 提取 f64
pub(crate) fn extract_value(dv: &DataValue) -> CollectorResult<f64> {
    match &dv.value {
        Some(variant) => match variant {
            opcua::types::Variant::Double(v) => Ok(*v),
            opcua::types::Variant::Float(v) => Ok(*v as f64),
            opcua::types::Variant::Int16(v) => Ok(*v as f64),
            opcua::types::Variant::UInt16(v) => Ok(*v as f64),
            opcua::types::Variant::Int32(v) => Ok(*v as f64),
            opcua::types::Variant::UInt32(v) => Ok(*v as f64),
            opcua::types::Variant::Int64(v) => Ok(*v as f64),
            opcua::types::Variant::UInt64(v) => Ok(*v as f64),
            opcua::types::Variant::Boolean(v) => Ok(if *v { 1.0 } else { 0.0 }),
            opcua::types::Variant::String(s) => s.as_ref().parse::<f64>().map_err(|_| {
                CollectorError::InvalidFrame(format!("cannot parse string '{}' as f64", s))
            }),
            _ => Err(CollectorError::Unsupported(format!(
                "unsupported OPC UA variant type: {:?}",
                variant.type_id()
            ))),
        },
        None => Err(CollectorError::InvalidFrame(
            "OPC UA DataValue has no value".into(),
        )),
    }
}
