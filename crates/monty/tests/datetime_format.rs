//! Date/time formatting and parsing checks that cannot use the dual-run harness:
//! platform-specific results, documented divergences and hostile formats.
//!
//! For an **unrecognised** directive, Monty deliberately matches **glibc/Linux**
//! CPython: the directive is passed through verbatim (`strftime('%Q') == '%Q'`).
//! macOS CPython instead drops the `%` (`'Q'`), so asserting these in a
//! test_case would fail on a macOS CI runner. They live here instead. See
//! limitations/datetime.md.

use std::time::{Duration, Instant};

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};

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
    assert_snapshot!(run_str(
        "from datetime import date, datetime, time, timezone\n\
         values = [date(2024, 6, 15), datetime(2024, 6, 15), \
         datetime(2024, 6, 15, tzinfo=timezone.utc), time(12), time(12, tzinfo=timezone.utc)]\n\
         '|'.join(value.strftime('%+ %Y %%+') for value in values)"
    ), @"%+ 2024 %+|%+ 2024 %+|%+ 2024 %+|%+ 1900 %+|%+ 1900 %+");
}

/// A directive ends at its first non-flag character, so `% ` cannot swallow the
/// next directive, and Jiff's own extensions stay literal as glibc's unknowns do.
#[test]
fn non_glibc_directives_pass_through_verbatim() {
    assert_snapshot!(run_str(
        "from datetime import datetime\n\
         d = datetime(2024, 6, 5, 7, 3, 4, 120)\n\
         '|'.join(d.strftime(f) for f in ['50% %Y', '% 年 %Y', '%q', '%.f', '%.3f', '%3f', '%-q', '%::z', '%-'])"
    ), @"50% 2024|% 年 2024|%q|%.f|%.3f|%3f|%-q|%::z|%-");
}

/// The f-string path (a dynamic spec carries the strftime string to runtime)
/// shares the same lenient formatter.
#[test]
fn fstring_unknown_directive_passes_through_verbatim() {
    assert_eq!(
        run_str("from datetime import datetime\nf'{datetime(2024, 6, 15):{\"%Q\"}}'"),
        "%Q"
    );
    assert_eq!(
        run_str("from datetime import datetime\nf'{datetime(2024, 6, 15):{\"%+ %Y\"}}'"),
        "%+ 2024"
    );
    assert_eq!(
        run_str("from datetime import datetime\n'{:%+ %Y}'.format(datetime(2024, 6, 15))"),
        "%+ 2024"
    );
}

/// The unsupported `%#z` directive still raises without panicking the host.
#[test]
fn unrenderable_directive_raises_not_panics() {
    assert_snapshot!(run_err("from datetime import date\ndate(2024, 6, 15).strftime('%#z')"), @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        date(2024, 6, 15).strftime('%#z')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    ValueError: Invalid format string
    "#);
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
    assert_eq!(
        run_str("import time\ntime.strftime('%+ %Y', time.gmtime(0))"),
        "%+ 1970"
    );
    assert_snapshot!(run_err("import time\ntime.strftime('%#z', time.gmtime(0))"), @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        time.strftime('%#z', time.gmtime(0))
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    ValueError: Invalid format string
    "#);
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

/// `mktime()` cannot place a wall clock past year 9999, where 64-bit CPython
/// returns a timestamp. See limitations/time.md.
#[test]
fn mktime_beyond_year_9999_overflows() {
    for tuple in [
        "(10000, 1, 1, 0, 0, 0, 5, 1, -1)",
        "(9999, 12, 31, 23, 59, 60, 4, 365, -1)",
    ] {
        assert_eq!(
            run_err(&format!("import time\ntime.mktime({tuple})")).lines().last(),
            Some("OverflowError: mktime argument out of range")
        );
    }
}

/// `strftime()` and `asctime()` refuse a year past ±9999, which Jiff cannot hold;
/// glibc and macOS CPython format it. See limitations/time.md.
#[test]
fn strftime_beyond_year_9999_raises() {
    for expr in [
        "time.strftime('%Y', (10000, 1, 1, 0, 0, 0, 5, 1, 0))",
        "time.strftime('%Y', (-10000, 1, 1, 0, 0, 0, 5, 1, 0))",
        "time.asctime((10000, 1, 1, 0, 0, 0, 5, 1, 0))",
    ] {
        assert_eq!(
            run_err(&format!("import time\n{expr}")).lines().last(),
            Some("ValueError: year out of range"),
            "{expr}"
        );
    }
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

/// `date.strptime` remains unimplemented; its CPython counterpart succeeds.
#[test]
fn date_strptime_is_not_implemented() {
    assert_snapshot!(
        run_err("from datetime import date\ndate.strptime('2020-01-01', '%Y-%m-%d')"),
        @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        date.strptime('2020-01-01', '%Y-%m-%d')
    AttributeError: type object 'datetime.date' has no attribute 'strptime'
    "#
    );
}

/// Ambiguous repeated fields must fail without exponential retries or recursion.
/// CPython rejects the duplicate groups at format compilation, so this is one-sided.
#[test]
fn hostile_strptime_fields_do_not_cause_exponential_backtracking() {
    let code = r"
from datetime import datetime
text = '1' * 36 + 'Y'
fmt = '%H' * 24 + 'X'
try:
    datetime.strptime(text, fmt)
    assert False, 'expected the final literal to fail'
except ValueError as exc:
    assert str(exc) == f'time data {text!r} does not match format {fmt!r}'
True
";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}

/// The memoized field search is still quadratic, so it polls the clock: a hostile
/// format that runs for seconds unpolled stops near the time limit.
#[test]
fn hostile_strptime_fields_respect_the_time_limit() {
    let code = "from datetime import datetime\ndatetime.strptime('1' * 6000 + 'Y', '%H' * 4000 + 'X')";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(50));
    let start = Instant::now();
    let exc = runner
        .run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
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
