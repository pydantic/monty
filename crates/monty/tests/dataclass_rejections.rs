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
    let mut run =
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("code should compile");
    match run.run_no_limits(vec![]) {
        Ok(value) => panic!("expected an exception, got {value:?}"),
        Err(err) => err.to_string(),
    }
}

/// Runs `code` and returns just the exception message, for cases checked in a
/// loop where a per-case traceback snapshot would say nothing extra.
fn expect_message(code: &str) -> String {
    let mut run =
        MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("code should compile");
    match run.run_no_limits(vec![]) {
        Ok(value) => panic!("expected an exception, got {value:?}"),
        Err(err) => err.message().map_or_else(|| err.to_string(), ToOwned::to_owned),
    }
}

#[test]
fn unimplemented_option_names_itself() {
    // Every option but the two needing `__slots__` and weakrefs is implemented;
    // those are refused by name rather than accepted and ignored.
    let err = expect_error(
        r"
from dataclasses import dataclass

@dataclass(slots=True)
class F:
    x: int
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 4, in <module>
        @dataclass(slots=True)
         ~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: dataclass() does not yet support the slots option
    "#);

    let err =
        expect_message("from dataclasses import dataclass\n@dataclass(weakref_slot=True)\nclass F:\n    x: int\n");
    assert_eq!(err, "dataclass() does not yet support the weakref_slot option");
}

/// `field()` refuses the argument it cannot honour, by name and at the call
/// itself: `metadata` needs `types.MappingProxyType`, which Monty does not have.
#[test]
fn unimplemented_field_argument_names_itself() {
    let err = expect_error(
        r"
from dataclasses import dataclass, field

@dataclass
class F:
    x: int = field(metadata={}, default=1)
",
    );
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 5, in <module>
        class F:
            x: int = field(metadata={}, default=1)
      File "test.py", line 6, in F
        x: int = field(metadata={}, default=1)
                 ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: field() does not yet support the metadata argument
    "#);
}

/// The decorator keys fields by interned name, and Monty cannot intern new text
/// at runtime, so `make_dataclass` refuses a field name the source never spells.
#[test]
fn make_dataclass_refuses_a_name_built_at_runtime() {
    let err = expect_message("from dataclasses import make_dataclass\nmake_dataclass('C', [''.join(['q', 'zz'])])\n");
    assert_snapshot!(err, @"make_dataclass() does not yet support the field name 'qzz', which appears nowhere in the source code");
}

/// CPython raises `ValueError` before it builds the `Field` at all, so it never
/// looks at the other arguments. Monty reads the flags to refuse them, and
/// `NotImplemented` is the one value whose truthiness raises — that must not
/// preempt the error CPython reports.
#[test]
fn both_defaults_outrank_an_unreadable_flag() {
    let err =
        expect_message("from dataclasses import field\nfield(default=1, default_factory=int, init=NotImplemented)\n");
    assert_eq!(err, "cannot specify both default and default_factory");
}

/// A refusal that fires *after* the default was captured must still release it.
/// Reading a flag's truthiness raises for `NotImplemented`, which is exactly the
/// window between capturing the default and allocating the `Field` that owns it.
#[test]
#[cfg(feature = "ref-count-return")]
fn an_unreadable_flag_releases_the_captured_default() {
    let mut run = MontyRun::new(
        r"
from dataclasses import field

shared = (1, 2)
try:
    field(default=shared, init=NotImplemented)
except TypeError:
    pass
"
        .to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .expect("code should compile");
    let output = run.run_ref_counts(vec![]).expect("should not raise");
    // The global binding is the only reference left; a leaked capture reads 2.
    assert_eq!(output.counts.get("shared"), Some(&1));
    assert!(output.unreachable.is_empty(), "leaked {:?}", output.unreachable);
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
    let mut run = MontyRun::new(
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
    let mut run = MontyRun::new(
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
