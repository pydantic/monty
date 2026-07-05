//! Benchmarks for the subprocess pool (`monty-pool`): the cost of spawning
//! workers, checking out sessions, and round-tripping values over the
//! protobuf wire protocol. These complement `main.rs` (which measures the
//! in-process interpreter) by measuring the overheads subprocess isolation
//! adds — process spawn, checkout handshake, and per-message framing.
//!
//! Unlike `main.rs` there is no CPython comparison: these costs are inherent
//! to monty's isolation model and have no CPython equivalent.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::Once,
};

#[cfg(codspeed)]
use codspeed_criterion_compat::{Bencher, Criterion, black_box, criterion_group, criterion_main};
#[cfg(not(codspeed))]
use criterion::{Bencher, Criterion, black_box, criterion_group, criterion_main};
use monty::{MontyObject, PrintStream};
use monty_pool::{Checkout, Pool, PoolConfig, ReplConfig, ResumeValue, TurnEvent};
#[cfg(all(not(codspeed), unix))]
use pprof::criterion::{Output, PProfProfiler};

/// Locates (building once if needed) the `monty` CLI binary the workers run.
/// Mirrors the resolution used by `monty-pool`'s integration tests: honour
/// `MONTY_TEST_BIN`, otherwise use (and build on demand) the workspace
/// `target/debug/monty`.
fn monty_binary() -> PathBuf {
    static BUILD: Once = Once::new();
    if let Ok(path) = env::var("MONTY_TEST_BIN") {
        return PathBuf::from(path);
    }
    // <workspace>/target/debug/monty, derived from this crate's manifest dir
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("target/debug/monty");
    BUILD.call_once(|| {
        if !path.exists() {
            let status = Command::new(env!("CARGO"))
                .args(["build", "-p", "monty-cli"])
                .status()
                .expect("failed to run cargo build -p monty-cli");
            assert!(status.success(), "building the monty binary failed");
        }
    });
    assert!(path.exists(), "monty binary missing at {}", path.display());
    path
}

/// Discards sandbox `print()` output — none of these benchmarks print.
fn no_print(_: PrintStream, _: &str) {}

/// Asserts a turn completed and returns its value, panicking on any
/// suspension — used by the benchmarks that feed code making no external
/// calls.
#[track_caller]
fn expect_complete(event: TurnEvent) -> MontyObject {
    match event {
        TurnEvent::Complete(value) => value,
        other => panic!("expected Complete, got {other:?}"),
    }
}

/// Drives a feed to completion, answering every external-function suspension
/// with `None`. This is the hot loop of the wire-protocol benchmark: each
/// `resume` is one request/reply pair across the framed protobuf channel.
#[track_caller]
fn drive_answering_calls(session: &mut Checkout, mut event: TurnEvent) -> MontyObject {
    loop {
        match event {
            TurnEvent::Complete(value) => break value,
            TurnEvent::FunctionCall { .. } => {
                event = session
                    .resume(ResumeValue::Return(MontyObject::None), &mut no_print)
                    .unwrap();
            }
            other => panic!("expected Complete or FunctionCall, got {other:?}"),
        }
    }
}

/// Full cold-start cost: build a pool (which eagerly spawns one worker
/// process), check out a session, run `1 + 1`, and finish. Each iteration
/// spawns and tears down a worker, so this is dominated by process startup —
/// the price paid once per fresh pool.
fn pool_create_session_run(bench: &mut Bencher) {
    let binary = monty_binary();
    bench.iter(|| {
        let pool = Pool::new(PoolConfig::subprocess(&binary)).unwrap();
        let mut session = pool.checkout(&ReplConfig::default()).unwrap();
        let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
        black_box(expect_complete(event));
        session.finish().unwrap();
    });
}

/// Warm-pool checkout cost: the worker already exists, so this measures the
/// per-session checkout handshake plus a trivial `1 + 1` feed — the overhead
/// every request pays once the pool is warm.
fn session_checkout_run(bench: &mut Bencher) {
    let pool = Pool::new(PoolConfig::subprocess(monty_binary())).unwrap();
    bench.iter(|| {
        let mut session = pool.checkout(&ReplConfig::default()).unwrap();
        let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
        black_box(expect_complete(event));
        session.finish().unwrap();
    });
}

