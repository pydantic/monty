use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

/// Helper to run a Python expression and return the result.
fn run_expr(code: &str) -> MontyObject {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    ex.run_no_limits(vec![]).unwrap()
}

// ==========================
// comb near the i64 limit
// ==========================

/// `math.comb(66, 33)` is the largest central binomial that fits an `i64`
/// (7219428434016265740); the result must demote back to a machine int.
#[test]
fn comb_large_but_fits_i64() {
    let result = run_expr("import math\nmath.comb(66, 33)");
    let v: i64 = (&result).try_into().unwrap();
    assert_eq!(v, 7_219_428_434_016_265_740);
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
