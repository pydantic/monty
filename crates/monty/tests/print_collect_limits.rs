//! Byte caps on `PrintWriter::CollectString` / `CollectStreams`.
//!
//! These tests lock the optional host-side `max_bytes` check: capped runs
//! raise `MemoryError` without growing past the limit, while `None` opts out.
//!
//! Loops stay at ~256 KiB — safe, not a real OOM.

use monty::MontyRun;
use monty_types::{
    COLLECT_STREAMS_ENTRY_OVERHEAD, CollectedStreams, CompileOptions, ExcType, PrintStream, PrintWriter,
    ResourceTracker,
};

/// One KiB payload reused across prints so heap growth stays small.
const CHUNK: &str = "A";
const CHUNK_REPS: usize = 1024;
const PRINTS: usize = 256;
/// Host collector target ≈ 256 KiB with `end=''`.
const EXPECTED_MIN_BYTES: usize = CHUNK_REPS * PRINTS;
/// Cap / heap limit well below collected output.
const LIMIT_BYTES: usize = 64 * 1024;

fn print_loop_code() -> String {
    format!("s = '{CHUNK}' * {CHUNK_REPS}\nfor _ in range({PRINTS}):\n    print(s, end='')\n")
}

fn monty_run(code: impl Into<String>) -> MontyRun {
    MontyRun::new(code.into(), "test.py", vec![], CompileOptions::default()).unwrap()
}

#[test]
fn collect_string_respects_max_bytes() {
    let ex = monty_run(print_loop_code());
    let mut output = String::new();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectString(&mut output, Some(LIMIT_BYTES)),
        )
        .expect_err("expected MemoryError when collect buffer exceeds max_bytes");

    assert_eq!(err.exc_type(), ExcType::MemoryError);
    let expected = format!(
        "memory limit exceeded: {} bytes > {LIMIT_BYTES} bytes",
        // first write that would cross the limit: LIMIT + one chunk
        LIMIT_BYTES + CHUNK_REPS
    );
    assert_eq!(err.message(), Some(expected.as_str()));
    assert!(
        output.len() <= LIMIT_BYTES,
        "buffer must stay at or under cap, got {}",
        output.len()
    );
    // Filled up to the last successful chunk boundary (exact multiple of CHUNK_REPS).
    assert_eq!(output.len() % CHUNK_REPS, 0);
    assert_eq!(output.len(), LIMIT_BYTES);
}

#[test]
fn collect_streams_respects_max_bytes() {
    let ex = monty_run(print_loop_code());
    let mut streams = CollectedStreams::default();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectStreams(&mut streams, Some(LIMIT_BYTES)),
        )
        .expect_err("expected MemoryError when collect buffer exceeds max_bytes");

    let total: usize = streams.entries().iter().map(|(_, s)| s.len()).sum();
    assert_eq!(err.exc_type(), ExcType::MemoryError);
    // The loop stays on stdout, so one entry overhead is charged on top of the
    // payload: the cap is reached a chunk earlier than the `CollectString` case.
    let expected = format!(
        "memory limit exceeded: {} bytes > {LIMIT_BYTES} bytes",
        COLLECT_STREAMS_ENTRY_OVERHEAD + total + CHUNK_REPS
    );
    assert_eq!(err.message(), Some(expected.as_str()));
    assert!(total <= LIMIT_BYTES, "buffer must stay at or under cap, got {total}");
    assert_eq!(total, LIMIT_BYTES - CHUNK_REPS);
}

/// Opt-out: `max_bytes=None` still allows growth past a 64 KiB would-be cap.
#[test]
fn collect_string_unlimited_allows_growth_past_64kib() {
    let ex = monty_run(print_loop_code());
    let mut output = String::new();

    ex.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::CollectString(&mut output, None),
    )
    .expect("unlimited collect should succeed");

    assert!(
        output.len() >= EXPECTED_MIN_BYTES,
        "expected >= {EXPECTED_MIN_BYTES} bytes, got {}",
        output.len()
    );
    assert!(
        output.len() > LIMIT_BYTES,
        "opt-out not shown: collected {} did not exceed {LIMIT_BYTES}",
        output.len()
    );
}

