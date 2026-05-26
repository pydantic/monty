//! Tests for the `_test_cm()` builtin and the `with` statement branches that
//! no production type currently exercises.
//!
//! **REMOVE THIS FILE** once a real production context manager covers these
//! cases. See `crates/monty/src/types/test_cm.rs` for the full removal
//! checklist.
//!
//! Each test pins one specific branch:
//!
//! - `suppress_path_swallows_exception` exercises the swallow path
//!   compiled by `compile_with` (the `JumpIfTrue swallow` → `Pop Pop
//!   ClearException` sequence). `OpenFile.__exit__` always returns
//!   `None`, so this path would otherwise be dead in tests.
//! - `exit_raising_during_normal_exit_propagates` and
//!   `exit_raising_during_exception_path_replaces` exercise the rule that
//!   an exception raised by `__exit__` propagates out of the `with`
//!   block, replacing any in-flight exception. `OpenFile.__exit__` can't
//!   fail.
//! - `enter_raising_skips_body` exercises the rare case where
//!   `__enter__` fails before the body runs (currently only triggered by
//!   `with closed_file:` in production).
//! - `enter_value_bound_to_as_target` verifies the `as` target receives
//!   the `__enter__` return value (not `self`), a case `OpenFile`
//!   doesn't cover because it always returns itself.
//! - `direct_dunder_calls` confirms `f.__enter__()` / `f.__exit__(...)`
//!   work through the trait's default `py_call_attr` dispatch and the
//!   manager's own override.
#![cfg(feature = "test-hooks")]

use monty::{ExcType, MontyObject, MontyRun};

/// Runs a snippet and asserts it succeeds, returning the result value.
fn run_ok(code: &str) -> MontyObject {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).expect("parse");
    runner.run_no_limits(vec![]).expect("run")
}

/// Runs a snippet and asserts it raises the given exception type with the
/// given message.
fn run_err(code: &str) -> (ExcType, Option<String>) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).expect("parse");
    let err = runner.run_no_limits(vec![]).expect_err("expected exception");
    (err.exc_type(), err.message().map(str::to_owned))
}

#[test]
fn passthrough_test_cm_behaves_like_open_file() {
    // Sanity check: a default `_test_cm()` runs the body, returns self on
    // enter, and propagates any in-flight exception.
    let result = run_ok(
        "
cm = _test_cm()
with cm as bound:
    captured = bound is cm
captured
",
    );
    assert_eq!(result, MontyObject::Bool(true));
}

#[test]
fn suppress_path_swallows_exception() {
    // `__exit__` returns True → exception is swallowed → control continues
    // past the `with` block. The compiler-emitted swallow path
    // (`Pop, Pop, ClearException`) is reachable only via this branch.
    let result = run_ok(
        "
cm = _test_cm('suppress')
try:
    with cm:
        raise ValueError('boom')
    # The swallow path falls through to here, so this assignment runs.
    outcome = 'swallowed'
except ValueError:
    outcome = 'propagated'
outcome
",
    );
    assert_eq!(result, MontyObject::String("swallowed".to_owned()));
}

#[test]
fn suppress_does_not_swallow_normal_completion() {
    // The suppress flag only matters on the exception path; a normal exit
    // ignores `__exit__`'s return value entirely (CPython behavior).
    let result = run_ok(
        "
cm = _test_cm('suppress')
with cm:
    inner = 'ran'
inner
",
    );
    assert_eq!(result, MontyObject::String("ran".to_owned()));
}

#[test]
fn enter_raising_skips_body() {
    // `__enter__` raises before the body runs — verify the body never
    // executes and the exception type/message match.
    let (exc_type, message) = run_err(
        "
cm = _test_cm('raise_on_enter', 'no entry')
with cm:
    raise RuntimeError('body should not run')
",
    );
    assert_eq!(exc_type, ExcType::ValueError);
    assert_eq!(message.as_deref(), Some("no entry"));
}

#[test]
fn exit_raising_during_normal_exit_propagates() {
    // On the normal-exit path `__exit__` raising replaces the (absent)
    // in-flight exception with the new one.
    let (exc_type, message) = run_err(
        "
cm = _test_cm('raise_on_exit', 'cleanup failed')
with cm:
    pass
",
    );
    assert_eq!(exc_type, ExcType::ValueError);
    assert_eq!(message.as_deref(), Some("cleanup failed"));
}

#[test]
fn exit_raising_during_exception_path_replaces() {
    // On the exception path, the original exception is replaced by the
    // one raised in `__exit__` (matches CPython semantics).
    let (exc_type, message) = run_err(
        "
cm = _test_cm('raise_on_exit', 'cleanup-wins')
with cm:
    raise RuntimeError('original')
",
    );
    assert_eq!(exc_type, ExcType::ValueError);
    assert_eq!(message.as_deref(), Some("cleanup-wins"));
}

