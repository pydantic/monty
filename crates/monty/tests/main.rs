use std::mem;

use monty::MontyRun;
use monty_types::{CompileOptions, DictPairs, MontyClassInstance, MontyClassType, MontyObject, MontyUuid};

/// Test we can reuse exec without borrow checker issues.
#[test]
fn repeat_exec() {
    let ex = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);
}

#[test]
fn test_get_interned_string() {
    let ex = MontyRun::new("'foobar'".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: String = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, "foobar");

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: String = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, "foobar");
}

/// Test that calling a method on a host class instance in standard execution
/// mode (without iter/external function support) returns a NotImplementedError.
/// This exercises the `FrameExit::MethodCall` path in `frame_exit_to_object`.
#[test]
fn class_instance_method_call_in_standard_mode_errors() {
    let point = MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: MontyClassType {
            name: "Point".to_string(),
            id: MontyUuid::from_u128(1),
            host_defined: true,
            is_dataclass: true,
            attrs: DictPairs::default(),
        },
        instance_id: MontyUuid::from_u128(2),
        attrs: vec![
            (MontyObject::String("x".to_string()), MontyObject::Int(1)),
            (MontyObject::String("y".to_string()), MontyObject::Int(2)),
        ]
        .into(),
    }));

    let ex = MontyRun::new(
        "point.sum()".to_owned(),
        "test.py",
        vec!["point".to_string()],
        CompileOptions::default(),
    )
    .unwrap();

    let err = ex.run_no_limits(vec![point]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Method call 'sum' not implemented with standard execution"),
        "Expected NotImplementedError for method call, got: {msg}"
    );
}

/// Test that subscript augmented matrix multiplication reports the dedicated
/// unsupported-operation compile error.
///
/// CPython supports `@=` syntax, so the comparative Python test-case suite
/// cannot cover Monty's current compile-time rejection of this operator. Keep
/// this as a Rust-side regression test until matrix multiplication support
/// exists.
#[test]
fn subscript_augassign_matmul_reports_not_supported() {
    let err = MontyRun::new(
        "d = {'x': 1}\nd['x'] @= 2".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 2\n    d['x'] @= 2\n    ~~~~~~\nSyntaxError: matrix multiplication augmented assignment (@=) is not yet supported"
    );
}

/// Multiline traceback previews dedent by the common leading-whitespace
/// *prefix* of the displayed lines; with mixed tab/space indentation there is
/// no common prefix, so lines keep their original indentation (matching
/// CPython) rather than having unrelated whitespace blindly stripped. Kept as
/// a Rust-side test because CPython adds caret anchors to the `in C` frame
/// that Monty omits, so the comparative test-case suite cannot cover it.
#[test]
fn multiline_preview_mixed_indentation_not_dedented() {
    let code = "if True:\n    class C:\n        x = (1 /\n\t0)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let err = ex.run_no_limits(vec![]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 2, in <module>\n        class C:\n            x = (1 /\n    \t0)\n  File \"test.py\", line 3, in C\n            x = (1 /\n    \t0)\nZeroDivisionError: division by zero"
    );
}

/// A class whose `__init__` is bound to an external function cannot suspend:
/// non-plain-function `__init__` runs synchronously via `evaluate_function`,
/// which cannot yield to the host, so the call raises `NotImplementedError`
/// (documented in `limitations/classes.md`). Kept as a Rust-side test because
/// on CPython the external is a real function and construction would succeed,
/// so the comparative test-case suite cannot cover it.
#[test]
fn external_function_as_init_raises_not_implemented() {
    let code = "class Foo:\n    __init__ = ext_fn\n\nFoo()";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::Function {
            name: "ext_fn".to_owned(),
            docstring: None,
        }])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 4, in <module>\n    Foo()\n    ~~~~~\nNotImplementedError: __init__: external function 'ext_fn' is not yet supported in this context"
    );
}

/// `functools.reduce` calls its function through `evaluate_function`, which
/// cannot suspend, so an external one raises `NotImplementedError` (documented
/// in `limitations/functools.md`). Rust-side for the same reason as
/// `external_function_as_init_raises_not_implemented`: on CPython the external
/// is a real function and the reduction would succeed.
#[test]
fn external_function_in_reduce_raises_not_implemented() {
    let code = "import functools\n\nfunctools.reduce(ext_fn, [1, 2, 3])";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::Function {
            name: "ext_fn".to_owned(),
            docstring: None,
        }])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 3, in <module>\n    functools.reduce(ext_fn, [1, 2, 3])\n    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\nNotImplementedError: reduce(): external function 'ext_fn' is not yet supported in this context"
    );
}

