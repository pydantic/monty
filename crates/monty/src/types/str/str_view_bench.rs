//! Test-only policy switching and backing-payload accounting for the runtime matrix.
use std::{cell::Cell, hint::black_box, time::Instant};

use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

use super::{MIN_VIEW_LEN, MIN_VIEW_RATIO, StringStorage};
use crate::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};

thread_local! {
    static POLICY: Cell<(usize, usize)> = const { Cell::new((MIN_VIEW_LEN, MIN_VIEW_RATIO)) };
    static STATS: Cell<Option<Stats>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Default)]
struct Stats {
    allocations: usize,
    created: usize,
    copied: usize,
    live: usize,
    peak: usize,
    owned_finalize: usize,
    peak_with_owned: usize,
}

pub(super) fn policy() -> (usize, usize) {
    POLICY.with(Cell::get)
}

fn update(f: impl FnOnce(&mut Stats)) {
    STATS.with(|cell| {
        if let Some(mut stats) = cell.get() {
            f(&mut stats);
            cell.set(Some(stats));
        }
    });
}

pub(super) fn allocated(storage: &StringStorage) {
    update(|stats| {
        let length = storage.backing_len();
        stats.allocations += 1;
        stats.created += length;
        stats.live += length;
        stats.peak = stats.peak.max(stats.live);
        stats.peak_with_owned = stats.peak_with_owned.max(stats.live);
    });
}

pub(super) fn copied(length: usize) {
    update(|stats| stats.copied += length);
}

pub(super) fn released(storage: &StringStorage) {
    if STATS.with(|cell| cell.get().is_none()) {
        return;
    }

    update(|stats| {
        stats.live = stats
            .live
            .checked_sub(storage.backing_len())
            .expect("tracked backing released");
    });
}

#[derive(Clone, Copy)]
struct Policy {
    name: &'static str,
    minimum: usize,
    ratio: usize,
}

impl Policy {
    fn install(self) {
        POLICY.with(|cell| cell.set((self.minimum, self.ratio)));
    }
}

struct Case {
    name: String,
    task: Task,
}

enum Task {
    Run(Box<MontyRun>, i64),
    Retain(String, i64),
    Snapshot(Vec<u8>),
    Input(Box<InputCase>),
}

struct InputCase {
    runner: MontyRun,
    source: String,
    value: MontyObject,
    expected: i64,
    retain: bool,
}

impl Case {
    fn run(name: &str, source: &str, expected: i64) -> Self {
        let runner = MontyRun::new(source.to_owned(), "views-matrix.py", vec![], CompileOptions::default()).unwrap();
        Self {
            name: name.to_owned(),
            task: Task::Run(Box::new(runner), expected),
        }
    }

    fn retain(name: &str, source: String, expected: i64) -> Self {
        Self {
            name: name.to_owned(),
            task: Task::Retain(source, expected),
        }
    }

    fn input(name: &str, source: &str, data: String, expected: i64, retain: bool) -> Self {
        let runner = MontyRun::new(
            source.to_owned(),
            "scenario.py",
            vec!["data".to_owned()],
            CompileOptions::default(),
        )
        .unwrap();
        Self {
            name: name.to_owned(),
            task: Task::Input(Box::new(InputCase {
                runner,
                source: source.to_owned(),
                value: MontyObject::string(data),
                expected,
                retain,
            })),
        }
    }

