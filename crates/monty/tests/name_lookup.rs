//! Tests for `NameLookup` — the mechanism by which the host resolves undefined names
//! during iterative execution.
//!
//! When the VM encounters an undefined global (or unassigned local at module scope),
//! it yields `RunProgress::NameLookup` so the host can provide a value or signal
//! that the name is truly undefined. These tests exercise that API directly:
//!
//! - Resolving names to various types (functions, ints, strings, lists, booleans)
//! - Returning `NameLookupResult::Undefined` to trigger `NameError`
//! - Caching: a resolved name should not yield another `NameLookup`
//! - Multiple distinct names each get their own lookup
//! - Builtins bypass the `NameLookup` mechanism entirely

use monty::{MontyObject, MontyRun, NameLookupResult, NoLimitTracker, PrintWriter, RunProgress};

/// Helper: drives execution through consecutive `NameLookup` yields,
/// resolving each by calling `resolver(name)`.
fn resolve_lookups_with(
    mut progress: RunProgress<NoLimitTracker>,
    resolver: impl Fn(&str) -> NameLookupResult,
) -> Result<RunProgress<NoLimitTracker>, monty::MontyException> {
    while let RunProgress::NameLookup(lookup) = progress {
        let result = resolver(&lookup.name);
        progress = lookup.resume(result, &mut PrintWriter::Stdout)?;
    }
    Ok(progress)
}

/// Helper: resolves all `NameLookup` yields as `Function` objects (the common case
/// for external function calls).
fn resolve_as_functions(
    progress: RunProgress<NoLimitTracker>,
) -> Result<RunProgress<NoLimitTracker>, monty::MontyException> {
    resolve_lookups_with(progress, |name| {
        NameLookupResult::Value(MontyObject::Function {
            name: name.to_string(),
            docstring: String::new(),
        })
    })
}

// ---------------------------------------------------------------------------
// Resolving to different types
// ---------------------------------------------------------------------------

/// NameLookup resolved as a Function → code can call it and use the result.
#[test]
fn resolve_as_function_and_call() {
    let runner = MontyRun::new("x = ext(10); x + 1".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    // Resolve NameLookup for 'ext' as a function
    let progress = resolve_as_functions(progress).unwrap();

    // Should now be at a FunctionCall for ext(10)
    let call = progress.into_function_call().expect("expected FunctionCall");
    assert_eq!(call.function_name, "ext");
    assert_eq!(call.args, vec![MontyObject::Int(10)]);

    // Resume with 42 → code evaluates 42 + 1 = 43
    let result = call.resume(MontyObject::Int(42), &mut PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(43));
}

/// NameLookup resolved as an integer constant — no function call involved.
#[test]
fn resolve_as_int() {
    let runner = MontyRun::new("PI + 1".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup for 'PI', got {other:?}"),
    };
    assert_eq!(lookup.name, "PI");

    let result = lookup.resume(MontyObject::Int(3), &mut PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(4));
}

/// NameLookup resolved as a string value.
#[test]
fn resolve_as_string() {
    let runner = MontyRun::new("GREETING + '!'".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup for 'GREETING', got {other:?}"),
    };
    assert_eq!(lookup.name, "GREETING");

    let result = lookup
        .resume(MontyObject::String("hello".to_string()), &mut PrintWriter::Stdout)
        .unwrap();
    assert_eq!(
        result.into_complete().unwrap(),
        MontyObject::String("hello!".to_string())
    );
}

/// NameLookup resolved as a boolean.
#[test]
fn resolve_as_bool() {
    let runner = MontyRun::new("not FLAG".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup for 'FLAG', got {other:?}"),
    };
    assert_eq!(lookup.name, "FLAG");

    let result = lookup
        .resume(MontyObject::Bool(true), &mut PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Bool(false));
}

/// NameLookup resolved as a list.
#[test]
fn resolve_as_list() {
    let runner = MontyRun::new("len(ITEMS)".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup for 'ITEMS', got {other:?}"),
    };
    assert_eq!(lookup.name, "ITEMS");

    let items = MontyObject::List(vec![MontyObject::Int(10), MontyObject::Int(20), MontyObject::Int(30)]);
    let result = lookup.resume(items, &mut PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(3));
}

/// NameLookup resolved as a float.
#[test]
fn resolve_as_float() {
    let runner = MontyRun::new("TAU + 0.5".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup for 'TAU', got {other:?}"),
    };
    assert_eq!(lookup.name, "TAU");

    let result = lookup
        .resume(MontyObject::Float(6.0), &mut PrintWriter::Stdout)
        .unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Float(6.5));
}

// ---------------------------------------------------------------------------
// Undefined → NameError
// ---------------------------------------------------------------------------

/// Returning `NameLookupResult::Undefined` causes `NameError` at global scope.
#[test]
fn undefined_raises_name_error() {
    let runner = MontyRun::new("unknown_thing".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup, got {other:?}"),
    };
    assert_eq!(lookup.name, "unknown_thing");

    let err = lookup
        .resume(NameLookupResult::Undefined, &mut PrintWriter::Stdout)
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("NameError: name 'unknown_thing' is not defined"),
        "Expected NameError, got: {msg}"
    );
}

/// In non-iterative mode (`run_no_limits`), undefined globals automatically raise `NameError`
/// without yielding to the host.
#[test]
fn standard_mode_raises_name_error() {
    let runner = MontyRun::new("unknown_fn(42)".to_owned(), "test.py", vec![]).unwrap();
    let err = runner.run_no_limits(vec![]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("NameError: name 'unknown_fn' is not defined"),
        "Expected NameError, got: {msg}"
    );
}

