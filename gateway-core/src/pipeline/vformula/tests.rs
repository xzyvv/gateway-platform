use super::*;

fn vals(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn test_basic_arithmetic() {
    assert_eq!(
        apply_formula_multi("1 + 2 * 3", &HashMap::new()).unwrap(),
        7.0
    );
    assert_eq!(
        apply_formula_multi("(1 + 2) * 3", &HashMap::new()).unwrap(),
        9.0
    );
    assert_eq!(
        apply_formula_multi("10 - 4 - 3", &HashMap::new()).unwrap(),
        3.0
    );
    assert_eq!(apply_formula_multi("10 / 4", &HashMap::new()).unwrap(), 2.5);
}

#[test]
fn test_modulo_operator() {
    assert_eq!(apply_formula_multi("7 % 3", &HashMap::new()).unwrap(), 1.0);
    assert_eq!(
        apply_formula_multi("-7 % 3", &HashMap::new()).unwrap(),
        -1.0
    );
}

#[test]
fn test_power_is_right_associative() {
    // 2^(3^2) = 512（前端 `^` 亦为右结合）
    assert_eq!(
        apply_formula_multi("2^3^2", &HashMap::new()).unwrap(),
        512.0
    );
    assert_eq!(
        apply_formula_multi("2^10", &HashMap::new()).unwrap(),
        1024.0
    );
}

#[test]
fn test_unary_minus() {
    assert_eq!(apply_formula_multi("-3 + 5", &HashMap::new()).unwrap(), 2.0);
    assert_eq!(
        apply_formula_multi("-(2+3)", &HashMap::new()).unwrap(),
        -5.0
    );
}

#[test]
fn test_log_is_natural_logarithm() {
    let v = apply_formula_multi("LOG(2.718281828459045)", &HashMap::new()).unwrap();
    assert!((v - 1.0).abs() < 1e-6, "LOG 应为自然对数，实际 {}", v);
}

#[test]
fn test_avg_is_variadic() {
    assert_eq!(
        apply_formula_multi("AVG(1, 2, 3, 4)", &HashMap::new()).unwrap(),
        2.5
    );
    assert_eq!(
        apply_formula_multi("AVG(10)", &HashMap::new()).unwrap(),
        10.0
    );
}

#[test]
fn test_max_min_variadic() {
    assert_eq!(
        apply_formula_multi("MAX(1, 9, 5)", &HashMap::new()).unwrap(),
        9.0
    );
    assert_eq!(
        apply_formula_multi("MIN(1, 9, 5)", &HashMap::new()).unwrap(),
        1.0
    );
}

#[test]
fn test_round_with_decimals() {
    // 用变量传入待舍入值，避免字面量被 clippy 误判为近似 PI 常量
    let mut vars = HashMap::new();
    vars.insert("v".to_string(), 12.34567);
    assert_eq!(apply_formula_multi("ROUND(v, 2)", &vars).unwrap(), 12.35);
    assert_eq!(apply_formula_multi("ROUND(v)", &vars).unwrap(), 12.0);
}

#[test]
fn test_if_treats_non_zero_as_true() {
    assert_eq!(
        apply_formula_multi("IF(1, 10, 20)", &HashMap::new()).unwrap(),
        10.0
    );
    assert_eq!(
        apply_formula_multi("IF(0, 10, 20)", &HashMap::new()).unwrap(),
        20.0
    );
}

#[test]
fn test_constants() {
    let pi = apply_formula_multi("PI", &HashMap::new()).unwrap();
    assert!((pi - std::f64::consts::PI).abs() < 1e-6);
    let e = apply_formula_multi("E", &HashMap::new()).unwrap();
    assert!((e - std::f64::consts::E).abs() < 1e-6);
}

#[test]
fn test_point_reference_with_braces() {
    let v = apply_formula_multi("{voltage} * 2", &vals(&[("voltage", 110.0)])).unwrap();
    assert_eq!(v, 220.0);
}

#[test]
fn test_point_reference_value_suffix_is_normalized() {
    let a = apply_formula_multi("{p1.Value}", &vals(&[("p1", 3.0)])).unwrap();
    let b = apply_formula_multi("{p1.value}", &vals(&[("p1", 3.0)])).unwrap();
    let c = apply_formula_multi("{p1}", &vals(&[("p1", 3.0)])).unwrap();
    assert_eq!(a, 3.0);
    assert_eq!(b, 3.0);
    assert_eq!(c, 3.0);
}

#[test]
fn test_compiled_refs_are_deduplicated_in_order() {
    let c = compile("{a} + {b} + {a}").unwrap();
    assert_eq!(c.refs(), &["a".to_string(), "b".to_string()]);
}

#[test]
fn test_missing_point_reports_error() {
    let err = apply_formula_multi("{missing} + 1", &vals(&[("other", 1.0)])).unwrap_err();
    match err {
        FormulaError::MissingPoint(name) => assert_eq!(name, "missing"),
        other => panic!("期望 MissingPoint，实际 {:?}", other),
    }
}

#[test]
fn test_division_by_zero_is_error() {
    assert!(matches!(
        apply_formula_multi("1/0", &HashMap::new()),
        Err(FormulaError::Eval(_))
    ));
    assert!(matches!(
        apply_formula_multi("1%0", &HashMap::new()),
        Err(FormulaError::Eval(_))
    ));
}

#[test]
fn test_invalid_character_is_syntax_error() {
    assert!(matches!(
        apply_formula_multi("1 & 2", &HashMap::new()),
        Err(FormulaError::Syntax(_))
    ));
}

#[test]
fn test_unbalanced_paren_is_syntax_error() {
    assert!(matches!(
        apply_formula_multi("(1 + 2", &HashMap::new()),
        Err(FormulaError::Syntax(_))
    ));
}

#[test]
fn test_missing_closing_brace_is_syntax_error() {
    assert!(matches!(
        apply_formula_multi("{p1 + 1", &HashMap::new()),
        Err(FormulaError::Syntax(_))
    ));
}

#[test]
fn test_invalid_number_is_syntax_error() {
    // 多小数点写法（"1.2.3"）必须被判非法，不能被静默解析成 1.2
    assert!(matches!(
        apply_formula_multi("1.2.3 + 1", &HashMap::new()),
        Err(FormulaError::Syntax(_))
    ));
}

#[test]
fn test_empty_point_reference_is_syntax_error() {
    assert!(matches!(
        apply_formula_multi("{} + 1", &HashMap::new()),
        Err(FormulaError::Syntax(_))
    ));
}

#[test]
fn test_empty_formula_is_syntax_error() {
    assert!(matches!(
        apply_formula_multi("   ", &HashMap::new()),
        Err(FormulaError::Syntax(_))
    ));
}

#[test]
fn test_arity_error() {
    assert!(matches!(
        apply_formula_multi("ABS(1, 2)", &HashMap::new()),
        Err(FormulaError::Eval(_))
    ));
    assert!(matches!(
        apply_formula_multi("SQRT()", &HashMap::new()),
        Err(FormulaError::Eval(_))
    ));
}

#[test]
fn test_sqrt_and_exp_and_pow() {
    assert_eq!(
        apply_formula_multi("SQRT(16)", &HashMap::new()).unwrap(),
        4.0
    );
    assert_eq!(
        apply_formula_multi("POW(2, 8)", &HashMap::new()).unwrap(),
        256.0
    );
    assert!((apply_formula_multi("EXP(0)", &HashMap::new()).unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn test_ceil_floor() {
    assert_eq!(
        apply_formula_multi("CEIL(1.2)", &HashMap::new()).unwrap(),
        2.0
    );
    assert_eq!(
        apply_formula_multi("FLOOR(1.8)", &HashMap::new()).unwrap(),
        1.0
    );
}

#[test]
fn test_round_to_decimals_uses_half_up() {
    // 注意：JS `Math.round` 是「朝 +∞ 取整」（Math.round(-0.5) === -0），
    // 与 Rust `f64::round`（远离零）不同。Rust 侧必须用 `(x+0.5).floor()` 对齐 JS。
    assert_eq!(round_to_decimals(2.5, 0), 3.0);
    assert_eq!(round_to_decimals(-2.5, 0), -2.0);
    assert_eq!(round_to_decimals(-0.5, 0), 0.0);
    assert_eq!(round_to_decimals(2.344, 2), 2.34);
    // 2.345 在二进制中实际略小于 2.345，与 JS Math.round(2.345*100)/100 结果一致
    assert_eq!(round_to_decimals(2.345, 2), 2.35);
}

#[test]
fn test_result_is_normalized_to_six_decimals() {
    assert_eq!(
        apply_formula_multi("1/3", &HashMap::new()).unwrap(),
        0.333333
    );
}

#[test]
fn test_case_insensitive_function_names() {
    assert_eq!(
        apply_formula_multi("abs(-3)", &HashMap::new()).unwrap(),
        3.0
    );
    assert_eq!(
        apply_formula_multi("Abs(-3)", &HashMap::new()).unwrap(),
        3.0
    );
}
