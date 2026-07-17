//! `strftime` behaviour that can't live in the dual-run `test_cases/` harness
//! because the reference (the *host* CPython the harness compares against)
//! differs by platform.
//!
//! For an **unrecognised** directive, Monty deliberately matches **glibc/Linux**
//! CPython: the directive is passed through verbatim (`strftime('%Q') == '%Q'`).
//! macOS CPython instead drops the `%` (`'Q'`), so asserting these in a
//! test_case would fail on a macOS CI runner. They live here instead. See
//! limitations/datetime.md.

use monty::{CompileOptions, MontyObject, MontyRun};

/// Runs a snippet and returns its result as a `String`.
fn run_str(code: &str) -> String {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let obj: MontyObject = ex.run_no_limits(vec![]).unwrap();
    (&obj).try_into().unwrap()
}

/// Runs a snippet expected to raise, returning the exception message.
/// `unwrap_err()` would itself panic if the snippet panicked the interpreter,
/// so reaching the assert proves "no host panic".
fn run_err(code: &str) -> String {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    ex.run_no_limits(vec![]).unwrap_err().to_string()
}

/// An unrecognised directive is emitted verbatim (`%` kept), matching
/// glibc/Linux CPython.
#[test]
fn unknown_directive_passes_through_verbatim() {
    assert_eq!(
        run_str("from datetime import date\ndate(2024, 6, 15).strftime('%Q')"),
        "%Q"
    );
    assert_eq!(
        run_str("from datetime import date\ndate(2024, 6, 15).strftime('%Y-%Q-%d')"),
        "2024-%Q-15"
    );
    // A lone trailing percent is emitted as-is.
    assert_eq!(
        run_str("from datetime import date\ndate(2024, 6, 15).strftime('%')"),
        "%"
    );
}

/// The f-string path (a dynamic spec carries the strftime string to runtime)
/// shares the same lenient formatter.
#[test]
fn fstring_unknown_directive_passes_through_verbatim() {
    assert_eq!(
        run_str("from datetime import datetime\nf'{datetime(2024, 6, 15):{\"%Q\"}}'"),
        "%Q"
    );
}

/// A directive that *parses* but can't be rendered for the value (a time
/// directive on a bare `date`, which Monty stores without a time component)
/// raises `ValueError` — and, critically, must NOT panic the host:
/// `chrono`'s `DelayedFormat::to_string()` panics here, which would be a
/// sandbox escape on untrusted input.
#[test]
fn unrenderable_directive_raises_not_panics() {
    let msg = run_err("from datetime import date\ndate(2024, 6, 15).strftime('%z')");
    assert!(
        msg.contains("ValueError") && msg.contains("Invalid format string"),
        "expected ValueError: Invalid format string, got: {msg}"
    );
}
