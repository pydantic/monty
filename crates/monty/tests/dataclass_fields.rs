//! The metadata `@dataclass` writes onto a class — the `__dataclass_fields__`
//! mapping with the `Field` objects in it, and `__dataclass_params__` — where
//! the behaviour cannot be dual-run against CPython: Monty stringizes
//! annotations, has no `MISSING` sentinel to render, and reads the options live
//! where CPython bakes them into dunders at decoration.
//!
//! Everything the two interpreters agree on lives in
//! `test_cases/dataclass__is_dataclass.py` instead.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};

const POINT: &str = r"
from dataclasses import dataclass
import typing

@dataclass
class Point:
    x: int
    y: int = 5
    seen: typing.ClassVar[int] = 0
";

/// Runs `POINT` followed by `expr` and returns the string it evaluates to.
fn eval_str(expr: &str) -> String {
    let code = format!("{POINT}\n{expr}\n");
    let run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("code should compile");
    match run.run_no_limits(vec![]).expect("code should run") {
        MontyObject::String(s) => s,
        other => panic!("expected a string, got {other:?}"),
    }
}

/// Runs `POINT` followed by `expr` and returns the exception message, falling
/// back to the rendered exception so a message-less failure is reported rather
/// than panicking with its type and traceback lost.
fn expect_error(expr: &str) -> String {
    let code = format!("{POINT}\n{expr}\n");
    let run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("code should compile");
    match run.run_no_limits(vec![]) {
        Ok(value) => panic!("expected an exception, got {value:?}"),
        Err(err) => err.message().map_or_else(|| err.to_string(), ToOwned::to_owned),
    }
}

/// CPython's `Field.__repr__` attribute for attribute, bar the two spellings
/// Monty cannot produce: `type` is annotation text (CPython evaluates it to
/// `<class 'int'>`) and the `MISSING` sentinel has no object of its own
/// (CPython renders `<dataclasses._MISSING_TYPE object at 0x..>`).
#[test]
fn field_repr_renders_missing_as_a_bare_name() {
    assert_snapshot!(
        eval_str("repr(Point.__dataclass_fields__['y'])"),
        @"Field(name='y',type='int',default=5,default_factory=MISSING,init=True,repr=True,hash=None,compare=True,metadata=mappingproxy({}),kw_only=False,doc=None,_field_type=_FIELD)"
    );
    assert_snapshot!(
        eval_str("repr(Point.__dataclass_fields__['x'])"),
        @"Field(name='x',type='int',default=MISSING,default_factory=MISSING,init=True,repr=True,hash=None,compare=True,metadata=mappingproxy({}),kw_only=False,doc=None,_field_type=_FIELD)"
    );
}

/// A required field's default *is* `MISSING` in CPython; with no such object,
/// Monty reports the gap rather than inventing a value.
#[test]
fn missing_default_is_not_implemented() {
    assert_snapshot!(
        expect_error("Point.__dataclass_fields__['x'].default"),
        @"Field.default is not yet supported, dataclasses.MISSING is not implemented"
    );
}

/// The `Field` attributes whose values need an object Monty does not have.
#[test]
fn unmodelled_field_attributes_are_not_implemented() {
    for (attr, missing) in [
        ("default_factory", "dataclasses.MISSING"),
        ("metadata", "types.MappingProxyType"),
        ("_field_type", "dataclasses._FIELD"),
    ] {
        assert_eq!(
            expect_error(&format!("Point.__dataclass_fields__['y'].{attr}")),
            format!("Field.{attr} is not yet supported, {missing} is not implemented")
        );
    }
}

/// CPython keeps `ClassVar` entries in `__dataclass_fields__` (marked
/// `_FIELD_CLASSVAR`) and filters them in `fields()`. Monty has no field kinds,
/// so the mapping *is* the field list and class variables never enter it.
#[test]
fn classvars_are_absent_from_the_mapping() {
    assert_snapshot!(eval_str("repr(list(Point.__dataclass_fields__))"), @"['x', 'y']");
}

/// Because the options are read live, borrowing a frozen class's params freezes
/// a class whose instances may already cycle — the one way the field-wise hash
/// can re-enter itself, so it must be bounded rather than overflow the host
/// stack. CPython cannot reach this: it hashes with the `__hash__` its
/// decoration generated, so the borrowed params change nothing and `Point`
/// stays unhashable.
///
/// Run against a tightened ceiling: the default depth is bounded only by the
/// interpreter thread's stack, which a test thread does not have.
#[test]
fn a_retro_frozen_cycle_hits_the_recursion_limit() {
    let code = format!(
        "{POINT}
@dataclass(frozen=True)
class Frozen:
    v: int

cyclic = Point(1)
cyclic.y = cyclic
Point.__dataclass_params__ = Frozen.__dataclass_params__
hash(cyclic)
"
    );
    let run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("code should compile");
    let limits = ResourceLimits::default().max_recursion_depth(10);
    let err = run
        .run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .expect_err("hashing a cyclic instance should exceed the recursion depth");

    assert_eq!(err.exc_type(), ExcType::RecursionError);
    assert_snapshot!(err.message().expect("RecursionError carries a message"), @"maximum recursion depth exceeded");
}
