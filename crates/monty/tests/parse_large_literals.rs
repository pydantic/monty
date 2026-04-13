//! Tests that very large numeric literals in source code are handled correctly.
//!
//! Decimal integer literals exceeding `INT_MAX_STR_DIGITS` (4300) are rejected
//! at parse time to prevent the O(n^2) `BigInt::parse` from running. Non-decimal
//! literals (hex, binary) and floats are unaffected.

use monty::{ExcType, MontyObject, MontyRun};

#[test]
fn large_decimal_literal_rejected() {
    // 4301 digits — exceeds the 4300 limit
    let literal = "1".repeat(4301);
    let code = format!("x = {literal}");
    let err = MontyRun::new(code, "test.py", vec![]).expect_err("should reject overlarge decimal literal");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message().expect("should have a message"),
        "Exceeds the limit (4300 digits) for integer string conversion: \
         value has 4301 digits; consider hexadecimal for large integer literals"
    );
}

#[test]
fn large_negative_decimal_literal_rejected() {
    // Negative overlarge literal: the minus is a unary op, so the literal itself is positive
    let literal = "1".repeat(4301);
    let code = format!("x = -{literal}");
    let err = MontyRun::new(code, "test.py", vec![]).expect_err("should reject overlarge negative decimal literal");
    assert_eq!(err.exc_type(), ExcType::SyntaxError);
    assert_eq!(
        err.message().expect("should have a message"),
        "Exceeds the limit (4300 digits) for integer string conversion: \
         value has 4301 digits; consider hexadecimal for large integer literals"
    );
}

#[test]
fn decimal_literal_at_limit_accepted() {
    // Exactly 4300 digits — should be accepted
    let literal = "1".repeat(4300);
    let code = format!("x = {literal}\nassert x > 0");
    let run = MontyRun::new(code, "test.py", vec![]).expect("4300-digit literal should parse");
    let result = run.run_no_limits(vec![]);
    assert!(result.is_ok(), "4300-digit literal should run: {result:?}");
}

#[test]
fn large_hex_literal_accepted() {
    // Hex literals use O(n) parsing and are not restricted by INT_MAX_STR_DIGITS.
    // 5000 hex digits ≈ 20000 bits ≈ 6000+ decimal digits.
    let hex_digits = "f".repeat(5000);
    let code = format!("x = 0x{hex_digits}\nassert x > 0");
    let run = MontyRun::new(code, "test.py", vec![]).expect("large hex literal should parse");
    let result = run.run_no_limits(vec![]);
    assert!(result.is_ok(), "large hex literal should run: {result:?}");
}

#[test]
fn large_binary_literal_accepted() {
    // Binary literals use O(n) parsing and are unrestricted.
    let bin_digits = "1".repeat(20000);
    let code = format!("x = 0b{bin_digits}\nassert x > 0");
    let run = MontyRun::new(code, "test.py", vec![]).expect("large binary literal should parse");
    let result = run.run_no_limits(vec![]);
    assert!(result.is_ok(), "large binary literal should run: {result:?}");
}

#[test]
fn large_float_literal_accepted() {
    // Very large float literals should parse fine — they just become inf.
    let code = "x = 1e308\nassert x == float('inf') or x > 0";
    let run = MontyRun::new(code.to_string(), "test.py", vec![]).expect("large float literal should parse");
    let result = run.run_no_limits(vec![]);
    assert!(result.is_ok(), "large float literal should run: {result:?}");
}

#[test]
fn very_large_float_literal_accepted() {
    // Float with many digits in the mantissa — ruff parses this fine.
    let digits = "1".repeat(1000);
    let code = format!("x = {digits}.0\nassert x > 0");
    let run = MontyRun::new(code, "test.py", vec![]).expect("float with many digits should parse");
    let result = run.run_no_limits(vec![]);
    assert!(result.is_ok(), "float with many digits should run: {result:?}");
}

#[test]
fn container_repr_with_huge_int_raises_value_error() {
    // repr() on a list containing a huge int should raise ValueError, not panic.
    // The list's py_repr_fmt calls the element's py_repr_fmt which propagates the
    // ValueError through the container's repr up to the builtin_repr caller.
    let code = "x = [10**5000]\nrepr(x)".to_string();
    let run = MontyRun::new(code, "test.py", vec![]).expect("should parse");
    let err = run.run_no_limits(vec![]).expect_err("repr([huge_int]) should fail");
    assert_eq!(err.exc_type(), ExcType::ValueError);
    assert_eq!(
        err.message().expect("should have a message"),
        "Exceeds the limit (4300 digits) for integer string conversion"
    );
}

#[test]
fn monty_object_repr_or_error_success() {
    // Returning a range produces MontyObject::Repr with the correct repr string.
    // This exercises the repr_or_error success path in MontyObject::from_value.
    let code = "range(0, 10, 2)".to_string();
    let run = MontyRun::new(code, "test.py", vec![]).expect("should parse");
    let result = run.run_no_limits(vec![]).expect("should run");
    let MontyObject::Repr(s) = result else {
        panic!("expected MontyObject::Repr, got: {result:?}");
    };
    assert_eq!(s, "range(0, 10, 2)");
}

#[test]
fn monty_object_repr_or_error_slice() {
    // Returning a slice produces MontyObject::Repr with the correct repr string.
    let code = "slice(1, 10, 2)".to_string();
    let run = MontyRun::new(code, "test.py", vec![]).expect("should parse");
    let result = run.run_no_limits(vec![]).expect("should run");
    let MontyObject::Repr(s) = result else {
        panic!("expected MontyObject::Repr, got: {result:?}");
    };
    assert_eq!(s, "slice(1, 10, 2)");
}

#[test]
fn monty_object_repr_or_error_dict_keys() {
    // Returning a dict_keys view produces MontyObject::Repr.
    let code = "{1: 'a', 2: 'b'}.keys()".to_string();
    let run = MontyRun::new(code, "test.py", vec![]).expect("should parse");
    let result = run.run_no_limits(vec![]).expect("should run");
    let MontyObject::Repr(s) = result else {
        panic!("expected MontyObject::Repr, got: {result:?}");
    };
    assert_eq!(s, "dict_keys([1, 2])");
}

#[test]
fn monty_object_repr_or_error_with_huge_int() {
    // Returning a dict_keys view containing a huge int triggers the error fallback
    // in repr_or_error. The MontyObject::Repr should contain the error message
    // instead of panicking or returning an empty string.
    let code = "d = {10**5000: 'v'}\nd.keys()".to_string();
    let run = MontyRun::new(code, "test.py", vec![]).expect("should parse");
    let result = run.run_no_limits(vec![]).expect("should run, not raise");
    let MontyObject::Repr(s) = result else {
        panic!("expected MontyObject::Repr, got: {result:?}");
    };
    assert_eq!(
        s,
        "<dict_keys object, error on repr(): ValueError('Exceeds the limit (4300 digits) for integer string conversion')>"
    );
}
