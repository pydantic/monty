//! [`AutoOsCalls`]: the OS calls the sandbox answers itself — the clock, the
//! sleeps and `random`'s first state — on every execution path.
//!
//! The clock expectations can't live in `test_cases/`, which runs every
//! fixture against a real CPython whose clock keeps moving. The values below
//! were therefore diffed against CPython 3.14 by hand: naive `now()` and
//! `today()` read local wall time, and `now(tz)` converts the instant into
//! the argument. `random`'s seeding is covered in `random_module.rs`.

use std::time::{Duration, Instant};

use chrono::{Local, Offset, TimeZone};
use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    AutoOsCalls, CompileOptions, DateTimeSource, MontyObject, OsFunctionCall, PrintWriter, ResourceLimits,
    ResourceTracker, SandboxTimeZone, SleepMode,
};

/// 2023-11-14 22:13:20 UTC — the instant the datatest fixtures already freeze
/// to, reused so both harnesses tell the same story.
const FIXTURE_SECONDS: i64 = 1_700_000_000;

/// 2023-11-14 22:13:59 UTC, i.e. [`FIXTURE_SECONDS`] moved onto the last
/// second of its minute — the only place chrono will read a microsecond past a
/// full second as a leap second instead of rejecting it.
const LAST_SECOND_OF_A_MINUTE: i64 = 1_700_000_039;

/// A clock frozen at [`FIXTURE_SECONDS`].
const FIXED: DateTimeSource = DateTimeSource::Fixed {
    unix_seconds: FIXTURE_SECONDS,
    microsecond: 123_456,
};

/// A UTC+02:00 local zone, which puts [`FIXED`]'s local date one day ahead of
/// the UTC one.
const PLUS_TWO: SandboxTimeZone = SandboxTimeZone::Fixed {
    offset_seconds: 7_200,
    name: None,
};

/// The defaults with `datetime` replaced; a fixed instant is read in
/// [`PLUS_TWO`] so its expectations do not depend on the host's zone.
fn with_datetime(datetime: DateTimeSource) -> AutoOsCalls {
    let timezone = match datetime {
        DateTimeSource::Fixed { .. } => PLUS_TWO,
        DateTimeSource::CallHost | DateTimeSource::System => SandboxTimeZone::System,
    };
    AutoOsCalls {
        datetime,
        timezone,
        ..AutoOsCalls::default()
    }
}

/// The defaults with `sleep` replaced.
fn with_sleep(sleep: SleepMode) -> AutoOsCalls {
    AutoOsCalls {
        sleep,
        ..AutoOsCalls::default()
    }
}

/// Every call marked for the host.
fn call_host() -> AutoOsCalls {
    AutoOsCalls {
        datetime: DateTimeSource::CallHost,
        sleep: SleepMode::CallHost,
        ..AutoOsCalls::default()
    }
}

/// A runner for `code` under `calls`.
fn runner(code: &str, calls: AutoOsCalls) -> MontyRun {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .with_auto_os_calls(calls)
}

/// Runs `code` under `calls` and returns its result, or the last line of the
/// error (`Type: message`, without the traceback).
fn run(code: &str, calls: AutoOsCalls) -> Result<MontyObject, String> {
    runner(code, calls)
        .run_no_limits(vec![])
        .map_err(|err| err.to_string().lines().last().unwrap_or_default().to_owned())
}

/// Runs a `datetime` expression under `datetime` and returns its `repr()`.
fn run_repr(expr: &str, datetime: DateTimeSource) -> String {
    run_repr_under(expr, with_datetime(datetime))
}

/// Runs a `datetime` expression under `calls` and returns its `repr()`.
fn run_repr_under(expr: &str, calls: AutoOsCalls) -> String {
    let code = format!("from datetime import date, datetime, timedelta, timezone\nrepr({expr})");
    let obj = run(&code, calls).unwrap();
    (&obj).try_into().unwrap()
}

/// Runs `code` under `calls` and returns its result with the wall time it took.
fn timed_run(code: &str, calls: AutoOsCalls) -> (MontyObject, Duration) {
    let started = Instant::now();
    let result = run(code, calls).unwrap();
    (result, started.elapsed())
}

