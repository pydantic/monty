use std::mem;

use insta::assert_snapshot;
use monty::{Dump, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, MontyUuid, PrintWriter, ResourceTracker,
    unstable::{self, MontyNode},
};

/// Test we can reuse exec without borrow checker issues.
#[test]
fn repeat_exec() {
    let mut ex = MontyRun::new("1 + 2".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);

    let r = ex.run_no_limits(vec![]).unwrap();
    let int_value: i64 = r.as_ref().try_into().unwrap();
    assert_eq!(int_value, 3);
}

/// Shared module code must remain usable as clones independently append runtime functions and literals.
#[test]
fn cloned_runners_compile_independently() {
    let mut runner = MontyRun::new(
        "exec(source)\nresult()".to_owned(),
        "test.py",
        vec!["source".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    for _ in 0..2 {
        let mut cloned = runner.clone();
        for (runner, text) in [(&mut runner, "original"), (&mut cloned, "cloned")] {
            let source = format!("def result():\n    return {text:?}");
            assert_eq!(
                runner.run_no_limits(vec![MontyObject::string(source)]).unwrap(),
                MontyObject::string(text.to_owned())
            );
        }
    }
}

#[test]
fn test_get_interned_string() {
    let mut ex = MontyRun::new("'foobar'".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

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
    let mut ex = MontyRun::new(
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

    let mut ex = MontyRun::new(
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
/// this as a Rust-side regression test until augmented matrix multiplication
/// is supported; ordinary `@` already dispatches user slots.
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
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
        .unwrap_err();
    assert_snapshot!(err, @r#"
    Traceback (most recent call last):
      File "test.py", line 4, in <module>
        Foo()
        ~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context
    "#);
}

/// `functools.reduce` runs its callback from a frozen Python frame, allowing
/// every reduction step to suspend and round-trip through a VM snapshot.
#[test]
fn external_function_in_reduce_suspends() {
    let code = "import functools\n\nfunctools.reduce(ext_fn, [1, 2, 3])";
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(
            vec![MontyObject::function("ext_fn".to_owned(), None)],
            ResourceTracker::default(),
            PrintWriter::Stdout,
        )
        .unwrap();

    let bytes = dump("test.py", None, &[], SessionRef::Running(&progress)).unwrap();
    let Session::Running(restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected running dump")
    };

    assert_eq!(complete_external_reduce(progress), MontyObject::int(6));
    assert_eq!(complete_external_reduce(*restored), MontyObject::int(6));
}

/// Resumes both external calls expected from the three-item reduce fixture.
fn complete_external_reduce(mut progress: RunProgress) -> MontyObject {
    for (expected_args, result) in [([1, 2], 3), ([3, 3], 6)] {
        let RunProgress::FunctionCall(call) = progress else {
            panic!("expected reduce callback, got {progress:?}")
        };
        assert_eq!(call.function_name, "ext_fn");
        assert_eq!(
            call.args.args().collect::<Vec<_>>(),
            expected_args.into_iter().map(MontyObject::int).collect::<Vec<_>>()
        );
        progress = call.resume(MontyObject::int(result), PrintWriter::Stdout).unwrap();
    }
    progress.into_complete().expect("expected Complete")
}

/// Nested caches around a host callable resume and store through ordinary frozen frames.
#[test]
fn external_function_in_cache_suspends() {
    let code = "import functools\ncached = functools.cache(functools.cache(ext_fn))\n[cached(1), cached(1), cached(2)]";
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(
            vec![MontyObject::function("ext_fn".to_owned(), None)],
            ResourceTracker::default(),
            PrintWriter::Disabled,
        )
        .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Running(&progress)).unwrap();
    let Session::Running(restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected running dump")
    };
    for mut progress in [progress, *restored] {
        for arg in [1, 2] {
            let RunProgress::FunctionCall(call) = progress else {
                panic!("expected cache miss")
            };
            assert_eq!(call.function_name, "ext_fn");
            assert_eq!(call.args.args().collect::<Vec<_>>(), vec![MontyObject::int(arg)]);
            progress = call.resume(MontyObject::int(arg * 10), PrintWriter::Disabled).unwrap();
        }
        assert_eq!(
            progress.into_complete(),
            Some(MontyObject::list(vec![
                MontyObject::int(10),
                MontyObject::int(10),
                MontyObject::int(20)
            ]))
        );
    }
}

/// The frozen implementation is an interpreter detail and must not appear in
/// tracebacks when a suspended callback fails in the host.
#[test]
fn frozen_reduce_frame_is_hidden_from_traceback() {
    let code = "import functools\n\nfunctools.reduce(ext_fn, [1, 2])";
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(
            vec![MontyObject::function("ext_fn".to_owned(), None)],
            ResourceTracker::default(),
            PrintWriter::Stdout,
        )
        .unwrap();
    let RunProgress::FunctionCall(call) = progress else {
        panic!("expected reduce callback, got {progress:?}")
    };
    let error = MontyException::new(ExcType::ValueError, Some("callback failed".to_owned()));
    let error = call
        .resume(ExtFunctionResult::Error(error), PrintWriter::Stdout)
        .unwrap_err();

    insta::assert_snapshot!(error, @r#"
    Traceback (most recent call last):
      File "test.py", line 3, in <module>
        functools.reduce(ext_fn, [1, 2])
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    ValueError: callback failed
    "#);
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
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::function("ext_fn", None)])
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 9, in <module>
        copy.deepcopy(Foo())
        ~~~~~~~~~~~~~~~~~~~~
      File "test.py", line 6, in __deepcopy__
        return ext_fn()
               ~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context
    "#);
}

/// `for` still requires synchronous advancement, so an external call in
/// `__next__` raises at that call site rather than suspending the loop.
#[test]
fn external_function_in_next_raises_not_implemented() {
    let code = "class Foo:\n    def __iter__(self):\n        return self\n\n    def __next__(self):\n        return ext_fn()\n\nfor _x in Foo():\n    pass";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 8, in <module>
        for _x in Foo():
                  ~~~~~
      File "test.py", line 6, in __next__
        return ext_fn()
               ~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context
    "#);
}

/// Iterator acquisition reports both the synchronous consumer and the suspended method in its traceback.
#[test]
fn synchronous_sum_acquisition_has_traceback() {
    let code = "class Iterable:\n    def __iter__(self):\n        return ext_fn()\n\nsum(Iterable())";
    let mut runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let error = runner
        .run_no_limits(vec![MontyObject::function("ext_fn", None)])
        .unwrap_err();
    assert_snapshot!(error, @r#"
    Traceback (most recent call last):
      File "test.py", line 5, in <module>
        sum(Iterable())
        ~~~~~~~~~~~~~~~
      File "test.py", line 3, in __iter__
        return ext_fn()
               ~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context
    "#);
}

/// Synchronous consumers reject both direct host calls and calls inside Python frames, without
/// consuming an outer handler or retaining pending arguments after the rejection.
#[test]
fn synchronous_iteration_rejection_restores_state() {
    let code = r#"
import functools

class RecoverIter:
    def __iter__(self):
        items = [1, 2]
        try:
            ext_fn(items)
        except NotImplementedError:
            return iter(items)

class RecoverNext:
    def __iter__(self):
        return self
    def __next__(self):
        try:
            return ext_fn(['owned'])
        except NotImplementedError:
            raise StopIteration

class FailIter:
    def __iter__(self):
        return ext_fn(['owned'])

class DirectNext:
    def __iter__(self):
        return self
    __next__ = functools.partial(ext_fn, ['owned'])

for _ in range(3):
    assert list(RecoverIter()) == [1, 2]
    assert list(RecoverNext()) == []
    try:
        list(FailIter())
        assert False, 'expected synchronous acquisition to fail'
    except NotImplementedError as exc:
        assert str(exc) == "external function 'ext_fn' cannot suspend in this synchronous context"
    try:
        list(DirectNext())
        assert False, 'expected synchronous advancement to fail'
    except NotImplementedError as exc:
        assert str(exc) == "external function 'ext_fn' cannot suspend in this synchronous context"
    assert list(RecoverIter()) == [1, 2]
42
"#;
    let mut runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    assert_eq!(
        runner
            .run_no_limits(vec![MontyObject::function("ext_fn", None)])
            .unwrap(),
        MontyObject::int(42)
    );
}

/// Rejecting native re-entry after dispatch must pop the unexecuted frame and release its budget.
#[test]
fn synchronous_iteration_reentry_limit_restores_state() {
    let code = r"
class Recursive:
    def __iter__(self):
        list(self)
        return iter([])

class Ordinary:
    def __iter__(self):
        return iter([1, 2])

for _ in range(3):
    try:
        list(Recursive())
        assert False, 'expected the native re-entry limit'
    except RecursionError as exc:
        assert str(exc) == 'maximum recursion depth exceeded'
    assert list(Ordinary()) == [1, 2]
42
";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::int(42));
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
    let mut ex = MontyRun::new(
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
    let mut ex = MontyRun::new(
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

/// The traceback identifies `list.sort()` and its key function without a context prefix.
#[test]
fn not_implemented_in_list_sort_key_has_traceback() {
    let code = "[1, 2].sort(key=lambda x: ext_fn())";
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["ext_fn".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let err = ex
        .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        [1, 2].sort(key=lambda x: ext_fn())
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
      File "test.py", line 1, in <lambda>
        [1, 2].sort(key=lambda x: ext_fn())
                                  ~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context
    "#);
}

/// Synchronous adaptor callbacks report a generic error with the consuming expression in the traceback.
/// `accumulate` needs two items, since the first is yielded untouched.
#[test]
fn external_function_as_itertools_callable_raises_not_implemented() {
    let mut errors = Vec::new();
    for call in [
        "itertools.takewhile(ext_fn, [1])",
        "itertools.dropwhile(ext_fn, [1])",
        "itertools.filterfalse(ext_fn, [1])",
        "itertools.starmap(ext_fn, [(1,)])",
        "itertools.accumulate([1, 2], ext_fn)",
        "itertools.groupby([1], ext_fn)",
    ] {
        let expr = format!("list({call})");
        let code = format!("import itertools\n\n{expr}");
        let mut ex = MontyRun::new(code, "test.py", vec!["ext_fn".to_owned()], CompileOptions::default()).unwrap();
        let err = ex
            .run_no_limits(vec![MontyObject::function("ext_fn".to_owned(), None)])
            .unwrap_err();
        errors.push(err.to_string());
    }
    assert_snapshot!(errors.join("\n\n"), @r#"
    Traceback (most recent call last):
      File "test.py", line 3, in <module>
        list(itertools.takewhile(ext_fn, [1]))
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context

    Traceback (most recent call last):
      File "test.py", line 3, in <module>
        list(itertools.dropwhile(ext_fn, [1]))
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context

    Traceback (most recent call last):
      File "test.py", line 3, in <module>
        list(itertools.filterfalse(ext_fn, [1]))
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context

    Traceback (most recent call last):
      File "test.py", line 3, in <module>
        list(itertools.starmap(ext_fn, [(1,)]))
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context

    Traceback (most recent call last):
      File "test.py", line 3, in <module>
        list(itertools.accumulate([1, 2], ext_fn))
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context

    Traceback (most recent call last):
      File "test.py", line 3, in <module>
        list(itertools.groupby([1], ext_fn))
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: external function 'ext_fn' cannot suspend in this synchronous context
    "#);
}

/// The 3-arg `type()` form rejects non-empty bases because Monty classes
/// cannot inherit (documented in `limitations/classes.md`). Kept as a
/// Rust-side test because CPython accepts bases, so the comparative
/// test-case suite cannot cover the divergence.
#[test]
fn dynamic_type_with_bases_raises_type_error() {
    let code = "type('A', (int,), {})";
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let (mut graph, root) = unstable::into_graph_parts(mem::replace(obj, MontyObject::none()));
    for node in graph.nodes_mut() {
        match node {
            MontyNode::ClassType(class) => class.id = MontyUuid::from_u128(0xE0),
            MontyNode::ClassInstance { instance_id, .. } => *instance_id = MontyUuid::from_u128(0xE1),
            _ => {}
        }
    }
    *obj = unstable::object_from_graph(graph, root).unwrap();
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
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(
        ex.run_no_limits(vec![]).unwrap().py_repr(),
        "[[[1, 2, 3]], Repr('functools.partial(<built-in function len>, m)'), Holder(x=[9])]"
    );
}
