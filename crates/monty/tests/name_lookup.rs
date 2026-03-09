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
        progress = lookup.resume(result, PrintWriter::Stdout)?;
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
            docstring: None,
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
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    // Resolve NameLookup for 'ext' as a function
    let progress = resolve_as_functions(progress).unwrap();

    // Should now be at a FunctionCall for ext(10)
    let call = progress.into_function_call().expect("expected FunctionCall");
    assert_eq!(call.function_name, "ext");
    assert_eq!(call.args, vec![MontyObject::Int(10)]);

    // Resume with 42 → code evaluates 42 + 1 = 43
    let result = call.resume(MontyObject::Int(42), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(43));
}

/// NameLookup resolved as an integer constant — no function call involved.
#[test]
fn resolve_as_int() {
    let runner = MontyRun::new("PI + 1".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "PI");

    let result = lookup.resume(MontyObject::Int(3), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(4));
}

/// NameLookup resolved as a string value.
#[test]
fn resolve_as_string() {
    let runner = MontyRun::new("GREETING + '!'".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "GREETING");

    let result = lookup
        .resume(MontyObject::String("hello".to_string()), PrintWriter::Stdout)
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
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "FLAG");

    let result = lookup.resume(MontyObject::Bool(true), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Bool(false));
}

/// NameLookup resolved as a list.
#[test]
fn resolve_as_list() {
    let runner = MontyRun::new("len(ITEMS)".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "ITEMS");

    let items = MontyObject::List(vec![MontyObject::Int(10), MontyObject::Int(20), MontyObject::Int(30)]);
    let result = lookup.resume(items, PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(3));
}

/// NameLookup resolved as a float.
#[test]
fn resolve_as_float() {
    let runner = MontyRun::new("TAU + 0.5".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "TAU");

    let result = lookup.resume(MontyObject::Float(6.0), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Float(6.5));
}

// ---------------------------------------------------------------------------
// Undefined → NameError
// ---------------------------------------------------------------------------

/// Returning `NameLookupResult::Undefined` causes `NameError` at global scope.
#[test]
fn undefined_raises_name_error() {
    let runner = MontyRun::new("unknown_thing".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "unknown_thing");

    let err = lookup
        .resume(NameLookupResult::Undefined, PrintWriter::Stdout)
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
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "missing");

    let err = lookup
        .resume(NameLookupResult::Undefined, PrintWriter::Stdout)
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

