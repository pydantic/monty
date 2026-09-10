//! `@dataclass` class bodies Monty rejects, where the message cannot be
//! dual-run: CPython either accepts the body or rejects it with different
//! wording (its class names are module-qualified, Monty's are bare).
//!
//! When the corresponding feature lands, delete the case rather than updating
//! the snapshot: the body should then execute, not raise.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::CompileOptions;

/// Runs `code` and returns the exception message, panicking if it succeeds.
fn expect_error(code: &str) -> String {
    let run =
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("code should compile");
    match run.run_no_limits(vec![]) {
        Ok(value) => panic!("expected an exception, got {value:?}"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn post_init_is_rejected() {
    // CPython calls `__post_init__`; Monty would silently skip it, leaving the
    // instance half-initialised, so the class is refused instead.
    let err = expect_error(
        r"
from dataclasses import dataclass

@dataclass
class R:
    x: int
    def __post_init__(self):
        self.x = 99
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 4, in <module>
        @dataclass
         ~~~~~~~~~
    NotImplementedError: dataclass() does not yet support __post_init__ in a class body, which would be silently skipped
    "#);
}

#[test]
fn initvar_is_rejected() {
    // Annotations are stringized and never evaluated, so `InitVar` need not be
    // imported to appear here — it would silently become an ordinary field.
    let err = expect_error(
        r"
from dataclasses import dataclass

@dataclass
class Q:
    x: int
    tmp: InitVar[int]
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 4, in <module>
        @dataclass
         ~~~~~~~~~
    NotImplementedError: dataclass() does not yet support InitVar (field tmp), which would become an ordinary field
    "#);
}

#[test]
fn unimplemented_option_names_itself() {
    // `eq` and `frozen` are implemented; the rest are refused by name rather
    // than accepted and ignored.
    let err = expect_error(
        r"
from dataclasses import dataclass

@dataclass(order=True)
class F:
    x: int
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 4, in <module>
        @dataclass(order=True)
         ~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: dataclass() does not yet support the order option
    "#);
}

/// A quoted `ClassVar` is excluded from the fields, matching what CPython does
/// when it can resolve `ClassVar` in the defining module.
///
/// Not in `test_cases/`: CPython's string-annotation check consults the class's
/// module namespace, which the dual-run harness's `exec` does not provide, so
/// CPython's answer there differs from running the same file as a script.
/// Monty's match is purely textual and therefore stable.
#[test]
fn quoted_classvar_is_excluded() {
    let run = MontyRun::new(
        r#"
from dataclasses import dataclass
from typing import ClassVar

@dataclass
class C:
    x: int
    quoted: "ClassVar[int]" = 2

repr(C(7))
"#
        .to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .expect("code should compile");
    let value = run.run_no_limits(vec![]).expect("should not raise");
    let repr: String = value.as_ref().try_into().expect("a str result");
    assert_snapshot!(repr, @"C(x=7)");
}

/// A user `__repr__` is not rejected: it *is* dispatched, so Monty already
/// matches CPython's precedence for it.
#[test]
fn user_repr_in_a_dataclass_body_is_allowed() {
    let run = MontyRun::new(
        r"
from dataclasses import dataclass

@dataclass
class C:
    x: int
    def __repr__(self):
        return 'CUSTOM'

repr(C(1))
"
        .to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .expect("code should compile");
    let value = run.run_no_limits(vec![]).expect("should not raise");
    let repr: String = value.as_ref().try_into().expect("a str result");
    assert_snapshot!(repr, @"CUSTOM");
}

/// CPython's mutable-default rule is hashability, so an instance of a class
/// that opts out of hashing is rejected like a `list` would be.
#[test]
fn unhashable_instance_default_is_rejected() {
    let err = expect_error(
        r"
from dataclasses import dataclass

class NoHash:
    __hash__ = None

@dataclass
class A:
    v: NoHash = NoHash()
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 7, in <module>
        @dataclass
         ~~~~~~~~~
    ValueError: mutable default <class 'NoHash'> for field v is not allowed: use default_factory
    "#);
}

/// The same rule reached the other way: defining `__eq__` makes a class
/// unhashable, so an instance of it is not a valid default either.
#[test]
fn eq_defining_instance_default_is_rejected() {
    let err = expect_error(
        r"
from dataclasses import dataclass

class Eq:
    def __eq__(self, other):
        return True

@dataclass
class B:
    v: Eq = Eq()
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 8, in <module>
        @dataclass
         ~~~~~~~~~
    ValueError: mutable default <class 'Eq'> for field v is not allowed: use default_factory
    "#);
}

#[test]
fn non_class_is_reported_directly() {
    // CPython's decorator instead trips over `cls.__module__` on the way in, so
    // the direct report is Monty's, through both spellings of the decorator.
    let err = expect_error(
        r"
from dataclasses import dataclass

dataclass(5)
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 4, in <module>
        dataclass(5)
        ~~~~~~~~~~~~
    TypeError: dataclass() should be called on a class, not 'int'
    "#);

    let err = expect_error(
        r"
from dataclasses import dataclass

dataclass(frozen=True)('nope')
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 4, in <module>
        dataclass(frozen=True)('nope')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    TypeError: dataclass() should be called on a class, not 'str'
    "#);
}