/// A user `__next__` calling an external function cannot suspend: like
/// `__repr__`/`__str__` it runs synchronously via `evaluate_function`, so the
/// call raises `NotImplementedError` at the `ext_fn()` call site inside
/// `__next__` (see `limitations/classes.md`). Rust-side for the same reason as
/// `external_function_as_init_raises_not_implemented`: on CPython the external
/// is a real function and the loop would succeed.
#[test]
fn external_function_in_next_raises_not_implemented() {
    let code = "class Foo:\n    def __iter__(self):\n        return self\n\n    def __next__(self):\n        return ext_fn()\n\nfor _x in Foo():\n    pass";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::Function {
            name: "ext_fn".to_owned(),
            docstring: None,
        }])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 6, in __next__\n    return ext_fn()\n           ~~~~~~~~\nNotImplementedError: __next__: external function 'ext_fn' is not yet supported in this context"
    );
}

/// Rejected suspensions are raised inside the key function's frame, where an
/// ordinary `try`/`except` can catch them and let the sort complete.
#[test]
fn not_implemented_in_sort_key_catchable_inside_key_fn() {
    let code = "
def key_fn(x):
    try:
        ext_fn()
    except NotImplementedError:
        return -x
    return 0

sorted([1, 2, 3], key=key_fn)
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Function {
            name: "ext_fn".to_owned(),
            docstring: None,
        }])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![MontyObject::Int(3), MontyObject::Int(2), MontyObject::Int(1)])
    );
}

/// An uncaught key-function error returns to the sorting call before an outer
/// handler runs, preserving the synchronous evaluation boundary.
#[test]
fn not_implemented_in_sort_key_catchable_outside_key_fn() {
    let code = "
seen = []

def key_fn(x):
    seen.append(x)
    ext_fn()

try:
    sorted([1, 2], key=key_fn)
except NotImplementedError:
    seen.append('caught')
seen.append('after')
seen
";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = ex
        .run_no_limits(vec![MontyObject::Function {
            name: "ext_fn".to_owned(),
            docstring: None,
        }])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::List(vec![
            MontyObject::Int(1),
            MontyObject::String("caught".to_owned()),
            MontyObject::String("after".to_owned()),
        ])
    );
}

/// Rejected-suspension errors identify `list.sort()` rather than `sorted()`.
#[test]
fn not_implemented_in_list_sort_key_names_sort() {
    let code = "[1, 2].sort(key=lambda x: ext_fn())";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::Function {
            name: "ext_fn".to_owned(),
            docstring: None,
        }])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 1, in <lambda>\n    [1, 2].sort(key=lambda x: ext_fn())\n                              ~~~~~~~~\nNotImplementedError: sort() key argument: external function 'ext_fn' is not yet supported in this context"
    );
}

/// The `itertools` adaptors that apply a callable drive it through
/// `evaluate_function`, so one reaching an external function cannot suspend and
/// raises `NotImplementedError` (see `limitations/itertools.md`). Rust-side for
/// the same reason as the tests above: on CPython the external is an ordinary
/// function and the call would succeed.
///
/// Both call sites are covered — the predicate helper shared by `takewhile`,
/// `dropwhile` and `filterfalse`, and `starmap`, which calls its function
/// itself and so names itself in the error separately.
#[test]
fn external_function_as_itertools_callable_raises_not_implemented() {
    for (call, adaptor) in [
        ("itertools.takewhile(ext_fn, [1])", "takewhile"),
        ("itertools.starmap(ext_fn, [(1,)])", "starmap"),
    ] {
        let expr = format!("list({call})");
        let code = format!("import itertools\n\n{expr}");
        let ex = MontyRun::new(code, "test.py", vec!["ext_fn".to_owned()], CompileOptions::default()).unwrap();
        let err = ex
            .run_no_limits(vec![MontyObject::Function {
                name: "ext_fn".to_owned(),
                docstring: None,
            }])
            .unwrap_err();
        let carets = "~".repeat(expr.len());
        assert_eq!(
            err.to_string(),
            format!(
                "Traceback (most recent call last):\n  File \"test.py\", line 3, in <module>\n    {expr}\n    {carets}\nNotImplementedError: {adaptor}(): external function 'ext_fn' is not yet supported in this context"
            )
        );
    }
}

/// The 3-arg `type()` form rejects non-empty bases because Monty classes
/// cannot inherit (documented in `limitations/classes.md`). Kept as a
/// Rust-side test because CPython accepts bases, so the comparative
/// test-case suite cannot cover the divergence.
#[test]
fn dynamic_type_with_bases_raises_type_error() {
    let code = "type('A', (int,), {})";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let err = ex.run_no_limits(vec![]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 1, in <module>\n    type('A', (int,), {})\n    ~~~~~~~~~~~~~~~~~~~~~\nTypeError: type() bases are not supported"
    );
}

