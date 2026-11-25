//! Generator behaviour that Python test cases cannot express: resource limits,
//! idle snapshots, abandoned frames and host boundaries.

use std::time::Duration;

use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, PrintWriter, ResourceLimits,
    ResourceTracker,
};

/// Runs `code` under `limits`, returning its error.
fn run_err(code: &str, limits: ResourceLimits) -> MontyException {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .unwrap_err()
}

/// Round-trips an idle REPL through a dump.
fn reload(repl: &MontyRepl) -> MontyRepl {
    let bytes = dump("test.py", None, &[], SessionRef::Idle(repl)).unwrap();
    let Session::Idle(restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session")
    };
    *restored
}

/// Each resumed generator frame counts toward the recursion limit, however it is advanced.
#[test]
fn generator_resumption_enforces_recursion_limits() {
    for expression in [
        "yield next(generate(depth - 1))",
        "yield from generate(depth - 1)",
        "for value in generate(depth - 1):\n            yield value",
        "yield sum(generate(depth - 1))",
    ] {
        let code =
            format!("def generate(depth):\n    if depth:\n        {expression}\n    yield 42\nnext(generate(100))");
        let error = run_err(&code, ResourceLimits::default().max_recursion_depth(10));
        assert_eq!(error.exc_type(), ExcType::RecursionError, "{expression}");
    }
}

/// A yield releases the level its resume reserved, so long-running generators stay within a small limit.
#[test]
fn repeated_resumption_releases_recursion_levels() {
    let code = "
def generate():
    for value in range(1000):
        yield value
def delegate():
    yield from generate()
for value in generate():
    pass
assert sum(delegate()) == 499500
";
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run(
            vec![],
            ResourceTracker::new(ResourceLimits::default().max_recursion_depth(4)),
            PrintWriter::Stdout,
        )
        .unwrap();
}

/// Instruction-time checks run inside a resumed generator frame and an empty delegation loop.
#[test]
fn time_limits_apply_inside_generator_frames() {
    for code in [
        "def generate():\n    yield 1\n    while True:\n        pass\ng = generate()\nnext(g)\nnext(g)",
        "def generate():\n    while True:\n        yield from ()\nnext(generate())",
    ] {
        let error = run_err(
            code,
            ResourceLimits::default().max_feed_duration(Duration::from_millis(10)),
        );
        assert_eq!(error.exc_type(), ExcType::TimeoutError, "{code}");
    }
}

/// Suspended generators survive an idle dump with their locals, shared objects and handled exceptions.
#[test]
fn suspended_generators_round_trip_through_idle_dumps() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        r"
shared = []
def generate(value):
    yield value
    value.append(42)
    yield value
a = generate(shared)
b = generate(shared)
unstarted = generate(shared)
assert next(a) is shared
assert next(b) is shared
def reraise():
    try:
        raise ValueError('original')
    except ValueError:
        yield 1
        raise
r = reraise()
assert next(r) == 1
def inner():
    yield shared
    return shared
def outer(g):
    assert (yield from g) is shared
    yield 'outer'
delegate = inner()
o = outer(delegate)
assert next(o) is shared
namespace = {'token': []}
exec('def scoped():\n    yield token\n    yield token', namespace)
s = namespace['scoped']()
assert next(s) is namespace['token']
",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let mut restored = reload(&repl);
    restored
        .feed_run(
            r"
assert next(a) is shared
assert next(b) is shared
assert shared == [42, 42]
assert next(a, None) is None
assert next(unstarted) is shared
try:
    next(r)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'original'
assert next(o) == 'outer'
assert next(delegate, None) is None
assert next(s) is namespace['token']
assert next(s, None) is None
",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
}

