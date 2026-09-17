use monty::{MontyRun, RunProgress};
use monty_types::{
    CompileOptions, ExcType, MontyNode, MontyObject, MontyUuid, NameLookupResult, PrintWriter, ResourceTracker,
};

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

/// Replacement fields are synchronous, so an OS-backed attribute cannot yield
/// to the host and must fail before the call escapes the formatter.
#[test]
fn str_format_os_attribute_reports_suspension_limit() {
    let ex = MontyRun::new(
        "import os\n'{0.environ}'.format(os)".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();

    let err = ex.run_no_limits(vec![]).unwrap_err();
    assert_eq!(err.exc_type(), ExcType::NotImplementedError);
    assert_eq!(err.message(), Some("str.format attribute access cannot suspend"));
}

/// Test that calling a method on a host class instance in standard execution
/// mode (without iter/external function support) returns a NotImplementedError.
/// This exercises the `FrameExit::MethodCall` path in `frame_exit_to_object`.
#[test]
fn class_instance_method_call_in_standard_mode_errors() {
    let point = MontyObject::class_instance(
        MontyObject::class_type("Point".to_string(), MontyUuid::from_u128(1), true, true, []),
        MontyUuid::from_u128(2),
        vec![
            (MontyObject::string("x".to_string()), MontyObject::int(1)),
            (MontyObject::string("y".to_string()), MontyObject::int(2)),
        ],
    );

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
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
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
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 3, in <module>\n    functools.reduce(ext_fn, [1, 2, 3])\n    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\nNotImplementedError: reduce(): external function 'ext_fn' is not yet supported in this context"
    );
}

/// A `__deepcopy__` reaching an external function cannot suspend either: the
/// hook runs through `evaluate_function` like any other dunder, so the copy
/// raises `NotImplementedError` at the `ext_fn()` call site inside the hook
/// (documented in `limitations/copy.md`). Rust-side for the same reason as the
/// tests above: on CPython the external is an ordinary function and the copy
/// would succeed.
#[test]
fn external_function_in_deepcopy_raises_not_implemented() {
    let code = "import copy\n\n\nclass Foo:\n    def __deepcopy__(self, memo):\n        return ext_fn()\n\n\ncopy.deepcopy(Foo())";
    let ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::function("ext_fn", None)])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Traceback (most recent call last):\n  File \"test.py\", line 6, in __deepcopy__\n    return ext_fn()\n           ~~~~~~~~\nNotImplementedError: __deepcopy__: external function 'ext_fn' is not yet supported in this context"
    );
}

