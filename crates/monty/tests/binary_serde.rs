//! Tests for binary serialization and deserialization of `MontyRun` and `RunProgress`.
//!
//! These tests verify that execution state can be serialized with postcard for:
//! - Caching parsed code to avoid re-parsing
//! - Snapshotting execution state for external function calls

use monty::{MontyObject, MontyRun, NameLookupResult, NoLimitTracker, PrintWriter, RunProgress};

/// Resolves consecutive `NameLookup` yields by providing a `Function` object for each name.
fn resolve_name_lookups<T: monty::ResourceTracker>(
    mut progress: RunProgress<T>,
) -> Result<RunProgress<T>, monty::MontyException> {
    while let RunProgress::NameLookup(lookup) = progress {
        let name = lookup.name.clone();
        progress = lookup.resume(
            NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
            PrintWriter::Stdout,
        )?;
    }
    Ok(progress)
}

// === MontyRun dump/load Tests ===

#[test]
fn monty_run_dump_load_simple() {
    // Create a runner, dump it, load it, and verify it produces the same result
    let runner = MontyRun::new("1 + 2".to_owned(), "test.py", vec![]).unwrap();
    let bytes = runner.dump().unwrap();
    let loaded = MontyRun::load(&bytes).unwrap();

    let result = loaded.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(3));
}

#[test]
fn monty_run_dump_load_with_inputs() {
    // Test that input names are preserved across dump/load
    let runner = MontyRun::new("x + y * 2".to_owned(), "test.py", vec!["x".to_owned(), "y".to_owned()]).unwrap();
    let bytes = runner.dump().unwrap();
    let loaded = MontyRun::load(&bytes).unwrap();

    let result = loaded
        .run_no_limits(vec![MontyObject::Int(10), MontyObject::Int(5)])
        .unwrap();
    assert_eq!(result, MontyObject::Int(20));
}

#[test]
fn monty_run_dump_load_preserves_code() {
    // Verify the code string is preserved
    let code = "def foo(x):\n    return x * 2\nfoo(21)".to_owned();
    let runner = MontyRun::new(code.clone(), "test.py", vec![]).unwrap();
    let bytes = runner.dump().unwrap();
    let loaded = MontyRun::load(&bytes).unwrap();

    assert_eq!(loaded.code(), code);
    let result = loaded.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn monty_run_dump_load_complex_code() {
    // Test with more complex code including functions, loops, conditionals
    let code = r"
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

result = []
for i in range(10):
    result.append(fib(i))
result
"
    .to_owned();

    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let bytes = runner.dump().unwrap();
    let loaded = MontyRun::load(&bytes).unwrap();

    let result = loaded.run_no_limits(vec![]).unwrap();
    // First 10 Fibonacci numbers: 0, 1, 1, 2, 3, 5, 8, 13, 21, 34
    let expected = MontyObject::List(vec![
        MontyObject::Int(0),
        MontyObject::Int(1),
        MontyObject::Int(1),
        MontyObject::Int(2),
        MontyObject::Int(3),
        MontyObject::Int(5),
        MontyObject::Int(8),
        MontyObject::Int(13),
        MontyObject::Int(21),
        MontyObject::Int(34),
    ]);
    assert_eq!(result, expected);
}

#[test]
fn monty_run_dump_load_multiple_runs() {
    // A loaded runner can be run multiple times
    let runner = MontyRun::new("x * 2".to_owned(), "test.py", vec!["x".to_owned()]).unwrap();
    let bytes = runner.dump().unwrap();
    let loaded = MontyRun::load(&bytes).unwrap();

    assert_eq!(
        loaded.run_no_limits(vec![MontyObject::Int(5)]).unwrap(),
        MontyObject::Int(10)
    );
    assert_eq!(
        loaded.run_no_limits(vec![MontyObject::Int(21)]).unwrap(),
        MontyObject::Int(42)
    );
}

// === RunProgress dump/load Tests ===

#[test]
fn run_progress_dump_load_roundtrip() {
    // Start execution with an external function, dump at the call, load and resume
    let runner = MontyRun::new("ext_fn(42) + 1".to_owned(), "test.py", vec![]).unwrap();

    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    // First resolve the NameLookup for ext_fn
    let progress = resolve_name_lookups(progress).unwrap();

    // Dump the progress at the external call
    let bytes = progress.dump().unwrap();

    // Load it back
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).unwrap();

    // Should still be at the external function call
    let call = loaded.into_function_call().expect("should be at function call");
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(42)]);

    // Resume execution with a return value
    let result = call.resume(MontyObject::Int(100), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(101)); // 100 + 1
}

