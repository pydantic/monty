use monty_type_checking::type_check;

/// Test that okay.py type-checks without errors.
///
/// This file uses `assert_type` from typing to verify that inferred types match expected types.
#[test]
fn type_check_okay() {
    let code = include_str!("okay.py");
    let result = type_check(code, Some("okay.py")).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}
