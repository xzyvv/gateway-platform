//! 公式引擎（§3.7 formula 字段）
//!
//! 基于 evalexpr crate，支持加减乘除组合运算。
//! 公式中 `raw` 变量表示原始解析值。
//!
//! 示例："raw*0.1-50"

use evalexpr::{context_map, eval_float_with_context, Value};

use crate::engine::error::{GatewayError, GatewayResult};

/// 公式白名单校验：只允许字符 `0-9 . + - * / ( )`、空白，以及连续的 `raw` 标识符。
fn is_formula_safe(formula: &str) -> bool {
    let bytes = formula.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'r' && i + 3 <= bytes.len() && &bytes[i..i + 3] == b"raw" {
            if i + 3 < bytes.len() {
                let next = bytes[i + 3];
                let next_ok = next.is_ascii_whitespace()
                    || matches!(next, b'+' | b'-' | b'*' | b'/' | b'(' | b')');
                if !next_ok {
                    return false;
                }
            }
            i += 3;
            continue;
        }
        let ok = b.is_ascii_digit()
            || matches!(b, b'.' | b'+' | b'-' | b'*' | b'/' | b'(' | b')')
            || b.is_ascii_whitespace();
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, level = "debug"))]
pub fn apply_formula(formula: &str, raw: f64) -> GatewayResult<f64> {
    if !is_formula_safe(formula) {
        return Err(GatewayError::Formula(format!(
            "formula rejected by allowlist (only digits and +-*/() with 'raw' are allowed): {}",
            formula
        )));
    }

    let context = context_map! {
        "raw" => Value::Float(raw),
    }
    .map_err(|e| GatewayError::Formula(format!("formula context error: {}", e)))?;

    let result = eval_float_with_context(formula, &context)
        .map_err(|e| GatewayError::Formula(format!("formula eval error ({}): {}", formula, e)))?;

    if !result.is_finite() {
        return Err(GatewayError::Formula(format!(
            "formula produced non-finite result (raw={}, formula={}): {}",
            raw, formula, result
        )));
    }

    Ok(result)
}

#[cfg(test)]
mod tests;