/// The 3-arg `type()` form rejects non-string namespace keys with a
/// `TypeError` — CPython only emits a `RuntimeWarning`, and Monty has no
/// warnings machinery, so silently accepting them would hide the mistake
/// (documented in `limitations/classes.md`). Kept as a Rust-side test
/// because CPython succeeds here, so the comparative test-case suite
/// cannot cover the divergence.
#[test]
fn dynamic_type_with_non_string_key_raises_type_error() {
    let code = "type('A', (), {1: 'one'})";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let err = ex.run_no_limits(vec![]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 1, in <module>\n    type('A', (), {1: 'one'})\n    ~~~~~~~~~~~~~~~~~~~~~~~~~\nTypeError: non-string key (int) in the namespace of class 'A'"
    );
}

// === Instance output-conversion tests ===
// Sandbox-defined class instances convert structurally to `ClassInstance`
// values: a user `__repr__` never runs during conversion, so it cannot mutate
// the containing collection. These containers keep all elements, and
// `Evil.__repr__` never fires.

/// Structured `ClassInstance` a sandbox `Evil()` instance converts to.
fn evil_instance() -> MontyObject {
    MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: MontyClassType {
            name: "Evil".to_owned(),
            id: MontyUuid::from_u128(0xE0),
            host_defined: false,
            is_dataclass: false,
            attrs: DictPairs::default(),
        },
        instance_id: MontyUuid::from_u128(0xE1),
        attrs: vec![].into(),
    }))
}

/// Replaces the worker-generated (random) class/instance uuids in `obj` with the
/// deterministic ids [`evil_instance`] uses, so structural comparison works.
fn normalize_instance_uuids(obj: &mut MontyObject) {
    match obj {
        MontyObject::ClassInstance(instance) => {
            let MontyClassInstance {
                class_type,
                instance_id,
                attrs,
            } = instance.as_mut();
            class_type.id = MontyUuid::from_u128(0xE0);
            *instance_id = MontyUuid::from_u128(0xE1);
            let pairs = mem::replace(attrs, DictPairs::from(vec![]))
                .into_iter()
                .map(|(mut key, mut value)| {
                    normalize_instance_uuids(&mut key);
                    normalize_instance_uuids(&mut value);
                    (key, value)
                })
                .collect::<Vec<_>>();
            *attrs = pairs.into();
        }
        MontyObject::List(items)
        | MontyObject::Tuple(items)
        | MontyObject::Set(items)
        | MontyObject::FrozenSet(items) => items.iter_mut().for_each(normalize_instance_uuids),
        MontyObject::Dict(pairs) => {
            let normalized = mem::replace(pairs, DictPairs::from(vec![]))
                .into_iter()
                .map(|(mut key, mut value)| {
                    normalize_instance_uuids(&mut key);
                    normalize_instance_uuids(&mut value);
                    (key, value)
                })
                .collect::<Vec<_>>();
            *pairs = normalized.into();
        }
        _ => {}
    }
}

#[test]
fn output_list_with_nested_instance() {
    let code = "\
class Evil:
    def __repr__(self):
        lst.clear()
        return 'evil'

lst = [Evil(), 1, 2]
lst";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut result = ex.run_no_limits(vec![]).unwrap();
    normalize_instance_uuids(&mut result);
    assert_eq!(
        result,
        MontyObject::List(vec![evil_instance(), MontyObject::Int(1), MontyObject::Int(2)])
    );
}

#[test]
fn output_dict_with_nested_instance() {
    let code = "\
class Evil:
    def __repr__(self):
        d.clear()
        return 'evil'

d = {'k': Evil(), 'a': 1}
d";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut result = ex.run_no_limits(vec![]).unwrap();
    normalize_instance_uuids(&mut result);
    assert_eq!(
        result,
        MontyObject::Dict(
            vec![
                (MontyObject::String("k".to_owned()), evil_instance()),
                (MontyObject::String("a".to_owned()), MontyObject::Int(1)),
            ]
            .into()
        )
    );
}

#[test]
fn output_deque_with_nested_instance() {
    let code = "\
from collections import deque

class Evil:
    def __repr__(self):
        d.clear()
        return 'evil'

d = deque([Evil(), 1, 2])
d";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut result = ex.run_no_limits(vec![]).unwrap();
    normalize_instance_uuids(&mut result);
    assert_eq!(
        result,
        MontyObject::List(vec![evil_instance(), MontyObject::Int(1), MontyObject::Int(2)])
    );
}
