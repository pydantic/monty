//! Clock and sleep policies across execution paths. Fixed-clock expectations
//! were checked against CPython 3.14; `test_cases/` uses a moving clock.
//! Random initialization is covered in `random_module.rs`.

use std::time::{Duration, Instant};

use chrono::{Local, Offset, TimeZone};
use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    AutoOsCalls, CompileOptions, DateTimeSource, MontyObject, OsFunctionCall, PrintWriter, ResourceLimits,
    ResourceTracker, SandboxTimeZone, SleepMode,
};

/// 2023-11-14 22:13:20 UTC, shared with the datatest fixtures.
const FIXTURE_SECONDS: i64 = 1_700_000_000;

/// 2023-11-14 22:13:59 UTC, where chrono accepts leap-second fractions.
const LAST_SECOND_OF_A_MINUTE: i64 = 1_700_000_039;

const FIXED: DateTimeSource = DateTimeSource::Fixed {
    unix_seconds: FIXTURE_SECONDS,
    microsecond: 123_456,
};

/// UTC+02:00 puts FIXED on the next calendar day.
const PLUS_TWO: SandboxTimeZone = SandboxTimeZone::Fixed {
    offset_seconds: 7_200,
    name: None,
};

/// Use UTC+02:00 for fixed instants to make expectations independent of the host zone.
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

fn with_sleep(sleep: SleepMode) -> AutoOsCalls {
    AutoOsCalls {
        sleep,
        ..AutoOsCalls::default()
    }
}

fn call_host() -> AutoOsCalls {
    AutoOsCalls {
        datetime: DateTimeSource::CallHost,
        sleep: SleepMode::CallHost,
        ..AutoOsCalls::default()
    }
}

fn runner(code: &str, calls: AutoOsCalls) -> MontyRun {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .with_auto_os_calls(calls)
}

/// Returns errors as `Type: message`, without tracebacks.
fn run(code: &str, calls: AutoOsCalls) -> Result<MontyObject, String> {
    runner(code, calls)
        .run_no_limits(vec![])
        .map_err(|err| err.to_string().lines().last().unwrap_or_default().to_owned())
}

fn run_repr(expr: &str, datetime: DateTimeSource) -> String {
    run_repr_under(expr, with_datetime(datetime))
}

fn run_repr_under(expr: &str, calls: AutoOsCalls) -> String {
    let code = format!("from datetime import date, datetime, timedelta, timezone\nrepr({expr})");
    let obj = run(&code, calls).unwrap();
    (&obj).try_into().unwrap()
}

fn timed_run(code: &str, calls: AutoOsCalls) -> (MontyObject, Duration) {
    let started = Instant::now();
    let result = run(code, calls).unwrap();
    (result, started.elapsed())
}

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

/// Epoch seconds are independent of the session timezone.
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

#[test]
fn now_attaches_the_timezone_argument() {
    let code = "from datetime import datetime, timedelta, timezone\n\
                tz = timezone(timedelta(hours=1))\n\
                datetime.now(tz).tzinfo is tz";
    assert_eq!(run(code, with_datetime(FIXED)).unwrap(), MontyObject::bool(true));
}

#[test]
fn aware_and_naive_agree_on_the_instant() {
    let code = "from datetime import datetime, timezone\n\
                (datetime.now(timezone.utc).hour - datetime.now().hour) % 24";
    assert_eq!(run(code, with_datetime(FIXED)).unwrap(), MontyObject::int(22));
}

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

/// Converting year 9999 UTC to UTC+01:00 overflows without falling back to the host.
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

    // Chrono accepts leap seconds, so read() must reject them explicitly.
    let leap_second = with_datetime(DateTimeSource::Fixed {
        unix_seconds: LAST_SECOND_OF_A_MINUTE,
        microsecond: 1_500_000,
    });
    assert_eq!(
        run("from datetime import datetime\ndatetime.now()", leap_second).unwrap_err(),
        "OverflowError: date value out of range"
    );
}

/// A CallHost zone delegates only naive now() and today(); time() and now(tz) stay local.
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
    let code = "from datetime import date, datetime\n\
                date.today() == datetime.now().date() and datetime.now().year >= 2026";
    assert_eq!(run(code, with_datetime(DateTimeSource::System)).unwrap(), {
        MontyObject::bool(true)
    });
}

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

/// The sleep exceeds max_feed_duration but must not count toward it.
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

/// The host receives capped delays and may resolve them as futures to overlap sleeps.
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
            matches!(call.function_call, OsFunctionCall::AsyncSystemSleep(delay) if delay == Duration::from_millis(20)),
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

/// The interpreter caps individual delays; the host enforces max_total_sleep.
#[test]
fn system_sleeps_reach_the_host_cut_but_uncharged() {
    let code = "import time\ntime.sleep(3600)\ntime.sleep(3600)";
    let calls = with_sleep(SleepMode::System(Duration::from_millis(100)));
    let tracker = ResourceTracker::new(ResourceLimits::default().max_total_sleep(Duration::from_millis(150)));
    let mut progress = runner(code, calls)
        .start(vec![], tracker, PrintWriter::Disabled)
        .unwrap();
    for _ in 0..2 {
        let RunProgress::OsCall(call) = progress else {
            panic!("expected time.sleep, got {progress:?}")
        };
        assert!(
            matches!(call.function_call, OsFunctionCall::SystemSleep(delay) if delay == Duration::from_millis(100)),
            "got {:?}",
            call.function_call
        );
        assert_eq!(call.tracker().max_total_sleep(), Some(Duration::from_millis(150)));
        progress = call.resume(MontyObject::none(), PrintWriter::Disabled).unwrap();
    }
    assert!(matches!(progress, RunProgress::Complete(_)), "got {progress:?}");
}

/// Waiting starts at the call, so a later feed awaits an already-settled sleep.
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
