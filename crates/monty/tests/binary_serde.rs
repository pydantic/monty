//! Tests for binary serialization and deserialization of `MontyRun` and `RunProgress`.
//!
//! These tests verify that execution state can be serialized with postcard for:
//! - Caching parsed code to avoid re-parsing
//! - Snapshotting execution state for external function calls

use monty::{MontyRun, RunProgress};
use monty_types::{CompileOptions, MontyException, MontyObject, NameLookupResult, PrintWriter, ResourceTracker};

/// Resolves consecutive `NameLookup` yields by providing a `Function` object for each name.
fn resolve_name_lookups(mut progress: RunProgress) -> Result<RunProgress, MontyException> {
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
fn dump_header_rejects_incompatible_data() {
    let runner = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let bytes = runner.dump().unwrap();
    assert_eq!(&bytes[..9], b"MONTY\0\x05\x00\x00");

    let legacy = postcard::to_allocvec(&runner).unwrap();
    assert_eq!(
        MontyRun::load(&legacy).unwrap_err(),
        postcard::Error::DeserializeBadEncoding
    );

    let mut wrong_version = bytes.clone();
    wrong_version[6] = 1;
    assert_eq!(
        MontyRun::load(&wrong_version).unwrap_err(),
        postcard::Error::DeserializeBadEncoding
    );

    let mut trailing_data = bytes.clone();
    trailing_data.push(0);
    assert_eq!(
        MontyRun::load(&trailing_data).unwrap_err(),
        postcard::Error::DeserializeBadEncoding
    );

    let mut wrong_kind = bytes;
    wrong_kind[8] = 2;
    assert_eq!(
        MontyRun::load(&wrong_kind).unwrap_err(),
        postcard::Error::DeserializeBadEncoding
    );
}

#[test]
fn monty_run_dump_load_simple() {
    // Create a runner, dump it, load it, and verify it produces the same result
    let runner = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let bytes = runner.dump().unwrap();
    let loaded = MontyRun::load(&bytes).unwrap();

    let result = loaded.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(3));
}

#[test]
fn monty_run_dump_load_with_inputs() {
    // Test that input names are preserved across dump/load
    let runner = MontyRun::new(
        "x + y * 2".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
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
    let runner = MontyRun::new(code.clone(), "test.py", vec![], CompileOptions::default()).unwrap();
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

    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
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

/// Captured comprehension cells and their closure metadata survive code serialization.
#[test]
fn monty_run_dump_load_comprehension_closure() {
    let code = "funcs = [lambda: item for item in ['first', 'second']]\nfuncs[0]()".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
    let loaded = MontyRun::load(&runner.dump().unwrap()).unwrap();

    assert_eq!(
        loaded.run_no_limits(vec![]).unwrap(),
        MontyObject::String("second".to_owned())
    );
}

#[test]
fn monty_run_dump_load_multiple_runs() {
    // A loaded runner can be run multiple times
    let runner = MontyRun::new(
        "x * 2".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
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
    let runner = MontyRun::new(
        "ext_fn(42) + 1".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();

    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    // First resolve the NameLookup for ext_fn
    let progress = resolve_name_lookups(progress).unwrap();

    // Dump the progress at the external call
    let bytes = progress.dump().unwrap();

    // Load it back
    let loaded: RunProgress = RunProgress::load(&bytes).unwrap();

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
    let runner = MontyRun::new(
        "x = ext_fn(1); y = ext_fn(2); x + y".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();

    // First call - resolve NameLookup for ext_fn first
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let bytes = progress.dump().unwrap();
    let loaded: RunProgress = RunProgress::load(&bytes).unwrap();
    let call = loaded.into_function_call().unwrap();
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(1)]);

    // Resume first call
    let progress = call.resume(MontyObject::Int(10), PrintWriter::Stdout).unwrap();
    // Resolve any NameLookup for the second ext_fn reference
    let progress = resolve_name_lookups(progress).unwrap();

    // Dump/load at second call
    let bytes = progress.dump().unwrap();
    let loaded: RunProgress = RunProgress::load(&bytes).unwrap();
    let call = loaded.into_function_call().unwrap();
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(2)]);

    // Resume second call to completion
    let result = call.resume(MontyObject::Int(20), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(30)); // 10 + 20
}

/// Live `itertools` iterators on the heap survive a dump/load with their state
/// intact — the only coverage that carries `HeapData::Itertools` through
/// postcard, since a `MontyRun` dump holds compiled code and no heap at all.
#[test]
fn run_progress_dump_load_preserves_itertools_iterators() {
    let code = r"
import itertools

c = itertools.count(10, 2)
r = itertools.repeat('x', 3)
next(c)
next(r)
ext_fn(0)
[next(c), next(r), repr(c), repr(r)]
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

    // Suspend at `ext_fn` with both iterators partly consumed and still live.
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let bytes = progress.dump().unwrap();

    // Both adaptors kept their position: the count carries `current`/`step`,
    // the repeat carries its object and remaining count.
    let expected = MontyObject::List(vec![
        MontyObject::Int(12),
        MontyObject::String("x".to_owned()),
        MontyObject::String("count(14, 2)".to_owned()),
        MontyObject::String("repeat('x', 1)".to_owned()),
    ]);

    // Both are resumed: an unresumed `RunProgress` leaves its globals' refs
    // unreleased, aborting under `memory-model-checks`. Pre-existing and not
    // itertools-specific (a plain `x = [1, 2]` global does it too).
    let original = progress.into_function_call().expect("should be at function call");
    assert_eq!(original.function_name, "ext_fn");
    let from_original = original.resume(MontyObject::Int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_original.into_complete().unwrap(), expected);

    let loaded: RunProgress = RunProgress::load(&bytes).unwrap();
    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::Int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_loaded.into_complete().unwrap(), expected);
}

