//! Calibrates the work clock: units charged against time taken, per category of work.
//!
//! Runs every `python/work_clock/*.py` script twice, once on the work clock for its units and
//! once on the wall clock for its execution time, and prints nanoseconds per unit. A category far
//! from the median is native work charged too little (high) or too much (low).
//!
//! `-- --check` fails when a category is more than [`MAX_DRIFT`] from the median. The native rate is
//! not `NANOS_PER_UNIT`, which is the wasm build's; `crates/monty-js/scripts/calibrate-work-clock.mjs`
//! measures that one. Run both with `make calibrate-work-clock`.

use std::{env, fs, path::PathBuf, process::ExitCode, time::Duration};

use monty::MontyRepl;
use monty_types::{CompileOptions, ExecutionClock, NANOS_PER_UNIT, PrintWriter, ResourceLimits, ResourceTracker};

// The worker's allocator, so allocation is charged here as it is in a worker.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// The calibration scripts, one per category of work.
const SCRIPTS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/python/work_clock");

/// Timed runs per script; the fastest is the least disturbed.
const RUNS: usize = 7;

/// How far a category's rate may sit from the median before `--check` fails.
const MAX_DRIFT: f64 = 3.0;

fn main() -> ExitCode {
    let check = env::args().any(|arg| arg == "--check");
    let mut rows: Vec<Row> = scripts().iter().map(Row::measure).collect();
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
    println!("native median: {median:.2} ns/unit (NANOS_PER_UNIT, the wasm rate, is {NANOS_PER_UNIT})");
    if check && drifted {
        eprintln!("a category is more than {MAX_DRIFT}x from the median");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
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
    fn measure((name, code): &(String, String)) -> Self {
        let work = execution_time(name, code, ExecutionClock::Work);
        let units = u64::try_from(work.as_nanos()).expect("units fit u64") / NANOS_PER_UNIT;
        let time = (0..RUNS)
            .map(|_| execution_time(name, code, ExecutionClock::Wall))
            .min()
            .expect("at least one run");
        Self {
            name: name.clone(),
            units,
            time,
        }
    }

    fn nanos_per_unit(&self) -> f64 {
        self.time.as_secs_f64() * 1e9 / self.units as f64
    }
}

/// Runs `code` in a fresh session and returns its execution time on `clock`.
fn execution_time(name: &str, code: &str, clock: ExecutionClock) -> Duration {
    let tracker = ResourceTracker::new(ResourceLimits::default()).with_execution_clock(clock);
    let mut repl = MontyRepl::new(name, tracker, CompileOptions::default());
    repl.feed_run(code, vec![], PrintWriter::Stdout)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    repl.tracker().elapsed()
}

/// Every calibration script as `(name, source)`, in name order.
fn scripts() -> Vec<(String, String)> {
    let mut paths: Vec<PathBuf> = fs::read_dir(SCRIPTS_DIR)
        .unwrap_or_else(|error| panic!("reading {SCRIPTS_DIR}: {error}"))
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
