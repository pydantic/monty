//! Tests that execution state survives serialization.
//!
//! A paused `RunProgress` goes through the real dump format ([`monty::dump`] /
//! [`monty::Dump::load`]), which is how a host would actually snapshot it.
//! `MontyRun` has no dump of its own — it is compiled code, not a session — but
//! it is `Serialize`/`Deserialize`, so it is round-tripped through postcard
//! directly to cover the serde impls a dump ultimately rests on.

use monty::{Dump, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyException, MontyObject, NameLookupResult, PrintWriter, ResourceTracker};
use serde::{Serialize, de::DeserializeOwned};

/// Round-trips compiled code through postcard.
fn round_trip<T: Serialize + DeserializeOwned>(value: &T) -> T {
    postcard::from_bytes(&postcard::to_allocvec(value).unwrap()).unwrap()
}

/// Round-trips a paused run through the dump format, asserting it comes back on
/// the arm it went out on.
fn round_trip_progress(progress: &RunProgress) -> RunProgress {
    let bytes = dump("test.py", None, SessionRef::Running(progress)).unwrap();
    match Dump::load(&bytes).unwrap().state {
        Session::Running(progress) => *progress,
        _ => panic!("dumped a running session, loaded something else"),
    }
}

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

// === MontyRun round-trip tests ===

#[test]
fn monty_run_round_trip_simple() {
    // Create a runner, round-trip it, and verify it produces the same result
    let runner = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let loaded = round_trip(&runner);

    let result = loaded.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(3));
}

#[test]
fn monty_run_round_trip_with_inputs() {
    // Test that input names are preserved across a round-trip
    let runner = MontyRun::new(
        "x + y * 2".to_owned(),
        "test.py",
        vec!["x".to_owned(), "y".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let loaded = round_trip(&runner);

    let result = loaded
        .run_no_limits(vec![MontyObject::Int(10), MontyObject::Int(5)])
        .unwrap();
    assert_eq!(result, MontyObject::Int(20));
}

#[test]
fn monty_run_round_trip_preserves_code() {
    // Verify the code string is preserved
    let code = "def foo(x):\n    return x * 2\nfoo(21)".to_owned();
    let runner = MontyRun::new(code.clone(), "test.py", vec![], CompileOptions::default()).unwrap();
    let loaded = round_trip(&runner);

    assert_eq!(loaded.code(), code);
    let result = loaded.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn monty_run_round_trip_complex_code() {
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
    let loaded = round_trip(&runner);

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
fn monty_run_round_trip_comprehension_closure() {
    let code = "funcs = [lambda: item for item in ['first', 'second']]\nfuncs[0]()".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
    let loaded = round_trip(&runner);

    assert_eq!(
        loaded.run_no_limits(vec![]).unwrap(),
        MontyObject::String("second".to_owned())
    );
}

#[test]
fn monty_run_round_trip_multiple_runs() {
    // A loaded runner can be run multiple times
    let runner = MontyRun::new(
        "x * 2".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let loaded = round_trip(&runner);

    assert_eq!(
        loaded.run_no_limits(vec![MontyObject::Int(5)]).unwrap(),
        MontyObject::Int(10)
    );
    assert_eq!(
        loaded.run_no_limits(vec![MontyObject::Int(21)]).unwrap(),
        MontyObject::Int(42)
    );
}

// === RunProgress round-trip tests ===

#[test]
fn run_progress_round_trip_at_external_call() {
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

    // Round-trip the progress suspended at the external call
    let loaded: RunProgress = round_trip_progress(&progress);

    // Should still be at the external function call
    let call = loaded.into_function_call().expect("should be at function call");
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(42)]);

    // Resume execution with a return value
    let result = call.resume(MontyObject::Int(100), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(101)); // 100 + 1
}

#[test]
fn run_progress_round_trip_multiple_calls() {
    // Test multiple external calls with a round-trip between each
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
    let loaded: RunProgress = round_trip_progress(&progress);
    let call = loaded.into_function_call().unwrap();
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(1)]);

    // Resume first call
    let progress = call.resume(MontyObject::Int(10), PrintWriter::Stdout).unwrap();
    // Resolve any NameLookup for the second ext_fn reference
    let progress = resolve_name_lookups(progress).unwrap();

    // Round-trip at second call
    let loaded: RunProgress = round_trip_progress(&progress);
    let call = loaded.into_function_call().unwrap();
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args, vec![MontyObject::Int(2)]);

    // Resume second call to completion
    let result = call.resume(MontyObject::Int(20), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(30)); // 10 + 20
}

/// Live `itertools` iterators on the heap survive a round-trip with their state
/// intact — the only coverage that carries `HeapData::Itertools` through
/// postcard, since a `MontyRun` dump holds compiled code and no heap at all.
#[test]
fn run_progress_round_trip_preserves_itertools_iterators() {
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
    let loaded: RunProgress = round_trip_progress(&progress);

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

    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::Int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_loaded.into_complete().unwrap(), expected);
}

#[test]
fn run_progress_complete_round_trip() {
    // When execution completes, we can still dump/load the Complete variant
    let runner = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    let loaded: RunProgress = round_trip_progress(&progress);

    assert_eq!(loaded.into_complete().unwrap(), MontyObject::Int(3));
}
