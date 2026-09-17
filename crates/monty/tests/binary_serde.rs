//! Tests that execution state survives serialization.
//!
//! A paused `RunProgress` goes through the real dump format ([`monty::dump`] /
//! [`monty::Dump::load`]), which is how a host would actually snapshot it.
//! `MontyRun` has no dump of its own — it is compiled code, not a session — but
//! it is `Serialize`/`Deserialize`, so it is round-tripped through postcard
//! directly to cover the serde impls a dump ultimately rests on.

use std::fmt::Write;

use monty::{Dump, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, MontyException, MontyObject, MontyType, NameLookupResult, PrintWriter, ResourceTracker,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::to_value;

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
            NameLookupResult::Value(MontyObject::function(name, None)),
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
    assert_eq!(result, MontyObject::int(3));
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
        .run_no_limits(vec![MontyObject::int(10), MontyObject::int(5)])
        .unwrap();
    assert_eq!(result, MontyObject::int(20));
}

#[test]
fn monty_run_round_trip_preserves_code() {
    // Verify the code string is preserved
    let code = "def foo(x):\n    return x * 2\nfoo(21)".to_owned();
    let runner = MontyRun::new(code.clone(), "test.py", vec![], CompileOptions::default()).unwrap();
    let loaded = round_trip(&runner);

    assert_eq!(loaded.code(), code);
    let result = loaded.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::int(42));
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
    let expected = MontyObject::list([
        MontyObject::int(0),
        MontyObject::int(1),
        MontyObject::int(1),
        MontyObject::int(2),
        MontyObject::int(3),
        MontyObject::int(5),
        MontyObject::int(8),
        MontyObject::int(13),
        MontyObject::int(21),
        MontyObject::int(34),
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
        MontyObject::string("second".to_owned())
    );
}

