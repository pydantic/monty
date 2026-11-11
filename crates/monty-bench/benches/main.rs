//! Interpreter benchmarks.
//!
//! Every top-level `python/*.py` script, and every `bench__*.py` test case, is read at run time
//! and registered as `<name>__monty`, plus `<name>__cpython` locally, so adding a file adds a
//! benchmark. Scripts are plain Python that check their own results with `assert`. The
//! `[bench]` table of a script's `# /// monty` header (`monty_fixture::Bench`) takes:
//!
//! - `data = '<path>'`: bind `DATA` to the contents of `<path>`, relative to the script
//! - `mount = true`: bind `DATA` to the root of the mounted [`scan_tree`]
//! - `overlay = true`: with `mount`, also register `<name>_overlay__monty` over an overlay mount
//! - `limits = true`: also register `<name>_limits__monty` with resource limits armed

use std::{
    fs, mem,
    path::{Path, PathBuf},
    time::Duration,
};

// Use codspeed-criterion-compat under the `codspeed` feature (CI), real criterion otherwise (for flamegraphs)
#[cfg(feature = "codspeed")]
use codspeed_criterion_compat::{Bencher, Criterion, black_box, criterion_group, criterion_main};
#[cfg(not(feature = "codspeed"))]
use criterion::{Bencher, Criterion, black_box, criterion_group, criterion_main};
use monty::{Dump, MontyRepl, MontyRun, RunProgress, SessionRef};
use monty_fixture::{Bench, Header};
use monty_fs::{MountCallOutcome, MountMode, MountTable, OverlayState};
use monty_types::{CompileOptions, ExtFunctionResult, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};
#[cfg(all(not(feature = "codspeed"), unix))]
use pprof::criterion::{Output, PProfProfiler};
// CPython benchmarks are only run locally, not on CodSpeed CI (requires Python + pyo3 setup)
#[cfg(not(feature = "codspeed"))]
use pyo3::{prelude::*, types::PyDict};
use tempfile::TempDir;

// The real worker allocator, so the interpreter benchmarks can price it:
// `cargo bench -p monty-bench --bench main --features worker-alloc` against a
// plain run measures what the ceiling costs on real workloads. No ceiling is
// armed — the cost is in the counting, not the comparison.
#[cfg(feature = "worker-alloc")]
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// The benchmark scripts. Subdirectories (`pool/`) hold scripts other benches drive.
const SCRIPTS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/python");

/// The interpreter test cases, whose `bench__*.py` files are also benchmarks.
const TEST_CASES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../monty/test_cases");

/// Source of the `end_to_end` benchmarks, which parse inside the timed loop.
const ADD_TWO: &str = include_str!("python/add_two.py");

/// Registers every script benchmark, then the Rust-API benchmarks.
fn criterion_benchmark(c: &mut Criterion) {
    let tree = scan_tree();
    for script in Script::load_all() {
        script.register(c, tree.path());
    }

    c.bench_function("end_to_end__monty", end_to_end_monty);
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("end_to_end__cpython", end_to_end_cpython);
    c.bench_function("parse_1k_assigns__monty", parse_1k_assigns);
    c.bench_function("clone_module__monty", clone_module);
    c.bench_function("repl_feed_after_2k_snippets__monty", repl_feed_after_2k_snippets);
    c.bench_function("session_dump__monty", session_dump);
    c.bench_function("session_load__monty", session_load);
}

/// A benchmark script with the options from its header's `[bench]` table.
struct Script {
    /// The file stem, without a test case's `bench__` prefix.
    name: String,
    /// The file's real name, for tracebacks.
    filename: String,
    code: String,
    input: Input,
    /// Also benchmark with resource limits armed.
    limits: bool,
    /// Also benchmark over an overlay mount; requires [`Input::Mount`].
    overlay: bool,
}

/// What a script's `DATA` global is bound to.
enum Input {
    None,
    /// The contents of the file `data` names.
    Data(String),
    /// The scan tree's root: `/data` for Monty's mount, the host path for CPython.
    Mount,
}

impl Script {
    /// Reads every script, sorted by path so benchmarks register in a stable order.
    fn load_all() -> Vec<Self> {
        let mut paths: Vec<PathBuf> = python_files(SCRIPTS_DIR).collect();
        paths.extend(python_files(TEST_CASES_DIR).filter(|path| file_stem(path).starts_with("bench__")));
        paths.sort();
        paths.iter().map(|path| Self::parse(path)).collect()
    }

