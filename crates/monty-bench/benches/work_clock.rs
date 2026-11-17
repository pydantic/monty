//! Calibrates the work clock: units charged against time taken.
//!
//! Every run is made twice, once on the work clock for its units and once on the wall clock for
//! its execution time. Three reports, each a different question:
//!
//! - categories (always): the `python/work_clock/*.py` scripts, one per kind of work. Their median
//!   is the native rate; `--check` fails when one is more than [`MAX_DRIFT`] from it.
//! - `--constructs`: each section of `python/work_clock_constructs.py` against its baseline, as
//!   units charged and units the time is worth. This is the report to set a weight from.
//! - `--corpus`: every self-contained case in `crates/monty/test_cases`, code the weights were not
//!   fitted to. An outlier there names work no construct covers.
//!
//! The median is what the native `NANOS_PER_UNIT` should be. The wasm build has its own, which
//! `crates/monty-js/scripts/calibrate-work-clock.mjs` measures. Run both with
//! `make calibrate-work-clock`.

use std::{env, fs, path::PathBuf, process::ExitCode, time::Duration};

use monty::MontyRepl;
use monty_fixture::Header;
use monty_types::{CompileOptions, ExecutionClock, NANOS_PER_UNIT, PrintWriter, ResourceLimits, ResourceTracker};

// The worker's allocator, so allocation is charged here as it is in a worker.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// The calibration scripts, one per category of work.
const SCRIPTS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/python/work_clock");

/// The per-construct sections.
const CONSTRUCTS_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/python/work_clock_constructs.py");

/// The interpreter's test cases, used as a corpus.
const CORPUS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../monty/test_cases");

/// Timed runs per script; the fastest is the least disturbed.
const RUNS: usize = 7;

/// How far a category's rate may sit from the median before `--check` fails.
const MAX_DRIFT: f64 = 3.0;

/// Units below which a corpus case is too short to time; it still counts toward the totals.
const CORPUS_MIN_UNITS: u64 = 2_000;

/// Work-clock budget for a corpus case, so one that never ends is skipped, not waited for.
const CORPUS_BUDGET: Duration = Duration::from_secs(2);

/// Corpus cases listed at each end of the rate distribution.
const CORPUS_OUTLIERS: usize = 15;

