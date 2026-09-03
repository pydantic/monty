//! Identity, equality, and export semantics for external function inputs (#347, #345).
//!
//! External functions are heap values cached by lookup name. Host object identity is
//! unavailable across the subprocess boundary, so live functions with the same name
//! share sandbox identity regardless of which conversion path produced them.

use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, NameLookupResult, PrintWriter, ResourceTracker};

/// Builds two `MontyObject::Function` inputs with the same `__name__` ("foo")
/// and runs `code` against them as inputs `a` and `b`.
fn run_with_same_named_callable_inputs(code: &str) -> MontyObject {
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["a".to_owned(), "b".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    runner
        .run_no_limits(vec![
            MontyObject::Function {
                name: "foo".to_owned(),
                docstring: None,
            },
            MontyObject::Function {
                name: "foo".to_owned(),
                docstring: None,
            },
        ])
        .unwrap()
}

/// Separate conversions of a live lookup name reuse its function object.
#[test]
fn same_named_callables_share_identity() {
    let result = run_with_same_named_callable_inputs("(a is b, a == b, id(a) == id(b))");
    assert_eq!(
        result,
        MontyObject::Tuple(vec![
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
        ]),
    );
}

/// Mentioning the host function's name in source does not change its identity.
#[test]
fn same_named_callables_share_identity_when_name_is_interned() {
    let result = run_with_same_named_callable_inputs("foo = None\n(a is b, a == b)");
    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Bool(true)]),
    );
}

/// The same live external function occupies one dictionary key.
#[test]
fn same_named_callables_share_dict_key() {
    let result = run_with_same_named_callable_inputs("d = {a: 42, b: 43}\n(len(d), d[a], d[b])");
    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Int(1), MontyObject::Int(43), MontyObject::Int(43)]),
    );
}

