//! Monty-specific `Decimal` sandbox guards that diverge from CPython and so
//! cannot live in the dual-run `test_cases/` (CPython, with arbitrary
//! precision, accepts these inputs instead of erroring).
//!
//! The guards under test: the constructor's coefficient/payload digit cap
//! (`DECIMAL_MAX_DIGITS` = 4300, mirroring `INT_MAX_STR_DIGITS`), the C-module
//! exponent literal bounds, and the resource-tracker charge on materialising a
//! huge-exponent integral value (`int()` / `hash()`).

use std::time::Duration;

use monty::{ExcType, LimitedTracker, MontyObject, MontyRun, PrintWriter, ResourceLimits};

/// Runs `code` with no resource limits and returns the exception, or panics if
/// it unexpectedly succeeds.
fn run_expect_error(code: &str) -> (ExcType, String) {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).expect("should parse");
    let exc = run.run_no_limits(vec![]).expect_err("expected an exception");
    (exc.exc_type(), exc.message().unwrap_or_default().to_owned())
}

/// Runs `code` with no resource limits and returns the result, panicking on error.
fn run_ok(code: &str) -> MontyObject {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).expect("should parse");
    run.run_no_limits(vec![]).expect("should run")
}

/// A coefficient past the 4300-digit cap is rejected with the Monty-specific
/// `ValueError` — from a string literal, an `int`, and a NaN payload alike.
/// CPython accepts all of these (documented divergence).
#[test]
fn digit_cap_rejected() {
    for expr in [
        "Decimal('1' * 4301)",
        "Decimal(10 ** 4301)",
        "Decimal('NaN' + '1' * 4301)",
        "Decimal((0, (1,) * 4301, 0))",
    ] {
        let code = format!("from decimal import Decimal\n{expr}");
        let (exc_type, msg) = run_expect_error(&code);
        assert_eq!(exc_type, ExcType::ValueError, "{expr}");
        assert_eq!(msg, "Decimal value exceeds the limit of 4300 digits", "{expr}");
    }
}

/// Values at the cap still construct: the guard must not over-reject.
#[test]
fn digit_cap_boundary_accepted() {
    assert_eq!(
        run_ok("from decimal import Decimal\nlen(str(Decimal('9' * 4300)))"),
        MontyObject::Int(4300)
    );
    // Leading zeros are stripped before the cap is applied, so a long-but-thin
    // literal is fine.
    assert_eq!(
        run_ok("from decimal import Decimal\nstr(Decimal('0' * 10000 + '7'))"),
        MontyObject::String("7".to_owned())
    );
}

/// Exponent literals follow the C module's 64-bit bounds: within them huge
/// exponents construct and round-trip; beyond them `InvalidOperation` is
/// raised exactly as in CPython.
#[test]
fn exponent_literal_bounds() {
    assert_eq!(
        run_ok("from decimal import Decimal\nstr(Decimal('1E+425000000'))"),
        MontyObject::String("1E+425000000".to_owned())
    );
    assert_eq!(
        run_ok("from decimal import Decimal\nstr(Decimal('1E+999999999999999998'))"),
        MontyObject::String("1E+999999999999999998".to_owned())
    );
    let (exc_type, msg) = run_expect_error("from decimal import Decimal\nDecimal('1E+1000000000000000000')");
    assert_eq!(exc_type, ExcType::DecimalInvalidOperation);
    assert_eq!(msg, "[<class 'decimal.InvalidOperation'>]");
}

/// Materialising a huge-exponent integral value (`int(d)` needs
/// `coeff · 10^exp`) is charged to the resource tracker, so under a memory
/// limit it raises `MemoryError` instead of allocating gigabytes. CPython
/// computes it (documented divergence).
#[test]
fn huge_exponent_int_hits_memory_limit() {
    let code = "from decimal import Decimal\nint(Decimal('1E+100000000'))";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let limits = ResourceLimits::new()
        .max_memory(1024 * 1024)
        .max_duration(Duration::from_secs(30));
    let exc = runner
        .run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .expect_err("huge power-of-ten materialisation must be bounded by the memory limit");
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// The same guard covers `hash()`, which also materialises the exact integer.
#[test]
fn huge_exponent_hash_hits_memory_limit() {
    let code = "from decimal import Decimal\nhash(Decimal('1E+100000000'))";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let limits = ResourceLimits::new()
        .max_memory(1024 * 1024)
        .max_duration(Duration::from_secs(30));
    let exc = runner
        .run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .expect_err("huge power-of-ten materialisation must be bounded by the memory limit");
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// A `Decimal` operand of arithmetic promoted from a too-large `int` hits the
/// same digit cap as construction (`Decimal(1) + 10**100` works; `+ 10**4301`
/// raises).
#[test]
fn promoted_int_operand_capped() {
    assert_eq!(
        run_ok("from decimal import Decimal\nstr(Decimal(1) + 10**100)"),
        MontyObject::String("1.000000000000000000000000000E+100".to_owned())
    );
    let (exc_type, msg) = run_expect_error("from decimal import Decimal\nDecimal(1) + 10**4301");
    assert_eq!(exc_type, ExcType::ValueError);
    assert_eq!(msg, "Decimal value exceeds the limit of 4300 digits");
}
