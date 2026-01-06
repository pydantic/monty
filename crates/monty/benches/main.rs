use std::ffi::CString;

use criterion::{black_box, criterion_group, criterion_main, Bencher, Criterion};
use monty::MontyRun;
use pprof::criterion::{Output, PProfProfiler};
use pyo3::{prelude::*, types::PyAny};

/// Runs a benchmark using the Monty interpreter.
/// Parses once, then benchmarks repeated execution.
fn run_monty(bench: &mut Bencher, code: &str, expected: i64) {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, expected);

    bench.iter(|| {
        let r = ex.run_no_limits(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Runs a benchmark using CPython.
/// Wraps code in main(), parses once, then benchmarks repeated execution.
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

/// Wraps code in a main() function for CPython execution.
/// Indents each line and converts the last expression to a return statement.
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

const ADD_TWO: &str = "1 + 2";

const LIST_APPEND: &str = "
a = []
a.append(42)
a[0]
";

const LOOP_MOD_13: &str = "
v = ''
for i in range(1_000):
    if i % 13 == 0:
        v += 'x'
len(v)
";

/// Comprehensive benchmark exercising most supported Python features.
/// Code is shared with test_cases/bench__kitchen_sink.py
const KITCHEN_SINK: &str = include_str!("../test_cases/bench__kitchen_sink.py");

const FUNC_CALL_KWARGS: &str = "
def add(a, b=2):
    return a + b

add(a=1)
";

const LIST_APPEND_STR: &str = "
a = []
for i in range(100_000):
    a.append(str(i))
len(a)
";

const LIST_APPEND_INT: &str = "
a = []
for i in range(100_000):
    a.append(i)
sum(a)
";

const FIB_25: &str = "
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(25)
";

/// Benchmarks end-to-end execution (parsing + running) using Monty.
/// This is different from other benchmarks as it includes parsing in the loop.
fn end_to_end_monty(bench: &mut Bencher) {
    bench.iter(|| {
        let ex = MontyRun::new(black_box("1 + 2").to_owned(), "test.py", vec![], vec![]).unwrap();
        let r = ex.run_no_limits(vec![]).unwrap();
        let int_value: i64 = r.as_ref().try_into().unwrap();
        black_box(int_value);
    });
}

/// Benchmarks end-to-end execution (parsing + running) using CPython.
/// This is different from other benchmarks as it includes parsing in the loop.
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

/// Configures all benchmark groups
fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_two");
    group.bench_function("monty", |b| run_monty(b, ADD_TWO, 3));
    group.bench_function("cpython", |b| run_cpython(b, ADD_TWO, 3));
    group.finish();

    let mut group = c.benchmark_group("list_append");
    group.bench_function("monty", |b| run_monty(b, LIST_APPEND, 42));
    group.bench_function("cpython", |b| run_cpython(b, LIST_APPEND, 42));
    group.finish();

    let mut group = c.benchmark_group("loop_mod_13");
    group.bench_function("monty", |b| run_monty(b, LOOP_MOD_13, 77));
    group.bench_function("cpython", |b| run_cpython(b, LOOP_MOD_13, 77));
    group.finish();

    let mut group = c.benchmark_group("end_to_end");
    group.bench_function("monty", end_to_end_monty);
    group.bench_function("cpython", end_to_end_cpython);
    group.finish();

    let mut group = c.benchmark_group("kitchen_sink");
    group.bench_function("monty", |b| run_monty(b, KITCHEN_SINK, 58));
    group.bench_function("cpython", |b| run_cpython(b, KITCHEN_SINK, 58));
    group.finish();

    let mut group = c.benchmark_group("func_call_kwargs");
    group.bench_function("monty", |b| run_monty(b, FUNC_CALL_KWARGS, 3));
    group.bench_function("cpython", |b| run_cpython(b, FUNC_CALL_KWARGS, 3));
    group.finish();

    let mut group = c.benchmark_group("list_append_str");
    group.bench_function("monty", |b| run_monty(b, LIST_APPEND_STR, 100_000));
    group.bench_function("cpython", |b| run_cpython(b, LIST_APPEND_STR, 100_000));
    group.finish();

    let mut group = c.benchmark_group("list_append_int");
    group.bench_function("monty", |b| run_monty(b, LIST_APPEND_INT, 4_999_950_000));
    group.bench_function("cpython", |b| run_cpython(b, LIST_APPEND_INT, 4_999_950_000));
    group.finish();

    let mut group = c.benchmark_group("fib");
    group.bench_function("monty", |b| run_monty(b, FIB_25, 75_025));
    group.bench_function("cpython", |b| run_cpython(b, FIB_25, 75_025));
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = criterion_benchmark
);
criterion_main!(benches);