    /// Retained sessions stay alive until their payload counters have been sampled.
    fn execute(&mut self) -> Option<MontyRepl> {
        match &mut self.task {
            Task::Run(runner, expected) => {
                let result = runner.run_no_limits(vec![]).unwrap();
                assert_eq!(result, MontyObject::int(*expected), "{}", self.name);
                None
            }
            Task::Retain(source, expected) => {
                let mut repl = MontyRepl::new("views-matrix.py", ResourceTracker::default(), CompileOptions::default());
                let result = repl.feed_run(source, vec![], PrintWriter::Disabled).unwrap();
                assert_eq!(result, MontyObject::int(*expected), "{}", self.name);
                Some(repl)
            }
            Task::Input(input) => {
                if input.retain {
                    let mut repl = MontyRepl::new("scenario.py", ResourceTracker::default(), CompileOptions::default());
                    let result = repl
                        .feed_run(
                            &input.source,
                            vec![("data".to_owned(), input.value.clone())],
                            PrintWriter::Disabled,
                        )
                        .unwrap();
                    assert_eq!(result, MontyObject::int(input.expected), "{}", self.name);
                    Some(repl)
                } else {
                    let result = input.runner.run_no_limits(vec![input.value.clone()]).unwrap();
                    assert_eq!(result, MontyObject::int(input.expected), "{}", self.name);
                    None
                }
            }
            Task::Snapshot(bytes) => {
                let Session::Idle(restored) = Dump::load(bytes).unwrap().state else {
                    panic!("expected idle snapshot");
                };
                let encoded = dump("views-matrix.py", None, &[], SessionRef::Idle(&restored)).unwrap();
                assert_eq!(encoded.len(), bytes.len());
                black_box(encoded);
                None
            }
        }
    }
}

fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case::run("half", "s = 'x' * 1048576\nv = s[:524288]\nlen(v)", 524_288),
        Case::run("prefix", "s = 'x' * 20000\nwhile len(s) > 1:\n    s = s[1:]\nlen(s)", 1),
        Case::run(
            "suffix",
            "s = 'x' * 20000\nwhile len(s) > 1:\n    s = s[:-1]\nlen(s)",
            1,
        ),
        Case::run(
            "prefix_small",
            "s = 'x' * 512\nwhile len(s) > 1:\n    s = s[1:]\nlen(s)",
            1,
        ),
        Case::run(
            "chain",
            "s = 'x' * 1048576\ns = s[:524288]\ns = s[:314572]\ns = s[:209715]\nlen(s)",
            209_715,
        ),
        Case::run("unicode", "s = 'é🌍' * 100000\nv = s[1000:100000]\nlen(v)", 99_000),
        Case::run(
            "unicode_prefix",
            "s = 'é🌍' * 500\nwhile len(s) > 1:\n    s = s[1:]\nlen(s)",
            1,
        ),
        Case::run("stepped", "s = 'é🌍' * 10000\nv = s[::2]\nlen(v)", 10_000),
        Case::run(
            "operations",
            "s = ' x' * 100000\nv = s[1000:100000]\nfor i in range(100):\n    v.strip()\n    hash(v)\n    v + 'a'\nlen(v)",
            99_000,
        ),
        Case::run(
            "operations_small",
            "s = ' x' * 256\nv = s[128:320]\nfor i in range(1000):\n    v.strip()\n    hash(v)\n    assert v == v[:]\n    v + 'a'\nlen(v)",
            192,
        ),
    ];
    for length in [48, 96, 192, 384] {
        let code = format!("s = ('x' * {length} + chr(10)) * 2000\nlines = s.splitlines()\nlen(lines)");
        cases.push(Case::run(&format!("lines_{length}"), &code, 2000));
    }
    for length in [8, 48, 96, 192, 384] {
        let code = format!("s = 'x' * 1048576\ntokens = [s[i:i+{length}] for i in range(1000)]\nlen(tokens)");
        cases.push(Case::run(&format!("tokens_{length}"), &code, 1000));
    }
    for length in [48, 96, 192, 131_072, 262_144, 524_288] {
        let source = format!("source = 'x' * 1048576\nview = source[:{length}]\nsource = None\nlen(view)");
        cases.push(Case::retain(&format!("retain_{length}"), source, length));
    }
    cases.push(Case::retain(
        "siblings",
        "s = 'x' * 1048576\na = s[:524288]\nb = a[:314572]\nc = b[:209715]\ns = None\nlen(c)".to_owned(),
        209_715,
    ));

    let mut repl = MontyRepl::new("views-matrix.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "source = 'é🌍' * 100000\nview = source[1000:100000]\nsource = None",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let bytes = dump("views-matrix.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    cases.push(Case {
        name: "snapshot".to_owned(),
        task: Task::Snapshot(bytes),
    });
    cases
}

