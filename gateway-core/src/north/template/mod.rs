//! 北向上行 — JSON 模板渲染引擎
//!
//! 允许用户在网关配置中定义模板，使用 ${var} 占位符引用内部字段。
//!
//! ## 设计原则：裸值语义
//!
//! 所有 ${var} 渲染为 **裸字面量**（不带引号、不转义）：
//! - 数值变量直接写数字：${value} → 220.5
//! - 字符串变量直接写字面字符：${device_id} → meter1
//! - 用户需要在 JSON 字符串位置时，自行写 `\"${var}\"`，由 JSON 上下文提供引号和转义
//!
//! 这样设计的优点：
//! - 同一变量既可作 JSON key 也可作 JSON value
//! - 模板语法和 Go template / Mustache 一致
//! - 不会因为变量是否含特殊字符破坏外层结构
//!
//! ## 内置转义辅助
//!
//! 为方便起见，字符串变量在用作 JSON value 时通过 render_str 工具函数单独包装。
//! 通常做法：在模板中手动 `\"${var}\"`，再由模板编译器识别 `\"...\"` 上下文自动转义。
//! 当前实现采取 **最简方案**：变量裸渲染，由用户负责 JSON 语法。
//!
//! ## 模块拆分
//!
//! - `parser`：模板文本 → Segment 序列（编译）
//! - `registry`：变量名 → 值的查找表（TemplateVars / ExtraValue）
//! - `renderer`：Segment + TemplateVars → 输出（执行）
//!
//! ## 示例
//!
//! ```ignore
//! use gateway_core::north::template::{TemplateEngine, TemplateVars};
//!
//! let engine = TemplateEngine::new(
//!     "{\"ts\":${timestamp_ms},\"device\":\"${device_id}\",\"value\":${value}}"
//! ).unwrap();
//! let mut vars = TemplateVars::new();
//! vars.timestamp_ms = 1727936400000;
//! vars.device_id = "meter1".into();
//! vars.value = 220.5;
//! let out = engine.render(&vars);
//! assert_eq!(out, "{\"ts\":1727936400000,\"device\":\"meter1\",\"value\":220.5}");
//! ```

mod parser;
mod registry;
mod renderer;

pub use parser::{TemplateEngine, TemplateError};
pub use registry::{ExtraValue, TemplateVars, MAX_EXTRA_ENTRIES};
pub use renderer::render_str;

/// 数字渲染（NaN/Inf 输出 null 以保证 JSON 合法）
///
/// 私有给 `registry` / `renderer` 共用，避免循环依赖。
fn render_num(n: f64) -> String {
    if n.is_finite() {
        if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
            format!("{}", n as i64)
        } else {
            let s = format!("{}", n);
            if s.contains('.') || s.contains('e') || s.contains('E') {
                s
            } else {
                format!("{}.0", s)
            }
        }
    } else {
        "null".to_string()
    }
}

#[cfg(test)]
mod tests;
