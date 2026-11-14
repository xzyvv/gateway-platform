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

// ===== 词法扫描终止性回归（mutation 防线）=====
//
// 词法扫描一旦「下标原地不动」就会无限循环，并把 tokens 撑到内存爆掉。
// 下面两条用例把「扫描必须有界收敛」钉死：实现一旦退回按下标死循环的写法，
// 这里会直接挂住（测试超时）而不是安静通过。
#[test]
fn test_surrounding_whitespace_is_ignored() {
    assert_eq!(
        apply_formula_multi("  1 + 2  ", &HashMap::new()).unwrap(),
        3.0
    );
    assert_eq!(
        apply_formula_multi("\t1\t+\t2\n", &HashMap::new()).unwrap(),
        3.0
    );
}

#[test]
fn test_pathological_inputs_terminate_within_bounded_scan() {
    // 退化输入（纯空白 / 长数字串 / 长运算符串 / 未闭合引用）必须快速收敛：
    // 要么编译成功，要么返回语法错误，不允许卡死。
    let cases = [
        " ".repeat(1024),
        "1".repeat(1024),
        "+".repeat(512),
        "{".repeat(256),
        "{a".repeat(256),
    ];
    for src in &cases {
        let _ = compile(src);
    }
}

// ===== 跨语言用例契约（O-7）=====
//
// 用例集在仓库根 `shared/formula_cases.json`，前端
// `gateway-web/src/utils/formulaEngine.spec.ts` 读的是**同一个文件**。
// 语义要改就改这份契约：只改单侧测试，另一侧会立刻转红。
//
// 只放两侧都成立的行为。单侧特有的（函数元数校验、错误文本）留在各自语言的测试里。
const SHARED_CASES: &str = include_str!("../../../../shared/formula_cases.json");

#[derive(serde::Deserialize)]
struct SharedCase {
    name: String,
    expr: String,
    #[serde(default)]
    args: HashMap<String, f64>,
    #[serde(default)]
    expected: Option<f64>,
    #[serde(default)]
    tolerance: Option<f64>,
    #[serde(default)]
    refs: Option<Vec<String>>,
    #[serde(default)]
    error: Option<String>,
}

#[test]
fn test_shared_cross_language_cases() {
    let doc: serde_json::Value =
        serde_json::from_str(SHARED_CASES).expect("共享用例 JSON 解析失败");
    let cases: Vec<SharedCase> =
        serde_json::from_value(doc["cases"].clone()).expect("共享用例 cases 字段解析失败");
    assert!(!cases.is_empty(), "共享用例集为空");

    for c in &cases {
        let label = &c.name;
        match c.error.as_deref() {
            Some("syntax") => {
                assert!(
                    matches!(
                        apply_formula_multi(&c.expr, &c.args),
                        Err(FormulaError::Syntax(_))
                    ),
                    "[{}] 期望语法错误: {}",
                    label,
                    c.expr
                );
            }
            Some("eval") => {
                assert!(
                    matches!(
                        apply_formula_multi(&c.expr, &c.args),
                        Err(FormulaError::Eval(_))
                    ),
                    "[{}] 期望求值错误: {}",
                    label,
                    c.expr
                );
            }
            Some("missing") => {
                assert!(
                    matches!(
                        apply_formula_multi(&c.expr, &c.args),
                        Err(FormulaError::MissingPoint(_))
                    ),
                    "[{}] 期望缺值错误: {}",
                    label,
                    c.expr
                );
            }
            Some(other) => panic!("[{}] 未知的 error 类别: {}", label, other),
            None => {
                if let Some(refs) = &c.refs {
                    let compiled =
                        compile(&c.expr).unwrap_or_else(|e| panic!("[{}] 编译失败: {}", label, e));
                    assert_eq!(
                        compiled.refs(),
                        refs.as_slice(),
                        "[{}] 引用提取不一致: {}",
                        label,
                        c.expr
                    );
                    continue;
                }
                let expected = c
                    .expected
                    .unwrap_or_else(|| panic!("[{}] 缺少 expected 字段", label));
                let actual = apply_formula_multi(&c.expr, &c.args)
                    .unwrap_or_else(|e| panic!("[{}] 求值失败: {}", label, e));
                let tol = c.tolerance.unwrap_or(1e-6);
                assert!(
                    (actual - expected).abs() <= tol,
                    "[{}] 期望 {}（容差 {}），实际 {}：{}",
                    label,
                    expected,
                    tol,
                    actual,
                    c.expr
                );
            }
        }
    }
}