fn shuffle<T>(items: &mut [T], state: &mut u32) {
    for i in (1..items.len()).rev() {
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let index = usize::try_from(*state).unwrap() % (i + 1);
        items.swap(i, index);
    }
}

struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        POLICY.with(|cell| cell.set((MIN_VIEW_LEN, MIN_VIEW_RATIO)));
        STATS.with(|cell| cell.set(None));
    }
}

/// Compares production interpreter paths; policy switching and counters exist only in test builds.
#[test]
#[ignore = "optimized runtime policy matrix; run with --ignored --nocapture --test-threads=1"]
fn benchmark_string_view_runtime_matrix() {
    let _reset = Reset;
    let mut policies = vec![
        Policy {
            name: "copy",
            minimum: usize::MAX,
            ratio: 1,
        },
        Policy {
            name: "view",
            minimum: 16,
            ratio: usize::MAX,
        },
    ];
    for minimum in [32, 64, 128, 256] {
        for ratio in [8, 4, 2] {
            policies.push(Policy {
                name: "heuristic",
                minimum,
                ratio,
            });
        }
    }
    run_matrix(cases(), policies, 11);
}

fn run_matrix(mut cases: Vec<Case>, policies: Vec<Policy>, requested_rounds: u32) {
    let _reset = Reset;
    let mut tasks = Vec::new();
    for policy in policies {
        for case_index in 0..cases.len() {
            tasks.push((policy, case_index));
        }
    }
    let mut seed = 0x51ce_2026;
    shuffle(&mut tasks, &mut seed);
    println!(
        "kind,policy,min_len,ratio,workload,round,batch,ns,backing_allocations,payload_created,slice_materialized,peak_payload,retained_payload,owned_finalize,peak_with_owned"
    );
    for (policy, case_index) in tasks.iter().copied() {
        policy.install();
        STATS.with(|cell| cell.set(Some(Stats::default())));
        let held = cases[case_index].execute();
        let stats = STATS.with(|cell| cell.replace(None)).unwrap();
        if held.is_none() {
            assert_eq!(stats.live, 0, "{}", cases[case_index].name);
        }
        drop(held);
        println!(
            "METRIC,{},{},{},{},0,1,0,{},{},{},{},{},{},{}",
            policy.name,
            policy.minimum,
            policy.ratio,
            cases[case_index].name,
            stats.allocations,
            stats.created,
            stats.copied,
            stats.peak,
            stats.live,
            stats.owned_finalize,
            stats.peak_with_owned
        );
    }
    let rounds = if cfg!(debug_assertions) { 0 } else { requested_rounds };
    if rounds == 0 {
        return;
    }
    let mut batches = vec![1_u32; tasks.len()];
    for (index, (policy, case_index)) in tasks.iter().enumerate() {
        policy.install();
        let case = &mut cases[*case_index];
        for _ in 0..2 {
            black_box(case.execute());
        }
        let start = Instant::now();
        black_box(case.execute());
        let ns = start.elapsed().as_nanos().max(1);
        batches[index] = u32::try_from((5_000_000 / ns).clamp(1, 100)).unwrap();
    }
    println!(
        "kind,policy,min_len,ratio,workload,round,batch,ns,backing_allocations,payload_created,slice_materialized,peak_payload,retained_payload,owned_finalize,peak_with_owned"
    );
    let mut order: Vec<_> = (0..tasks.len()).collect();
    for round in 0..rounds {
        shuffle(&mut order, &mut seed);
        for &index in &order {
            let (policy, case_index) = tasks[index];
            policy.install();
            let case = &mut cases[case_index];
            let batch = batches[index];
            let start = Instant::now();
            for _ in 0..batch {
                black_box(case.execute());
            }
            let ns = start.elapsed().as_nanos() / u128::from(batch);
            println!(
                "TIME,{},{},{},{},{round},{batch},{ns},0,0,0,0,0,0,0",
                policy.name, policy.minimum, policy.ratio, case.name
            );
        }
    }
}

#[path = "str_realistic_bench.rs"]
mod realistic;