/// Calls an undefined name (which becomes an external-function suspension)
/// 1,000 times in a loop. Each call is a full round trip — the worker
/// suspends and frames the call, the parent answers with a framed reply —
/// so this isolates the per-message protobuf encode/decode + framing cost.
const EXT_CALL_LOOP: &str = "
for i in range(1000):
    ext_function(i)
";

/// Wire-protocol throughput: 1,000 external-call round trips over the framed
/// protobuf channel on a single warm session. Reuses the session across
/// iterations so only the messaging cost is measured, not checkout.
fn ext_calls_1000(bench: &mut Bencher) {
    let pool = Pool::new(PoolConfig::subprocess(monty_binary())).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    bench.iter(|| {
        let event = session
            .feed(EXT_CALL_LOOP, vec![], vec![], false, &mut no_print)
            .unwrap();
        black_box(drive_answering_calls(&mut session, event));
    });
    session.finish().unwrap();
}

/// Sums `amount * quantity` over every row of every external-call result —
/// the aggregation a code-mode agent runs over a SQL tool's rows.
const EXT_ROWS_LOOP: &str = "
total = 0
for i in range(20):
    rows = fetch_rows(i)
    for row in rows:
        total += row['amount'] * row['quantity']
total
";

/// Builds a 100-row result set shaped like a SQL tool reply: a list of dicts
/// with string keys and mixed str/int values. This is the payload shape real
/// agents pull across the wire on every external call.
fn make_rows() -> MontyObject {
    MontyObject::List(
        (0..100)
            .map(|i| {
                MontyObject::dict(vec![
                    (MontyObject::String("order_id".to_owned()), MontyObject::Int(i)),
                    (
                        MontyObject::String("customer".to_owned()),
                        MontyObject::String(format!("customer-{i}@example.com")),
                    ),
                    (
                        MontyObject::String("region".to_owned()),
                        MontyObject::String("north".to_owned()),
                    ),
                    (
                        MontyObject::String("amount".to_owned()),
                        MontyObject::Int((i * 37) % 500 + 1),
                    ),
                    (MontyObject::String("quantity".to_owned()), MontyObject::Int(i % 7 + 1)),
                ])
            })
            .collect(),
    )
}

/// Large-payload wire throughput: 20 external calls per iteration, each
/// answered with a 100-row list-of-dicts. Unlike `ext_calls_1000` (which
/// replies `None` and isolates framing), this measures WireObject
/// encode/decode and heap conversion of realistic tool results — the
/// dominant wire cost for data-analysis agents.
fn ext_call_rows(bench: &mut Bencher) {
    let rows = make_rows();
    // Expected sandbox result: 20 identical calls, each summing amount * quantity.
    let per_call: i64 = (0..100).map(|i| ((i * 37) % 500 + 1) * (i % 7 + 1)).sum();
    let expected = MontyObject::Int(per_call * 20);

    let pool = Pool::new(PoolConfig::subprocess(monty_binary())).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    bench.iter(|| {
        let mut event = session
            .feed(EXT_ROWS_LOOP, vec![], vec![], false, &mut no_print)
            .unwrap();
        let value = loop {
            match event {
                TurnEvent::Complete(value) => break value,
                TurnEvent::FunctionCall { .. } => {
                    event = session
                        .resume(ResumeValue::Return(rows.clone()), &mut no_print)
                        .unwrap();
                }
                other => panic!("expected Complete or FunctionCall, got {other:?}"),
            }
        };
        assert_eq!(value, expected);
        black_box(value);
    });
    session.finish().unwrap();
}

/// Configures the pool benchmarks.
fn pool_benchmark(c: &mut Criterion) {
    c.bench_function("pool_create_session_run", pool_create_session_run);
    c.bench_function("session_checkout_run", session_checkout_run);
    c.bench_function("ext_calls_1000", ext_calls_1000);
    c.bench_function("ext_call_rows", ext_call_rows);
}

// Use pprof flamegraph profiler when running locally on Unix (not on CodSpeed or Windows)
#[cfg(all(not(codspeed), unix))]
criterion_group!(
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = pool_benchmark
);

// Use default config on CodSpeed or Windows (pprof is Unix-only)
#[cfg(any(codspeed, not(unix)))]
criterion_group!(benches, pool_benchmark);

criterion_main!(benches);