/// A runner that was never configured answers the clock itself: standard
/// execution has no host to ask, so denying by default is what made ordinary
/// date-handling scripts raise.
#[test]
fn the_system_clock_is_the_default() {
    let code = "from datetime import date, datetime\n(date.today().year, datetime.now().year)";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let value = runner.run_no_limits(vec![]).unwrap();
    let Some(years) = value.as_ref().items() else {
        panic!("expected a tuple of years");
    };
    for year in years {
        let Some(year) = year.as_int() else {
            panic!("expected an int year");
        };
        assert!((2026..=2100).contains(&year), "implausible year {year}");
    }
}

/// `CallHost` hands the calls to the host; standard execution has none, so
/// they fail exactly as every other unanswered OS call does there.
#[test]
fn call_host_refuses_every_clock_call_under_standard_execution() {
    let calls = with_datetime(DateTimeSource::CallHost);
    assert_eq!(
        run("from datetime import datetime\ndatetime.now()", calls.clone()).unwrap_err(),
        "NotImplementedError: OS function 'datetime.now' not implemented with standard execution"
    );
    assert_eq!(
        run("from datetime import date\ndate.today()", calls.clone()).unwrap_err(),
        "NotImplementedError: OS function 'date.today' not implemented with standard execution"
    );
    assert_eq!(
        run("import time\ntime.time()", calls).unwrap_err(),
        "NotImplementedError: OS function 'time.time' not implemented with standard execution"
    );
}

