//! Reference-heavy execution and graph relocation benchmarks for heap representations.

#[cfg(feature = "codspeed")]
use codspeed_criterion_compat::{Bencher, Criterion, black_box, criterion_group, criterion_main};
#[cfg(not(feature = "codspeed"))]
use criterion::{Bencher, Criterion, black_box, criterion_group, criterion_main};
use monty::{Dump, MontyRun, SessionRef, dump};
use monty_bench::{
    heap_checkpoint as checkpoint, heap_graph_session as graph_session, heap_suspend_resume as suspend_resume,
    heap_suspension_program,
};
use monty_types::{CompileOptions, MontyObject};

/// Repeatedly retains and releases aliases rather than allocating new leaf objects.
const COPY_REFERENCES: &str = "
values = [[i] for i in range(256)]
result = 0
for i in range(20_000):
    item = values[i % 256]
    alias = item
    refs = [item, alias, item]
    result += len(refs)
result
";

/// Follows dependent pointers through a graph that fits in the CPU caches.
const FOLLOW_REFERENCES: &str = "
root = [1]
for _ in range(128):
    root = [root]
result = 0
for _ in range(1_000):
    item = root
    for _ in range(128):
        item = item[0]
    result += item[0]
result
";

/// Parses outside timing, but includes each execution's heap allocation and cleanup.
fn execute(bench: &mut Bencher, code: &str, expected: i64) {
    let mut run = MontyRun::new(code.to_owned(), "bench.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::int(expected));
    bench.iter(|| black_box(run.run_no_limits(vec![]).unwrap()));
}

/// Prices both graph traversal and restoration, not merely intern-table serialization.
fn benchmarks(c: &mut Criterion) {
    c.bench_function("heap_copy_refs", |b| execute(b, COPY_REFERENCES, 60_000));
    c.bench_function("heap_follow_refs", |b| execute(b, FOLLOW_REFERENCES, 1_000));
    let run = heap_suspension_program();
    assert_eq!(suspend_resume(&run), MontyObject::int(42));
    c.bench_function("heap_suspend_resume", |b| b.iter(|| black_box(suspend_resume(&run))));

    let repl = graph_session();
    let bytes = dump("bench.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    eprintln!("heap graph dump: {} bytes", bytes.len());
    c.bench_function("heap_graph_dump", |b| {
        b.iter(|| black_box(dump("bench.py", None, &[], SessionRef::Idle(black_box(&repl))).unwrap()));
    });
    c.bench_function("heap_graph_load", |b| {
        b.iter(|| black_box(Dump::load(black_box(&bytes)).unwrap()));
    });
    for passes in [0, 1, 10, 100] {
        let code = format!(
            "total = 0\nfor _ in range({passes}):\n    for i in range(5_000):\n        total += items[i][0]\ntotal"
        );
        let (output, result) = checkpoint(&bytes, &code);
        assert_eq!(result, MontyObject::int(passes * 12_497_500));
        eprintln!("checkpoint {passes}: {} bytes", output.len());
        c.bench_function(&format!("heap_checkpoint_{passes}"), |b| {
            b.iter(|| black_box(checkpoint(black_box(&bytes), black_box(&code))));
        });
    }
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
