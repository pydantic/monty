//! Benchmarks for host-side decoding of child frames: how long one frame can
//! occupy a thread in `Worker::recv`. Two payload shapes bracket the per-byte
//! cost — one big string (bulk copy + UTF-8 validation) and a list of row
//! dicts (allocation-heavy, the realistic tool-result shape).

#[cfg(codspeed)]
use codspeed_criterion_compat::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
#[cfg(not(codspeed))]
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use monty_proto::{WireObject, decode_frame, encode_to_capped_vec, pb};
use monty_types::MontyObject;
#[cfg(all(not(codspeed), unix))]
use pprof::criterion::{Output, PProfProfiler};

const KIB: usize = 1024;
const MIB: usize = 1024 * 1024;

/// Decodes one pre-encoded frame per iteration; `iter_with_large_drop` keeps
/// the decoded value's drop untimed (in `recv` the value moves onward).
fn decode_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_frame");
    // large frames take up to ~1s per iteration: keep sampling small
    group.sample_size(10);
    let payloads = [64 * KIB, 4 * MIB, 64 * MIB, 192 * MIB]
        .map(|size| ("str", size, str_frame(size)))
        .into_iter()
        .chain([64 * KIB, 4 * MIB, 64 * MIB].map(|size| ("rows", size, rows_frame(size))));
    for (shape, size, frame) in payloads {
        group.throughput(Throughput::Bytes(frame.len() as u64));
        group.bench_with_input(BenchmarkId::new(shape, size_label(size)), &frame, |bench, frame| {
            bench.iter_with_large_drop(|| decode_frame::<pb::ChildEvent>(black_box(frame)).unwrap());
        });
    }
    group.finish();
}

/// Encodes a `Complete` turn event carrying `value` — byte-identical to the
/// frame body `Worker::recv` hands to `decode_frame`.
fn complete_frame(value: MontyObject) -> Vec<u8> {
    let event = pb::ChildEvent {
        total_execution_micros: 0,
        max_duration_micros: None,
        max_suspensions: None,
        restored_script_name: None,
        kind: Some(pb::child_event::Kind::Complete(pb::Complete {
            value: Some(WireObject(Some(value))),
        })),
    };
    encode_to_capped_vec(&event).expect("frame within MAX_FRAME_LEN")
}

/// A frame whose payload is a single string of roughly `target` bytes.
fn str_frame(target: usize) -> Vec<u8> {
    complete_frame(MontyObject::String("x".repeat(target)))
}

/// A frame of roughly `target` bytes of row dicts, sized by measuring the
/// encoded cost of a small batch first.
fn rows_frame(target: usize) -> Vec<u8> {
    let per_row = complete_frame(rows(1024)).len() / 1024;
    complete_frame(rows((target / per_row).try_into().expect("row count fits i64")))
}

/// A list of `n` dicts shaped like a SQL tool reply (short string keys,
/// mixed str/int values) — same shape as `pool.rs`'s `make_rows`, scaled.
fn rows(n: i64) -> MontyObject {
    MontyObject::List(
        (0..n)
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

/// Renders a byte count as the "64KiB" / "4MiB" style benchmark id suffix.
fn size_label(bytes: usize) -> String {
    if bytes >= MIB {
        format!("{}MiB", bytes / MIB)
    } else {
        format!("{}KiB", bytes / KIB)
    }
}

// Use pprof flamegraph profiler when running locally on Unix (not on CodSpeed or Windows)
#[cfg(all(not(codspeed), unix))]
criterion_group!(
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = decode_benchmark
);

// Use default config on CodSpeed or Windows (pprof is Unix-only)
#[cfg(any(codspeed, not(unix)))]
criterion_group!(benches, decode_benchmark);

criterion_main!(benches);