/// `time.time()` is the same instant as `datetime.now()`, read as epoch
/// seconds rather than as local wall time — no timezone applies to it.
#[test]
fn fixed_clock_reads_epoch_seconds() {
    assert_eq!(run("import time\ntime.time()", with_datetime(FIXED)).unwrap(), {
        MontyObject::float(1_700_000_000.123_456)
    });
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

/// The `tz` argument itself is attached, as CPython does, so identity holds.
#[test]
fn now_attaches_the_timezone_argument() {
    let code = "from datetime import datetime, timedelta, timezone\n\
                tz = timezone(timedelta(hours=1))\n\
                datetime.now(tz).tzinfo is tz";
    assert_eq!(run(code, with_datetime(FIXED)).unwrap(), MontyObject::bool(true));
}

/// An aware `now(tz)` and a naive one are the same instant, whatever the
/// clock's own local offset is.
#[test]
fn aware_and_naive_agree_on_the_instant() {
    let code = "from datetime import datetime, timezone\n\
                (datetime.now(timezone.utc).hour - datetime.now().hour) % 24";
    assert_eq!(run(code, with_datetime(FIXED)).unwrap(), MontyObject::int(22));
}

/// A fixed instant outside `datetime`'s 1..=9999 years raises rather than
/// producing an out-of-range value.
#[test]
fn unrepresentable_fixed_instant_raises() {
    let far_future = with_datetime(DateTimeSource::Fixed {
        unix_seconds: 300_000_000_000,
        microsecond: 0,
    });
    assert_eq!(
        run("from datetime import date\ndate.today()", far_future.clone()).unwrap_err(),
        "OverflowError: date value out of range"
    );
    // `time.time()` has no year range of its own, but a clock answers every call or none
    assert_eq!(
        run("import time\ntime.time()", far_future).unwrap_err(),
        "OverflowError: date value out of range"
    );
}

/// An instant the sandbox owns but cannot represent in the requested zone
/// raises too, rather than falling back to the host's clock: the last second
/// of year 9999 UTC is year 10000 one hour east.
#[test]
fn unrepresentable_fixed_instant_in_a_timezone_raises() {
    let last_second = with_datetime(DateTimeSource::Fixed {
        unix_seconds: 253_402_300_799,
        microsecond: 0,
    });
    assert_eq!(
        run(
            "from datetime import datetime, timedelta, timezone\ndatetime.now(timezone(timedelta(hours=1)))",
            last_second
        )
        .unwrap_err(),
        "OverflowError: date value out of range"
    );
}

#[test]
fn out_of_range_microsecond_raises() {
    let overflowing = with_datetime(DateTimeSource::Fixed {
        unix_seconds: FIXTURE_SECONDS,
        microsecond: 1_500_000,
    });
    assert_eq!(
        run("from datetime import datetime\ndatetime.now()", overflowing).unwrap_err(),
        "OverflowError: date value out of range"
    );

    // On the last second of a minute chrono reads nanoseconds past a full
    // second as a leap second and accepts them, so `from_timestamp` alone does
    // not bound this — only `read()`'s own check does.
    let leap_second = with_datetime(DateTimeSource::Fixed {
        unix_seconds: LAST_SECOND_OF_A_MINUTE,
        microsecond: 1_500_000,
    });
    assert_eq!(
        run("from datetime import datetime\ndatetime.now()", leap_second).unwrap_err(),
        "OverflowError: date value out of range"
    );
}

/// The zone is the session's own: a fixed instant read in the system zone
/// lands on the host's wall clock, and `CallHost` on the zone alone sends
/// only the calls that need it — naive `now()` and `today()` — to the host.
#[test]
fn the_zone_is_chosen_separately_from_the_instant() {
    let system_zone = AutoOsCalls {
        datetime: FIXED,
        timezone: SandboxTimeZone::System,
        ..AutoOsCalls::default()
    };
    let code = "from datetime import datetime, timezone\n\
                (datetime.now() - datetime.now(timezone.utc).replace(tzinfo=None)).total_seconds()";
    // the host's offset at the instant read, not now: DST may differ
    let host_offset = Local
        .timestamp_opt(FIXTURE_SECONDS, 0)
        .unwrap()
        .offset()
        .fix()
        .local_minus_utc();
    assert_eq!(
        run(code, system_zone).unwrap(),
        MontyObject::float(f64::from(host_offset))
    );

    let host_zone = AutoOsCalls {
        datetime: FIXED,
        timezone: SandboxTimeZone::CallHost,
        ..AutoOsCalls::default()
    };
    assert_eq!(
        run("import time\ntime.time()", host_zone.clone()).unwrap(),
        MontyObject::float(1_700_000_000.123_456)
    );
    assert_eq!(
        run_repr_under("datetime.now(timezone.utc)", host_zone.clone()),
        "datetime.datetime(2023, 11, 14, 22, 13, 20, 123456, tzinfo=datetime.timezone.utc)"
    );
    assert_eq!(
        run("from datetime import datetime\ndatetime.now()", host_zone.clone()).unwrap_err(),
        "NotImplementedError: OS function 'datetime.now' not implemented with standard execution"
    );
    assert_eq!(
        run("from datetime import date\ndate.today()", host_zone).unwrap_err(),
        "NotImplementedError: OS function 'date.today' not implemented with standard execution"
    );
}

#[test]
fn system_clock_returns_a_plausible_now() {
    // Written 2026; a system clock that reads before then is broken, not stale.
    let code = "from datetime import date, datetime\n\
                date.today() == datetime.now().date() and datetime.now().year >= 2026";
    assert_eq!(run(code, with_datetime(DateTimeSource::System)).unwrap(), {
        MontyObject::bool(true)
    });
}

/// The sandbox answers the clock on the suspending path too; only `CallHost`
/// lets the host see (and so deny or fake) the call.
#[test]
fn iterative_execution_answers_the_clock_unless_told_to_call_the_host() {
    let code = "from datetime import date\nrepr(date.today())";
    let progress = runner(code, with_datetime(FIXED))
        .start(vec![], ResourceTracker::default(), PrintWriter::Disabled)
        .unwrap();
    assert_eq!(
        progress.into_complete().expect("answered in the sandbox"),
        MontyObject::string("datetime.date(2023, 11, 15)")
    );

    let progress = runner(code, with_datetime(DateTimeSource::CallHost))
        .start(vec![], ResourceTracker::default(), PrintWriter::Disabled)
        .unwrap();
    let call = progress.into_os_call().expect("date.today() suspends to the host");
    assert_eq!(call.function_call.name(), "date.today");
}

#[test]
fn repl_sessions_take_the_configuration_too() {
    let mut repl = MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default())
        .with_auto_os_calls(with_datetime(FIXED));
    let result = repl
        .feed_run(
            "from datetime import date\nrepr(date.today())",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
    assert_eq!(result, MontyObject::string("datetime.date(2023, 11, 15)".to_owned()));
}

/// The configuration is the session's, so which entry point runs the code
/// must not change what the code can do.
#[test]
fn call_function_takes_the_session_configuration_too() {
    let mut repl = MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default())
        .with_auto_os_calls(with_datetime(FIXED));
    repl.feed_run(
        "from datetime import date\ndef when():\n    return repr(date.today())",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();

    let result = repl.call_function("when", vec![], PrintWriter::Disabled).unwrap();
    assert_eq!(result, MontyObject::string("datetime.date(2023, 11, 15)".to_owned()));
}

/// `CallHost` refuses through `call_function` too, which has no host either.
#[test]
fn call_function_honours_call_host_too() {
    let mut repl = MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default())
        .with_auto_os_calls(with_datetime(DateTimeSource::CallHost));
    repl.feed_run(
        "from datetime import date\ndef when():\n    return date.today()",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();

    let err = repl.call_function("when", vec![], PrintWriter::Disabled).unwrap_err();
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

/// The configuration is part of the serialized session, so a restored dump
/// must still answer with it.
#[test]
fn the_configuration_survives_a_dump() {
    let mut repl = MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default())
        .with_auto_os_calls(with_datetime(FIXED));
    repl.feed_run("x = 1", vec![], PrintWriter::Disabled).unwrap();

    let bytes = dump("<test>", None, SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };

    let result = restored
        .feed_run(
            "from datetime import date\nrepr(date.today())",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
    assert_eq!(result, MontyObject::string("datetime.date(2023, 11, 15)".to_owned()));
}

// ---------------------------------------------------------------------------
// Sleeping
// ---------------------------------------------------------------------------

/// A sandbox sleep really waits, off the execution clock: `max_feed_duration` is
/// far shorter than the sleep and does not trip.
#[test]
fn sandbox_sleep_waits_without_spending_execution_time() {
    let code = "import time\ntime.sleep(0.05)\n'awake'";
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(20)));
    let started = Instant::now();
    let result = runner(code, AutoOsCalls::default())
        .run(vec![], tracker, PrintWriter::Disabled)
        .unwrap();
    assert_eq!(result, MontyObject::string("awake"));
    assert!(started.elapsed() >= Duration::from_millis(50));
}

