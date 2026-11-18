//! Writes the data `scripts/fit_work_clock.py` fits the work clock's weights to.
//!
//! One row per script: how often each opcode ran and each [`WorkEvent`] was charged, and the
//! work the tracker charged for it. The scripts are written beside the rows so the wasm side
//! can time the same code; counts are the same in every build, times are not.

use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use monty_types::{ALLOCATION_OVERHEAD_BYTES, ExecutionClock, NANOS_PER_UNIT, ResourceLimits, WorkEvent, work_profile};
use serde_json::{Value, json};
use strum::IntoEnumIterator;

use super::{
    CONSTRUCTS_FILE, CORPUS_BUDGET, CORPUS_DIR, SCRIPTS_DIR, execution_time, python_files, sections, self_contained,
};

/// Times a corpus case is repeated, so a short test case runs long enough to time in wasm.
const CORPUS_REPEATS: u64 = 30;

/// Units below which a repeated corpus case is still too short to time.
const MIN_UNITS: u64 = 20_000;

/// How far the work charged may sit from what the counts predict before the data is refused:
/// a larger gap is a charge the profile does not see.
const MAX_UNEXPLAINED: f64 = 0.02;

/// Measures every script and writes `scripts/*.py` and `counts.json` under `dir`.
pub(crate) fn write(dir: &Path) {
    let scripts_dir = dir.join("scripts");
    let _ = fs::remove_dir_all(&scripts_dir);
    fs::create_dir_all(&scripts_dir).unwrap_or_else(|error| panic!("{}: {error}", scripts_dir.display()));

    let opcodes = monty::work_profile_opcodes();
    let names: BTreeMap<u8, &str> = opcodes.iter().map(|(byte, name, _)| (*byte, name.as_str())).collect();
    let extras: BTreeMap<u8, u64> = opcodes.iter().map(|(byte, _, extra)| (*byte, *extra)).collect();

    let mut rows = Vec::new();
    for (name, code) in scripts() {
        let Some(row) = Row::measure(&name, &code) else {
            continue;
        };
        let unexplained = row.unexplained(&extras);
        assert!(
            unexplained.abs() <= MAX_UNEXPLAINED,
            "{name}: charged work is {:.2}% from what its counts predict",
            unexplained * 100.0
        );
        fs::write(scripts_dir.join(format!("{name}.py")), &code).expect("writing a script");
        rows.push(row.json(&names));
    }
    let data = json!({
        "nanos_per_unit": NANOS_PER_UNIT,
        "opcodes": opcodes.iter().map(|(byte, name, extra)| json!({"byte": byte, "name": name, "extra": extra})).collect::<Vec<_>>(),
        "events": WorkEvent::iter().map(|event| json!({"name": <&str>::from(event), "milli_units": event.milli_units()})).collect::<Vec<_>>(),
        "rows": rows,
    });
    let path = dir.join("counts.json");
    fs::write(&path, serde_json::to_string(&data).expect("serializing counts")).expect("writing counts");
    println!("\nwrote {} scripts and {}", rows.len(), path.display());
}

/// Every script to fit against as `(name, source)`: the categories, each construct after the
/// prelude, and each self-contained test case repeated.
fn scripts() -> Vec<(String, String)> {
    let categories = python_files(SCRIPTS_DIR)
        .into_iter()
        .map(|(name, code)| (format!("category__{name}"), code));

    let constructs = fs::read_to_string(CONSTRUCTS_FILE).unwrap_or_else(|error| panic!("{CONSTRUCTS_FILE}: {error}"));
    let mut constructs = sections(&constructs);
    let (_, prelude) = constructs.remove(0);
    let constructs = constructs
        .into_iter()
        .map(move |(name, body)| (format!("construct__{name}"), format!("{prelude}\n{body}")));

    let corpus = python_files(CORPUS_DIR)
        .into_iter()
        .filter(|(_, code)| self_contained(code))
        .filter_map(|(name, code)| Some((format!("corpus__{name}"), repeated(&name, &code)?)));

    categories.chain(constructs).chain(corpus).collect()
}

/// Wraps a test case in a loop, or returns `None` if the loop does not repeat its work: the
/// case raises, or indenting it changes what it does.
fn repeated(name: &str, code: &str) -> Option<String> {
    let limits = || ResourceLimits::default().max_feed_duration(CORPUS_BUDGET);
    let mut body = format!("for _repeat in range({CORPUS_REPEATS}):\n");
    for line in code.lines() {
        if !line.trim().is_empty() {
            body.push_str("    ");
        }
        body.push_str(line);
        body.push('\n');
    }
    let once = execution_time(name, code, ExecutionClock::Work, limits());
    let looped = execution_time(name, &body, ExecutionClock::Work, limits());
    let repeats = looped.as_secs_f64() / once.as_secs_f64();
    let full = repeats > CORPUS_REPEATS as f64 * 0.9 && looped < CORPUS_BUDGET;
    full.then_some(body)
}

/// One script's counts and charged work.
struct Row {
    name: String,
    /// Thousandths of a unit the tracker charged.
    milli_units: u64,
    profile: work_profile::WorkProfile,
    /// Fastest wall time of this build, which the profile's counters slow down.
    time: Duration,
}

impl Row {
    /// Measures `code`, or returns `None` if it is too short to time.
    fn measure(name: &str, code: &str) -> Option<Self> {
        let _ = work_profile::take();
        let work = execution_time(name, code, ExecutionClock::Work, ResourceLimits::default());
        let profile = work_profile::take();
        let milli_units = u64::try_from(work.as_nanos() * 1000 / u128::from(NANOS_PER_UNIT)).expect("work fits u64");
        (milli_units >= MIN_UNITS * 1000).then(|| Self {
            name: name.to_owned(),
            milli_units,
            profile,
            time: (0..3)
                .map(|_| execution_time(name, code, ExecutionClock::Wall, ResourceLimits::default()))
                .min()
                .expect("at least one run"),
        })
    }

    /// The share of the charged work the counts do not account for, signed.
    fn unexplained(&self, extras: &BTreeMap<u8, u64>) -> f64 {
        let instructions: u64 = self
            .profile
            .opcodes
            .iter()
            .map(|(byte, count)| count * (WorkEvent::Instruction.milli_units() + extras[byte]))
            .sum();
        let events: u64 = self
            .profile
            .events
            .iter()
            .map(|(event, count)| count * charged_milli_units(*event))
            .sum();
        (self.milli_units as f64 - (instructions + events) as f64) / self.milli_units as f64
    }

    fn json(&self, names: &BTreeMap<u8, &str>) -> Value {
        let opcodes: BTreeMap<&str, u64> = self
            .profile
            .opcodes
            .iter()
            .map(|(byte, count)| (names[byte], *count))
            .collect();
        let events: BTreeMap<&str, u64> = self
            .profile
            .events
            .iter()
            .map(|(event, count)| (<&str>::from(*event), *count))
            .collect();
        json!({
            "name": self.name,
            "milli_units": self.milli_units,
            "native_nanos": u64::try_from(self.time.as_nanos()).expect("time fits u64"),
            "opcodes": opcodes,
            "events": events,
        })
    }
}

/// Thousandths of a unit one `event` is charged. An allocation is charged as whole bytes of
/// overhead, which rounds its weight down.
fn charged_milli_units(event: WorkEvent) -> u64 {
    if event == WorkEvent::Allocation {
        ALLOCATION_OVERHEAD_BYTES as u64 * WorkEvent::AllocatedByte.milli_units()
    } else {
        event.milli_units()
    }
}
