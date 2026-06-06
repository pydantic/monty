//! Sandbox-safety tests for `date`/`datetime` strftime on *unsupported*
//! directives.
//!
//! The agreeing behaviour (valid specs via `strftime` and f-strings) is tested
//! in the dual-run harness (`test_cases/datetime__format.py`). What's left here
//! is the one case that *can't* be dual-run: an unsupported directive (`%Q`).
//! CPython is lenient and passes it through, but that pass-through is
//! platform-dependent (macOS yields `'Q'`, glibc `'%Q'`), so there is nothing
//! cross-platform to assert against. Monty is deterministic and raises
//! `ValueError: Invalid format string`.
//!
//! The property that genuinely matters — and the reason these are real tests
//! rather than just a limitations note — is that a bad directive must NEVER
//! panic the host: `chrono`'s `DelayedFormat::to_string()` panics on an invalid
//! directive, which would be a sandbox escape on untrusted input. Monty routes
//! through a non-panicking writer instead (see `date::render_strftime`).

use monty::MontyRun;

/// Runs a Python snippet expected to raise, returning the exception message.
/// `run_no_limits().unwrap_err()` would itself panic if the snippet instead
/// panicked the interpreter, so reaching the assert proves "no host panic".
fn run_err(code: &str) -> String {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    ex.run_no_limits(vec![]).unwrap_err().to_string()
}

/// An unsupported directive via the `strftime` method raises `ValueError`
/// (not a host panic).
#[test]
fn strftime_method_bad_directive_raises_not_panics() {
    let msg = run_err("from datetime import date\ndate(2024, 6, 15).strftime('%Q')");
    assert!(
        msg.contains("ValueError") && msg.contains("Invalid format string"),
        "expected ValueError: Invalid format string, got: {msg}"
    );
}

/// The same directive reached through an f-string (a dynamic spec, so it
/// survives to runtime) takes the identical non-panicking path.
#[test]
fn fstring_bad_directive_raises_not_panics() {
    let msg = run_err("from datetime import datetime\nf'{datetime(2024, 6, 15):{\"%Q\"}}'");
    assert!(
        msg.contains("ValueError") && msg.contains("Invalid format string"),
        "expected ValueError: Invalid format string, got: {msg}"
    );
}

/// A lone `%` — which the format mini-language parses as the percent type and
/// `chrono` rejects — must also not panic when reached as a dynamic spec.
#[test]
fn fstring_lone_percent_does_not_panic() {
    let msg = run_err("from datetime import datetime\nf'{datetime(2024, 6, 15):{\"%\"}}'");
    assert!(msg.contains("ValueError"), "expected a ValueError, got: {msg}");
}
