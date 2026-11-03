//! 配置 JSON Schema 校验
//!
//! 用 `schemars` 从 `GatewayConfig` 派生 JSON Schema（单一真源，不与代码漂移），
//! 用 `jsonschema` crate 在 serde 反序列化之前校验原始 JSON `Value`，
//! 给出精确的结构错误（含 JSON Pointer 路径）。
//!
//! 与现有 `validator` 派生 + `validate_all` 语义校验分层互补：
//! - JSON Schema：结构层（必填字段、类型、枚举值）
//! - validator：字段值规则（波特率白名单、QoS 范围等）
//! - validate_all：跨字段业务逻辑（slave_id 范围、采集周期冲突）
//!
//! 迁移安全：schemars 默认不加 `additionalProperties:false`，容忍旧字段；
//! `required` 字段集与 serde 必填集一致，不会拒绝 serde 能接受的配置。
//! Schema 校验在 migrate 之前运行，旧配置额外字段被允许。

use std::sync::OnceLock;

use schemars::schema_for;

use super::GatewayConfig;

/// 全局缓存的 JSON Schema（运行期一次性构造，微秒级）。
static SCHEMA_VALUE: OnceLock<serde_json::Value> = OnceLock::new();

/// 返回 `GatewayConfig` 的 JSON Schema（schemars 派生，编译期生成代码）。
// 经审计保留的 expect（见下方说明）：函数返回 `&'static`（OnceLock 缓存）无法传播 `Result`，
// 且 schemars 派生类型仅为字符串键 map 与标量，序列化不会失败；启动期 fail-fast 优于静默降级。
#[allow(clippy::expect_used)]
pub fn schema() -> &'static serde_json::Value {
    SCHEMA_VALUE.get_or_init(|| {
        let schema = schema_for!(GatewayConfig);
        // 经审计保留的 expect：schemars 派生类型仅含字符串键 map 与标量，序列化不会失败；
        // 本函数返回 &'static（OnceLock 缓存）无法传播 Result。若不变量被破坏，
        // 启动期 fail-fast 优于静默降级——损坏的 schema 会让结构校验形同虚设。
        serde_json::to_value(&schema).expect("schema serialization must not fail")
    })
}

/// 校验原始 JSON Value 是否符合 `GatewayConfig` Schema。
///
/// 收集**全部**错误（非短路），每个错误带 JSON Pointer 路径，
/// 返回 `Vec<String>` 与现有 `validate_all` 风格一致，便于一次性向运维报告所有问题。
pub fn validate_value(value: &serde_json::Value) -> Result<(), Vec<String>> {
    let validator = match jsonschema::validator_for(schema()) {
        Ok(v) => v,
        Err(e) => return Err(vec![format!("build schema validator failed: {e}")]),
    };
    let mut errors: Vec<String> = Vec::new();
    for err in validator.iter_errors(value) {
        let p = err.instance_path().as_str();
        if p.is_empty() {
            errors.push(format!("{err}"));
        } else {
            errors.push(format!("{p}: {err}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// 导出 schema 到文件（供前端 Vue 用 ajv 消费做表单校验/生成）。
pub fn export_to_file(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(schema())?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests;
