// Use codspeed-criterion-compat under the `codspeed` feature (CI), real criterion otherwise (for flamegraphs)
#[cfg(not(feature = "codspeed"))]
use std::ffi::CString;
use std::{fs, path::Path, time::Duration};

#[cfg(feature = "codspeed")]
use codspeed_criterion_compat::{Bencher, Criterion, black_box, criterion_group, criterion_main};
#[cfg(not(feature = "codspeed"))]
use criterion::{Bencher, Criterion, black_box, criterion_group, criterion_main};
use monty::{Dump, MontyRepl, MontyRun, RunProgress, SessionRef};
use monty_fs::{MountCallOutcome, MountMode, MountTable, OverlayState};
use monty_types::{CompileOptions, ExtFunctionResult, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};
#[cfg(all(not(feature = "codspeed"), unix))]
use pprof::criterion::{Output, PProfProfiler};
// CPython benchmarks are only run locally, not on CodSpeed CI (requires Python + pyo3 setup)
#[cfg(not(feature = "codspeed"))]
use pyo3::prelude::*;
use tempfile::TempDir;

// The real worker allocator, so the interpreter benchmarks can price it:
// `cargo bench -p monty-bench --bench main --features worker-alloc` against a
// plain run measures what the ceiling costs on real workloads. No ceiling is
// armed — the cost is in the counting, not the comparison.
#[cfg(feature = "worker-alloc")]
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// Runs a benchmark using the Monty interpreter.
/// Parses once, then benchmarks repeated execution.
fn run_monty(bench: &mut Bencher, code: &str, expected: i64) {
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, expected);

    bench.iter(|| {
        let r = ex.run_no_limits(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Runs a benchmark using the Monty interpreter with a single string input bound to `DATA`.
/// Parses once, then benchmarks repeated execution with the same input.
fn run_monty_with_data(bench: &mut Bencher, code: &str, data: &str, expected: i64) {
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["DATA".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let make_input = || vec![MontyObject::string(data.to_owned())];
    let r = ex.run_no_limits(make_input()).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, expected);

    bench.iter(|| {
        let r = ex.run_no_limits(make_input()).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Runs a benchmark with `host_root` mounted at `/data` in `mode`, bound to
/// `DATA`, answering every OS call from the mount as a host would.
///
/// Each iteration starts a clone of the parsed program, since `start` consumes it.
fn run_monty_mounted(bench: &mut Bencher, code: &str, host_root: &Path, mode: MountMode, expected: i64) {
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["DATA".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let mut mounts = MountTable::new();
    mounts.mount("/data", host_root, mode, None).unwrap();
    let mut run = || {
        let inputs = vec![MontyObject::string("/data".to_owned())];
        let mut progress = ex
            .clone()
            .start(inputs, ResourceTracker::default(), PrintWriter::Stdout)
            .unwrap();
        loop {
            progress = match progress {
                RunProgress::Complete(value) => break i64::try_from(value.as_ref()).unwrap(),
                RunProgress::OsCall(call) => call
                    .resume_with(PrintWriter::Stdout, |call| match mounts.handle_os_call(call) {
                        MountCallOutcome::Handled(Ok(value)) => value.into(),
                        MountCallOutcome::Handled(Err(err)) => err.into_exception().into(),
                        MountCallOutcome::NotHandled(call) => ExtFunctionResult::from(call.on_no_handler()),
                    })
                    .unwrap(),
                _ => panic!("the scan benchmarks only suspend for OS calls"),
            };
        }
    };
    assert_eq!(run(), expected);
    bench.iter(|| black_box(run()));
}

/// Builds the tree the directory-scan benchmarks read: 20 packages, each with
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

/// Runs a benchmark with production-shaped resource limits armed (generous
/// budgets that never trip), measuring the amortized limit-checking path
/// sandboxes actually run rather than the no-limits fast path.
fn run_monty_limits(bench: &mut Bencher, code: &str, expected: i64) {
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default()
        .max_feed_duration(Duration::from_mins(10))
        .max_memory(1 << 40);
    let mut run = |limits: &ResourceLimits| {
        let r = ex
            .run(vec![], ResourceTracker::new(limits.clone()), PrintWriter::Stdout)
            .unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        int_value
    };
    assert_eq!(run(&limits), expected);
    bench.iter(|| black_box(run(&limits)));
}

/// Runs a benchmark using CPython.
/// Wraps code in main(), parses once, then benchmarks repeated execution.
#[cfg(not(feature = "codspeed"))]
fn run_cpython(bench: &mut Bencher, code: &str, expected: i64) {
    Python::attach(|py| {
        let wrapped = wrap_for_cpython(code);
        let code_cstr = CString::new(wrapped).expect("Invalid C string in code");
        let fun: Py<PyAny> = PyModule::from_code(py, &code_cstr, c"test.py", c"main")
            .unwrap()
            .getattr("main")
            .unwrap()
            .into();

        let r_py = fun.call0(py).unwrap();
        let r: i64 = r_py.extract(py).unwrap();
        assert_eq!(r, expected);

        bench.iter(|| {
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

/// Runs a benchmark using CPython with a single string argument bound to `DATA`.
/// Wraps code in `def main(DATA):`, parses once, then benchmarks repeated execution.
#[cfg(not(feature = "codspeed"))]
fn run_cpython_with_data(bench: &mut Bencher, code: &str, data: &str, expected: i64) {
    Python::attach(|py| {
        let wrapped = wrap_for_cpython_with_param(code, "DATA");
        let code_cstr = CString::new(wrapped).expect("Invalid C string in code");
        let fun: Py<PyAny> = PyModule::from_code(py, &code_cstr, c"test.py", c"main")
            .unwrap()
            .getattr("main")
            .unwrap()
            .into();

        let r_py = fun.call1(py, (data,)).unwrap();
        let r: i64 = r_py.extract(py).unwrap();
        assert_eq!(r, expected);

        bench.iter(|| {
            let r_py = fun.call1(py, (data,)).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

/// Like [`wrap_for_cpython`] but produces `def main(<param>):` so a single argument can be passed.
#[cfg(not(feature = "codspeed"))]
fn wrap_for_cpython_with_param(code: &str, param: &str) -> String {
    let base = wrap_for_cpython(code);
    // Replace the first "def main():" header with one that takes `param`.
    base.replacen("def main():", &format!("def main({param}):"), 1)
}

/// Wraps code in a main() function for CPython execution.
/// Indents each line and converts the last expression to a return statement.
#[cfg(not(feature = "codspeed"))]
fn wrap_for_cpython(code: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut last_expr = String::new();

    for line in code.lines() {
        // Skip test metadata comments
        if line.starts_with("# Return=") || line.starts_with("# Raise=") || line.starts_with("# skip=") {
            continue;
        }
        // Track the last non-empty, non-comment line as potential return expression
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            last_expr = line.to_string();
        }
        lines.push(format!("    {line}"));
    }

    // Replace last expression with return statement
    if let Some(last) = lines.iter().rposition(|l| l.trim() == last_expr.trim()) {
        lines[last] = format!("    return {}", last_expr.trim());
    }

    format!("def main():\n{}", lines.join("\n"))
}

const ADD_TWO: &str = include_str!("python/add_two.py");

const LOOP_MOD_13: &str = include_str!("python/loop_mod_13.py");

/// Comprehensive benchmark exercising most supported Python features.
/// Code is shared with test_cases/bench__kitchen_sink.py
const KITCHEN_SINK: &str = include_str!("../../monty/test_cases/bench__kitchen_sink.py");

const FUNC_CALL_KWARGS: &str = include_str!("python/func_call_kwargs.py");

/// Builtin-method argument binding benchmark: a tight loop of cheap calls whose
/// cost is dominated by `#[derive(FromArgs)]` dispatch (positional fill, kwarg
/// matching, defaults) rather than the operations themselves. Guards the
/// FromArgs binding path against regressions.
const BUILTIN_ARGS: &str = include_str!("python/builtin_args.py");

/// Exercises several attribute names with cheap, non-allocating method bodies:
/// operations on an empty list, a one-entry dict and a single-character string.
/// Loop, argument binding, branching and arithmetic remain part of the measurement.
const ATTR_DISPATCH_MINIMAL: &str = include_str!("python/attr_dispatch_minimal.py");

const LIST_APPEND_STR: &str = include_str!("python/list_append_str.py");

const LIST_APPEND_INT: &str = include_str!("python/list_append_int.py");

const FIB_25: &str = include_str!("python/fib_25.py");

/// Dict comprehension benchmark - creates 500 unique keys (i // 2 deduplicates pairs).
const DICT_COMP: &str = include_str!("python/dict_comp.py");

/// Empty tuple creation benchmark - creates 100,000 empty tuples in a list.
const EMPTY_TUPLES: &str = include_str!("python/empty_tuples.py");

/// 2-tuple creation benchmark - creates 100,000 2-tuples in a list.
const PAIR_TUPLES: &str = include_str!("python/pair_tuples.py");

/// Container repr throughput over 1,000-element containers (half tracked
/// strings, so per-item refcount bumps are priced). Guards the
/// mutation-safe repr paths: live checked iteration for list/tuple/dict,
/// snapshotting for deque/set/Counter, and the amortized time polls.
const CONTAINER_REPR: &str = include_str!("python/container_repr.py");

// --- Agent-workload benchmarks -------------------------------------------
//
// Monty's primary real-world use is "code mode" agents (pydantic-ai
// CodeMode, the examples/ directory, pydantic/talks demos): LLM-written
// Python that pulls rows out of a database/API via external functions,
// aggregates them with plain dicts/lists, and formats a report. The
// benchmarks below mirror those hot loops so regressions in the paths that
// dominate real agent runs (str-keyed dict access, f-string formatting,
// str parsing methods, datetime and re) show up in CI.

/// Group-by aggregation over rows-of-dicts — the core loop of every
/// data-analysis agent (group revenue by segment, rank, filter). Dominated
/// by str-keyed dict get/insert, `sorted` with a lambda key, tuple
/// unpacking, and a filtered generator-expression `sum`.
const AGG_ROWS: &str = include_str!("python/agg_rows.py");

/// Report formatting with f-strings — how agents present results (markdown
/// tables, aligned columns, percentages, thousands separators). Exercises
/// the format mini-language (`fstring.rs`), float formatting, and
/// `str.join` over a large list of tracked strings.
const FSTRING_REPORT: &str = include_str!("python/fstring_report.py");

/// Line-oriented text parsing — the scraping/ETL shape (split lines, split
/// fields, strip/lower/startswith/`in`, `int()` conversion). Measures str
/// method throughput on real text rather than `FromArgs` binding (which
/// `builtin_args` covers with tiny strings).
const STR_PARSE: &str = include_str!("python/str_parse.py");

/// Timestamp cohort analysis — parse ISO timestamps, bucket by weekday and
/// `strftime` month key, subtract datetimes. The retention/day-of-week
/// questions agents get asked constantly; covers `fromisoformat`,
/// `strftime`, `weekday`, and timedelta arithmetic.
const DATETIME_OPS: &str = include_str!("python/datetime_ops.py");

/// Regex extraction over a large string — `finditer` with group capture and
/// `re.split`. The engine is fancy-regex, but Monty's wrapper (match-object
/// allocation, group extraction into tracked strings) is what this guards.
const RE_EXTRACT: &str = include_str!("python/re_extract.py");

/// Single-collection latency benchmark.
///
/// 50,000 self-referencing lists (each `a; a.append(a)` forms a cycle that ref
/// counting alone cannot free).
///
/// The trailing `0` exists so the runner's `i64` extraction sees a known value:
/// Monty's `gc.collect()` always returns `0` but CPython returns the count of
/// unreachable objects collected, which we don't care to assert on.
const GC_COLLECT: &str = include_str!("python/gc_collect.py");

/// JSON payload used by the `json_loads` / `json_dumps` benchmarks.
/// Sourced from `medium_response.json` (a jiter bench fixture).
const JSON_MEDIUM: &str = include_str!("medium_response.json");

/// Parses a ~2 KB JSON document into a Python object 1,000 times per Monty run.
/// Looping inside Monty amortises per-call VM/import/input-binding overhead so the
/// measurement reflects steady-state parse cost rather than startup.
/// Returns the number of top-level keys (2) as the verification value.
const JSON_LOADS: &str = include_str!("python/json_loads.py");

/// Parses JSON then serialises it back to a string 1,000 times per Monty run.
/// Loop sits inside Monty for the same reason as `JSON_LOADS`.
/// Returns the length of the final serialised output so the result can be verified.
const JSON_DUMPS: &str = include_str!("python/json_dumps.py");

/// A recursive glob: every directory is listed.
const GLOB_RECURSIVE: &str = include_str!("python/glob_recursive.py");

/// A glob whose wildcards prune: only `src` directories below the top are listed.
const GLOB_WILDCARD: &str = include_str!("python/glob_wildcard.py");

/// A top-down walk counting files.
const WALK_FILES: &str = include_str!("python/walk_files.py");

/// CSV formatting — how agents hand tabular results back (`writer.writerow`
/// in a loop, `writerows` under `QUOTE_ALL`, and `DictWriter`). Fields mix
/// plain values with ones that force quoting (commas, embedded quotes,
/// newlines), the common shape of real data. The `write()` target is a
/// class, since Monty has no `io.StringIO`; it counts characters written.
const CSV_WRITE: &str = include_str!("python/csv_write.py");

/// CSV parsing — reading tabular input with `reader`, `QUOTE_NONNUMERIC`
/// float conversion and `DictReader`. The lines are formatted once up front
/// with every string quoted, so all three readers share one input; records
/// with embedded newlines span several source lines. Exercises the
/// per-character parser state machine and the per-row dict construction.
const CSV_READ: &str = include_str!("python/csv_read.py");

/// Benchmarks end-to-end execution (parsing + running) using Monty.
/// This is different from other benchmarks as it includes parsing in the loop.
fn end_to_end_monty(bench: &mut Bencher) {
    bench.iter(|| {
        let mut ex = MontyRun::new(
            black_box("1 + 2").to_owned(),
            "test.py",
            vec![],
            CompileOptions::default(),
        )
        .unwrap();
        let r = ex.run_no_limits(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
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

/// Benchmarks end-to-end execution (parsing + running) using CPython.
/// This is different from other benchmarks as it includes parsing in the loop.
#[cfg(not(feature = "codspeed"))]
fn end_to_end_cpython(bench: &mut Bencher) {
    Python::attach(|py| {
        bench.iter(|| {
            let fun: Py<PyAny> =
                PyModule::from_code(py, black_box(c"def main():\n  return 1 + 2"), c"test.py", c"main")
                    .unwrap()
                    .getattr("main")
                    .unwrap()
                    .into();
            let r_py = fun.call0(py).unwrap();
            let r: i64 = r_py.extract(py).unwrap();
            black_box(r);
        });
    });
}

/// Configures all benchmarks in a single group.
fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("add_two__monty", |b| run_monty(b, ADD_TWO, 3));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("add_two__cpython", |b| run_cpython(b, ADD_TWO, 3));

    c.bench_function("loop_mod_13__monty", |b| run_monty(b, LOOP_MOD_13, 77));
    c.bench_function("loop_mod_13_limits__monty", |b| run_monty_limits(b, LOOP_MOD_13, 77));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("loop_mod_13__cpython", |b| run_cpython(b, LOOP_MOD_13, 77));

    c.bench_function("end_to_end__monty", end_to_end_monty);
    c.bench_function("parse_1k_assigns__monty", parse_1k_assigns);
    c.bench_function("clone_module__monty", clone_module);
    c.bench_function("repl_feed_after_2k_snippets__monty", repl_feed_after_2k_snippets);
    c.bench_function("session_dump__monty", session_dump);
    c.bench_function("session_load__monty", session_load);
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("end_to_end__cpython", end_to_end_cpython);

    c.bench_function("kitchen_sink__monty", |b| run_monty(b, KITCHEN_SINK, 373));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("kitchen_sink__cpython", |b| run_cpython(b, KITCHEN_SINK, 373));

    c.bench_function("func_call_kwargs__monty", |b| run_monty(b, FUNC_CALL_KWARGS, 3));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("func_call_kwargs__cpython", |b| run_cpython(b, FUNC_CALL_KWARGS, 3));

    c.bench_function("builtin_args__monty", |b| run_monty(b, BUILTIN_ARGS, 60_000));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("builtin_args__cpython", |b| run_cpython(b, BUILTIN_ARGS, 60_000));

    c.bench_function("attr_dispatch_minimal__monty", |b| {
        run_monty(b, ATTR_DISPATCH_MINIMAL, 30_000);
    });
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("attr_dispatch_minimal__cpython", |b| {
        run_cpython(b, ATTR_DISPATCH_MINIMAL, 30_000);
    });

    c.bench_function("list_append_str__monty", |b| run_monty(b, LIST_APPEND_STR, 100_000));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("list_append_str__cpython", |b| run_cpython(b, LIST_APPEND_STR, 100_000));

    c.bench_function("list_append_int__monty", |b| {
        run_monty(b, LIST_APPEND_INT, 4_999_950_000);
    });
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("list_append_int__cpython", |b| {
        run_cpython(b, LIST_APPEND_INT, 4_999_950_000);
    });

    c.bench_function("fib__monty", |b| run_monty(b, FIB_25, 75_025));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("fib__cpython", |b| run_cpython(b, FIB_25, 75_025));

    c.bench_function("dict_comp__monty", |b| run_monty(b, DICT_COMP, 500));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("dict_comp__cpython", |b| run_cpython(b, DICT_COMP, 500));

    c.bench_function("empty_tuples__monty", |b| run_monty(b, EMPTY_TUPLES, 100_000));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("empty_tuples__cpython", |b| run_cpython(b, EMPTY_TUPLES, 100_000));

    c.bench_function("pair_tuples__monty", |b| run_monty(b, PAIR_TUPLES, 100_000));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("pair_tuples__cpython", |b| run_cpython(b, PAIR_TUPLES, 100_000));

    c.bench_function("container_repr__monty", |b| run_monty(b, CONTAINER_REPR, 628_320));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("container_repr__cpython", |b| run_cpython(b, CONTAINER_REPR, 628_320));

    c.bench_function("json_loads__monty", |b| {
        run_monty_with_data(b, JSON_LOADS, JSON_MEDIUM, 2);
    });
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("json_loads__cpython", |b| {
        run_cpython_with_data(b, JSON_LOADS, JSON_MEDIUM, 2);
    });

    c.bench_function("json_dumps__monty", |b| {
        run_monty_with_data(b, JSON_DUMPS, JSON_MEDIUM, 1815);
    });
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("json_dumps__cpython", |b| {
        run_cpython_with_data(b, JSON_DUMPS, JSON_MEDIUM, 1815);
    });

    c.bench_function("agg_rows__monty", |b| run_monty(b, AGG_ROWS, 909_800));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("agg_rows__cpython", |b| run_cpython(b, AGG_ROWS, 909_800));

    c.bench_function("fstring_report__monty", |b| run_monty(b, FSTRING_REPORT, 102_017));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("fstring_report__cpython", |b| run_cpython(b, FSTRING_REPORT, 102_017));

    c.bench_function("str_parse__monty", |b| run_monty(b, STR_PARSE, 655_040));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("str_parse__cpython", |b| run_cpython(b, STR_PARSE, 655_040));

    c.bench_function("datetime_ops__monty", |b| run_monty(b, DATETIME_OPS, 360_100));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("datetime_ops__cpython", |b| run_cpython(b, DATETIME_OPS, 360_100));

    c.bench_function("re_extract__monty", |b| run_monty(b, RE_EXTRACT, 652_500));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("re_extract__cpython", |b| run_cpython(b, RE_EXTRACT, 652_500));

    c.bench_function("csv_write__monty", |b| run_monty(b, CSV_WRITE, 265_163));
    c.bench_function("csv_write_limits__monty", |b| run_monty_limits(b, CSV_WRITE, 265_163));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("csv_write__cpython", |b| run_cpython(b, CSV_WRITE, 265_163));

    c.bench_function("csv_read__monty", |b| run_monty(b, CSV_READ, 2_629_450));
    c.bench_function("csv_read_limits__monty", |b| run_monty_limits(b, CSV_READ, 2_629_450));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("csv_read__cpython", |b| run_cpython(b, CSV_READ, 2_629_450));

    c.bench_function("gc_collect__monty", |b| run_monty(b, GC_COLLECT, 0));
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("gc_collect__cpython", |b| run_cpython(b, GC_COLLECT, 0));

    let tree = scan_tree();
    #[cfg(not(feature = "codspeed"))]
    let host_root = tree.path().to_str().expect("temporary directories have UTF-8 paths");
    c.bench_function("glob_recursive__monty", |b| {
        run_monty_mounted(b, GLOB_RECURSIVE, tree.path(), MountMode::ReadOnly, 1_020);
    });
    c.bench_function("glob_recursive_overlay__monty", |b| {
        let mode = MountMode::OverlayMemory(OverlayState::new());
        run_monty_mounted(b, GLOB_RECURSIVE, tree.path(), mode, 1_020);
    });
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("glob_recursive__cpython", |b| {
        run_cpython_with_data(b, GLOB_RECURSIVE, host_root, 1_020);
    });
    c.bench_function("glob_wildcard__monty", |b| {
        run_monty_mounted(b, GLOB_WILDCARD, tree.path(), MountMode::ReadOnly, 20);
    });
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("glob_wildcard__cpython", |b| {
        run_cpython_with_data(b, GLOB_WILDCARD, host_root, 20);
    });
    c.bench_function("walk_files__monty", |b| {
        run_monty_mounted(b, WALK_FILES, tree.path(), MountMode::ReadOnly, 4_020);
    });
    #[cfg(not(feature = "codspeed"))]
    c.bench_function("walk_files__cpython", |b| {
        run_cpython_with_data(b, WALK_FILES, host_root, 4_020);
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
