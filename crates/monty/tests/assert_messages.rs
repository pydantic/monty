//! Tests for pytest-style assert failure messages — a deliberate CPython
//! divergence (CPython raises an empty `AssertionError`), so these live here
//! rather than in `test_cases/` (which runs every fixture on both interpreters).
//! See `limitations/assert.md`.

use insta::assert_snapshot;
use monty::{CompileOptions, ExcType, MontyException, MontyObject, MontyRepl, MontyRun, NoLimitTracker, PrintWriter};

/// Runs `code` and returns the exception it raises.
fn get_err(code: &str) -> MontyException {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).expect("should compile");
    run.run_no_limits(vec![]).expect_err("expected an exception")
}

/// Runs `code` and returns the failed assert's `AssertionError` message.
fn assert_msg(code: &str) -> String {
    let err = get_err(code);
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    err.message().expect("AssertionError should carry a message").to_owned()
}

#[test]
fn comparison_operators() {
    assert_snapshot!(assert_msg("assert 2 == 5"), @"assert 2 == 5");
    assert_snapshot!(assert_msg("assert 3 != 3"), @"assert 3 != 3");
    assert_snapshot!(assert_msg("assert 5 < 2"), @"assert 5 < 2");
    assert_snapshot!(assert_msg("assert 5 <= 2"), @"assert 5 <= 2");
    assert_snapshot!(assert_msg("assert 2 > 5"), @"assert 2 > 5");
    assert_snapshot!(assert_msg("assert 2 >= 5"), @"assert 2 >= 5");
    assert_snapshot!(assert_msg("x = 5\nassert x is None"), @"assert 5 is None");
    assert_snapshot!(assert_msg("x = None\nassert x is not None"), @"assert None is not None");
    assert_snapshot!(assert_msg("assert 3 in [1, 2]"), @"assert 3 in [1, 2]");
    assert_snapshot!(assert_msg("assert 1 not in [1, 2]"), @"assert 1 not in [1, 2]");
}

#[test]
fn operand_reprs() {
    // Strings keep their quotes (repr, not str).
    assert_snapshot!(assert_msg("assert 'a' == 'b'"), @"assert 'a' == 'b'");
    assert_snapshot!(assert_msg("assert [1, 2] == [3]"), @"assert [1, 2] == [3]");
    assert_snapshot!(assert_msg("assert {'k': 1} == {}"), @"assert {'k': 1} == {}");
    assert_snapshot!(assert_msg("x = None\nassert x == 1"), @"assert None == 1");
}

#[test]
fn falsy_value_fallback() {
    // Non-comparison tests show the falsy value's repr.
    assert_snapshot!(assert_msg("assert []"), @"assert []");
    assert_snapshot!(assert_msg("assert 0"), @"assert 0");
    assert_snapshot!(assert_msg("assert None"), @"assert None");
    assert_snapshot!(assert_msg("assert ''"), @"assert ''");
    // `not` / chained comparisons / boolean ops evaluate to a bool first.
    assert_snapshot!(assert_msg("assert not True"), @"assert False");
    assert_snapshot!(assert_msg("assert 1 < 2 > 3"), @"assert False");
    // `x % n == k` is fused into a ModEq comparison during preparation, so it
    // takes the generic path rather than showing lhs/rhs (see limitations).
    assert_snapshot!(assert_msg("assert 5 % 3 == 0"), @"assert False");
}

