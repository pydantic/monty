//! The work clock in the interpreter: bytecode and native operations charge work, waits do not.
//!
//! These tests run without `monty-alloc`, so allocation charges nothing here; the subprocess
//! tests cover that floor.

use std::{fmt::Write, fs, path::PathBuf, thread, time::Duration};

use monty::MontyRepl;
use monty_types::{
    CompileOptions, ExcType, ExecutionClock, MontyException, MontyObject, NANOS_PER_UNIT, PrintWriter, ResourceLimits,
    ResourceTracker,
};

const LOOP: &str = "x = 0\nfor i in range(10_000):\n    x = x + i\nx";

/// A session on the work clock, with an optional feed budget.
fn work_repl(max_feed_duration: Option<Duration>) -> MontyRepl {
    let limits = match max_feed_duration {
        Some(limit) => ResourceLimits::default().max_feed_duration(limit),
        None => ResourceLimits::default(),
    };
    let tracker = ResourceTracker::new(limits).with_execution_clock(ExecutionClock::Work);
    MontyRepl::new("test.py", tracker, CompileOptions::default())
}

fn feed(repl: &mut MontyRepl, code: &str) -> Result<MontyObject, MontyException> {
    repl.feed_run(code, vec![], PrintWriter::Stdout)
}

/// Units the session has been charged since it started.
fn units(repl: &MontyRepl) -> u64 {
    u64::try_from(repl.tracker().elapsed().as_nanos()).unwrap() / NANOS_PER_UNIT
}

#[test]
fn the_same_code_costs_the_same() {
    let costs: Vec<u64> = (0..3)
        .map(|_| {
            let mut repl = work_repl(None);
            assert_eq!(feed(&mut repl, LOOP).unwrap(), MontyObject::int(49_995_000));
            units(&repl)
        })
        .collect();
    assert_eq!(costs[0], costs[1]);
    assert_eq!(costs[0], costs[2]);
    // a handful of instructions per iteration
    assert!((30_000..200_000).contains(&costs[0]), "{} units", costs[0]);
}

#[test]
fn a_run_shorter_than_the_checkpoint_interval_is_charged() {
    let mut repl = work_repl(None);
    assert_eq!(feed(&mut repl, "1 + 1").unwrap(), MontyObject::int(2));
    let first = units(&repl);
    assert!(first > 0);
    assert_eq!(repl.tracker().feed_elapsed(), repl.tracker().elapsed());
    feed(&mut repl, "2 + 2").unwrap();
    assert_eq!(units(&repl), first * 2);
}

#[test]
fn costlier_instructions_are_charged_more() {
    // units for 1000 iterations of `body`
    let cost = |body: &str| {
        let mut repl = work_repl(None);
        feed(
            &mut repl,
            "class P:\n    def m(self):\n        return 1\np = P()\np.x = 1",
        )
        .unwrap();
        let before = units(&repl);
        feed(&mut repl, &format!("for i in range(1000):\n    {body}")).unwrap();
        units(&repl) - before
    };
    // The weights are fitted, so only their order is fixed: a global load and store is the
    // cheapest, and each of these does strictly more.
    let plain = cost("y = i");
    let attribute = cost("y = p.x");
    assert!(attribute > plain, "{attribute} > {plain}");
    let method = cost("y = p.m()");
    assert!(method > attribute, "{method} > {attribute}");
    let (one, two) = (cost("y = {'a': i}"), cost("y = {'a': i, 'b': i}"));
    assert!(two > one, "{two} > {one}");
}

#[test]
fn feed_limit_raises_the_usual_timeout_and_the_session_continues() {
    let mut repl = work_repl(Some(Duration::from_millis(1)));
    let error = feed(&mut repl, "x = 0\nfor i in range(10**12):\n    x = x + 1").unwrap_err();
    assert_eq!(error.exc_type(), ExcType::TimeoutError);
    let message = error.message().unwrap();
    assert!(
        message.starts_with("feed time limit exceeded: 1.00") && message.ends_with("ms > 1ms"),
        "{message}"
    );
    // detected at the first checkpoint past the budget
    let overshoot = repl
        .tracker()
        .feed_elapsed()
        .checked_sub(Duration::from_millis(1))
        .unwrap();
    assert!(overshoot < Duration::from_nanos(600 * NANOS_PER_UNIT), "{overshoot:?}");
    assert_eq!(feed(&mut repl, "x > 0").unwrap(), MontyObject::bool(true));
}

