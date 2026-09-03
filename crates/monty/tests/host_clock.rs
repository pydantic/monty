//! [`HostClock`]: the opt-in clock that answers `date.today()` and
//! `datetime.now()` on the non-suspending run paths.
//!
//! These can't live in `test_cases/`, which has no way to grant a clock and
//! runs every fixture against a real CPython whose clock keeps moving. The
//! expected values below were therefore diffed against CPython 3.14 by hand,
//! not produced by Monty: naive `now()` and `today()` read local wall time,
//! and `now(tz)` converts the instant into the argument. That conversion is
//! new arithmetic here — under the OS-call path the host performs it, so
//! `datetime__core.py` does not cover it.

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, HostClock, MontyObject, ResourceTracker};

/// 2023-11-14 22:13:20 UTC — the instant the datatest fixtures already freeze
/// to, reused so both harnesses tell the same story.
const FIXTURE_SECONDS: i64 = 1_700_000_000;

/// A clock frozen at [`FIXTURE_SECONDS`] in a UTC+02:00 local zone, which puts
/// the local date one day ahead of the UTC one.
const FIXED: HostClock = HostClock::Fixed {
    unix_seconds: FIXTURE_SECONDS,
    microsecond: 123_456,
    local_offset_seconds: 7_200,
};

/// Runs `code` under `clock` and returns its result.
fn run(code: &str, clock: HostClock) -> Result<MontyObject, String> {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .with_host_clock(clock)
        .run_no_limits(vec![])
        .map_err(|err| err.to_string())
}

/// Runs a `datetime` expression under `clock` and returns its `repr()`.
fn run_repr(expr: &str, clock: HostClock) -> String {
    let code = format!("from datetime import date, datetime, timedelta, timezone\nrepr({expr})");
    let obj = run(&code, clock).unwrap();
    (&obj).try_into().unwrap()
}

/// A runner that was never given a clock still answers both calls: standard
/// execution has no host to ask, so denying by default is what made ordinary
/// date-handling scripts raise.
#[test]
fn the_host_clock_is_the_default() {
    let code = "from datetime import date, datetime\n(date.today().year, datetime.now().year)";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let MontyObject::Tuple(years) = runner.run_no_limits(vec![]).unwrap() else {
        panic!("expected a tuple of years");
    };
    for year in years {
        let MontyObject::Int(year) = year else {
            panic!("expected an int year");
        };
        assert!((2026..=2100).contains(&year), "implausible year {year}");
    }
}

/// `Denied` is how an embedder takes the default clock away again.
#[test]
fn a_denied_clock_refuses_both_calls() {
    assert_eq!(
        run("from datetime import datetime\ndatetime.now()", HostClock::Denied).unwrap_err(),
        "NotImplementedError: OS function 'datetime.now' not implemented with standard execution"
    );
    assert_eq!(
        run("from datetime import date\ndate.today()", HostClock::Denied).unwrap_err(),
        "NotImplementedError: OS function 'date.today' not implemented with standard execution"
    );
}

#[test]
fn fixed_clock_reads_local_wall_time() {
    // 22:13:20 UTC + 2h, so both the time and the date roll over.
    assert_eq!(
        run_repr("datetime.now()", FIXED),
        "datetime.datetime(2023, 11, 15, 0, 13, 20, 123456)"
    );
    assert_eq!(run_repr("date.today()", FIXED), "datetime.date(2023, 11, 15)");
}

#[test]
fn fixed_clock_converts_into_the_requested_timezone() {
    assert_eq!(
        run_repr("datetime.now(timezone.utc)", FIXED),
        "datetime.datetime(2023, 11, 14, 22, 13, 20, 123456, tzinfo=datetime.timezone.utc)"
    );
    assert_eq!(
        run_repr("datetime.now(timezone(timedelta(hours=-5), 'EST'))", FIXED),
        "datetime.datetime(2023, 11, 14, 17, 13, 20, 123456, \
         tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=68400), 'EST'))"
    );
}

/// An aware `now(tz)` and a naive one are the same instant, whatever the
/// clock's own local offset is.
#[test]
fn aware_and_naive_agree_on_the_instant() {
    let code = "from datetime import datetime, timezone\n\
                (datetime.now(timezone.utc).hour - datetime.now().hour) % 24";
    assert_eq!(run(code, FIXED).unwrap(), MontyObject::Int(22));
}

