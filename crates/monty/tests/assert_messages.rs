//! Tests for pytest-style assert failure messages — a deliberate CPython
//! divergence (CPython raises an empty `AssertionError`), so these live here
//! rather than in `test_cases/` (which runs every fixture on both interpreters).
//! See `limitations/assert.md`.

use insta::assert_snapshot;
use monty::{
    AssertMessageAnnotations, CompileOptions, ExcType, LimitedTracker, MontyException, MontyObject, MontyRepl,
    MontyRun, NoLimitTracker, PrintWriter, ResourceLimits,
};

/// Runs `code` and returns the exception it raises.
fn get_err(code: &str) -> MontyException {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should compile");
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
}

#[test]
fn false_test_value_carries_no_message() {
    // Bool `False` adds no useful detail, so it matches CPython's bare error.
    for code in [
        "assert False",
        "assert not True",
        "assert 1 < 2 > 3",
        "x = False\nassert x",
    ] {
        let err = get_err(code);
        assert_eq!(err.exc_type(), ExcType::AssertionError);
        assert_eq!(err.message(), None, "expected no message for {code:?}");
    }
}

#[test]
fn computed_operands_show_their_values() {
    // Operands are arbitrary expressions; the message shows what they
    // evaluated to, not the source text that produced them.
    assert_snapshot!(assert_msg("assert 5 % 3 == 0"), @"assert 2 == 0");
    assert_snapshot!(assert_msg("x = 7\nassert x % 4 == 1"), @"assert 3 == 1");
    assert_snapshot!(assert_msg("assert len('abc') == 4"), @"assert 3 == 4");
    assert_snapshot!(assert_msg("assert 5 % 3 == 0, 'not divisible'"), @r"
    not divisible
    assert 2 == 0
    ");
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
    assert_snapshot!(assert_msg("assert False, 'msg'"), @"msg");
    assert_snapshot!(assert_msg("assert False, 123"), @"123");
    // An empty message is treated as absent: detail only, no leading newline.
    assert_snapshot!(assert_msg("assert 1 == 2, ''"), @"assert 1 == 2");
    // Empty message with no detail (`False`) yields a message-less error.
    let err = get_err("assert False, ''");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), None);
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
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let result = run.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::Int(0));
}

#[test]
fn passing_asserts_release_retained_operands() {
    // Heap operands in a loop catch missed drops on the success path.
    let code = "
xs = [1, 2]
for _ in range(100):
    assert xs == [1, 2]
    assert 'a' in 'abc'
    assert xs, 'must not be empty'
len(xs)
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let result = run.run_no_limits(vec![]).unwrap();
    assert_eq!(result, MontyObject::String("assert 1 == 2".into()));
}

#[test]
fn traceback_shape_unchanged() {
    // Frames and caret behavior should match the old bytecode.
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
    // Each operand's repr is capped at 120 chars with a `…` suffix.
    let msg = assert_msg("assert list(range(200)) == []");
    assert_snapshot!(msg, @"assert [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 3… == []");
}

#[test]
fn custom_truncation_limit() {
    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::from_max_bytes(10),
    };
    let run = MontyRun::new("assert list(range(200)) == []".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_snapshot!(err.message().unwrap(), @"assert [0, 1, 2, … == []");

    // A limit above the repr length leaves it untouched.
    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::from_max_bytes(10_000),
    };
    let run = MontyRun::new("assert list(range(50)) == []".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    let msg = err.message().unwrap();
    assert!(msg.ends_with("48, 49] == []"), "{msg}");
}

#[test]
fn huge_operand_repr_is_streamed_not_materialized() {
    // A ~2MB repr under a 1MB memory limit: streaming stops at the cap, so
    // the AssertionError stays catchable instead of a terminal MemoryError.
    let code = "
xs = ['x' * 500] * 4000
try:
    assert xs == []
    r = 'no error'
except AssertionError as e:
    r = str(e)
r[:10] + '|' + r[-9:] + '|' + str(len(r))
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::new().max_memory(1_048_576);
    let result = run
        .run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .expect("AssertionError must stay catchable under the memory limit");
    // 7 ("assert ") + 121 (120-char repr + `…`) + 6 (" == []") = 134 chars.
    assert_eq!(result, MontyObject::String("assert ['x|xx… == []|134".into()));
}

