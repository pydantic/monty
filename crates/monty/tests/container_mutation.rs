//! Runtime tests for container walks that user code can re-enter.
//!
//! These cannot be datatest `test_cases` (which dual-run against CPython):
//! the point is that Monty *diverges* here. Where a user `__eq__` or `__hash__`
//! resizes the container being walked, Monty refuses the walk with a
//! `RuntimeError` while CPython carries on over its live table and returns a
//! result. The divergences are recorded in `limitations/builtins.md`; these
//! tests pin the runtime behaviour the page describes.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyException};

/// Runs `code` with no limits and returns the exception it raises.
fn run_err(code: &str) -> MontyException {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    run.run_no_limits(vec![]).expect_err("expected the run to raise")
}

/// Set algebra walks one operand and compares against the other, so a user
/// `__eq__` reached through a hash collision can empty the set mid-walk.
/// CPython finishes over its live table and returns `set()`.
#[test]
fn set_difference_rejects_an_eq_that_clears_the_walked_set() {
    let err = run_err(
        r"
armed = False


class E:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return 0  # collide, so `__eq__` is reached

    def __eq__(self, other):
        if armed:
            s.clear()
        return isinstance(other, E) and self.n == other.n


s = {E(1), E(2)}
t = {E(1)}
armed = True
s - t
",
    );
    assert_eq!(err.exc_type(), ExcType::RuntimeError);
    assert_snapshot!(err.message().unwrap(), @"Set changed size during iteration");
}

/// A dict view's set operators collect the view's own keys through a live walk
/// that hashes each one, so a `__hash__` clearing the dict is caught. CPython
/// probes with each key's stored hash, never calls it, and returns `set()`.
#[test]
fn dict_keys_difference_rejects_a_hash_that_clears_the_dict() {
    let err = run_err(
        r"
armed = False


class K:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        if armed:
            d.clear()
        return self.n

    def __eq__(self, other):
        return isinstance(other, K) and self.n == other.n


d = {K(1): 1, K(2): 2}
s = {K(1), K(2)}
armed = True
d.keys() - s
",
    );
    assert_eq!(err.exc_type(), ExcType::RuntimeError);
    assert_snapshot!(err.message().unwrap(), @"dictionary changed size during iteration");
}