/// Function calls in call context bypass `NameLookup` entirely — they go
/// directly to `FunctionCall` via `LoadGlobalCallable` + `ExtFunction`.
#[test]
fn resolved_name_is_cached() {
    let code = "a = ext(1); b = ext(2); a + b".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let mut call_count = 0;
    loop {
        match progress {
            RunProgress::FunctionCall(call) => {
                assert_eq!(call.function_name, "ext");
                call_count += 1;
                let val: i64 = (&call.args[0]).try_into().unwrap();
                progress = call.resume(MontyObject::Int(val * 10), PrintWriter::Stdout).unwrap();
            }
            RunProgress::Complete(result) => {
                // ext(1) -> 10, ext(2) -> 20 → 30
                assert_eq!(result, MontyObject::Int(30));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
    assert_eq!(call_count, 2, "should get FunctionCall for each ext() call");
}

/// A non-function constant resolved once is also cached.
#[test]
fn resolved_constant_is_cached() {
    // Use the same constant twice — should only yield one NameLookup
    let code = "X + X".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let mut lookup_count = 0;
    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                assert_eq!(lookup.name, "X");
                lookup_count += 1;
                progress = lookup.resume(MontyObject::Int(21), PrintWriter::Stdout).unwrap();
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

/// Different undefined names in call context each yield `FunctionCall` directly
/// (via `LoadGlobalCallable`), not `NameLookup`.
#[test]
fn multiple_names_each_looked_up() {
    let code = "a = foo(1); b = bar(2); a + b".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let mut called_names = Vec::new();
    loop {
        match progress {
            RunProgress::FunctionCall(call) => {
                called_names.push(call.function_name.clone());
                let val: i64 = (&call.args[0]).try_into().unwrap();
                progress = call.resume(MontyObject::Int(val * 100), PrintWriter::Stdout).unwrap();
            }
            RunProgress::Complete(result) => {
                // foo(1) -> 100, bar(2) -> 200 → 300
                assert_eq!(result, MontyObject::Int(300));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
    assert_eq!(called_names, vec!["foo", "bar"]);
}

/// Mix of function calls and constant name lookups in the same execution.
/// `ext` in call context goes directly to `FunctionCall` (no `NameLookup`).
/// `OFFSET` in non-call context yields `NameLookup`.
#[test]
fn mixed_function_and_constant_lookups() {
    let code = "ext(OFFSET)".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let mut progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    let mut looked_up_names = Vec::new();
    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                looked_up_names.push(name.clone());
                let value = match name.as_str() {
                    "OFFSET" => MontyObject::Int(100),
                    _ => panic!("unexpected name lookup: {name}"),
                };
                progress = lookup.resume(value, PrintWriter::Stdout).unwrap();
            }
            RunProgress::FunctionCall(call) => {
                // ext goes directly to FunctionCall via LoadGlobalCallable
                assert_eq!(call.function_name, "ext");
                assert_eq!(call.args, vec![MontyObject::Int(100)]);
                progress = call.resume(MontyObject::Int(999), PrintWriter::Stdout).unwrap();
            }
            RunProgress::Complete(result) => {
                assert_eq!(result, MontyObject::Int(999));
                break;
            }
            other => panic!("unexpected progress: {other:?}"),
        }
    }
    // Only 'OFFSET' yields NameLookup; 'ext' bypasses it via LoadGlobalCallable
    assert_eq!(looked_up_names, vec!["OFFSET"]);
}

// ---------------------------------------------------------------------------
// Builtins bypass NameLookup
// ---------------------------------------------------------------------------

/// Known builtins like `len` and `range` do NOT trigger `NameLookup`.
#[test]
fn builtins_do_not_trigger_lookup() {
    let runner = MontyRun::new("len([1, 2, 3])".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    assert_eq!(progress.into_complete().unwrap(), MontyObject::Int(3));
}

/// `range` is a builtin — should complete without any NameLookup.
#[test]
fn range_builtin_no_lookup() {
    let runner = MontyRun::new("list(range(3))".to_owned(), "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();
    assert_eq!(
        progress.into_complete().unwrap(),
        MontyObject::List(vec![MontyObject::Int(0), MontyObject::Int(1), MontyObject::Int(2)])
    );
}

// ---------------------------------------------------------------------------
// Function passed as input (no NameLookup)
// ---------------------------------------------------------------------------

/// A function passed as an input is already in the namespace — calling it should
/// yield a `FunctionCall` directly without any `NameLookup`.
#[test]
fn input_function_no_lookup() {
    let runner = MontyRun::new("my_fn(10)".to_owned(), "test.py", vec!["my_fn".to_string()]).unwrap();

    let progress = runner
        .start(
            vec![MontyObject::Function {
                name: "my_fn".to_string(),
                docstring: None,
            }],
            NoLimitTracker,
            PrintWriter::Stdout,
        )
        .unwrap();

    // Should go straight to FunctionCall — no NameLookup
    let call = progress
        .into_function_call()
        .expect("expected FunctionCall, not NameLookup");
    assert_eq!(call.function_name, "my_fn");
    assert_eq!(call.args, vec![MontyObject::Int(10)]);

    let result = call.resume(MontyObject::Int(99), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(99));
}

/// A function input assigned to a new variable and called via the alias should
/// still yield a `FunctionCall` without any `NameLookup`.
#[test]
fn input_function_reassigned_then_called() {
    let runner = MontyRun::new(
        "alias = my_fn; alias(5)".to_owned(),
        "test.py",
        vec!["my_fn".to_string()],
    )
    .unwrap();

    let progress = runner
        .start(
            vec![MontyObject::Function {
                name: "my_fn".to_string(),
                docstring: None,
            }],
            NoLimitTracker,
            PrintWriter::Stdout,
        )
        .unwrap();

    // No NameLookup — my_fn is an input, alias is a local assignment
    let call = progress
        .into_function_call()
        .expect("expected FunctionCall, not NameLookup");
    assert_eq!(call.function_name, "my_fn");
    assert_eq!(call.args, vec![MontyObject::Int(5)]);

    let result = call.resume(MontyObject::Int(50), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(50));
}

/// A function input used alongside a name-looked-up constant: the function should
/// not trigger NameLookup but the constant should.
#[test]
fn input_function_with_looked_up_arg() {
    let runner = MontyRun::new("my_fn(OFFSET)".to_owned(), "test.py", vec!["my_fn".to_string()]).unwrap();

    let mut progress = runner
        .start(
            vec![MontyObject::Function {
                name: "my_fn".to_string(),
                docstring: None,
            }],
            NoLimitTracker,
            PrintWriter::Stdout,
        )
        .unwrap();

    // OFFSET is undefined — should yield NameLookup (my_fn should NOT)
    let lookup = match progress {
        RunProgress::NameLookup(l) => l,
        other => panic!("expected NameLookup for 'OFFSET', got {other:?}"),
    };
    assert_eq!(lookup.name, "OFFSET");
    progress = lookup.resume(MontyObject::Int(42), PrintWriter::Stdout).unwrap();

    // Now should be at FunctionCall for my_fn(42)
    let call = progress.into_function_call().expect("expected FunctionCall");
    assert_eq!(call.function_name, "my_fn");
    assert_eq!(call.args, vec![MontyObject::Int(42)]);

    let result = call.resume(MontyObject::Int(100), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(100));
}

/// When a NameLookup resolves to a Function whose name differs from the variable
/// name (i.e., the function's `__name__` is not interned), the VM stores it as
/// `HeapData::ExtFunction(String)`. Calling it should yield a `FunctionCall` with
/// the function's actual name, not the variable name.
#[test]
fn resolve_function_with_non_interned_name() {
    // `x = foobar` triggers NameLookup for 'foobar', we resolve it as a function
    // named 'not_foobar'. Then `x()` calls the function.
    let code = "x = foobar; x()".to_owned();
    let runner = MontyRun::new(code, "test.py", vec![]).unwrap();
    let progress = runner.start(vec![], NoLimitTracker, PrintWriter::Stdout).unwrap();

    // First: NameLookup for 'foobar'
    let lookup = progress.into_name_lookup().unwrap();
    assert_eq!(lookup.name, "foobar");

    // Resolve with a function whose name is NOT 'foobar' — it won't be interned
    let progress = lookup
        .resume(
            NameLookupResult::Value(MontyObject::Function {
                name: "not_foobar".to_string(),
                docstring: None,
            }),
            PrintWriter::Stdout,
        )
        .unwrap();

    // The VM calls x() which is HeapData::ExtFunction("not_foobar") → FunctionCall
    let call = progress
        .into_function_call()
        .expect("expected FunctionCall for 'not_foobar'");
    assert_eq!(call.function_name, "not_foobar");
    assert!(call.args.is_empty());
    assert!(call.kwargs.is_empty());

    // Resume with a return value
    let result = call.resume(MontyObject::Int(42), PrintWriter::Stdout).unwrap();
    assert_eq!(result.into_complete().unwrap(), MontyObject::Int(42));
}
