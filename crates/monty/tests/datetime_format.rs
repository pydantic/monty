//! `strftime` behaviour that can't live in the dual-run `test_cases/` harness
//! because the reference (the *host* CPython the harness compares against)
//! differs by platform.
//!
//! For an **unrecognised** directive, Monty deliberately matches **glibc/Linux**
//! CPython: the directive is passed through verbatim (`strftime('%Q') == '%Q'`).
//! macOS CPython instead drops the `%` (`'Q'`), so asserting these in a
//! test_case would fail on a macOS CI runner. They live here instead. See
//! limitations/datetime.md.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

/// Runs a snippet and returns its result as a `String`.
fn run_str(code: &str) -> String {
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let obj: MontyObject = ex.run_no_limits(vec![]).unwrap();
    (&obj).try_into().unwrap()
}

/// Runs a snippet expected to raise, returning the exception message.
/// `unwrap_err()` would itself panic if the snippet panicked the interpreter,
/// so reaching the assert proves "no host panic".
fn run_err(code: &str) -> String {
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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

/// A directive chrono *parses* but can't render for the value (`%+`, its RFC
/// 3339 form, needs an offset the naive components lack) raises `ValueError`
/// where CPython passes `%+` to the C library (glibc echoes it); see
/// `limitations/datetime.md`. Critically, it must NOT panic the host: `chrono`'s
/// `DelayedFormat::to_string()` panics here, which would be a sandbox escape
/// on untrusted input.
#[test]
fn unrenderable_directive_raises_not_panics() {
    let msg = run_err("from datetime import date\ndate(2024, 6, 15).strftime('%+')");
    assert!(
        msg.contains("ValueError") && msg.contains("Invalid format string"),
        "expected ValueError: Invalid format string, got: {msg}"
    );
}

/// `time.strftime` shares `datetime`'s lenient formatter: unknown directives
/// pass through, unrenderable ones raise, and `%f` renders zeros where glibc
/// CPython echoes it — platform-dependent, so not a `test_cases/` case.
/// See limitations/time.md.
#[test]
fn time_strftime_shares_the_lenient_formatter() {
    assert_eq!(run_str("import time\ntime.strftime('%Q', time.gmtime(0))"), "%Q");
    assert_eq!(run_str("import time\ntime.strftime('%f', time.gmtime(0))"), "000000");
    assert_eq!(
        run_str("import time\ntime.strftime('%Y-%m-%dT%H:%M:%S.%f%z', time.gmtime(0))"),
        "1970-01-01T00:00:00.000000+0000"
    );
    let msg = run_err("import time\ntime.strftime('%+', time.gmtime(0))");
    assert!(
        msg.contains("ValueError") && msg.contains("Invalid format string"),
        "expected ValueError: Invalid format string, got: {msg}"
    );
}

/// A time tuple's `tm_wday`/`tm_yday` are `i64` in Monty where CPython takes a
/// C `int`, so CPython refuses these at conversion and the fixture cannot hold
/// them. Monty must bound them without the `%U`/`%W` arithmetic overflowing.
#[test]
fn hostile_time_tuple_fields_raise_instead_of_panicking() {
    let big = i64::MAX;
    let small = i64::MIN;
    for (expr, expected) in [
        (
            format!("time.strftime('%j %U %W', (2024, 1, 1, 0, 0, 0, 0, {small}, -1))"),
            "day of year out of range",
        ),
        (
            format!("time.strftime('%j %U %W', (2024, 1, 1, 0, 0, 0, 0, {big}, -1))"),
            "day of year out of range",
        ),
        (
            format!("time.asctime((2024, 1, 1, 0, 0, 0, {small}, 1, -1))"),
            "day of week out of range",
        ),
    ] {
        let msg = run_err(&format!("import time\n{expr}"));
        assert!(msg.contains(expected), "{expr}: {msg}");
    }
    // a huge non-negative weekday folds mod 7, as CPython's `(wday + 1) % 7`
    // would if a C `int` could hold it: `i64::MAX` is a multiple of 7, so Monday
    assert_eq!(
        run_str(&format!(
            "import time\ntime.strftime('%a %U', (2024, 1, 1, 0, 0, 0, {big}, 1, -1))"
        )),
        "Mon 00"
    );
}

/// `gmtime()` names its zone `UTC`, as macOS CPython does; glibc CPython says
/// `GMT`, so the name cannot be asserted in a dual-run fixture. See
/// limitations/time.md.
#[test]
fn gmtime_zone_is_named_utc() {
    assert_eq!(run_str("import time\ntime.strftime('%Z', time.gmtime(0))"), "UTC");
    assert_eq!(run_str("import time\ntime.gmtime(0).tm_zone"), "UTC");
}

/// CPython 3.14 added `time.strptime`; Monty does not implement it, so this
/// cannot live in `test_cases/` — the harness's reference CPython succeeds.
/// See limitations/datetime.md.
#[test]
fn time_strptime_is_not_implemented() {
    assert_snapshot!(
        run_err("from datetime import time\ntime.strptime('12:30', '%H:%M')"),
        @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        time.strptime('12:30', '%H:%M')
    AttributeError: type object 'datetime.time' has no attribute 'strptime'
    "#
    );
}

/// `date.strptime` is CPython 3.14's other new `strptime`, also unimplemented —
/// and `datetime.strptime` is not a substitute for a time-only format, since it
/// requires the string to carry a date. Both are one-sided, so neither can live
/// in `test_cases/`. See limitations/datetime.md.
#[test]
fn strptime_gaps_on_date_and_datetime() {
    assert_snapshot!(
        run_err("from datetime import date\ndate.strptime('2020-01-01', '%Y-%m-%d')"),
        @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        date.strptime('2020-01-01', '%Y-%m-%d')
    AttributeError: type object 'datetime.date' has no attribute 'strptime'
    "#
    );
    assert_snapshot!(
        run_err("from datetime import datetime\ndatetime.strptime('12:30', '%H:%M')"),
        @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        datetime.strptime('12:30', '%H:%M')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    ValueError: time data '12:30' does not match format '%H:%M'
    "#
    );
}

/// A `%z` offset carrying a colon before its seconds only. Both interpreters
/// refuse it, but CPython's own check never runs — it goes on to `int(':0')`
/// and lets that error out — so the wording cannot be shared with a fixture in
/// `test_cases/`. See limitations/datetime.md.
#[test]
fn strptime_rejects_a_colon_before_the_seconds_only() {
    assert_snapshot!(
        run_err("from datetime import datetime\ndatetime.strptime('2024-06-15 +0102:03', '%Y-%m-%d %z')"),
        @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        datetime.strptime('2024-06-15 +0102:03', '%Y-%m-%d %z')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    ValueError: Inconsistent use of : in +0102:03
    "#
    );
}