/// A fixed instant outside `datetime`'s 1..=9999 years is refused rather than
/// producing an out-of-range value.
#[test]
fn unrepresentable_fixed_instant_reads_as_denied() {
    let far_future = HostClock::Fixed {
        unix_seconds: 300_000_000_000,
        microsecond: 0,
        local_offset_seconds: 0,
    };
    assert_eq!(
        run("from datetime import date\ndate.today()", far_future).unwrap_err(),
        "NotImplementedError: OS function 'date.today' not implemented with standard execution"
    );
}

#[test]
fn out_of_range_microsecond_reads_as_denied() {
    // Nothing in `instant()` bounds this itself — `from_timestamp` rejects the
    // nanoseconds it becomes, so this pins the behaviour the field documents
    // against a chrono that might one day accept them as a leap second.
    let overflowing = HostClock::Fixed {
        unix_seconds: FIXTURE_SECONDS,
        microsecond: 1_500_000,
        local_offset_seconds: 0,
    };
    assert_eq!(
        run("from datetime import datetime\ndatetime.now()", overflowing).unwrap_err(),
        "NotImplementedError: OS function 'datetime.now' not implemented with standard execution"
    );
}

#[test]
fn system_clock_returns_a_plausible_now() {
    // Written 2026; a system clock that reads before then is broken, not stale.
    let code = "from datetime import date, datetime\n\
                date.today() == datetime.now().date() and datetime.now().year >= 2026";
    assert_eq!(run(code, HostClock::System).unwrap(), MontyObject::Bool(true));
}

/// The clock is a *standard execution* fallback: with a host loop present the
/// call still reaches the host, which is what lets a host deny or fake it.
#[test]
fn iterative_execution_still_suspends() {
    let runner = MontyRun::new(
        "from datetime import date\ndate.today()".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap()
    .with_host_clock(HostClock::System);

    let progress = runner
        .start(vec![], ResourceTracker::default(), monty_types::PrintWriter::Disabled)
        .unwrap();
    let call = progress.into_os_call().expect("date.today() suspends to the host");
    assert_eq!(call.function_call.name(), "date.today");
}

#[test]
fn repl_sessions_take_a_clock_too() {
    let mut repl =
        MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default()).with_host_clock(FIXED);
    let result = repl
        .feed_run(
            "from datetime import date\nrepr(date.today())",
            vec![],
            monty_types::PrintWriter::Disabled,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("datetime.date(2023, 11, 15)".to_owned()));
}

/// The clock is granted on the session, so which entry point runs the code
/// must not change what the code can do.
#[test]
fn call_function_takes_the_session_clock_too() {
    let mut repl =
        MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default()).with_host_clock(FIXED);
    repl.feed_run(
        "from datetime import date\ndef when():\n    return repr(date.today())",
        vec![],
        monty_types::PrintWriter::Disabled,
    )
    .unwrap();

    let result = repl
        .call_function("when", vec![], monty_types::PrintWriter::Disabled)
        .unwrap();
    assert_eq!(result, MontyObject::String("datetime.date(2023, 11, 15)".to_owned()));
}

/// A denied clock refuses through `call_function` too, as it does `feed_run`.
#[test]
fn call_function_honours_a_denied_clock_too() {
    let mut repl = MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default())
        .with_host_clock(HostClock::Denied);
    repl.feed_run(
        "from datetime import date\ndef when():\n    return date.today()",
        vec![],
        monty_types::PrintWriter::Disabled,
    )
    .unwrap();

    let err = repl
        .call_function("when", vec![], monty_types::PrintWriter::Disabled)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "<python-input-1>", line 1, in <module>
        when()
        ~~~~~~
      File "<python-input-0>", line 3, in when
        return date.today()
               ~~~~~~~~~~~~
    NotImplementedError: MontyRepl::call_function: OS function 'date.today' is not yet supported in this context
    "#);
}

/// The clock is part of the serialized session, so a restored dump must still
/// answer with it — a dropped field would silently turn a granted clock back
/// into a denied one.
#[test]
fn a_granted_clock_survives_a_dump() {
    let mut repl =
        MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default()).with_host_clock(FIXED);
    repl.feed_run("x = 1", vec![], monty_types::PrintWriter::Disabled)
        .unwrap();

    let bytes = dump("<test>", None, SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };

    let result = restored
        .feed_run(
            "from datetime import date\nrepr(date.today())",
            vec![],
            monty_types::PrintWriter::Disabled,
        )
        .unwrap();
    assert_eq!(result, MontyObject::String("datetime.date(2023, 11, 15)".to_owned()));
}