/// Functions with different names are also distinct objects.
#[test]
fn different_named_callables_remain_distinct() {
    let runner = MontyRun::new(
        "(a is b, a == b, id(a) == id(b))".to_owned(),
        "test.py",
        vec!["a".to_owned(), "b".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = runner
        .run_no_limits(vec![
            MontyObject::Function {
                name: "foo".to_owned(),
                docstring: None,
            },
            MontyObject::Function {
                name: "bar".to_owned(),
                docstring: None,
            },
        ])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::Tuple(vec![
            MontyObject::Bool(false),
            MontyObject::Bool(false),
            MontyObject::Bool(false),
        ]),
    );
}

/// An external function exports as a `MontyObject::Function` even when its name
/// also appears in source.
#[test]
fn callable_exports_as_function_object() {
    let runner = MontyRun::new(
        "foo = None\nx".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = runner
        .run_no_limits(vec![MontyObject::Function {
            name: "foo".to_owned(),
            docstring: None,
        }])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::Function {
            name: "foo".to_owned(),
            docstring: None,
        },
    );
}

/// Same callable, same export, regardless of source mention.
#[test]
fn callable_export_stable_across_source_mention() {
    let func_input = || {
        vec![MontyObject::Function {
            name: "foo".to_owned(),
            docstring: None,
        }]
    };
    let r1 = MontyRun::new(
        "x".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(func_input())
    .unwrap();
    let r2 = MontyRun::new(
        "foo = None\nx".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(func_input())
    .unwrap();
    assert_eq!(r1, r2);
}

/// A live external function retains object identity across REPL feeds.
#[test]
fn repl_extfunction_identity_across_feeds() {
    let repl = MontyRepl::new("session.py", ResourceTracker::default(), CompileOptions::default());

    // Feed 1 resolves "foobar" to a host function named "ext_fn".
    let progress = repl.feed_start("x = foobar", vec![], PrintWriter::Stdout).unwrap();
    let lookup = progress.into_name_lookup().expect("expected NameLookup for 'foobar'");
    assert_eq!(lookup.name, "foobar");
    let progress = lookup
        .resume(
            NameLookupResult::Value(MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }),
            PrintWriter::Stdout,
        )
        .unwrap();
    let (repl, _) = progress.into_complete().expect("feed 1 should complete");

    // Feed 2 resolves another name to a function with the same lookup name.
    let progress = repl
        .feed_start(
            "ext_fn = 1\ny = barbaz\n(x is y, x == y, id(x) == id(y), hash(x) == hash(y))",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let lookup = progress.into_name_lookup().expect("expected NameLookup for 'barbaz'");
    assert_eq!(lookup.name, "barbaz");
    let progress = lookup
        .resume(
            NameLookupResult::Value(MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }),
            PrintWriter::Stdout,
        )
        .unwrap();
    let (_repl, result) = progress.into_complete().expect("feed 2 should complete");

    // The second conversion reuses the live function object cached by name.
    assert_eq!(
        result,
        MontyObject::Tuple(vec![
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
            MontyObject::Bool(true),
        ]),
    );
}

/// Snapshot loading rebuilds the weak cache from live external functions.
#[test]
fn extfunction_cache_is_rebuilt_after_snapshot_load() {
    let runner = MontyRun::new(
        "gate()\ny = missing\nx is y".to_owned(),
        "test.py",
        vec!["x".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(
            vec![MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }],
            ResourceTracker::default(),
            PrintWriter::Stdout,
        )
        .unwrap();
    let bytes = dump("test.py", None, SessionRef::Running(&progress)).unwrap();
    assert_eq!(resume_snapshot_identity_test(progress), MontyObject::Bool(true));

    let Session::Running(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped a running session")
    };
    let progress = *progress;
    assert_eq!(resume_snapshot_identity_test(progress), MontyObject::Bool(true));
}

/// Completes the snapshot cache test while preserving refcount cleanup.
fn resume_snapshot_identity_test(progress: RunProgress) -> MontyObject {
    let call = progress.into_function_call().expect("expected call to 'gate'");
    assert_eq!(call.function_name, "gate");

    let progress = call.resume(MontyObject::None, PrintWriter::Stdout).unwrap();
    let lookup = progress.into_name_lookup().expect("expected NameLookup for 'missing'");
    let progress = lookup
        .resume(
            NameLookupResult::Value(MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }),
            PrintWriter::Stdout,
        )
        .unwrap();
    progress.into_complete().unwrap()
}

/// Dropping the last reference removes the weak-cache entry before slot reuse.
#[test]
fn repl_extfunction_cache_does_not_retain_freed_id() {
    let repl = MontyRepl::new("session.py", ResourceTracker::default(), CompileOptions::default());

    let progress = repl.feed_start("x = foobar", vec![], PrintWriter::Stdout).unwrap();
    let lookup = progress.into_name_lookup().expect("expected NameLookup for 'foobar'");
    let progress = lookup
        .resume(
            NameLookupResult::Value(MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }),
            PrintWriter::Stdout,
        )
        .unwrap();
    let (repl, _) = progress.into_complete().expect("feed 1 should complete");

    let progress = repl
        .feed_start("x = None\noccupied = []", vec![], PrintWriter::Stdout)
        .unwrap();
    let (repl, _) = progress.into_complete().expect("feed 2 should complete");

    let progress = repl
        .feed_start(
            "y = barbaz\n(y is occupied, y == occupied)",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let lookup = progress.into_name_lookup().expect("expected NameLookup for 'barbaz'");
    let progress = lookup
        .resume(
            NameLookupResult::Value(MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }),
            PrintWriter::Stdout,
        )
        .unwrap();
    let (_repl, result) = progress.into_complete().expect("feed 3 should complete");

    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Bool(false), MontyObject::Bool(false)]),
    );
}

/// Cycle collection removes weak entries for external-function children.
#[cfg(feature = "test-hooks")]
#[test]
fn gc_freed_ext_function_does_not_retain_freed_id() {
    let code = "\
import gc
d = {}
d['f'] = a
d['self'] = d
d = None
a = None
gc.collect()
occupied = [1, 2, 3]
y = missing
(y is occupied, repr(y))
";
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["a".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(
            vec![MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }],
            ResourceTracker::default(),
            PrintWriter::Stdout,
        )
        .unwrap();

    let lookup = progress.into_name_lookup().expect("expected NameLookup for 'missing'");
    assert_eq!(lookup.name, "missing");
    let progress = lookup
        .resume(
            NameLookupResult::Value(MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }),
            PrintWriter::Stdout,
        )
        .unwrap();
    let result = progress.into_complete().expect("run should complete");

    assert_eq!(
        result,
        MontyObject::Tuple(vec![
            MontyObject::Bool(false),
            MontyObject::String("<function 'ext_fn' external>".to_owned()),
        ]),
    );
}