#[test]
fn a_budget_above_the_cost_does_not_fire() {
    let mut repl = work_repl(None);
    feed(&mut repl, LOOP).unwrap();
    let cost = repl.tracker().elapsed();

    let mut repl = work_repl(Some(cost));
    feed(&mut repl, LOOP).unwrap();
    let mut repl = work_repl(Some(cost / 2));
    assert_eq!(feed(&mut repl, LOOP).unwrap_err().exc_type(), ExcType::TimeoutError);
}

#[test]
fn waiting_on_the_host_charges_nothing() {
    let mut waited = Vec::new();
    for wait in [Duration::ZERO, Duration::from_millis(20)] {
        let repl = work_repl(Some(Duration::from_millis(1)));
        let call = repl
            .feed_start("host() + 1", vec![], PrintWriter::Stdout)
            .map_err(|error| error.error)
            .unwrap()
            .into_function_call()
            .expect("a host call");
        thread::sleep(wait);
        let (repl, value) = call
            .resume(MontyObject::int(41), PrintWriter::Stdout)
            .map_err(|error| error.error)
            .unwrap()
            .into_complete()
            .expect("completion");
        assert_eq!(value, MontyObject::int(42));
        waited.push(repl.tracker().elapsed());
    }
    assert_eq!(waited[0], waited[1]);
    assert!(waited[0] > Duration::ZERO);
}

#[test]
fn native_operations_charge_work() {
    let mut repl = work_repl(None);
    let setup =
        "import json, re\nvalues = list(range(200_000, 0, -1))\ntext = '0123456789' * 100_000\nbig = 3**200_000";
    feed(&mut repl, setup).unwrap();
    repl.tracker_mut().set_max_feed_duration(Duration::from_millis(1));
    for code in [
        "sorted(values)",
        "sum(values)",
        "values.index(-1)",
        "[1] * 100_000_000 == []",
        "re.sub('[0-9]', '#', text)",
        "re.findall('[0-9]', text)",
        "json.dumps(values)",
        "big * big",
    ] {
        let error = feed(&mut repl, code).unwrap_err();
        assert_eq!(error.exc_type(), ExcType::TimeoutError, "{code}");
        repl.tracker_mut().set_max_feed_duration(Duration::from_millis(1));
    }
}

/// Work that produces nothing, or little, is still charged: a scan that fails, a base reduced
/// before a modular power, an encoding that raises, a search bounded by character offsets.
#[test]
fn work_without_a_result_is_charged() {
    let mut repl = work_repl(None);
    let setup = "import json, re\nvalues = list(range(200_000))\ntext = '0123456789' * 100_000\n\
                 big = 3**200_000\nuni = 'é' * 500_000";
    feed(&mut repl, setup).unwrap();
    let mut cost = |code: &str| {
        let before = units(&repl);
        feed(&mut repl, code).unwrap();
        units(&repl) - before
    };
    for (code, at_least) in [
        ("re.fullmatch('[0-9]+x', text)", 50_000),
        ("re.match('[0-9]+x', text)", 50_000),
        ("re.split('x', text)", 50_000),
        ("pow(big, 2, 7)", 1_000),
        (
            "try:\n    json.dumps(values + [{1}])\nexcept TypeError:\n    pass",
            100_000,
        ),
        ("uni.find('x', 499_999)", 50_000),
    ] {
        let units = cost(code);
        assert!(units >= at_least, "{code}: {units} units");
    }
    // nothing is divided when the dividend is the shorter operand
    let units = cost("1 // big");
    assert!(units < 100, "{units} units");
}

/// The calibration scripts `make calibrate-work-clock` measures, one per category of work.
const CALIBRATION_SCRIPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../monty-bench/benches/python/work_clock");

/// Pins what each calibration script is charged, so a change to the accounting shows in review.
/// A new native operation that charges nothing shows instead as drift in `make calibrate-work-clock`.
#[test]
fn calibration_scripts_cost_fixed_units() {
    let mut paths: Vec<PathBuf> = fs::read_dir(CALIBRATION_SCRIPTS)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "py"))
        .collect();
    paths.sort();
    let mut report = String::new();
    for path in paths {
        let mut repl = work_repl(None);
        feed(&mut repl, &fs::read_to_string(&path).unwrap()).unwrap();
        let name = path.file_stem().unwrap().to_str().unwrap();
        writeln!(report, "{name}: {}", units(&repl)).unwrap();
    }
    insta::assert_snapshot!(report, @"
    arith_loop: 3861648
    attr_access: 6923927
    big_ints: 233463
    bytes_ops: 4185792
    comprehensions: 10998909
    dict_ops: 4687520
    func_calls: 1768680
    json_roundtrip: 4664361
    list_ops: 4487639
    regex: 1606159
    script_style: 11768467
    sorting: 13035777
    str_build: 2621725
    str_scan: 827938
    ");
}