/// A cached *host* function is never cached: the call suspends to the host and
/// its result comes back outside the frame that would have stored it, so every
/// call reaches the host again (documented in `limitations/functools.md`).
/// Rust-side because on CPython the external is a real function and the second
/// call would be a hit.
#[test]
fn lru_cache_around_an_external_function_does_not_cache() {
    let code = "\
import functools

cached = functools.cache(ext_fn)
first = cached(1)
[first, cached(1), cached.cache_info().hits, cached.cache_info().misses]";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();

    // Answer the name lookup for `ext_fn`, then each call it makes.
    let mut calls = 0;
    let result = loop {
        progress = match progress {
            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                lookup
                    .resume(
                        NameLookupResult::Value(MontyObject::function(name, None)),
                        PrintWriter::Stdout,
                    )
                    .unwrap()
            }
            RunProgress::FunctionCall(call) => {
                calls += 1;
                call.resume(MontyObject::int(100), PrintWriter::Stdout).unwrap()
            }
            RunProgress::Complete(result) => break result,
            other => panic!("unexpected suspension: {other:?}"),
        };
    };

    assert_eq!(calls, 2, "the host function should be called twice, not cached");
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::int(100),
            MontyObject::int(100),
            MontyObject::int(0),
            MontyObject::int(2),
        ])
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
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
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
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::list([MontyObject::int(3), MontyObject::int(2), MontyObject::int(1)])
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
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
        .unwrap();
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::int(1),
            MontyObject::string("caught".to_owned()),
            MontyObject::string("after".to_owned()),
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
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
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
/// Every call site is covered — the predicate helper shared by `takewhile`,
/// `dropwhile` and `filterfalse`, plus `starmap`, `accumulate` and `groupby`,
/// which each call their callable themselves and so name themselves in the
/// error. `accumulate` needs two items, since the first is yielded untouched.
#[test]
fn external_function_as_itertools_callable_raises_not_implemented() {
    for (call, adaptor) in [
        ("itertools.takewhile(ext_fn, [1])", "takewhile"),
        ("itertools.starmap(ext_fn, [(1,)])", "starmap"),
        ("itertools.accumulate([1, 2], ext_fn)", "accumulate"),
        ("itertools.groupby([1], ext_fn)", "groupby"),
    ] {
        let expr = format!("list({call})");
        let code = format!("import itertools\n\n{expr}");
        let ex = MontyRun::new(code, "test.py", vec!["ext_fn".to_owned()], CompileOptions::default()).unwrap();
        let err = ex
            .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
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
    MontyObject::class_instance(
        MontyObject::class_type("Evil".to_owned(), MontyUuid::from_u128(0xE0), false, false, []),
        MontyUuid::from_u128(0xE1),
        vec![],
    )
}

/// Replaces the worker-generated (random) class/instance uuids in `obj` with the
/// deterministic ids [`evil_instance`] uses, so structural comparison works.
fn normalize_instance_uuids(obj: &mut MontyObject) {
    for node in obj.graph.nodes_mut() {
        match node {
            MontyNode::ClassType(class) => class.id = MontyUuid::from_u128(0xE0),
            MontyNode::ClassInstance { instance_id, .. } => *instance_id = MontyUuid::from_u128(0xE1),
            _ => {}
        }
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
        MontyObject::list([evil_instance(), MontyObject::int(1), MontyObject::int(2)])
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
        MontyObject::dict(vec![
            (MontyObject::string("k".to_owned()), evil_instance()),
            (MontyObject::string("a".to_owned()), MontyObject::int(1)),
        ])
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
        MontyObject::list([evil_instance(), MontyObject::int(1), MontyObject::int(2)])
    );
}

/// A `groupby` key comparison that steps the same `groupby` and consumes the
/// pair it was comparing must not leave the skip loop with nothing to open a
/// group from.
///
/// Rust-side because CPython segfaults on this program (its `_grouper` reaches
/// through a parent whose state the comparison invalidated), so there is no
/// shared behaviour for a `test_cases` fixture to assert. Monty reads the next
/// pair instead, as CPython's own loop condition intends, and the run finishes
/// with an ordinary `StopIteration` the program can catch.
#[test]
fn reentrant_groupby_key_comparison_does_not_panic() {
    let code = "import itertools

depth = [0]
holder = [None]

class Key:
    def __eq__(self, other):
        if depth[0] == 0 and holder[0] is not None:
            depth[0] += 1
            try:
                key, group = next(holder[0])
                next(group, None)
            except StopIteration:
                pass
            depth[0] -= 1
        return False

grouped = itertools.groupby([Key(), Key(), Key(), Key(), Key()])
holder[0] = grouped
seen = 0
try:
    while True:
        next(grouped)
        seen += 1
except StopIteration:
    pass
seen";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(ex.run_no_limits(vec![]).unwrap(), MontyObject::int(1));
}

/// Exporting a `functools.partial` runs the `__repr__` of its bound instance,
/// which can free an already-exported object; a later allocation reusing its
/// heap slot must export as itself, not as the freed object's node.
#[test]
fn export_pins_memoized_objects_across_a_user_repr() {
    let code = "import functools

class Holder:
    pass

class Mutator:
    def __repr__(self):
        inner.clear()
        holder.x = [9]
        return 'm'

inner = [[1, 2, 3]]
holder = Holder()
p = functools.partial(len, Mutator())
[inner, p, holder]";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(
        ex.run_no_limits(vec![]).unwrap().py_repr(),
        "[[[1, 2, 3]], Repr('functools.partial(<built-in function len>, m)'), Holder(x=[9])]"
    );
}