/// Dicts and sets survive a dump/load even though their hashes and index
/// tables are never serialized: lookups after restore trigger the lazy
/// `ensure_indices` rebuild, and instance-attribute access exercises the
/// VM-less `get_by_str` linear fallback. Covers str/int/tuple keys, both set
/// types, post-restore mutation, equality, and set algebra.
#[test]
fn run_progress_dump_load_rebuilds_dict_and_set_indices() {
    let code = r"
class C:
    def __init__(self):
        self.x = 1
        self.y = 2

obj = C()
d = {'a': 1, 2: 'two', (3, 'x'): 4}
s = {1, 'two', (3, 'x')}
fs = frozenset([7, 'eight'])
ext_fn(0)
d['new'] = 5
s.add(99)
[
    d['a'], d[2], d[(3, 'x')], d['new'],
    (3, 'x') in s, 'two' in s, 42 in s, 99 in s,
    7 in fs, 'eight' in fs, 8 in fs,
    d.pop(2),
    len(d), len(s),
    d == {'a': 1, (3, 'x'): 4, 'new': 5},
    s | {100} == {1, 'two', (3, 'x'), 99, 100},
    obj.x + obj.y,
    list(d)[0],
]
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

    // Suspend at `ext_fn` with the containers live on the heap.
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let bytes = progress.dump().unwrap();

    let expected = MontyObject::List(vec![
        MontyObject::Int(1),
        MontyObject::String("two".to_owned()),
        MontyObject::Int(4),
        MontyObject::Int(5),
        MontyObject::Bool(true),
        MontyObject::Bool(true),
        MontyObject::Bool(false),
        MontyObject::Bool(true),
        MontyObject::Bool(true),
        MontyObject::Bool(true),
        MontyObject::Bool(false),
        MontyObject::String("two".to_owned()),
        MontyObject::Int(3),
        MontyObject::Int(4),
        MontyObject::Bool(true),
        MontyObject::Bool(true),
        MontyObject::Int(3),
        MontyObject::String("a".to_owned()),
    ]);

    // Resume both (an unresumed `RunProgress` leaves its globals' refs
    // unreleased, aborting under `memory-model-checks`).
    let original = progress.into_function_call().expect("should be at function call");
    let from_original = original.resume(MontyObject::Int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_original.into_complete().unwrap(), expected);

    let loaded: RunProgress = RunProgress::load(&bytes).unwrap();
    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::Int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_loaded.into_complete().unwrap(), expected);
}

#[test]
fn run_progress_complete_roundtrip() {
    // When execution completes, we can still dump/load the Complete variant
    let runner = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let bytes = progress.dump().unwrap();
    let loaded: RunProgress = RunProgress::load(&bytes).unwrap();

    assert_eq!(loaded.into_complete().unwrap(), MontyObject::Int(3));
}
