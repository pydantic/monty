//! Clock and sleep policies across execution paths. Fixed-clock expectations
//! were checked against CPython 3.14; `test_cases/` uses a moving clock.
//! Random initialization is covered in `random_module.rs`.

use std::time::{Duration, Instant};

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    AutoOsCalls, CompileOptions, DateTimeSource, MontyObject, OsFunctionCall, PrintWriter, ProcessTime, ResourceLimits,
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

/// Fixed instants use UTC+02:00, so the date-changing offset is exercised; the rest keep the UTC default.
fn with_datetime(datetime: DateTimeSource) -> AutoOsCalls {
    let timezone = match datetime {
        DateTimeSource::Fixed { .. } => PLUS_TWO,
        DateTimeSource::CallHost | DateTimeSource::System => SandboxTimeZone::default(),
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
    let code = format!("import time\nfrom datetime import date, datetime, timedelta, timezone\nrepr({expr})");
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

/// Every wall clock in the module reads the same session instant, so a fixed
/// clock pins `monotonic` and `perf_counter` too (see `limitations/time.md`).
#[test]
fn fixed_clock_pins_every_wall_clock() {
    for clock in ["time.time()", "time.monotonic()", "time.perf_counter()"] {
        assert_eq!(
            run(&format!("import time\n{clock}"), with_datetime(FIXED)).unwrap(),
            MontyObject::float(1_700_000_000.123_456),
            "{clock}"
        );
    }
    for clock in ["time.time_ns()", "time.monotonic_ns()", "time.perf_counter_ns()"] {
        assert_eq!(
            run(&format!("import time\n{clock}"), with_datetime(FIXED)).unwrap(),
            MontyObject::int(1_700_000_000_123_456_000),
            "{clock}"
        );
    }
}

/// The conversion functions read the same clock, and the session zone.
#[test]
fn fixed_clock_feeds_the_conversion_functions() {
    // UTC+02:00, so the local wall clock is on the next day.
    let local = "time.struct_time(tm_year=2023, tm_mon=11, tm_mday=15, tm_hour=0, tm_min=13, tm_sec=20, tm_wday=2, tm_yday=319, tm_isdst=0, tm_gmtoff=7200, tm_zone='UTC+02:00')";
    let utc = "time.struct_time(tm_year=2023, tm_mon=11, tm_mday=14, tm_hour=22, tm_min=13, tm_sec=20, tm_wday=1, tm_yday=318, tm_isdst=0, tm_gmtoff=0, tm_zone='UTC')";
    assert_eq!(run_repr("time.localtime()", FIXED), local);
    assert_eq!(run_repr("time.gmtime()", FIXED), utc);
    assert_eq!(run_repr("time.ctime()", FIXED), "'Wed Nov 15 00:13:20 2023'");
    assert_eq!(run_repr("time.asctime()", FIXED), "'Wed Nov 15 00:13:20 2023'");
    assert_eq!(
        run_repr("time.strftime('%Y-%m-%d %H:%M %Z')", FIXED),
        "'2023-11-15 00:13 UTC+02:00'"
    );
}

/// `process_time` has its own policy, so a fixed clock does not pin it and the
/// default keeps elapsed execution time out of the sandbox entirely.
#[test]
fn the_process_clocks_default_to_zero() {
    let expr = "(time.process_time(), time.thread_time(), time.process_time_ns(), time.thread_time_ns())";
    assert_eq!(run_repr_under(expr, AutoOsCalls::default()), "(0.0, 0.0, 0, 0)");
    // a fixed wall clock changes nothing: the two policies are independent
    assert_eq!(run_repr(expr, FIXED), "(0.0, 0.0, 0, 0)");
}

#[test]
fn the_process_clocks_report_execution_time_when_asked() {
    let calls = AutoOsCalls {
        process_time: ProcessTime::Elapsed,
        ..AutoOsCalls::default()
    };
    // the clock only advances while the VM runs, so burn some instructions
    let code = "import time\nstart = time.process_time()\nfor _ in range(200000):\n    pass\n(time.process_time() > start, time.process_time_ns() > 0)";
    assert_eq!(
        run(code, calls).unwrap(),
        MontyObject::tuple([MontyObject::bool(true), MontyObject::bool(true)])
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

/// The default zone is UTC; a named zone shifts only
/// naive now() and today(), while time() and now(tz) stay UTC.
#[test]
fn the_zone_is_chosen_separately_from_the_instant() {
    let utc_zone = AutoOsCalls {
        datetime: FIXED,
        ..AutoOsCalls::default()
    };
    let code = "from datetime import datetime, timezone\n\
                (datetime.now() - datetime.now(timezone.utc).replace(tzinfo=None)).total_seconds()";
    assert_eq!(run(code, utc_zone).unwrap(), MontyObject::float(0.0));

    // a named zone applies its rules at the instant: London is on GMT in November
    let london = AutoOsCalls {
        datetime: FIXED,
        timezone: SandboxTimeZone::named("Europe/London").unwrap(),
        ..AutoOsCalls::default()
    };
    assert_eq!(
        run("import time\ntime.time()", london.clone()).unwrap(),
        MontyObject::float(1_700_000_000.123_456)
    );
    assert_eq!(
        run_repr_under("datetime.now(timezone.utc)", london.clone()),
        "datetime.datetime(2023, 11, 14, 22, 13, 20, 123456, tzinfo=datetime.timezone.utc)"
    );
    assert_eq!(
        run_repr_under("datetime.now()", london.clone()),
        "datetime.datetime(2023, 11, 14, 22, 13, 20, 123456)"
    );
    assert_eq!(run_repr_under("date.today()", london), "datetime.date(2023, 11, 14)");
}

/// `astimezone()`, the `time` constants and `%Z` all read the sandbox zone: UTC
/// unless configured, with the configured name.
#[test]
fn astimezone_and_the_time_constants_read_the_sandbox_zone() {
    let utc = AutoOsCalls::default();
    assert_eq!(
        run_repr_under("datetime(2024, 6, 15, 12, 30).astimezone()", utc.clone()),
        "datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(0), 'UTC'))"
    );
    assert_eq!(
        run_repr_under("(time.timezone, time.altzone, time.daylight, time.tzname)", utc),
        "(0, 0, 0, ('UTC', 'UTC'))"
    );

    let eet = AutoOsCalls {
        datetime: FIXED,
        timezone: SandboxTimeZone::Fixed {
            offset_seconds: 7_200,
            name: Some("EET".to_owned()),
        },
        ..AutoOsCalls::default()
    };
    assert_eq!(
        run_repr_under("datetime(2024, 6, 15, 12, 30).astimezone()", eet.clone()),
        "datetime.datetime(2024, 6, 15, 12, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=7200), 'EET'))"
    );
    assert_eq!(
        run_repr_under(
            "datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone()",
            eet.clone()
        ),
        "datetime.datetime(2024, 6, 15, 14, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=7200), 'EET'))"
    );
    // a naive value is read in the sandbox zone, then converted to the explicit one
    assert_eq!(
        run_repr_under("datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc)", eet.clone()),
        "datetime.datetime(2024, 6, 15, 10, 30, tzinfo=datetime.timezone.utc)"
    );
    assert_eq!(
        run_repr_under(
            "datetime.now().astimezone().strftime('%Y-%m-%d %H:%M %Z %z')",
            eet.clone()
        ),
        "'2023-11-15 00:13 EET +0200'"
    );
    assert_eq!(
        run_repr_under("(time.timezone, time.altzone, time.daylight, time.tzname)", eet),
        "(-7200, -7200, 0, ('EET', 'EET'))"
    );
}

/// A named zone carries its DST rules: the offset and abbreviation follow the
/// instant, a naive value is read at its first occurrence (CPython's `fold=0`),
/// and the `time` constants come from January and July of the clock's year.
/// Expectations were checked against CPython under `TZ=Europe/London` and
/// `TZ=Australia/Sydney`.
#[test]
fn a_named_zone_applies_its_dst_rules() {
    let london = AutoOsCalls {
        datetime: FIXED,
        timezone: SandboxTimeZone::named("Europe/London").unwrap(),
        ..AutoOsCalls::default()
    };
    assert_eq!(
        run_repr_under(
            "datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).astimezone()",
            london.clone()
        ),
        "datetime.datetime(2024, 6, 15, 13, 30, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'BST'))"
    );
    assert_eq!(
        run_repr_under("datetime(2024, 6, 15, 12, 30).astimezone(timezone.utc)", london.clone()),
        "datetime.datetime(2024, 6, 15, 11, 30, tzinfo=datetime.timezone.utc)"
    );
    // 01:30 happens twice when the clocks go back; the first occurrence is still BST
    assert_eq!(
        run_repr_under("datetime(2024, 10, 27, 1, 30).astimezone(timezone.utc)", london.clone()),
        "datetime.datetime(2024, 10, 27, 0, 30, tzinfo=datetime.timezone.utc)"
    );
    // 01:30 never happens when the clocks go forward; the offset from before the gap applies
    assert_eq!(
        run_repr_under("datetime(2024, 3, 31, 1, 30).astimezone(timezone.utc)", london.clone()),
        "datetime.datetime(2024, 3, 31, 1, 30, tzinfo=datetime.timezone.utc)"
    );
    assert_eq!(
        run_repr_under(
            "datetime.now().astimezone().strftime('%Y-%m-%d %H:%M %Z %z')",
            london.clone()
        ),
        "'2023-11-14 22:13 GMT +0000'"
    );
    assert_eq!(
        run_repr_under("(time.timezone, time.altzone, time.daylight, time.tzname)", london),
        "(0, -3600, 1, ('GMT', 'BST'))"
    );
    // south of the equator January is the daylight half, so the halves swap
    let sydney = AutoOsCalls {
        datetime: FIXED,
        timezone: SandboxTimeZone::named("Australia/Sydney").unwrap(),
        ..AutoOsCalls::default()
    };
    assert_eq!(
        run_repr_under("(time.timezone, time.altzone, time.daylight, time.tzname)", sydney),
        "(-36000, -39600, 1, ('AEST', 'AEDT'))"
    );
    // the constants need the clock's year, which a CallHost clock cannot give at import
    let no_clock = AutoOsCalls {
        datetime: DateTimeSource::CallHost,
        timezone: SandboxTimeZone::named("Europe/London").unwrap(),
        ..AutoOsCalls::default()
    };
    assert_eq!(
        run("import time\ntime.tzname", no_clock).unwrap_err(),
        "AttributeError: 'module' object has no attribute 'tzname'"
    );
    assert_eq!(
        run_repr_under("(time.timezone, time.tzname)", call_host()),
        "(0, ('UTC', 'UTC'))"
    );
}

/// A named zone covers `datetime`'s whole range, where the instants sit outside
/// what jiff's own timestamps hold: before the epoch, where its sub-second
/// component is negative, and on the last day, past its maximum timestamp.
/// Expectations were checked against CPython under `TZ=Europe/London`.
#[test]
fn a_named_zone_spans_the_full_datetime_range() {
    let london = AutoOsCalls {
        datetime: FIXED,
        timezone: SandboxTimeZone::named("Europe/London").unwrap(),
        ..AutoOsCalls::default()
    };
    assert_eq!(
        run_repr_under(
            "datetime(1960, 6, 15, 12, 30, 0, 123456).astimezone(timezone.utc)",
            london.clone()
        ),
        "datetime.datetime(1960, 6, 15, 11, 30, 0, 123456, tzinfo=datetime.timezone.utc)"
    );
    assert_eq!(
        run_repr_under(
            "datetime(1960, 1, 15, 12, 30, 0, 123456).astimezone(timezone.utc)",
            london.clone()
        ),
        "datetime.datetime(1960, 1, 15, 12, 30, 0, 123456, tzinfo=datetime.timezone.utc)"
    );
    // December is GMT; jiff's last timestamp is a day earlier, but no zone changes then
    assert_eq!(
        run_repr_under(
            "datetime(9999, 12, 31, 12, 0, tzinfo=timezone.utc).astimezone()",
            london.clone()
        ),
        "datetime.datetime(9999, 12, 31, 12, 0, tzinfo=datetime.timezone(datetime.timedelta(0), 'GMT'))"
    );
    assert_eq!(
        run_repr_under(
            "datetime(9999, 12, 31, 12, 0, tzinfo=timezone.utc).astimezone().strftime('%Y-%m-%d %H:%M %Z %z')",
            london
        ),
        "'9999-12-31 12:00 GMT +0000'"
    );
}

/// CPython renders the days either side of a naive value to find its local
/// offset, so `astimezone()` refuses the first and last representable days in
/// every zone; an aware value on those days converts. Expectations were checked
/// against CPython under `TZ=UTC` and `TZ=Europe/London`.
#[test]
fn naive_astimezone_refuses_the_first_and_last_day() {
    for zone in [SandboxTimeZone::utc(), SandboxTimeZone::named("Europe/London").unwrap()] {
        let calls = AutoOsCalls {
            datetime: FIXED,
            timezone: zone,
            ..AutoOsCalls::default()
        };
        let refused = |expr: &str| {
            let code = format!("from datetime import datetime, timezone\n{expr}");
            run(&code, calls.clone()).unwrap_err()
        };
        assert_eq!(
            refused("datetime(9999, 12, 31, 12, 0).astimezone(timezone.utc)"),
            "ValueError: year must be in 1..9999, not 10000"
        );
        assert_eq!(
            refused("datetime(1, 1, 1, 12, 0).astimezone(timezone.utc)"),
            "ValueError: year must be in 1..9999, not 0"
        );
        // the day either side is fine, and an aware value never probes
        assert_eq!(
            run_repr_under("datetime(9999, 12, 30, 12, 0).astimezone(timezone.utc)", calls.clone()),
            "datetime.datetime(9999, 12, 30, 12, 0, tzinfo=datetime.timezone.utc)"
        );
        assert_eq!(
            run_repr_under(
                "datetime(9999, 12, 31, 12, 0, tzinfo=timezone.utc).astimezone(timezone.utc)",
                calls
            ),
            "datetime.datetime(9999, 12, 31, 12, 0, tzinfo=datetime.timezone.utc)"
        );
    }
}

/// A naive `timestamp()` reads the session zone, and reproduces the range error
/// CPython's own solve raises: always on the first representable day, and on the
/// last only where the zone shifts past the end of the range. Expectations were
/// checked against CPython under `TZ=Europe/London` and `TZ=Asia/Kathmandu`.
#[test]
fn naive_timestamp_reads_the_session_zone() {
    let under = |zone: SandboxTimeZone| AutoOsCalls {
        datetime: FIXED,
        timezone: zone,
        ..AutoOsCalls::default()
    };
    let london = under(SandboxTimeZone::named("Europe/London").unwrap());
    // BST, so an hour earlier in UTC than the same wall clock read as UTC
    assert_eq!(
        run_repr_under("datetime(2024, 6, 15, 12, 30).timestamp()", london.clone()),
        "1718451000.0"
    );
    assert_eq!(
        run_repr_under("datetime(2024, 1, 15, 12, 30).timestamp()", london.clone()),
        "1705321800.0"
    );
    // an aware value carries its own offset and never consults the zone
    assert_eq!(
        run_repr_under(
            "datetime(2024, 6, 15, 12, 30, tzinfo=timezone.utc).timestamp()",
            london.clone()
        ),
        "1718454600.0"
    );
    let refused = |expr: &str, calls: AutoOsCalls| {
        let code = format!("from datetime import datetime, timezone\n{expr}");
        run(&code, calls).unwrap_err()
    };
    assert_eq!(
        refused("datetime(1, 1, 1, 12, 0).timestamp()", london.clone()),
        "ValueError: year must be in 1..9999, not 0"
    );
    // GMT in December, so the last day still lands inside the range
    assert_eq!(
        run_repr_under("datetime(9999, 12, 31, 23, 0).timestamp()", london),
        "253402297200.0"
    );
    // +05:45 pushes 9999-12-31 19:00 into year 10000, but 18:00 stays inside
    let kathmandu = under(SandboxTimeZone::named("Asia/Kathmandu").unwrap());
    assert_eq!(
        refused("datetime(9999, 12, 31, 19, 0).timestamp()", kathmandu.clone()),
        "ValueError: year must be in 1..9999, not 10000"
    );
    assert_eq!(
        run_repr_under("datetime(9999, 12, 31, 18, 0).timestamp()", kathmandu),
        "253402258500.0"
    );
}

/// Zone names are validated before the database sees them, and the database
/// answers for `UTC` and every IANA key.
#[test]
fn zone_names_are_resolved_or_refused() {
    assert_eq!(
        SandboxTimeZone::named("UTC").unwrap().iana_name(),
        Some("UTC"),
        "UTC is in every database"
    );
    assert_eq!(
        SandboxTimeZone::named("Europe/London").unwrap().iana_name(),
        Some("Europe/London")
    );
    for name in [
        "",
        "Mars/Olympus",
        ".",
        "../zoneinfo/UTC",
        "Europe/../UTC",
        "Europe//London",
        "Europe/London\0",
        // jiff's nameless placeholder zone, which nothing could re-resolve by name
        "Etc/Unknown",
    ] {
        assert_eq!(
            SandboxTimeZone::named(name).unwrap_err().to_string(),
            format!("unknown timezone '{name}'")
        );
    }
    assert_eq!(SandboxTimeZone::utc().iana_name(), None);
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
    let calls = AutoOsCalls {
        process_time: ProcessTime::Elapsed,
        ..with_datetime(FIXED)
    };
    let mut repl =
        MontyRepl::new("<test>", ResourceTracker::default(), CompileOptions::default()).with_auto_os_calls(calls);
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
    // the process clock is a policy of its own, so it must survive too
    let elapsed = restored
        .feed_run("import time\ntime.process_time() > 0.0", vec![], PrintWriter::Disabled)
        .unwrap();
    assert_eq!(elapsed, MontyObject::bool(true));
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