#[test]
fn run_progress_dump_load_multiple_calls() {
    // Test multiple external calls with dump/load between each
    let runner = MontyRun::new("x = ext_fn(1); y = ext_fn(2); x + y".to_owned(), "test.py", vec![]).unwrap();

    // First call - resolve NameLookup for ext_fn first
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let bytes = progress.dump().unwrap();
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).unwrap();
    let call = loaded.into_function_call().unwrap();
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(1)]);

    // Resume first call
    let progress = call.resume(MontyObject::Int(10), PrintWriter::Stdout).unwrap();
    // Resolve any NameLookup for the second ext_fn reference
    let progress = resolve_name_lookups(progress).unwrap();

    // Dump/load at second call
    let bytes = progress.dump().unwrap();
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).unwrap();
    let call = loaded.into_function_call().unwrap();
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(2)]);

    // Resume second call to completion
    let result = call.resume(MontyObject::Int(20), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(30)); // 10 + 20
}

#[test]
fn run_progress_dump_load_decimal_in_heap() {
    // A `Decimal` held in a local survives a heap snapshot taken at an external
    // call: its hand-written serde re-parses the canonical string on load, so
    // post-restore arithmetic — including a raising op (`1/0`) — works and
    // raises correctly.
    let code = r"
from decimal import Decimal
saved = Decimal('1.50')
ext_fn(0)
out = str(saved + Decimal('2.5'))
try:
    Decimal(1) / Decimal(0)
    out += ' NORAISE'
except ZeroDivisionError:
    out += ' caught'
out
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    let progress = resolve_name_lookups(progress).unwrap();

    // Dump while paused at `ext_fn(0)` — the heap (with `saved`) is serialized.
    let bytes = progress.dump().unwrap();
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).unwrap();

    let call = loaded.into_function_call().expect("paused at ext_fn");
    assert_eq!(call.function_name, "ext_fn");
    let result = call.resume(MontyObject::Int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(
        result.into_complete().unwrap(),
        MontyObject::String("4.00 caught".to_owned())
    );
}

#[test]
fn run_progress_load_rejects_corrupt_decimal() {
    // A snapshot is untrusted input. A hand-tampered snapshot carrying a
    // `Decimal` whose canonical string no longer parses must be *rejected* on
    // load — `Decimal`'s `Deserialize` re-validates through the same parser as
    // `Decimal(str)`. The valid `1e16000` (serialized canonically as
    // `1E+16000`, which the lowercase source spelling never collides with)
    // loads fine; patching only its heap bytes to the same-length malformed
    // `+E+16000` (postcard framing intact) must make load fail.
    let code = r"
from decimal import Decimal
saved = Decimal('1e16000')
ext_fn(0)
saved
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let bytes = progress.dump().unwrap();

    // The untampered snapshot loads AND resumes to completion (resuming, not
    // just loading, both proves the restored heap value is usable and tears
    // the loaded refs down properly — under `memory-model-checks` a loaded
    // snapshot that is merely dropped trips the bare-`Ref` drop guard).
    let loaded = RunProgress::<NoLimitTracker>::load(&bytes).expect("untampered snapshot loads");
    let call = loaded.into_function_call().expect("paused at ext_fn");
    let result = call.resume(MontyObject::Int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(
        result.into_complete().unwrap(),
        MontyObject::Decimal("1E+16000".to_owned())
    );

    // Patch the heap value's canonical string to a malformed literal.
    let tampered = replace_all(&bytes, b"1E+16000", b"+E+16000");
    assert_ne!(tampered, bytes, "expected the canonical decimal string in the snapshot");
    assert!(
        RunProgress::<NoLimitTracker>::load(&tampered).is_err(),
        "corrupt decimal string must be rejected on load"
    );
}

/// Replaces every non-overlapping occurrence of `from` with the equal-length
/// `to` in `haystack`. Equal length keeps postcard's length-prefixed framing
/// valid, so the only change is the bytes themselves.
fn replace_all(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    assert_eq!(from.len(), to.len(), "replacement must preserve framing");
    let mut out = haystack.to_vec();
    let mut i = 0;
    while i + from.len() <= out.len() {
        if &out[i..i + from.len()] == from {
            out[i..i + from.len()].copy_from_slice(to);
            i += from.len();
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn run_progress_complete_roundtrip() {
    // When execution completes, we can still dump/load the Complete variant
    let runner = MontyRun::new("1 + 2".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let bytes = progress.dump().unwrap();
    let loaded: RunProgress<NoLimitTracker> = RunProgress::load(&bytes).unwrap();

    assert_eq!(loaded.into_complete().unwrap(), MontyObject::Int(3));
}