/// Undefined inside a function that does NOT assign the name locally should
/// still raise `NameError` (not `UnboundLocalError`), since the name lookup
/// falls through to the global scope.
#[test]
fn undefined_in_function_raises_name_error() {
    // `missing` is not assigned inside `f()`, so Python treats it as a global lookup
    let code = "def f():\n    return missing\nf()".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup for 'missing', got {other:?}"),
    };
    assert_eq!(lookup.name, "missing");

    let err = lookup
        .resume(NameLookupResult::Undefined, &mut PrintWriter::Stdout)
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("NameError: name 'missing' is not defined"),
        "Expected NameError, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Caching
// ---------------------------------------------------------------------------

/// A name resolved via `NameLookup` is cached in the namespace — using the same
/// name twice should yield only one `NameLookup`.
#[test]
fn resolved_name_is_cached() {
    let code = "a = ext(1); b = ext(2); a + b".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let mut lookup_count = 0;
    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                assert_eq!(lookup.name, "ext", "unexpected name lookup");
                lookup_count += 1;
                progress = lookup
                    .resume(
                        MontyObject::Function {
                            name: "ext".to_string(),
                            docstring: String::new(),
                        },
                        &mut PrintWriter::Stdout,
                    )
                    .unwrap();
            }
            RunProgress::FunctionCall(call) => {
                let val: i64 = (&call.args[0]).try_into().unwrap();
                progress = call
                    .resume(MontyObject::Int(val * 10), &mut PrintWriter::Stdout)
                    .unwrap();
            }
            RunProgress::Complete(result) => {
                // ext(1) -> 10, ext(2) -> 20 → 30
                assert_eq!(result, MontyObject::Int(30));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
    assert_eq!(lookup_count, 1, "NameLookup should fire only once for a cached name");
}

/// A non-function constant resolved once is also cached.
#[test]
fn resolved_constant_is_cached() {
    // Use the same constant twice — should only yield one NameLookup
    let code = "X + X".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let mut lookup_count = 0;
    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                assert_eq!(lookup.name, "X");
                lookup_count += 1;
                progress = lookup.resume(MontyObject::Int(21), &mut PrintWriter::Stdout).unwrap();
            }
            RunProgress::Complete(result) => {
                assert_eq!(result, MontyObject::Int(42));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
    assert_eq!(lookup_count, 1, "constant should be cached after first lookup");
}

// ---------------------------------------------------------------------------
// Multiple names
// ---------------------------------------------------------------------------

/// Different undefined names each get their own `NameLookup`, in access order.
#[test]
fn multiple_names_each_looked_up() {
    let code = "a = foo(1); b = bar(2); a + b".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let mut looked_up_names = Vec::new();
    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                looked_up_names.push(name.clone());
                progress = lookup
                    .resume(
                        MontyObject::Function {
                            name,
                            docstring: String::new(),
                        },
                        &mut PrintWriter::Stdout,
                    )
                    .unwrap();
            }
            RunProgress::FunctionCall(call) => {
                let val: i64 = (&call.args[0]).try_into().unwrap();
                progress = call
                    .resume(MontyObject::Int(val * 100), &mut PrintWriter::Stdout)
                    .unwrap();
            }
            RunProgress::Complete(result) => {
                // foo(1) -> 100, bar(2) -> 200 → 300
                assert_eq!(result, MontyObject::Int(300));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
    assert_eq!(looked_up_names, vec!["foo", "bar"]);
}

/// Mix of function and non-function name lookups in the same execution.
#[test]
fn mixed_function_and_constant_lookups() {
    let code = "ext(OFFSET)".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();

    let mut looked_up_names = Vec::new();
    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                looked_up_names.push(name.clone());
                let value = match name.as_str() {
                    "ext" => MontyObject::Function {
                        name,
                        docstring: String::new(),
                    },
                    "OFFSET" => MontyObject::Int(100),
                    _ => panic!("unexpected name: {name}"),
                };
                progress = lookup.resume(value, &mut PrintWriter::Stdout).unwrap();
            }
            RunProgress::FunctionCall(call) => {
                assert_eq!(call.function_name, "ext");
                assert_eq!(call.args, vec![MontyObject::Int(100)]);
                progress = call.resume(MontyObject::Int(999), &mut PrintWriter::Stdout).unwrap();
            }
            RunProgress::Complete(result) => {
                assert_eq!(result, MontyObject::Int(999));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
    // 'ext' is looked up first (function position), then 'OFFSET' (argument)
    assert_eq!(looked_up_names, vec!["ext", "OFFSET"]);
}

// ---------------------------------------------------------------------------
// Builtins bypass NameLookup
// ---------------------------------------------------------------------------

/// Known builtins like `len` and `range` do NOT trigger `NameLookup`.
#[test]
fn builtins_do_not_trigger_lookup() {
    let runner = MontyRun::new("len([1, 2, 3])".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();
    assert_eq!(progress.into_complete().unwrap(), MontyObject::Int(3));
}

/// `range` is a builtin — should complete without any NameLookup.
#[test]
fn range_builtin_no_lookup() {
    let runner = MontyRun::new("list(range(3))".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, &mut PrintWriter::Stdout).unwrap();
    assert_eq!(
        progress.into_complete().unwrap(),
        MontyObject::List(vec![MontyObject::Int(0), MontyObject::Int(1), MontyObject::Int(2)])
    );
}
