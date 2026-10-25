//! `typing.get_type_hints()` over host functions, and hints surviving dump/restore.
//!
//! Sandbox-only behaviour is covered by `test_cases/typing__get_type_hints.py`;
//! these cases need a host to send hints or a dump to round-trip them.

use std::mem;

use insta::assert_snapshot;
use monty::{Dump, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, MontyObject, MontyType, PrintWriter, ResourceTracker,
    unstable::{self, MontyNode},
};

/// A host function sending a builtin type, an unresolvable string and `None`.
fn hinted_function() -> MontyObject {
    MontyObject::function_with_annotations(
        "fetch",
        None,
        [
            ("url", MontyObject::type_object(MontyType::Str)),
            ("retries", MontyObject::string("int | None")),
            ("return", MontyObject::type_object(MontyType::NoneType)),
        ],
    )
}

/// The hints `get_type_hints(f)` returns for [`hinted_function`].
fn hinted_function_hints() -> MontyObject {
    MontyObject::dict([
        (MontyObject::string("url"), MontyObject::type_object(MontyType::Str)),
        (MontyObject::string("retries"), MontyObject::string("int | None")),
        (
            MontyObject::string("return"),
            MontyObject::type_object(MontyType::NoneType),
        ),
    ])
}

/// Runs `code` with `f` bound to `input`.
fn run_with_function(code: &str, input: MontyObject) -> Result<MontyObject, String> {
    let mut runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["f".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    runner.run_no_limits(vec![input]).map_err(|err| err.to_string())
}

/// Host hints come back as sent, with nothing evaluated in the sandbox.
#[test]
fn host_function_hints() {
    let result = run_with_function(
        "from typing import get_type_hints\nget_type_hints(f)",
        hinted_function(),
    );
    assert_eq!(result.unwrap(), hinted_function_hints());
}

/// A plain `MontyObject::function` has no hints.
#[test]
fn host_function_without_hints() {
    let result = run_with_function(
        "from typing import get_type_hints\nget_type_hints(f)",
        MontyObject::function("fetch", None),
    );
    assert_eq!(result.unwrap(), MontyObject::dict([]));
}

/// The returned dict is fresh: mutating it leaves the function's hints alone.
#[test]
fn host_function_hints_are_copied() {
    let code = "\
from typing import get_type_hints
get_type_hints(f).clear()
get_type_hints(f)";
    assert_eq!(
        run_with_function(code, hinted_function()).unwrap(),
        hinted_function_hints()
    );
}

/// A hint that is neither a builtin type nor a string is rejected on import.
#[test]
fn host_function_invalid_hint_value() {
    let input = MontyObject::function_with_annotations("fetch", None, [("url", MontyObject::int(1))]);
    let err = run_with_function("f", input).unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid input type: function annotations must be builtin types or strings");
}

/// A hint name that is not a string is rejected on import.
#[test]
fn host_function_invalid_hint_name() {
    let (mut graph, root) = unstable::into_graph_parts(hinted_function());
    for node in graph.nodes_mut() {
        if let MontyNode::String(name) = node
            && name == "url"
        {
            *node = MontyNode::Int(1);
        }
    }
    let input = unstable::object_from_graph(graph, root).unwrap();
    let err = run_with_function("f", input).unwrap_err();
    assert_snapshot!(err, @"RuntimeError: invalid input type: function annotation names must be strings");
}

/// Sandbox and host hints both survive a dump taken mid-run, and the restored
/// session still evaluates sandbox hints (rebuilding the compiled-hint cache).
#[test]
fn hints_survive_dump_and_restore() {
    let code = "\
from typing import get_type_hints
class Foo:
    pass
def g(a: int, b: 'Foo') -> list['Foo']:
    pass
before = get_type_hints(g)
gate()
(before == get_type_hints(g), get_type_hints(g)['b'] is Foo, get_type_hints(f))";
    let runner = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["f".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = runner
        .start(vec![hinted_function()], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Running(&progress)).unwrap();
    mem::drop(progress);

    let Session::Running(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped a running session")
    };
    assert_eq!(
        resume_gate(*progress),
        MontyObject::tuple([
            MontyObject::bool(true),
            MontyObject::bool(true),
            hinted_function_hints(),
        ]),
    );
}

/// Resumes the `gate()` call and runs to completion.
fn resume_gate(progress: RunProgress) -> MontyObject {
    let call = progress.into_function_call().expect("expected call to 'gate'");
    assert_eq!(call.function_name, "gate");
    call.resume(MontyObject::none(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap()
}