/// The cap charges both streams against one total, and a refused fragment books
/// nothing. Each switch starts a new entry, so this is also what pins the
/// running total against the entries it is meant to track.
#[test]
fn collect_streams_charges_both_streams_against_one_cap() {
    // 'a' and 'b' alternate a byte at a time, so each of the four writes starts
    // an entry: 4 overheads plus 8 payload bytes. A cap one byte short of that
    // refuses the last newline.
    const CAP: usize = 4 * COLLECT_STREAMS_ENTRY_OVERHEAD + 7;
    let ex = monty_run("import sys\nfor i in range(2):\n    print('a')\n    print('b', file=sys.stderr)\n");
    let mut streams = CollectedStreams::default();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectStreams(&mut streams, Some(CAP)),
        )
        .expect_err("expected MemoryError once both streams together pass the cap");

    assert_eq!(err.exc_type(), ExcType::MemoryError);
    let expected = format!("memory limit exceeded: {} bytes > {CAP} bytes", CAP + 1);
    assert_eq!(err.message(), Some(expected.as_str()));
    assert_eq!(
        streams.entries(),
        [
            (PrintStream::Stdout, "a\n".to_owned()),
            (PrintStream::Stderr, "b\n".to_owned()),
            (PrintStream::Stdout, "a\n".to_owned()),
            (PrintStream::Stderr, "b".to_owned()),
        ]
    );
}

/// Covers `PrintWriter::collect_streams` and `stdout_push` → `CollectedStreams::push_char`
/// (the `end=''` loop tests never push a terminator).
#[test]
fn collect_streams_helper_merges_newline_push() {
    let ex = monty_run("print('hi')");
    let mut streams = CollectedStreams::default();

    ex.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_streams(&mut streams),
    )
    .expect("default-capped collect_streams should accept a short print");

    assert_eq!(streams.entries(), [(PrintStream::Stdout, "hi\n".to_owned())]);
}

/// Bare `print()` only `stdout_push`es `'\n'` — exercises the empty-buffer branch
/// of `CollectedStreams::push_char`.
#[test]
fn collect_streams_empty_print_pushes_newline_entry() {
    let ex = monty_run("print()");
    let mut streams = CollectedStreams::default();

    ex.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_streams(&mut streams),
    )
    .expect("empty print should succeed");

    assert_eq!(streams.entries(), [(PrintStream::Stdout, "\n".to_owned())]);
}

/// Cap of 1 byte: `print('a')` writes `'a'` then fails on the newline push.
#[test]
fn collect_string_max_bytes_rejects_newline_push() {
    let ex = monty_run("print('a')");
    let mut output = String::new();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectString(&mut output, Some(1)),
        )
        .expect_err("expected MemoryError on newline push past max_bytes");

    assert_eq!(err.exc_type(), ExcType::MemoryError);
    assert_eq!(err.message(), Some("memory limit exceeded: 2 bytes > 1 bytes"));
    assert_eq!(output, "a");
}

/// Same as the string case, but through CollectStreams' char-append path: the
/// cap has room for one entry and its byte, so the newline is what crosses it.
#[test]
fn collect_streams_max_bytes_rejects_newline_push() {
    const CAP: usize = COLLECT_STREAMS_ENTRY_OVERHEAD + 1;
    let ex = monty_run("print('a')");
    let mut streams = CollectedStreams::default();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectStreams(&mut streams, Some(CAP)),
        )
        .expect_err("expected MemoryError on newline push past max_bytes");

    assert_eq!(err.exc_type(), ExcType::MemoryError);
    let expected = format!("memory limit exceeded: {} bytes > {CAP} bytes", CAP + 1);
    assert_eq!(err.message(), Some(expected.as_str()));
    assert_eq!(streams.entries(), [(PrintStream::Stdout, "a".to_owned())]);
}

/// The overhead is what stops a stream-switching run from holding far more host
/// memory than its text: every fragment starts an entry, so the cap bounds the
/// entries retained rather than the handful of bytes printed.
#[test]
fn collect_streams_bounds_entries_not_just_payload() {
    const CAP: usize = 4 * 1024;
    /// Each fragment is one byte and starts its own entry.
    const PER_ENTRY: usize = COLLECT_STREAMS_ENTRY_OVERHEAD + 1;
    let ex = monty_run(
        "import sys\nfor _ in range(200):\n    print('a', end='')\n    print('b', end='', file=sys.stderr)\n",
    );
    let mut streams = CollectedStreams::default();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectStreams(&mut streams, Some(CAP)),
        )
        .expect_err("expected MemoryError once the per-entry charge fills the cap");

    assert_eq!(err.exc_type(), ExcType::MemoryError);
    assert_eq!(streams.entries().len(), CAP / PER_ENTRY);
    let payload: usize = streams.entries().iter().map(|(_, s)| s.len()).sum();
    assert_eq!(payload, CAP / PER_ENTRY);
}
