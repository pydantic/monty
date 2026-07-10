//! Identity, equality, and export semantics for external function inputs (#347, #345).
//!
//! Monty represents external function inputs in two ways depending on whether the
//! function's `__name__` was interned during parsing:
//!
//! - inline `Value::ExtFunction(StringId)` when the name appears in source
//! - heap `HeapData::ExtFunction(String)` otherwise
//!
//! Both refer to the same logical callable: `is`/`==`/`id()`/`hash()` and export
//! conversion must agree on a single answer based on the name string, regardless
//! of which path the conversion took.

use monty::{CompileOptions, MontyObject, MontyRepl, MontyRun, NameLookupResult, NoLimitTracker, PrintWriter};

/// Builds two `MontyObject::Function` inputs with the same `__name__` ("foo")
/// and runs `code` against them as inputs `a` and `b`.
fn run_with_same_callable_inputs(code: &str) -> MontyObject {
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

/// Source does not mention `foo`, so the function name is not interned and the
/// conversion takes the heap path. Two separate `to_value` calls allocate two
/// distinct `HeapId`s, but they refer to the same logical callable.
#[test]
fn same_callable_is_and_eq_via_heap_path() {
    let result = run_with_same_callable_inputs("(a is b, a == b)");
    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Bool(true)]),
    );
}

/// Source mentions `foo`, so the function name is interned and the conversion
/// takes the inline `Value::ExtFunction(StringId)` path. Same logical callable,
/// different representation — identity must agree.
#[test]
fn same_callable_is_and_eq_via_inline_path() {
    let result = run_with_same_callable_inputs("foo = None\n(a is b, a == b)");
    assert_eq!(
        result,
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Bool(true)]),
    );
}

/// `id()` must agree with `is`: equal-by-name → equal-by-id.
#[test]
fn same_callable_id_matches() {
    let result = run_with_same_callable_inputs("id(a) == id(b)");
    assert_eq!(result, MontyObject::Bool(true));
}

/// `hash()` must agree with `==`: equal callables hash equally, regardless of
/// representation. This invariant is required for the dict-key contract.
#[test]
fn same_callable_hash_matches() {
    let result = run_with_same_callable_inputs("hash(a) == hash(b)");
    assert_eq!(result, MontyObject::Bool(true));
}

/// Using one binding as a dict key and looking up via the other exercises the
/// full hash + eq pipeline.
#[test]
fn same_callable_round_trips_through_dict() {
    let result = run_with_same_callable_inputs("d = {a: 42}\nd[b]");
    assert_eq!(result, MontyObject::Int(42));
}

/// Two callables with different `__name__` values must remain distinct — Monty
/// distinguishes external functions by name (the most identity it has after
/// `MontyObject::Function` conversion has discarded host object identity).
#[test]
fn different_named_callables_remain_distinct() {
    let runner = MontyRun::new(
        "(a is b, a == b)".to_owned(),
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
        MontyObject::Tuple(vec![MontyObject::Bool(false), MontyObject::Bool(false)]),
    );
}

/// Round-trip export through the inline path (the #345 bug): when the function
/// name is interned in source, `Value::ExtFunction` previously fell through to
/// `repr_or_error` and exported as a string rather than `MontyObject::Function`.
/// After the fix the export representation must be the same as the heap path.
#[test]
fn inline_callable_exports_as_function_object() {
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

/// REPL-driven cross-representation scenario.
///
/// `MontyRepl` accumulates interned strings across feeds (see `repl.rs:252`
/// where the executor's interns commit back to the session). This makes a
/// mixed inline/heap `ExtFunction` state reachable: a host-supplied function
/// name that the first feed didn't intern becomes interned by a later feed,
/// so a subsequent resolution to the same callable takes the inline path
/// while the first feed's binding is still the heap `Ref`.
///
/// This exercises the cross-representation arms in `Value::is`, `Value::id`,
/// `Value::py_eq`, and the hash alignment — the production path the
/// unit-input tests above cannot reach.
#[test]
fn repl_cross_representation_extfunction_identity() {
    let repl = MontyRepl::new("session.py", NoLimitTracker, CompileOptions::default());

    // Feed 1: `x = foobar` triggers NameLookup for "foobar"; host returns a
    // `Function` whose `__name__` ("ext_fn") does not appear in feed 1's
    // source, so it is not interned and the conversion takes the heap path.
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

    // Feed 2: source mentions `ext_fn` as a name (interning it), then resolves
    // a second NameLookup ("barbaz") to the same `Function(name="ext_fn")`.
    // That conversion now finds "ext_fn" interned and takes the inline path —
    // creating the inline-vs-heap mix between `y` and `x`.
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

    // All four cross-representation checks must agree:
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