#[test]
fn zero_returns_at_once() {
    let code = "import asyncio, time\ntime.sleep(5)\nasyncio.run(asyncio.sleep(5, 'woken'))";
    let (result, elapsed) = timed_run(code, with_sleep(SleepMode::Zero));
    assert_eq!(result, MontyObject::string("woken"));
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn the_system_maximum_cuts_a_long_sleep_short() {
    let code = "import asyncio, time\ntime.sleep(5)\nasyncio.run(asyncio.sleep(5, 'woken'))";
    let (result, elapsed) = timed_run(code, with_sleep(SleepMode::System(Duration::from_millis(20))));
    assert_eq!(result, MontyObject::string("woken"));
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

/// Sandbox sleeps are charged to `max_total_sleep`, which `max_duration`
/// cannot see: the sleep that would take the total over is refused before it
/// waits, uncatchably, for `time.sleep` and a sandbox `asyncio.sleep` alike.
#[test]
fn sandbox_sleeps_are_bounded_by_max_total_sleep() {
    // exact binary fractions, so the reported total is exact too
    let code = "import asyncio, time\n\
        time.sleep(0.125)\n\
        try:\n    asyncio.run(asyncio.sleep(0.5))\n\
        except TimeoutError:\n    pass\n";
    let tracker = ResourceTracker::new(ResourceLimits::default().max_total_sleep(Duration::from_millis(150)));
    let started = Instant::now();
    let err = runner(code, AutoOsCalls::default())
        .run(vec![], tracker, PrintWriter::Disabled)
        .unwrap_err();
    assert_eq!(
        err.to_string().lines().last().unwrap(),
        "TimeoutError: sleep limit exceeded: 625ms > 150ms"
    );
    // refused up front: the 500 ms sleep never ran
    assert!(started.elapsed() < Duration::from_millis(500));
}

/// Under `CallHost` neither sleep is served in-process; standard execution
/// then refuses them like any other OS call.
#[test]
fn call_host_refuses_sleeping_under_standard_execution() {
    assert_eq!(
        run("import time\ntime.sleep(0)", with_sleep(SleepMode::CallHost)).unwrap_err(),
        "NotImplementedError: OS function 'time.sleep' not implemented with standard execution"
    );
    assert_eq!(
        run(
            "import asyncio\nasyncio.run(asyncio.sleep(0))",
            with_sleep(SleepMode::CallHost)
        )
        .unwrap_err(),
        "NotImplementedError: OS function 'asyncio.sleep' not implemented with standard execution"
    );
}

/// The argument is validated the same way whatever the mode.
#[test]
fn sleep_arguments_are_validated_in_every_mode() {
    for calls in [AutoOsCalls::default(), with_sleep(SleepMode::Zero), call_host()] {
        assert_eq!(
            run("import time\ntime.sleep(-1)", calls.clone()).unwrap_err(),
            "ValueError: sleep length must be non-negative"
        );
        assert_eq!(
            run("import asyncio\nasyncio.sleep(float('nan'))", calls).unwrap_err(),
            "ValueError: Invalid delay: NaN (not a number)"
        );
    }
}

/// A gathered `asyncio.sleep` reaches the host as an `asyncio.sleep` OS call
/// it may answer with a future, so the other tasks run while it waits and
/// the host decides whether sleeps overlap; the delay arrives already cut to
/// the mode's maximum.
#[test]
fn gathered_sleeps_are_the_hosts_to_overlap() {
    let code = "import asyncio\n\
                async def wait_then(n):\n    \
                    await asyncio.sleep(3600)\n    \
                    return n * 2\n\
                async def main():\n    \
                    return await asyncio.gather(wait_then(1), wait_then(2))\n\
                asyncio.run(main())";
    let mut progress = runner(code, with_sleep(SleepMode::System(Duration::from_millis(20))))
        .start(vec![], ResourceTracker::default(), PrintWriter::Disabled)
        .unwrap();
    let mut call_ids = vec![];
    for _ in 0..2 {
        let RunProgress::OsCall(call) = progress else {
            panic!("expected asyncio.sleep, got {progress:?}")
        };
        assert!(
            matches!(call.function_call, OsFunctionCall::AsyncSleep(delay) if delay == Duration::from_millis(20)),
            "got {:?}",
            call.function_call
        );
        let call_id = call.call_id;
        call_ids.push(call_id);
        progress = call
            .resume(monty_types::ExtFunctionResult::Future(call_id), PrintWriter::Disabled)
            .unwrap();
    }
    let RunProgress::ResolveFutures(state) = progress else {
        panic!("expected both sleeps pending, got {progress:?}")
    };
    let mut pending = state.pending_call_ids().to_vec();
    pending.sort_unstable();
    assert_eq!(pending, call_ids);
    let results = call_ids.iter().map(|id| (*id, MontyObject::none().into())).collect();
    let progress = state.resume(results, PrintWriter::Disabled).unwrap();
    assert_eq!(
        progress.into_complete().expect("expected Complete"),
        MontyObject::list([2, 4].map(MontyObject::int))
    );
}

/// `result` never leaves the sandbox, so a value with no host form works.
#[test]
fn sandbox_sleep_result_need_not_be_convertible() {
    let code = "import asyncio\n\
                def f():\n    return 42\n\
                async def main():\n    \
                    fs = await asyncio.gather(asyncio.sleep(0.01, f), asyncio.sleep(0.02, f))\n    \
                    return fs[0]() + fs[1]()\n\
                asyncio.run(main())";
    assert_eq!(run(code, AutoOsCalls::default()).unwrap(), MontyObject::int(84));
}

/// Under `System` a sleep reaches the host already cut to the maximum and
/// charged to `max_total_sleep`: one over budget is refused before it
/// suspends, so the host never performs a wait the budget denies.
#[test]
fn system_sleeps_reach_the_host_cut_and_charged() {
    let code = "import time\ntime.sleep(3600)\ntime.sleep(3600)";
    let calls = with_sleep(SleepMode::System(Duration::from_millis(100)));
    let tracker = ResourceTracker::new(ResourceLimits::default().max_total_sleep(Duration::from_millis(150)));
    let progress = runner(code, calls)
        .start(vec![], tracker, PrintWriter::Disabled)
        .unwrap();
    let RunProgress::OsCall(call) = progress else {
        panic!("expected time.sleep, got {progress:?}")
    };
    assert!(
        matches!(call.function_call, OsFunctionCall::Sleep(delay) if delay == Duration::from_millis(100)),
        "got {:?}",
        call.function_call
    );
    let err = call.resume(MontyObject::none(), PrintWriter::Disabled).unwrap_err();
    assert_eq!(
        err.to_string().lines().last().unwrap(),
        "TimeoutError: sleep limit exceeded: 200ms > 150ms"
    );
}

/// A sleep created in one feed is waited out at the call, so awaiting it in
/// a later feed finds it settled.
#[test]
fn a_sleep_saved_across_repl_feeds_is_settled() {
    let mut repl = MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "import asyncio\nsleeper = asyncio.sleep(0.01, 'slept')",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let result = repl
        .feed_run(
            "async def main():\n    return await sleeper\nasyncio.run(main())",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
    assert_eq!(result, MontyObject::string("slept"));
}
