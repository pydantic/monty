// Use codspeed-criterion-compat when running on CodSpeed (CI), real criterion otherwise (for flamegraphs)
#[cfg(codspeed)]
use codspeed_criterion_compat::{Criterion, black_box, criterion_group, criterion_main};
#[cfg(not(codspeed))]
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use monty_type_checking::{SourceFile, type_check};
#[cfg(all(not(codspeed), unix))]
use pprof::criterion::{Output, PProfProfiler};

/// Force one reusable pooled database to exist before the measurement loop.
///
/// Every benchmark except `type_check__first_call` calls this at setup so the reported
/// numbers reflect steady-state reuse cost, not the first cold call.
fn prewarm() {
    let _ = type_check(&SourceFile::new("pass", "warmup.py"), None);
}

/// Steady-state cost of type-checking a trivial snippet. This is the headline metric —
/// it isolates per-call overhead (check out a pooled db, write one file, run
/// `check_types`, scrub the file, return the db) from the one-time cold start.
fn bench_warm_trivial(c: &mut Criterion) {
    prewarm();
    c.bench_function("type_check__warm_trivial", |b| {
        b.iter(|| {
            let out = type_check(&SourceFile::new("x = 1", "main.py"), None).unwrap();
            black_box(out);
        });
    });
}

/// Steady-state cost of type-checking a snippet that exercises a builtin (`int.__add__`).
/// Slightly heavier than `warm_trivial` because it actually resolves a type.
fn bench_warm_builtin(c: &mut Criterion) {
    prewarm();
    c.bench_function("type_check__warm_builtin", |b| {
        b.iter(|| {
            let out = type_check(&SourceFile::new("x = 1 + 2", "main.py"), None).unwrap();
            black_box(out);
        });
    });
}

/// Realistic REPL-like pattern: each iteration type-checks a growing accumulated-stubs
/// context plus a new "current" snippet. Mirrors how the REPL would call `type_check`
/// per feed_run.
fn bench_repl_sequence(c: &mut Criterion) {
    prewarm();
    c.bench_function("type_check__repl_sequence", |b| {
        b.iter(|| {
            let mut stubs = String::new();
            for (i, snippet) in [
                "x = 1",
                "y = x + 2",
                "def f(a: int) -> int:\n    return a * 2",
                "z = f(y)",
            ]
            .iter()
            .enumerate()
            {
                let path = format!("step_{i}.py");
                let stubs_src = SourceFile::new(&stubs, "type_stubs.pyi");
                let main_src = SourceFile::new(snippet, &path);
                let out = type_check(&main_src, Some(&stubs_src))
                    .expect("repl-sequence benchmark should not hit internal type-check failures");
                assert!(
                    out.is_none(),
                    "repl-sequence benchmark snippet should type-check cleanly: {snippet}"
                );
                black_box(out);
                stubs.push_str(snippet);
                stubs.push('\n');
            }
        });
    });
}

/// Configures the type-checking benchmarks.
fn criterion_benchmark(c: &mut Criterion) {
    bench_warm_trivial(c);
    bench_warm_builtin(c);
    bench_repl_sequence(c);
}

// Use pprof flamegraph profiler when running locally on Unix (not on CodSpeed or Windows)
#[cfg(all(not(codspeed), unix))]
criterion_group!(
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = criterion_benchmark
);

// Use default config on CodSpeed or Windows (pprof is Unix-only)
#[cfg(any(codspeed, not(unix)))]
criterion_group!(benches, criterion_benchmark);

criterion_main!(benches);