fn main() -> ExitCode {
    let flag = |name: &str| env::args().any(|arg| arg == name);
    let (median, drifted) = report_categories();
    if flag("--constructs") {
        report_constructs(median);
    }
    if flag("--corpus") {
        report_corpus(median);
    }
    if flag("--check") && drifted {
        eprintln!("a category is more than {MAX_DRIFT}x from the median");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Prints each category's rate and returns the median with whether any drifted from it.
fn report_categories() -> (f64, bool) {
    let mut rows: Vec<Row> = python_files(SCRIPTS_DIR)
        .iter()
        .map(|(name, code)| Row::measure(name, code, ResourceLimits::default()).expect("calibration scripts complete"))
        .collect();
    rows.sort_by(|a, b| a.nanos_per_unit().total_cmp(&b.nanos_per_unit()));
    let median = rows[rows.len() / 2].nanos_per_unit();

    println!(
        "{:<18} {:>12} {:>10} {:>9} {:>8}",
        "script", "units", "time", "ns/unit", "/median"
    );
    let mut drifted = false;
    for row in &rows {
        let ratio = row.nanos_per_unit() / median;
        let flag = if (1.0 / MAX_DRIFT..=MAX_DRIFT).contains(&ratio) {
            ""
        } else {
            drifted = true;
            "  <-- drift"
        };
        println!(
            "{:<18} {:>12} {:>8.2}ms {:>9.2} {:>8.2}{flag}",
            row.name,
            row.units,
            row.time.as_secs_f64() * 1000.0,
            row.nanos_per_unit(),
            ratio,
        );
    }
    println!("native median: {median:.2} ns/unit (NANOS_PER_UNIT is {NANOS_PER_UNIT})");
    (median, drifted)
}

/// Prints, per construct, the units charged beyond the baseline and the units its extra time is
/// worth at `rate` nanoseconds per unit.
fn report_constructs(rate: f64) {
    let source = fs::read_to_string(CONSTRUCTS_FILE).unwrap_or_else(|error| panic!("{CONSTRUCTS_FILE}: {error}"));
    let mut sections = sections(&source);
    let (_, prelude) = sections.remove(0);
    let iterations = iterations(&prelude);
    let mut rows = sections.iter().map(|(name, body)| {
        Row::measure(name, &format!("{prelude}\n{body}"), ResourceLimits::default())
            .unwrap_or_else(|| panic!("construct {name} failed"))
    });
    let baseline = rows.next().expect("a baseline section");

    println!(
        "\n{:<22} {:>9} {:>9} {:>9}",
        "construct", "charged", "measured", "meas/chg"
    );
    for row in rows {
        let charged = (row.units as f64 - baseline.units as f64) / iterations;
        let nanos = (row.time.as_secs_f64() - baseline.time.as_secs_f64()) * 1e9 / iterations;
        let measured = nanos / rate;
        // A construct charged what the baseline is has no ratio to report.
        let ratio = if charged >= 0.5 {
            format!("{:.2}", measured / charged)
        } else {
            "-".to_owned()
        };
        println!("{:<22} {charged:>9.1} {measured:>9.1} {ratio:>9}", row.name);
    }
    println!("units per iteration beyond `baseline`; measured is the extra time at {rate:.2} ns/unit");
}

/// Splits the constructs file at its `# === name ===` lines into `(name, body)`, in file order.
fn sections(source: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in source.lines() {
        let name = line.strip_prefix("# === ").and_then(|rest| rest.strip_suffix(" ==="));
        if let Some(name) = name {
            sections.push((name.to_owned(), String::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    sections
}

/// The loop count the prelude assigns to `N`.
fn iterations(prelude: &str) -> f64 {
    let value = prelude
        .lines()
        .find_map(|line| line.strip_prefix("N = "))
        .expect("the prelude assigns N");
    value.replace('_', "").parse().expect("N is an integer")
}

/// Prints the spread of rates over the test cases, and the cases at each end of it.
fn report_corpus(median: f64) {
    let limits = || ResourceLimits::default().max_feed_duration(CORPUS_BUDGET);
    let all: Vec<Row> = python_files(CORPUS_DIR)
        .iter()
        .filter(|(_, code)| self_contained(code))
        .filter_map(|(name, code)| Row::measure(name, code, limits()))
        .collect();
    let units: u64 = all.iter().map(|row| row.units).sum();
    let time: Duration = all.iter().map(|row| row.time).sum();
    let mut rows: Vec<&Row> = all.iter().filter(|row| row.units >= CORPUS_MIN_UNITS).collect();
    rows.sort_by(|a, b| a.nanos_per_unit().total_cmp(&b.nanos_per_unit()));
    let percentile = |percent: usize| rows[(rows.len() - 1) * percent / 100].nanos_per_unit() / median;

    println!(
        "\ncorpus: {} cases, {units} units in {:.1}ms, {:.2} of the category median overall",
        all.len(),
        time.as_secs_f64() * 1000.0,
        time.as_secs_f64() * 1e9 / units as f64 / median,
    );
    println!(
        "{} cases of {CORPUS_MIN_UNITS}+ units, rate / median: p5 {:.2}  p25 {:.2}  p50 {:.2}  p75 {:.2}  p95 {:.2}",
        rows.len(),
        percentile(5),
        percentile(25),
        percentile(50),
        percentile(75),
        percentile(95),
    );
    // Short cases are straight-line code, long ones loops: the two are charged differently.
    for (low, high) in [(CORPUS_MIN_UNITS, 10_000), (10_000, 100_000), (100_000, u64::MAX)] {
        let bucket: Vec<&&Row> = rows.iter().filter(|row| (low..high).contains(&row.units)).collect();
        if let Some(middle) = bucket.get(bucket.len() / 2) {
            println!(
                "  from {low:>6} units: {:>3} cases, p50 {:.2}",
                bucket.len(),
                middle.nanos_per_unit() / median
            );
        }
    }
    let outliers = CORPUS_OUTLIERS.min(rows.len() / 2);
    for (title, rows) in [
        ("charged too much", &rows[..outliers]),
        ("charged too little", &rows[rows.len() - outliers..]),
    ] {
        println!("{title}:");
        for row in rows {
            println!(
                "  {:<44} {:>10} {:>8.2}",
                row.name,
                row.units,
                row.nanos_per_unit() / median
            );
        }
    }
}

/// Whether a test case runs without a host: no external calls, event loop or mount.
fn self_contained(code: &str) -> bool {
    Header::parse(code).is_ok_and(|header| !(header.call_external || header.run_async || header.mount_fs))
}

/// One script's measurements.
struct Row {
    name: String,
    /// Units charged on the work clock.
    units: u64,
    /// Fastest execution time on the wall clock.
    time: Duration,
}

impl Row {
    /// Measures `code`, or returns `None` if it charges nothing or spends the feed budget.
    ///
    /// A script that raises is still measured: its work up to the exception is the same every run.
    fn measure(name: &str, code: &str, limits: ResourceLimits) -> Option<Self> {
        let budget = limits.max_feed_duration;
        let work = execution_time(name, code, ExecutionClock::Work, limits);
        let units = u64::try_from(work.as_nanos()).expect("units fit u64") / NANOS_PER_UNIT;
        if units == 0 || budget.is_some_and(|budget| work >= budget) {
            None
        } else {
            // The wall runs carry no limit, which would add a clock read to every checkpoint.
            let time = (0..RUNS)
                .map(|_| execution_time(name, code, ExecutionClock::Wall, ResourceLimits::default()))
                .min()
                .expect("at least one run");
            Some(Self {
                name: name.to_owned(),
                units,
                time,
            })
        }
    }

    fn nanos_per_unit(&self) -> f64 {
        self.time.as_secs_f64() * 1e9 / self.units as f64
    }
}

/// Runs `code` in a fresh session and returns its execution time on `clock`, raised or not.
fn execution_time(name: &str, code: &str, clock: ExecutionClock, limits: ResourceLimits) -> Duration {
    let tracker = ResourceTracker::new(limits).with_execution_clock(clock);
    let mut repl = MontyRepl::new(name, tracker, CompileOptions::default());
    let _ = repl.feed_run(code, vec![], PrintWriter::Disabled);
    repl.tracker().elapsed()
}

/// Every `*.py` file in `dir` as `(name, source)`, in name order.
fn python_files(dir: &str) -> Vec<(String, String)> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("reading {dir}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "py"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
            (name, fs::read_to_string(&path).unwrap())
        })
        .collect()
}