/// A feed abandoned while a generator frame is suspended on a host call completes the generator.
#[test]
fn abandoned_running_generators_complete() {
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl
        .feed_start(
            "def generate():\n    yield host_call()\ng = generate()\nnext(g)",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let mut repl = progress.into_repl();
    repl.feed_run("assert next(g, 'done') == 'done'", vec![], PrintWriter::Stdout)
        .unwrap();
    let mut restored = reload(&repl);
    restored
        .feed_run("assert next(g, 'done') == 'done'", vec![], PrintWriter::Stdout)
        .unwrap();
}

/// A host function raising `StopIteration('done')` inside a delegate's `__next__`
/// ends the delegation with the message as its value.
#[test]
fn host_stop_iteration_ends_yield_from() {
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl
        .feed_start(
            r"
class Iterator:
    def __iter__(self):
        return self
    def __next__(self):
        return host_call()
def outer():
    assert (yield from Iterator()) == 'done'
    yield 42
assert next(outer()) == 42
",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let error = MontyException::new(ExcType::StopIteration, Some("done".to_owned()));
    progress
        .into_function_call()
        .unwrap()
        .resume(ExtFunctionResult::Error(error), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
}

/// A host function bound directly as `__next__` cannot be driven by `yield from`;
/// the error is raised inside the generator, which then completes.
#[test]
fn yield_from_rejects_external_next() {
    let code = r"
class Iterator:
    __next__ = host_next
    def __iter__(self):
        return self
def outer():
    yield from Iterator()
g = outer()
try:
    next(g)
    assert False, 'expected NotImplementedError'
except NotImplementedError as exc:
    assert str(exc) == 'yield from: this iterator cannot suspend to the host'
assert next(g, None) is None
";
    MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["host_next".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![MontyObject::function("host_next", None)])
    .unwrap();
}

/// A consumer that advances in Rust cannot suspend, so a host call in the generator
/// body raises a catchable `NotImplementedError` there; `for` and `next()` suspend normally.
#[test]
fn synchronous_consumers_cannot_suspend_generators() {
    let code = r#"
def generate():
    try:
        yield host_call()
    except NotImplementedError as exc:
        yield str(exc)
assert list(generate()) == [
    "external function 'host_call' cannot suspend in this synchronous context"
]
"#;
    MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["host_call".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![MontyObject::function("host_call", None)])
    .unwrap();
}

/// A string `StopIteration` value is its message once it escapes to the host.
#[test]
fn stop_iteration_message_reaches_host() {
    for code in [
        "raise StopIteration('done')",
        "def g():\n    return 'done'\n    yield\nnext(g())",
    ] {
        let error = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
            .unwrap()
            .run_no_limits(vec![])
            .unwrap_err();
        assert_eq!(error.exc_type(), ExcType::StopIteration, "{code}");
        assert_eq!(error.message(), Some("done"), "{code}");
    }
}

/// Formatting nested `StopIteration` values is bounded by the recursion limit and releases its levels.
#[test]
fn nested_stop_iteration_formatting_is_bounded() {
    for formatter in ["repr", "str"] {
        let code = format!("e = StopIteration()\nfor _ in range(10000):\n    e = StopIteration(e)\n{formatter}(e)");
        let error = run_err(&code, ResourceLimits::default().max_recursion_depth(24));
        assert_eq!(error.exc_type(), ExcType::RecursionError, "{formatter}");
    }
    let code = r"
e = StopIteration()
for _ in range(100):
    e = StopIteration(e)
for formatter in [repr, str]:
    for _ in range(50):
        try:
            formatter(e)
            assert False, 'expected RecursionError'
        except RecursionError:
            pass
        assert repr(StopIteration(42)) == 'StopIteration(42)'
        assert str(StopIteration(42)) == '42'
";
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run(
            vec![],
            ResourceTracker::new(ResourceLimits::default().max_recursion_depth(24)),
            PrintWriter::Stdout,
        )
        .unwrap();
}

/// Unreachable generators, including ones in cycles and ones referenced by a
/// `StopIteration` value, release every reference.
#[test]
#[cfg(feature = "ref-count-return")]
fn unreachable_generators_release_references() {
    let code = r"
def make_cycle():
    value = []
    def generate(arg):
        yield arg
        yield value
    g = generate(value)
    value.append(g)
    next(g)
for _ in range(100):
    make_cycle()
def temporary():
    value = [[] for _ in range(10)]
    def generate(arg):
        yield arg
    generate(value)
    g = generate(value)
    next(g)
for _ in range(100):
    temporary()
def exception_cycle():
    value = []
    def generate():
        yield 1
        return value
    g = generate()
    next(g)
    try:
        next(g)
    except StopIteration as exc:
        value.append(exc)
for _ in range(100):
    exception_cycle()
def delegation_cycle():
    values = []
    def inner():
        yield values
    def outer():
        yield from inner()
    g = outer()
    values.append(g)
    next(g)
for _ in range(100):
    delegation_cycle()
";
    let output = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_ref_counts(vec![])
        .unwrap();
    assert!(output.unreachable.is_empty(), "{:?}", output.unreachable);
    assert!(output.heap_count < 50, "{}", output.heap_count);
}

/// A generator returned to the host keeps its function's name, as a coroutine does.
#[test]
fn exported_generator_repr_names_its_function() {
    let code = "def numbers():\n    yield 1\nnumbers()";
    let result = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap();
    assert_eq!(result, MontyObject::repr("<generator object numbers>"));
}