#[test]
fn truncation_cuts_on_char_boundaries() {
    // The cap is in bytes but never splits a char: a 5-byte budget keeps
    // `'日` (4 bytes); the next 3-byte char is dropped whole.
    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::from_max_bytes(5),
    };
    let run = MontyRun::new("assert '日本語です' == ''".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    assert_snapshot!(err.message().unwrap(), @"assert '日… == ''");
}

#[test]
fn failing_repr_mid_stream_falls_back_to_bare_error() {
    // A `__repr__` raising after `[1, ` was already written is a genuine
    // error, not a truncation — the whole detail is dropped.
    let code = "
class Bad:
    def __repr__(self):
        raise ValueError('nope')

assert [1, Bad()] == 2
";
    let err = get_err(code);
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), None);
}

#[test]
fn custom_limit_survives_repl_snippets() {
    // The runtime limit rides with the compiled program, so every snippet
    // fed to a session (and any snapshot of it) formats the same way.
    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::from_max_bytes(5),
    };
    let mut repl = MontyRepl::new("repl.py", NoLimitTracker, options);
    repl.feed_run("x = 'abcdefghij'", vec![], PrintWriter::Stdout).unwrap();
    let err = repl
        .feed_run("assert x == ''", vec![], PrintWriter::Stdout)
        .expect_err("assert should fail");
    assert_eq!(err.message(), Some("assert 'abcd… == ''"));
}

#[test]
fn zero_limit_means_off_not_a_zero_length_repr() {
    // `0` encodes `Off` on the wire, so it must never reach the compiler as an
    // enabled-but-empty limit: that would annotate `assert … == …` in-process
    // while a worker ran the same session with annotations off.
    assert_eq!(
        AssertMessageAnnotations::from_max_bytes(0),
        AssertMessageAnnotations::Off
    );
    assert!(!AssertMessageAnnotations::from_max_bytes(0).enabled());
    assert_eq!(AssertMessageAnnotations::Off.max_bytes(), 0);

    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::from_max_bytes(0),
    };
    let run = MontyRun::new("assert 2 == 5".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), None);
}

#[test]
fn forged_snapshot_cannot_smuggle_in_a_zero_limit() {
    // Dumps are untrusted. The `MaxBytes(1)` case pins the encoding (variant
    // index 1, then the u32) so the rejection can't pass for another reason.
    let valid: AssertMessageAnnotations = postcard::from_bytes(&[1u8, 1u8]).expect("MaxBytes(1) should decode");
    assert_eq!(valid, AssertMessageAnnotations::from_max_bytes(1));
    postcard::from_bytes::<AssertMessageAnnotations>(&[1u8, 0u8]).expect_err("MaxBytes(0) must not decode");
}

#[test]
fn max_bytes_round_trips_through_the_wire_encoding() {
    // Every representable value survives `Configure`, so in-process and worker
    // sessions always agree.
    for value in [
        AssertMessageAnnotations::Off,
        AssertMessageAnnotations::default(),
        AssertMessageAnnotations::from_max_bytes(1),
        AssertMessageAnnotations::from_max_bytes(u32::MAX),
    ] {
        assert_eq!(AssertMessageAnnotations::from_max_bytes(value.max_bytes()), value);
    }
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
    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::Off,
    };
    let run = MontyRun::new("assert 1 == 2".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), None);

    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::Off,
    };
    let run = MontyRun::new("assert False, 'msg'".to_owned(), "test.py", vec![], options).unwrap();
    let err = run.run_no_limits(vec![]).expect_err("assert should fail");
    assert_eq!(err.message(), Some("msg"));
}

#[test]
fn assert_inside_repl_gets_messages() {
    let mut repl = MontyRepl::new("repl.py", NoLimitTracker, CompileOptions::default());
    repl.feed_run("x = 3", vec![], PrintWriter::Stdout).unwrap();
    let err = repl
        .feed_run("assert x == 4", vec![], PrintWriter::Stdout)
        .expect_err("assert should fail");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), Some("assert 3 == 4"));
}

#[test]
fn repl_opt_out_applies_to_every_snippet() {
    let options = CompileOptions {
        assert_message_annotations: AssertMessageAnnotations::Off,
    };
    let mut repl = MontyRepl::new("repl.py", NoLimitTracker, options);
    repl.feed_run("x = 3", vec![], PrintWriter::Stdout).unwrap();
    let err = repl
        .feed_run("assert x == 4", vec![], PrintWriter::Stdout)
        .expect_err("assert should fail");
    assert_eq!(err.exc_type(), ExcType::AssertionError);
    assert_eq!(err.message(), None);
}