/// A static tag is not part of the wire identity: text unknown to the loading
/// build remains a usable owned interner entry at the same `StringId`.
#[test]
fn static_interns_deserialize_as_unknown_text() {
    let runner = MontyRun::new("'partial'".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut bytes = postcard::to_allocvec(&runner).unwrap();
    let positions: Vec<_> = bytes
        .windows(b"partial".len())
        .enumerate()
        .filter_map(|(index, value)| (value == b"partial").then_some(index))
        .collect();
    assert_eq!(positions.len(), 2, "expected interner text and source text");
    bytes[positions[0]..positions[0] + b"mystery".len()].copy_from_slice(b"mystery");

    let loaded: MontyRun = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(
        loaded.run_no_limits(vec![]).unwrap(),
        MontyObject::string("mystery".to_owned()),
    );
}

/// Reserved strings retain their IDs across snapshots without occupying local slots.
#[test]
fn reserved_strings_round_trip_without_local_entries() {
    let mut code = String::from("['',");
    let mut expected = vec![MontyObject::string(String::new())];
    for byte in 0..128u8 {
        write!(code, "'\\x{byte:02x}',").unwrap();
        expected.push(MontyObject::string(char::from(byte).to_string()));
    }
    code.push(']');
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
    let serialized = to_value(&runner).unwrap();
    for entry in serialized["executor"]["interns"]["strings"].as_array().unwrap() {
        assert!(entry.as_str().unwrap().len() > 1);
    }
    let loaded = round_trip(&runner);
    assert_eq!(loaded.run_no_limits(vec![]).unwrap(), MontyObject::list(expected));
}

/// Heap-only allocation paths, builders and static attributes reuse the empty ID.
#[test]
fn empty_string_allocation_after_snapshot() {
    let code = "
import sys
empty = ''
values = [empty, x, str(), str(encoding='utf-8'), b''.decode(),
          '{}'.format(x), f'{x}', ''.join([]), 'x'[:0], 'x' * 0,
          empty + empty, 'x'.replace('x', ''), sys.prefix]
assert len(set(values)) == 1
assert len({value: 1 for value in values}) == 1
[value is empty for value in values]
";
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let loaded = round_trip(&runner);
    assert_eq!(
        loaded.run_no_limits(vec![MontyObject::string(String::new())]).unwrap(),
        MontyObject::list(vec![MontyObject::bool(true); 13]),
    );
}

/// Module attributes are absent from compiled snapshots and interned lazily
/// during execution, including when running the same loaded program twice.
#[test]
fn execution_interns_module_static_strings() {
    let runner = MontyRun::new(
        "import functools\n1".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let bytes = postcard::to_allocvec(&runner).unwrap();
    assert_eq!(
        bytes
            .windows(b"partial".len())
            .filter(|text| *text == b"partial")
            .count(),
        0
    );
    let loaded: MontyRun = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(loaded.run_no_limits(vec![]).unwrap(), MontyObject::int(1));
    assert_eq!(loaded.run_no_limits(vec![]).unwrap(), MontyObject::int(1));
}

/// Each module can lazily construct its complete namespace after loading,
/// without source attribute references masking missing interner entries.
#[test]
fn module_imports_after_snapshot() {
    for module in [
        "sys",
        "typing",
        "asyncio",
        "pathlib",
        "os",
        "math",
        "json",
        "re",
        "datetime",
        "unicodedata",
        "itertools",
        "dataclasses",
        "collections",
        "functools",
        "base64",
        "binascii",
    ] {
        let runner = MontyRun::new(
            format!("import {module}\n42"),
            "test.py",
            vec![],
            CompileOptions::default(),
        )
        .unwrap();
        let loaded = round_trip(&runner);
        assert_eq!(loaded.run_no_limits(vec![]).unwrap(), MontyObject::int(42));
        let loaded = round_trip(&loaded);
        assert_eq!(loaded.run_no_limits(vec![]).unwrap(), MontyObject::int(42));
    }
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
        loaded.run_no_limits(vec![MontyObject::int(5)]).unwrap(),
        MontyObject::int(10)
    );
    assert_eq!(
        loaded.run_no_limits(vec![MontyObject::int(21)]).unwrap(),
        MontyObject::int(42)
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
    assert_eq!(call.args.args().collect::<Vec<_>>(), vec![MontyObject::int(42)]);

    // Resume execution with a return value
    let result = call.resume(MontyObject::int(100), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::int(101)); // 100 + 1
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
    assert_eq!(call.args.args().collect::<Vec<_>>(), vec![MontyObject::int(1)]);

    // Resume first call
    let progress = call.resume(MontyObject::int(10), PrintWriter::Stdout).unwrap();
    // Resolve any NameLookup for the second ext_fn reference
    let progress = resolve_name_lookups(progress).unwrap();

    // Round-trip at second call
    let loaded: RunProgress = round_trip_progress(&progress);
    let call = loaded.into_function_call().unwrap();
    assert_eq!(call.function_name, "ext_fn");
    assert_eq!(call.args.args().collect::<Vec<_>>(), vec![MontyObject::int(2)]);

    // Resume second call to completion
    let result = call.resume(MontyObject::int(20), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::int(30)); // 10 + 20
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
    let expected = MontyObject::list([
        MontyObject::int(12),
        MontyObject::string("x".to_owned()),
        MontyObject::string("count(14, 2)".to_owned()),
        MontyObject::string("repeat('x', 1)".to_owned()),
    ]);

    // Both are resumed: an unresumed `RunProgress` leaves its globals' refs
    // unreleased, aborting under `memory-model-checks`. Pre-existing and not
    // itertools-specific (a plain `x = [1, 2]` global does it too).
    let original = progress.into_function_call().expect("should be at function call");
    assert_eq!(original.function_name, "ext_fn");
    let from_original = original.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_original.into_complete().unwrap(), expected);

    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_loaded.into_complete().unwrap(), expected);
}

/// A cached call suspended mid-flight keeps its pending cache store across a
/// round-trip, so the result still lands in the cache and the repeat call is a
/// hit — the only coverage that carries a frame's parked store operands through
/// postcard.
#[test]
fn run_progress_round_trip_preserves_pending_cache_store() {
    let code = r"
import functools


@functools.cache
def fetch(n):
    return ext_fn(n) + n


first = fetch(1)
[first, fetch(1), fetch.cache_info().hits, fetch.cache_info().misses]
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

    // Suspend inside `fetch`, whose frame is waiting to store the result.
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let loaded: RunProgress = round_trip_progress(&progress);

    // One external call only: the second `fetch(1)` is answered from the cache.
    let expected = MontyObject::list([
        MontyObject::int(101),
        MontyObject::int(101),
        MontyObject::int(1),
        MontyObject::int(1),
    ]);

    // Both are resumed for the reason given in the itertools round-trip above.
    let original = progress.into_function_call().expect("should be at function call");
    assert_eq!(original.function_name, "ext_fn");
    let from_original = original.resume(MontyObject::int(100), PrintWriter::Stdout).unwrap();
    assert_eq!(from_original.into_complete().unwrap(), expected);

    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::int(100), PrintWriter::Stdout).unwrap();
    assert_eq!(from_loaded.into_complete().unwrap(), expected);
}