#[test]
fn explicit_message_appends_detail() {
    // `assert test, msg` puts the message first, detail on a new line.
    assert_snapshot!(assert_msg("assert 1 == 2, 'my message'"), @r"
    my message
    assert 1 == 2
    ");
    assert_snapshot!(assert_msg("assert [], 'no items'"), @r"
    no items
    assert []
    ");
    assert_snapshot!(assert_msg("assert False, 'msg'"), @r"
    msg
    assert False
    ");
    // Non-str messages are rendered with str().
    assert_snapshot!(assert_msg("assert False, 123"), @r"
    123
    assert False
    ");
}

#[test]
fn message_expression_only_evaluated_on_failure() {
    let code = "
calls = []
def msg():
    calls.append(1)
    return 'boom'
assert 1 == 1, msg()
assert 2 == 2, msg()
len(calls)
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = run.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(0));
}

#[test]
fn passing_asserts_release_retained_operands() {
    // The success path of a comparison assert drops the `Dup2`-retained
    // operands; heap operands in a loop would trip the refcount checks
    // (memory-model-checks / cycle collection) if a Pop were missed.
    let code = "
xs = [1, 2]
for _ in range(100):
    assert xs == [1, 2]
    assert 'a' in 'abc'
    assert xs, 'must not be empty'
len(xs)
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = run.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(2));
}

#[test]
fn operands_evaluated_once() {
    let code = "
calls = []
def side():
    calls.append(1)
    return 0
try:
    assert side() == 1
except AssertionError:
    pass
len(calls)
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = run.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(1));
}

#[test]
fn message_visible_via_str_in_sandbox() {
    let code = "
try:
    assert 1 == 2
except AssertionError as e:
    r = str(e)
r
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = run.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::String("assert 1 == 2".into()));
}

#[test]
fn traceback_shape_unchanged() {
    // The message lands after `AssertionError:`; frames and caret behavior
    // (hidden for assert, like `raise`) are identical to the old bytecode.
    let code = "
def check(v):
    assert v == 99

check(7)
";
    let err = get_err(code);
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 5, in <module>
        check(7)
        ~~~~~~~~
      File "test.py", line 3, in check
        assert v == 99
    AssertionError: assert 7 == 99
    "#);
}

#[test]
fn failing_repr_falls_back_to_bare_error() {
    // A user `__repr__` that raises must not replace the AssertionError.
    let code = "
class Bad:
    def __repr__(self):
        raise ValueError('nope')

assert Bad() == 1
";
    let err = get_err(code);
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), None);
}

#[test]
fn failing_repr_keeps_explicit_message() {
    let code = "
class Bad:
    def __repr__(self):
        raise ValueError('nope')

assert Bad() == 1, 'custom'
";
    let err = get_err(code);
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), Some("custom"));
}

#[test]
fn failing_message_str_keeps_detail() {
    let code = "
class BadStr:
    def __str__(self):
        raise ValueError('nope')

assert 1 == 2, BadStr()
";
    let err = get_err(code);
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), Some("assert 1 == 2"));
}

#[test]
fn operand_reprs_truncated() {
    // Each operand's repr is capped at 120 chars with a `...` suffix.
    let msg = assert_msg("assert list(range(200)) == []");
    assert_snapshot!(msg, @"assert [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 3... == []");
}

#[test]
fn comparison_type_errors_still_raise() {
    // The retained operands don't change comparison error behavior.
    let err = get_err("assert 1 < 'a'");
    assert_eq!(err.exc_type(), ExcType::TypeError);
    assert_snapshot!(
        err.message().unwrap(),
        @"'<' not supported between instances of 'int' and 'str'"
    );
}

#[test]
fn opt_out_restores_cpython_behavior() {
    let options = CompileOptions { assert_messages: false };
    let run = MontyRun::new_with_options("assert 1 == 2".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), None);

    let options = CompileOptions { assert_messages: false };
    let run = MontyRun::new_with_options("assert False, 'msg'".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    assert_eq!(err.message(), Some("msg"));
}

#[test]
fn assert_inside_repl_gets_messages() {
    // The REPL surface (which backs the subprocess workers and thus the
    // Python/JS packages) always compiles with assert messages on.
    let mut repl = MontyRepl::new("repl.py", NoLimitTracker);
    repl.feed_run("x = 3", vec![], PrintWriter::Stdout).unwrap();
    let err = repl
        .feed_run("assert x == 4", vec![], PrintWriter::Stdout)
        .expect_err("assert should fail");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), Some("assert 3 == 4"));
}
