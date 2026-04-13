use monty::{MontyObject, MontyRun};

/// Helper to run a Python expression and return the result.
fn run_expr(code: &str) -> MontyObject {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    ex.run_no_limits(vec![]).unwrap()
}

/// Helper to run Python code that is expected to raise an exception.
/// Returns the exception message string.
fn run_expect_error(code: &str) -> String {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let err = ex.run_no_limits(vec![]).unwrap_err();
    err.to_string()
}

// ==========================
// Overflow tests (i64-specific)
// ==========================

/// `math.factorial(21)` overflows i64 (21! = 51090942171709440000 > i64::MAX).
/// Monty raises OverflowError since it doesn't have big integer support.
#[test]
fn factorial_i64_overflow() {
    let msg = run_expect_error("import math\nmath.factorial(21)");
    assert!(
        msg.contains("OverflowError"),
        "Expected OverflowError for factorial(21), got: {msg}"
    );
}

/// `math.comb(66, 33)` fits in i64 (7219428434016265740) thanks to GCD reduction
/// that avoids intermediate overflow. Verify it computes the correct value.
#[test]
fn comb_large_but_fits_i64() {
    let result = run_expr("import math\nmath.comb(66, 33)");
    let v: i64 = (&result).try_into().unwrap();
    assert_eq!(v, 7_219_428_434_016_265_740);
}

/// `math.comb(68, 34)` overflows i64 even with GCD reduction
/// (68C34 = 28048800420600 * ... > i64::MAX).
#[test]
fn comb_i64_overflow() {
    let msg = run_expect_error("import math\nmath.comb(68, 34)");
    assert!(
        msg.contains("OverflowError"),
        "Expected OverflowError for comb(68, 34), got: {msg}"
    );
}

/// `math.perm(21, 21)` overflows i64 (same as 21! which exceeds i64::MAX).
#[test]
fn perm_i64_overflow() {
    let msg = run_expect_error("import math\nmath.perm(21, 21)");
    assert!(
        msg.contains("OverflowError"),
        "Expected OverflowError for perm(21, 21), got: {msg}"
    );
}

// ==========================
// ldexp negative exponent loop
// ==========================

/// `math.ldexp(1.0, -1050)` exercises the negative exponent loop in `math_ldexp`
/// because -1050 is between -1074 and -1022, requiring iterative halving.
#[test]
fn ldexp_large_negative_exponent_loop() {
    let result = run_expr("import math\nmath.ldexp(1.0, -1050)");
    let f: f64 = (&result).try_into().unwrap();
    // ldexp(1.0, -1050) is a very small subnormal but not zero
    assert!(f > 0.0, "ldexp(1.0, -1050) should be positive, got: {f}");
    assert!(f < 1e-300, "ldexp(1.0, -1050) should be tiny, got: {f}");
}

/// `math.ldexp(1.0, -1074)` is the smallest representable positive float (subnormal).
#[test]
fn ldexp_minimum_subnormal() {
    let result = run_expr("import math\nmath.ldexp(1.0, -1074)");
    let f: f64 = (&result).try_into().unwrap();
    // Compare bits directly since this is an exact IEEE 754 subnormal value
    assert_eq!(
        f.to_bits(),
        5e-324_f64.to_bits(),
        "ldexp(1.0, -1074) should equal 5e-324"
    );
}

// ==========================
// isqrt Newton's method refinement
// ==========================

/// `math.isqrt` with values near i64::MAX where f64 sqrt loses precision,
/// triggering the Newton's method refinement and overshoot correction.
#[test]
fn isqrt_large_values_newton_refinement() {
    // i64::MAX = 9223372036854775807
    // isqrt(i64::MAX) = 3037000499 (3037000499^2 = 9223372030926249001 <= i64::MAX)
    let result = run_expr("import math\nmath.isqrt(9223372036854775807)");
    let v: i64 = (&result).try_into().unwrap();
    assert_eq!(v, 3_037_000_499);

    // 3037000499^2 = 9223372030926249001 (perfect square)
    let result = run_expr("import math\nmath.isqrt(9223372030926249001)");
    let v: i64 = (&result).try_into().unwrap();
    assert_eq!(v, 3_037_000_499);

    // 3037000499^2 - 1: the initial f64 estimate overshoots by 1,
    // triggering both the delta==0 break and the overshoot correction loop.
    let result = run_expr("import math\nmath.isqrt(9223372030926249000)");
    let v: i64 = (&result).try_into().unwrap();
    assert_eq!(v, 3_037_000_498);
}
