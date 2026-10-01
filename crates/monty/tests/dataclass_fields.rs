//! The metadata `@dataclass` writes into a class namespace, where the
//! behaviour cannot be dual-run against CPython: Monty stringizes annotations,
//! and reads `__dataclass_params__` and `__post_init__` live where CPython
//! bakes them into the methods it generates.
//!
//! Everything the two interpreters agree on lives in
//! `test_cases/dataclass__is_dataclass.py` instead.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::CompileOptions;

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
    let mut run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("code should compile");
    let value = run.run_no_limits(vec![]).expect("code should run");
    let Some(s) = value.as_ref().as_str() else {
        panic!("expected a string, got {value:?}");
    };
    s.to_owned()
}

/// Runs `POINT` followed by `expr` and returns the exception message, falling
/// back to the rendered exception so a message-less failure is reported rather
/// than panicking with its type and traceback lost.
fn expect_error(expr: &str) -> String {
    let code = format!("{POINT}\n{expr}\n");
    let mut run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("code should compile");
    match run.run_no_limits(vec![]) {
        Ok(value) => panic!("expected an exception, got {value:?}"),
        Err(err) => err.message().map_or_else(|| err.to_string(), ToOwned::to_owned),
    }
}

/// CPython's `Field.__repr__` attribute for attribute, bar `type`: annotation
/// text where CPython evaluates it to `<class 'int'>`. The `MISSING` repr is
/// swapped for its name, since it carries the sentinel's `id()`.
#[test]
fn field_repr_renders_type_as_annotation_text() {
    let repr = |name: &str| {
        eval_str(&format!(
            "from dataclasses import MISSING\nrepr(Point.__dataclass_fields__['{name}']).replace(repr(MISSING), 'MISSING')"
        ))
    };
    assert_snapshot!(
        repr("y"),
        @"Field(name='y',type='int',default=5,default_factory=MISSING,init=True,repr=True,hash=None,compare=True,metadata=mappingproxy({}),kw_only=False,doc=None,_field_type=_FIELD)"
    );
    assert_snapshot!(
        repr("x"),
        @"Field(name='x',type='int',default=MISSING,default_factory=MISSING,init=True,repr=True,hash=None,compare=True,metadata=mappingproxy({}),kw_only=False,doc=None,_field_type=_FIELD)"
    );
}

/// The `Field` attributes whose values need an object Monty does not have.
/// `default`/`default_factory` are no longer among them: `MISSING` exists.
#[test]
fn unmodelled_field_attributes_are_not_implemented() {
    for (attr, missing) in [
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

/// The options are read from `__dataclass_params__` at each use, so lending a
/// class another one's params changes how it behaves. CPython's generated
/// methods ignore the swap.
#[test]
fn rebound_params_change_the_options_in_force() {
    let lend_frozen = "
@dataclass(frozen=True)
class Frozen:
    a: int

Point.__dataclass_params__ = Frozen.__dataclass_params__
p = Point(1)
";
    assert_eq!(
        expect_error(&format!("{lend_frozen}p.x = 2")),
        "cannot assign to field 'x'"
    );
    assert_eq!(eval_str(&format!("{lend_frozen}str(hash(p) == hash((1, 5)))")), "True");
}

/// Params rebound to something else leave the class on CPython's default
/// options rather than without any.
#[test]
fn foreign_params_fall_back_to_the_defaults() {
    assert_eq!(
        eval_str("Point.__dataclass_params__ = None\nstr(Point(1) == Point(1))"),
        "True"
    );
    assert_eq!(
        expect_error("Point.__dataclass_params__ = None\nhash(Point(1))"),
        "unhashable type: 'Point'"
    );
}

/// `__post_init__` is looked up at each construction, so a hook attached after
/// decoration runs. CPython decides at decoration and never calls it.
#[test]
fn late_post_init_runs() {
    assert_eq!(
        eval_str("def hook(self):\n    self.y = 99\nPoint.__post_init__ = hook\nstr(Point(1).y)"),
        "99"
    );
}