    /// Reads one script and its header, panicking on a malformed header, inconsistent options or
    /// a test-case key, which a benchmark would otherwise silently ignore.
    fn parse(path: &Path) -> Self {
        let code = fs::read_to_string(path).unwrap();
        let mut header = Header::parse(&code).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let Bench {
            data,
            mount,
            overlay,
            limits,
        } = mem::take(&mut header.bench);
        assert!(
            header == Header::default(),
            "{}: only `[bench]` options apply to benchmarks",
            path.display()
        );
        let input = match (data, mount) {
            (Some(_), true) => panic!("{}: `data` with `mount`", path.display()),
            (Some(data), false) => Input::Data(fs::read_to_string(path.parent().unwrap().join(data)).unwrap()),
            (None, true) => Input::Mount,
            (None, false) => Input::None,
        };
        assert!(!overlay || mount, "{}: `overlay` needs `mount`", path.display());
        assert!(!limits || !mount, "{}: `limits` with `mount`", path.display());
        let stem = file_stem(path);
        Self {
            name: stem.strip_prefix("bench__").unwrap_or(stem).to_owned(),
            filename: path.file_name().unwrap().to_str().unwrap().to_owned(),
            code,
            input,
            limits,
            overlay,
        }
    }

    /// Registers this script's benchmarks; `tree` is the host root of the scan tree.
    fn register(&self, c: &mut Criterion, tree: &Path) {
        let name = &self.name;
        let code = self.code.as_str();
        let filename = &self.filename;
        let data = match &self.input {
            Input::None => None,
            Input::Data(data) => Some(data.as_str()),
            Input::Mount => None,
        };
        if matches!(self.input, Input::Mount) {
            c.bench_function(&format!("{name}__monty"), |b| {
                run_monty_mounted(b, code, filename, tree, MountMode::ReadOnly);
            });
            if self.overlay {
                c.bench_function(&format!("{name}_overlay__monty"), |b| {
                    let mode = MountMode::OverlayMemory(OverlayState::new());
                    run_monty_mounted(b, code, filename, tree, mode);
                });
            }
        } else {
            c.bench_function(&format!("{name}__monty"), |b| run_monty(b, code, filename, data));
            if self.limits {
                c.bench_function(&format!("{name}_limits__monty"), |b| {
                    run_monty_limits(b, code, filename, data);
                });
            }
        }
        #[cfg(not(feature = "codspeed"))]
        {
            let host_root = tree.to_str().expect("temporary directories have UTF-8 paths");
            let data = if matches!(self.input, Input::Mount) {
                Some(host_root)
            } else {
                data
            };
            c.bench_function(&format!("{name}__cpython"), |b| run_cpython(b, code, filename, data));
        }
    }
}

/// The `.py` files directly in `dir`.
fn python_files(dir: &str) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("reading {dir}: {err}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "py"))
}

/// A path's file stem, which every script path has.
fn file_stem(path: &Path) -> &str {
    path.file_stem().and_then(|stem| stem.to_str()).unwrap()
}

/// Parses once, then benchmarks repeated execution, binding `DATA` when `data` is given.
fn run_monty(bench: &mut Bencher, code: &str, filename: &str, data: Option<&str>) {
    let mut ex = parse_with_data(code, filename, data);
    bench.iter(|| black_box(ex.run_no_limits(data_inputs(data)).unwrap()));
}

/// Runs with production-shaped resource limits armed (generous budgets that never trip),
/// measuring the amortized limit-checking path sandboxes run rather than the no-limits path.
fn run_monty_limits(bench: &mut Bencher, code: &str, filename: &str, data: Option<&str>) {
    let mut ex = parse_with_data(code, filename, data);
    let limits = ResourceLimits::default()
        .max_feed_duration(Duration::from_mins(10))
        .max_memory(1 << 40);
    bench.iter(|| {
        let tracker = ResourceTracker::new(limits.clone());
        black_box(ex.run(data_inputs(data), tracker, PrintWriter::Stdout).unwrap())
    });
}