#[test]
fn enter_value_bound_to_as_target() {
    // `__enter__` returns an int → the `as` target gets that int, not
    // the context manager itself.
    let result = run_ok(
        "
with _test_cm('enter_value', 42) as v:
    captured = v
captured
",
    );
    assert_eq!(result, MontyObject::Int(42));
}

#[test]
fn direct_enter_call_returns_self_by_default() {
    // `f.__enter__()` should work the same as `with f:`'s entry — the
    // `py_call_attr` dispatch is the same code path.
    let result = run_ok(
        "
cm = _test_cm()
result = cm.__enter__()
result is cm
",
    );
    assert_eq!(result, MontyObject::Bool(true));
}

#[test]
fn direct_exit_call_returns_none_by_default() {
    let result = run_ok(
        "
cm = _test_cm()
cm.__exit__(None, None, None)
",
    );
    assert_eq!(result, MontyObject::None);
}

#[test]
fn direct_exit_call_with_suppress_returns_none_on_normal_path() {
    // Direct invocation has no in-flight exception, so even a
    // `suppress`-configured manager returns None (the suppress flag only
    // affects the exception path).
    let result = run_ok(
        "
cm = _test_cm('suppress')
cm.__exit__(None, None, None)
",
    );
    assert_eq!(result, MontyObject::None);
}

#[test]
fn direct_exit_call_with_exception_value_routes_to_exception_path() {
    // Forwarding a non-None val to a `suppress`-configured manager should
    // hit the exception branch of `__exit__` and return True. This pins the
    // "val is forwarded to py_exit" fix in `dispatch_exit`.
    let result = run_ok(
        "
cm = _test_cm('suppress')
cm.__exit__(ValueError, ValueError('x'), None)
",
    );
    assert_eq!(result, MontyObject::Bool(true));
}

#[test]
fn direct_exit_call_wrong_arity_raises_type_error() {
    // `cm.__exit__()` and `cm.__exit__(1, 2)` are both arity errors —
    // CPython's `__exit__` is a 3-arg function and accepts nothing else.
    let (exc_type, _) = run_err("_test_cm().__exit__()");
    assert_eq!(exc_type, ExcType::TypeError);
    let (exc_type, _) = run_err("_test_cm().__exit__(None, None)");
    assert_eq!(exc_type, ExcType::TypeError);
    let (exc_type, _) = run_err("_test_cm().__exit__(None, None, None, None)");
    assert_eq!(exc_type, ExcType::TypeError);
}

#[test]
fn unpack_failure_inside_with_calls_exit() {
    // `with cm as (a, b):` where the unpack of `__enter__`'s result fails
    // should still invoke `__exit__` — the unpack lives inside the
    // protected region. `_test_cm()` returns self from `__enter__`, and
    // `TestContextManager` is not iterable, so the unpack raises
    // `TypeError`. A `raise_on_exit` manager makes `__exit__` raise its
    // own `ValueError` that *replaces* the in-flight `TypeError`; if
    // `__exit__` had not been called we'd see the bare `TypeError`.
    let (exc_type, message) = run_err(
        "
with _test_cm('raise_on_exit', 'cleanup-called') as (a, b):
    pass
",
    );
    assert_eq!(exc_type, ExcType::ValueError);
    assert_eq!(message.as_deref(), Some("cleanup-called"));
}

#[test]
fn unknown_behavior_raises_type_error() {
    let (exc_type, message) = run_err("_test_cm('not-a-behavior')");
    assert_eq!(exc_type, ExcType::TypeError);
    assert_eq!(message.as_deref(), Some("_test_cm() unknown behavior 'not-a-behavior'"));
}

#[test]
fn non_string_behavior_raises_type_error() {
    let (exc_type, message) = run_err("_test_cm(None, 1)");
    assert_eq!(exc_type, ExcType::TypeError);
    assert_eq!(
        message.as_deref(),
        Some("_test_cm() behavior must be str, not NoneType")
    );
}

#[test]
fn enter_value_requires_int_payload() {
    let (exc_type, message) = run_err("_test_cm('enter_value', 'not-an-int')");
    assert_eq!(exc_type, ExcType::TypeError);
    assert_eq!(
        message.as_deref(),
        Some("_test_cm('enter_value', n) requires int payload, not str")
    );
}

#[test]
fn raise_on_enter_requires_str_payload() {
    let (exc_type, message) = run_err("_test_cm('raise_on_enter', 7)");
    assert_eq!(exc_type, ExcType::TypeError);
    assert_eq!(
        message.as_deref(),
        Some("_test_cm('raise_on_enter', msg) requires str payload, not int")
    );
}
