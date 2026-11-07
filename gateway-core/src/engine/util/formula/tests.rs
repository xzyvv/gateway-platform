use super::*;

#[test]
fn test_scale_formula() {
    assert!((apply_formula("raw*0.1", 2205.0).unwrap() - 220.5).abs() < 0.001);
}

#[test]
fn test_offset_formula() {
    assert!((apply_formula("raw-50", 100.0).unwrap() - 50.0).abs() < 0.001);
}

#[test]
fn test_complex_formula() {
    assert!((apply_formula("raw*0.1-50", 1000.0).unwrap() - 50.0).abs() < 0.001);
}

#[test]
fn test_invalid_formula() {
    assert!(apply_formula("raw ** invalid", 1.0).is_err());
}

#[test]
fn test_division_by_zero_yields_error() {
    assert!(apply_formula("raw/0", 1.0).is_err());
    assert!(apply_formula("raw%0", 1.0).is_err());
}

#[test]
fn test_overflow_yields_error() {
    assert!(apply_formula("raw*raw", 1e300).is_err());
}

#[test]
fn test_allowlist_rejects_function_calls() {
    assert!(apply_formula("min(raw, 100)", 50.0).is_err());
    assert!(apply_formula("length(raw)", 50.0).is_err());
}

#[test]
fn test_allowlist_rejects_undeclared_identifiers() {
    assert!(apply_formula("pi*raw", 2.0).is_err());
    assert!(apply_formula("x*raw", 2.0).is_err());
}

#[test]
fn test_nested_expression_still_works() {
    assert!((apply_formula("(raw+1)*(raw-1)", 10.0).unwrap() - 99.0).abs() < 0.001);
}

#[test]
fn test_negative_raw_still_works() {
    assert!((apply_formula("raw*2", -5.0).unwrap() + 10.0).abs() < 0.001);
}

#[test]
fn test_allowlist_rejects_raw_prefix_partial() {
    assert!(apply_formula("rawa", 1.0).is_err());
    assert!(apply_formula("rawx*2", 1.0).is_err());
}