/// Parses `code`, declaring a `DATA` input when `data` is given.
fn parse_with_data(code: &str, filename: &str, data: Option<&str>) -> MontyRun {
    let input_names = data.map(|_| vec!["DATA".to_owned()]).unwrap_or_default();
    MontyRun::new(code.to_owned(), filename, input_names, CompileOptions::default()).unwrap()
}

/// The input values matching [`parse_with_data`]'s declaration.
fn data_inputs(data: Option<&str>) -> Vec<MontyObject> {
    data.map(|data| vec![MontyObject::string(data.to_owned())])
        .unwrap_or_default()
}

/// Runs with `host_root` mounted at `/data` in `mode`, bound to `DATA`, answering every
/// OS call from the mount as a host would.
///
/// Each iteration starts a clone of the parsed program, since `start` consumes it.
fn run_monty_mounted(bench: &mut Bencher, code: &str, filename: &str, host_root: &Path, mode: MountMode) {
    let ex = MontyRun::new(
        code.to_owned(),
        filename,
        vec!["DATA".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let mut mounts = MountTable::new();
    mounts.mount("/data", host_root, mode, None).unwrap();
    bench.iter(|| {
        let inputs = vec![MontyObject::string("/data".to_owned())];
        let mut progress = ex
            .clone()
            .start(inputs, ResourceTracker::default(), PrintWriter::Stdout)
            .unwrap();
        loop {
            progress = match progress {
                RunProgress::Complete(value) => break black_box(value),
                RunProgress::OsCall(call) => call
                    .resume_with(PrintWriter::Stdout, |call| match mounts.handle_os_call(call) {
                        MountCallOutcome::Handled(Ok(value)) => value.into(),
                        MountCallOutcome::Handled(Err(err)) => err.into_exception().into(),
                        MountCallOutcome::NotHandled(call) => ExtFunctionResult::from(call.on_no_handler()),
                    })
                    .unwrap(),
                _ => panic!("the mounted benchmarks only suspend for OS calls"),
            };
        }
    });
}

/// Builds the tree the mounted benchmarks read: 20 packages, each with
/// `src/lib.rs` and 10 module directories of 5 `.rs` and 15 `.txt` files.
///
/// 4,020 files and 240 directories, of which 1,020 files match `**/*.rs`.
fn scan_tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    for package in 0..20 {
        let src = dir.path().join(format!("pkg{package}/src"));
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.rs"), "").unwrap();
        for module in 0..10 {
            let module_dir = src.join(format!("mod{module}"));
            fs::create_dir(&module_dir).unwrap();
            for file in 0..20 {
                let extension = if file < 5 { "rs" } else { "txt" };
                fs::write(module_dir.join(format!("file{file}.{extension}")), "").unwrap();
            }
        }
    }
    dir
}

/// Compiles once, then benchmarks executing the module in fresh globals, at module scope as
/// Monty runs it, binding `DATA` when `data` is given.
#[cfg(not(feature = "codspeed"))]
fn run_cpython(bench: &mut Bencher, code: &str, filename: &str, data: Option<&str>) {
    Python::attach(|py| {
        let builtins = py.import("builtins").unwrap();
        let compiled = builtins.call_method1("compile", (code, filename, "exec")).unwrap();
        let exec = builtins.getattr("exec").unwrap();
        bench.iter(|| {
            let globals = PyDict::new(py);
            if let Some(data) = data {
                globals.set_item("DATA", data).unwrap();
            }
            exec.call1((&compiled, &globals)).unwrap();
            black_box(globals);
        });
    });
}

/// Benchmarks end-to-end execution (parsing + running) using Monty.
/// This is different from other benchmarks as it includes parsing in the loop.
fn end_to_end_monty(bench: &mut Bencher) {
    bench.iter(|| {
        let mut ex = MontyRun::new(
            black_box(ADD_TWO).to_owned(),
            "add_two.py",
            vec![],
            CompileOptions::default(),
        )
        .unwrap();
        black_box(ex.run_no_limits(vec![]).unwrap());
    });
}

/// Benchmarks end-to-end execution (compiling + running) using CPython.
/// This is different from other benchmarks as it includes compiling in the loop.
#[cfg(not(feature = "codspeed"))]
fn end_to_end_cpython(bench: &mut Bencher) {
    Python::attach(|py| {
        let builtins = py.import("builtins").unwrap();
        let exec = builtins.getattr("exec").unwrap();
        bench.iter(|| {
            let compiled = builtins
                .call_method1("compile", (black_box(ADD_TWO), "add_two.py", "exec"))
                .unwrap();
            exec.call1((&compiled, PyDict::new(py))).unwrap();
        });
    });
}

/// Parses 1,000 repetitions of `x = 1` to track the cost of Monty's Ruff-AST → Monty-AST
/// conversion pass on many trivial statements. A scaled-down version of the `latency.py`
/// workload (which uses 100,000 lines) — kept small enough for criterion while still
/// large enough that per-statement conversion cost dominates fixed overhead.
fn parse_1k_assigns(bench: &mut Bencher) {
    let code: String = "x = 1\n".repeat(1_000);
    bench.iter(|| {
        let ex = MontyRun::new(black_box(code.clone()), "test.py", vec![], CompileOptions::default()).unwrap();
        black_box(ex);
    });
}

/// Clones a large module without function definitions, isolating module-code sharing
/// from the independently cloned intern and name tables.
fn clone_module(bench: &mut Bencher) {
    let runner = MontyRun::new("x = 1\n".repeat(10_000), "test.py", vec![], CompileOptions::default()).unwrap();
    bench.iter(|| black_box(runner.clone()));
}

/// Feeds a trivial snippet into a REPL session that has already run 2,000
/// snippets (a mix of function definitions and assignments, so the intern,
/// function and name tables are all large). Guards against per-feed cost
/// scaling with session size: the session's tables must be moved into each
/// snippet, never cloned, so this should cost the same as a fresh feed.
fn repl_feed_after_2k_snippets(bench: &mut Bencher) {
    let mut repl = MontyRepl::new("bench.py", ResourceTracker::default(), CompileOptions::default());
    for i in 0..2_000 {
        let code = if i % 2 == 0 {
            format!("def func_{i}(a, b):\n    return a + b * {i}")
        } else {
            format!("value_{i} = 'literal {i}'")
        };
        repl.feed_run(&code, vec![], PrintWriter::Stdout).unwrap();
    }
    bench.iter(|| {
        let r = repl.feed_run(black_box("1 + 1"), vec![], PrintWriter::Stdout).unwrap();
        black_box(r);
    });
}

/// Builds a REPL session carrying the state the dump benchmarks serialize:
/// 500 snippets of function definitions and string literals, so the interner,
/// function and name tables are all large enough for per-entry costs to
/// dominate the fixed framing cost.
fn session_with_state() -> MontyRepl {
    let mut repl = MontyRepl::new("bench.py", ResourceTracker::default(), CompileOptions::default());
    for i in 0..500 {
        let code = if i % 2 == 0 {
            format!("def func_{i}(a, b):\n    return a + b * {i}")
        } else {
            format!("value_{i} = 'literal {i}'")
        };
        repl.feed_run(&code, vec![], PrintWriter::Stdout).unwrap();
    }
    repl
}

/// Serializes an idle session snapshot. Every interner entry is written out
/// individually, so this prices the write side of the dump wire format —
/// the cost hosts pay each time they park a session.
fn session_dump(bench: &mut Bencher) {
    let repl = session_with_state();
    bench.iter(|| {
        let bytes = monty::dump("bench.py", None, &[], SessionRef::Idle(black_box(&repl))).unwrap();
        black_box(bytes);
    });
}

/// Restores a session snapshot. Loading deserializes the string table and
/// rebuilds the interner's reverse lookups from it, so this prices the read
/// side of the dump wire format — the cost hosts pay to resume a session.
fn session_load(bench: &mut Bencher) {
    let repl = session_with_state();
    let bytes = monty::dump("bench.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    bench.iter(|| {
        let dump = Dump::load(black_box(&bytes)).unwrap();
        black_box(dump);
    });
}

// Use pprof flamegraph profiler when running locally on Unix (not on CodSpeed or Windows)
#[cfg(all(not(feature = "codspeed"), unix))]
criterion_group!(
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = criterion_benchmark
);

// Use default config on CodSpeed or Windows (pprof is Unix-only)
#[cfg(any(feature = "codspeed", not(unix)))]
criterion_group!(benches, criterion_benchmark);

criterion_main!(benches);
