//! Byte caps on `PrintWriter::CollectString` / `CollectStreams`.
//!
//! These tests lock the optional host-side `max_bytes` check: capped runs
//! raise `MemoryError` without growing past the limit, while `None` opts out.
//!
//! Loops stay at ~256 KiB — safe, not a real OOM.

use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, PrintStream, PrintWriter, ResourceTracker};

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
    let mut streams: Vec<(PrintStream, String)> = Vec::new();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectStreams(&mut streams, Some(LIMIT_BYTES)),
        )
        .expect_err("expected MemoryError when collect buffer exceeds max_bytes");

    let total: usize = streams.iter().map(|(_, s)| s.len()).sum();
    assert_eq!(err.exc_type(), ExcType::MemoryError);
    let expected = format!(
        "memory limit exceeded: {} bytes > {LIMIT_BYTES} bytes",
        LIMIT_BYTES + CHUNK_REPS
    );
    assert_eq!(err.message(), Some(expected.as_str()));
    assert!(total <= LIMIT_BYTES, "buffer must stay at or under cap, got {total}");
    assert_eq!(total, LIMIT_BYTES);
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

/// Covers `PrintWriter::collect_streams` and `stdout_push` → `append_streams_char`
/// (the `end=''` loop tests never push a terminator).
#[test]
fn collect_streams_helper_merges_newline_push() {
    let ex = monty_run("print('hi')");
    let mut streams: Vec<(PrintStream, String)> = Vec::new();

    ex.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_streams(&mut streams),
    )
    .expect("default-capped collect_streams should accept a short print");

    assert_eq!(streams, vec![(PrintStream::Stdout, "hi\n".to_owned())]);
}

/// Bare `print()` only `stdout_push`es `'\n'` — exercises the empty-buffer branch
/// of `append_streams_char`.
#[test]
fn collect_streams_empty_print_pushes_newline_entry() {
    let ex = monty_run("print()");
    let mut streams: Vec<(PrintStream, String)> = Vec::new();

    ex.run(
        vec![],
        ResourceTracker::default(),
        PrintWriter::collect_streams(&mut streams),
    )
    .expect("empty print should succeed");

    assert_eq!(streams, vec![(PrintStream::Stdout, "\n".to_owned())]);
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

/// Same as the string case, but through CollectStreams' char-append path.
#[test]
fn collect_streams_max_bytes_rejects_newline_push() {
    let ex = monty_run("print('a')");
    let mut streams: Vec<(PrintStream, String)> = Vec::new();

    let err = ex
        .run(
            vec![],
            ResourceTracker::default(),
            PrintWriter::CollectStreams(&mut streams, Some(1)),
        )
        .expect_err("expected MemoryError on newline push past max_bytes");

    assert_eq!(err.exc_type(), ExcType::MemoryError);
    assert_eq!(err.message(), Some("memory limit exceeded: 2 bytes > 1 bytes"));
    assert_eq!(streams, vec![(PrintStream::Stdout, "a".to_owned())]);
}
