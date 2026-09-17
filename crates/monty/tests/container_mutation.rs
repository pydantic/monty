//! Runtime tests for container walks that user code can re-enter.
//!
//! These cannot be datatest `test_cases` (which dual-run against CPython):
//! the point is that Monty *diverges* here. Where a user `__eq__` or `__hash__`
//! resizes a container mid-operation, Monty and CPython observe the change at
//! different moments — Monty refuses a walk with a `RuntimeError` where CPython
//! returns a result, and in a few places the reverse. The divergences are
//! recorded in `limitations/builtins.md`; these tests pin the runtime behaviour
//! the page describes.

use insta::{allow_duplicates, assert_snapshot};
use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyException, MontyNode, MontyObject};

/// Two colliding elements in `s`, whose `__eq__` clears `s` once `armed`.
///
/// The collision is what makes `__eq__` reachable at all: every set operation
/// below compares an element of one set against a stored element of the other.
const EQ_CLEARS_S: &str = r"
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
";

/// A dict `d` whose keys clear it from `__hash__`, and a set `s` of equal keys.
const HASH_CLEARS_D: &str = r"
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
";

/// Set algebra walks one operand and compares against the other, so a user
/// `__eq__` reached through a hash collision can empty the set mid-walk.
/// CPython finishes over its live table and returns `set()` for both of these.
#[test]
fn set_algebra_rejects_an_eq_that_clears_the_walked_set() {
    allow_duplicates! {
        for expr in ["s - t", "s ^ t"] {
            let err = run_err(EQ_CLEARS_S, expr);
            assert_eq!(err.exc_type(), ExcType::RuntimeError, "{expr}");
            assert_snapshot!(err.message().unwrap(), @"Set changed size during iteration");
        }
    }
}

/// The operand Monty is not walking is snapshotted before the operation starts,
/// so clearing it is not observed: the merge sees all of `s`'s elements. CPython
/// merges from the live table and stops when it empties, giving `[1]` for both.
#[test]
fn set_union_does_not_observe_an_eq_that_clears_the_source_operand() {
    assert_snapshot!(run_repr(EQ_CLEARS_S, "sorted([x.n for x in t | s])"), @"[1, 2]");
    assert_snapshot!(
        run_repr(EQ_CLEARS_S, "t.update(s)\nsorted([x.n for x in t])"),
        @"[1, 2]"
    );
}

/// `isdisjoint` probes rather than walks, so neither engine treats the mutation
/// as a resize — but they answer from different sides of it. Monty keeps the
/// comparison that found `E(1)` in both sets; CPython restarts the probe after
/// the clear, finds nothing left, and calls the sets disjoint.
#[test]
fn set_isdisjoint_answers_before_an_eq_that_clears_the_probed_set() {
    assert_snapshot!(run_repr(EQ_CLEARS_S, "s.isdisjoint(t)"), @"False");
}

/// A dict view's set operators collect the view's own keys through a live walk
/// that hashes each one, so a `__hash__` clearing the dict is caught. CPython
/// probes with each key's stored hash, never calls it, and completes.
#[test]
fn dict_keys_operators_reject_a_hash_that_clears_the_dict() {
    allow_duplicates! {
        for expr in ["d.keys() - s", "d.keys() | s", "d.keys() ^ s", "d.keys().isdisjoint(s)"] {
            let err = run_err(HASH_CLEARS_D, expr);
            assert_eq!(err.exc_type(), ExcType::RuntimeError, "{expr}");
            assert_snapshot!(err.message().unwrap(), @"dictionary changed size during iteration");
        }
    }
}

/// Intersection diverges the other way. Monty always walks the other operand and
/// probes the live dict, so the view's keys are never hashed and the result comes
/// back empty. CPython walks the view whenever the dict is no larger than the
/// other operand, hashes each key into that operand, and raises on the clear.
#[test]
fn dict_keys_intersection_completes_where_cpython_raises() {
    assert_snapshot!(run_repr(HASH_CLEARS_D, "len(d.keys() & s)"), @"0");
}

/// Runs `preamble` followed by `expr` with no limits, returning the exception.
fn run_err(preamble: &str, expr: &str) -> MontyException {
    run(preamble, expr).expect_err("expected the run to raise")
}

/// Runs `preamble` followed by `repr(expr)`, returning the repr as a string.
fn run_repr(preamble: &str, expr: &str) -> String {
    let (head, tail) = expr.rsplit_once('\n').unwrap_or(("", expr));
    let value = run(preamble, &format!("{head}\nrepr({tail})")).expect("expected the run to succeed");
    match value.root_node() {
        MontyNode::String(repr) => repr.clone(),
        other => panic!("expected a string, got {other:?}"),
    }
}

/// Compiles and runs `preamble` followed by `expr` with no resource limits.
fn run(preamble: &str, expr: &str) -> Result<MontyObject, MontyException> {
    let code = format!("{preamble}{expr}\n");
    let mut run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("should parse");
    run.run_no_limits(vec![])
}
