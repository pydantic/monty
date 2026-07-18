//! Tests for Monty's module-level dunder variables (`__name__`, `__debug__`,
//! `__doc__`, `__annotations__`, `__spec__`, `__package__`, `__loader__`).
//!
//! Monty exposes these with fixed values for CPython compatibility but, having
//! no module object or `globals()` dict, treats them as read-only. The
//! behaviours that match CPython's `__main__` script run (`__name__`,
//! `__debug__`) are covered by the dual-run `test_cases/module__dunders.py`.
//!
//! This file covers the Monty-specific behaviours that intentionally diverge
//! from CPython and so cannot live in the dual-run harness:
//!
//! - `__loader__` raises `NameError` (CPython exposes a real loader object,
//!   never `None`, so Monty declines to expose it rather than diverge on type)
//! - `__annotations__` is always an empty dict (CPython 3.14 raises
//!   `NameError` when a module has no annotations, per PEP 649)
//! - reassigning any of these at module/global scope is rejected at compile
//!   time with `NotImplementedError` (CPython allows it, except `__debug__`
//!   which it rejects with `SyntaxError`)

use monty::{CompileOptions, DictPairs, ExcType, MontyObject, MontyRun};

/// Runs `code` to completion with no resource limits and returns the value of
/// its final expression.
fn eval(code: &str) -> MontyObject {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap()
}

// ---------------------------------------------------------------------------
// Reads — fixed values exposed for CPython compatibility
// ---------------------------------------------------------------------------

#[test]
fn name_is_main() {
    assert_eq!(eval("__name__"), MontyObject::String("__main__".to_owned()));
}

#[test]
fn debug_is_true() {
    assert_eq!(eval("__debug__"), MontyObject::Bool(true));
}

#[test]
fn doc_spec_package_are_none() {
    for name in ["__doc__", "__spec__", "__package__"] {
        assert_eq!(eval(name), MontyObject::None, "{name} should be None");
    }
}

#[test]
fn loader_raises_name_error() {
    // `__loader__` is intentionally not exposed: CPython always binds it to a
    // loader object (never `None`), so Monty leaves it unresolved rather than
    // diverge on type — reading it raises `NameError` like any unbound name.
    let err = MontyRun::new("__loader__".to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .expect_err("expected NameError");
    assert_eq!(err.exc_type(), ExcType::NameError);
    assert_eq!(err.message().unwrap(), "name '__loader__' is not defined");
}

#[test]
fn annotations_is_empty_dict() {
    // Module-level annotations are not stored (see limitations/typing.md), so
    // `__annotations__` is always an empty dict. CPython 3.14 instead raises
    // NameError when a module has no annotations.
    assert_eq!(eval("__annotations__"), MontyObject::Dict(DictPairs::from(vec![])));
}

#[test]
fn name_resolves_inside_function() {
    // A module-level global read from within a function still falls back to the
    // dunder value rather than escalating to a host name lookup.
    assert_eq!(
        eval("def f():\n    return __name__\nf()"),
        MontyObject::String("__main__".to_owned()),
    );
}

// ---------------------------------------------------------------------------
// Writes — reassignment is rejected at compile time
// ---------------------------------------------------------------------------

/// Asserts that compiling `code` fails with a `NotImplementedError` whose
/// message names the offending dunder.
fn assert_reassignment_rejected(code: &str, dunder: &str) {
    let err = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .expect_err("expected compile error");
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    assert_eq!(
        err.message().unwrap(),
        format!("cannot reassign read-only module attribute '{dunder}'"),
    );
}

#[test]
fn reassign_name_at_module_scope_rejected() {
    assert_reassignment_rejected("__name__ = 'foo'", "__name__");
}

#[test]
fn reassign_doc_at_module_scope_rejected() {
    // CPython allows this (it sets the module docstring); Monty does not.
    assert_reassignment_rejected("__doc__ = 'my module'", "__doc__");
}

#[test]
fn reassign_via_global_in_function_rejected() {
    let code = "def f():\n    global __debug__\n    __debug__ = False\nf()";
    assert_reassignment_rejected(code, "__debug__");
}

#[test]
fn augmented_assign_to_dunder_rejected() {
    // `+=` also binds the name, so it is rejected too.
    assert_reassignment_rejected("__name__ += 'x'", "__name__");
}

// ---------------------------------------------------------------------------
// Function-local shadowing is allowed (the name is an ordinary local)
// ---------------------------------------------------------------------------

#[test]
fn function_local_shadowing_allowed() {
    // Binding a dunder name as a function local is fine — it is a distinct
    // namespace, not the module value. (CPython agrees, except for __debug__.)
    let code = "def f():\n    __name__ = 'local'\n    return __name__\nf()";
    assert_eq!(eval(code), MontyObject::String("local".to_owned()));
}