/// A live `functools.partial` on the heap survives a round-trip with its bound
/// callable, positionals and keywords intact — the only coverage that carries
/// `HeapData::Partial` through postcard.
#[test]
fn run_progress_round_trip_preserves_partial() {
    let code = r"
import functools


def target(a, b, c=0):
    return a * 100 + b * 10 + c


p = functools.partial(target, 1, c=3)
ext_fn(0)
[p(2), p.args, p.keywords, repr(p.func is target)]
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

    // Suspend at `ext_fn` with the partial built and still live.
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let loaded: RunProgress = round_trip_progress(&progress);

    // The wrapped function is reached by id, so it still resolves to the same
    // object after a reload rather than to a copy.
    let expected = MontyObject::list([
        MontyObject::int(123),
        MontyObject::tuple([MontyObject::int(1)]),
        MontyObject::dict([(MontyObject::string("c".to_owned()), MontyObject::int(3))]),
        MontyObject::string("True".to_owned()),
    ]);

    // Both are resumed for the reason given in the itertools round-trip above.
    let original = progress.into_function_call().expect("should be at function call");
    assert_eq!(original.function_name, "ext_fn");
    let from_original = original.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_original.into_complete().unwrap(), expected);

    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_loaded.into_complete().unwrap(), expected);
}

/// A live `types.GenericAlias` on the heap survives a round-trip with its
/// origin and `__args__` tuple intact — the only coverage that carries
/// `HeapData::GenericAlias` through postcard.
#[test]
fn run_progress_round_trip_preserves_generic_alias() {
    let code = r"
Record = tuple[int, str, ...]
ext_fn(0)
[repr(Record), Record.__args__, repr(Record.__origin__), Record((1, 2)), Record == tuple[int, str, ...]]
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

    // Suspend at `ext_fn` with the alias built and still live.
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let loaded: RunProgress = round_trip_progress(&progress);

    let expected = MontyObject::list([
        MontyObject::string("tuple[int, str, ...]".to_owned()),
        MontyObject::tuple([
            MontyObject::type_object(MontyType::Int),
            MontyObject::type_object(MontyType::Str),
            MontyObject::ellipsis(),
        ]),
        MontyObject::string("<class 'tuple'>".to_owned()),
        MontyObject::tuple([MontyObject::int(1), MontyObject::int(2)]),
        MontyObject::bool(true),
    ]);

    // Both are resumed for the reason given in the itertools round-trip above.
    let original = progress.into_function_call().expect("should be at function call");
    assert_eq!(original.function_name, "ext_fn");
    let from_original = original.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_original.into_complete().unwrap(), expected);

    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_loaded.into_complete().unwrap(), expected);
}

/// A live `typing.Union` on the heap survives a round-trip with its members
/// intact — the only coverage that carries `HeapData::Union` through postcard.
#[test]
fn run_progress_round_trip_preserves_union() {
    let code = r"
Maybe = None | list[int]
ext_fn(0)
[repr(Maybe), Maybe.__args__, Maybe == list[int] | None, isinstance(None, Maybe), isinstance(3, int | Maybe)]
"
    .to_owned();
    let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

    // Suspend at `ext_fn` with the union built and still live.
    let progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let progress = resolve_name_lookups(progress).unwrap();
    let loaded: RunProgress = round_trip_progress(&progress);

    let expected = MontyObject::list([
        MontyObject::string("None | list[int]".to_owned()),
        MontyObject::tuple([
            MontyObject::type_object(MontyType::NoneType),
            MontyObject::repr("list[int]".to_owned()),
        ]),
        MontyObject::bool(true),
        MontyObject::bool(true),
        MontyObject::bool(true),
    ]);

    // Both are resumed for the reason given in the itertools round-trip above.
    let original = progress.into_function_call().expect("should be at function call");
    assert_eq!(original.function_name, "ext_fn");
    let from_original = original.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
    assert_eq!(from_original.into_complete().unwrap(), expected);

    let call = loaded.into_function_call().expect("should be at function call");
    let from_loaded = call.resume(MontyObject::int(0), PrintWriter::Stdout).unwrap();
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

    assert_eq!(loaded.into_complete().unwrap(), MontyObject::int(3));
}
