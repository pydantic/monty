//! `typing.get_type_hints()` over host functions and classes, and hints surviving dump/restore.
//!
//! `test_cases/typing__get_type_hints*.py` compare against CPython, the `_external` ones through
//! the datatest harness's host; these cases check the raw OS call, unresolved text hints and dumps.

use monty::{Dump, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, GET_TYPE_HINTS_CALL, MontyObject, MontyType, MontyUuid, OsFunctionCall, PrintWriter,
    ResourceTracker, TypeHintsTarget,
};

/// The id of the host class in [`host_class`].
const CLASS_ID: MontyUuid = MontyUuid::from_u128(0x1234);

/// A host class with no eager attrs.
fn host_class() -> MontyObject {
    MontyObject::class_type("Point", CLASS_ID, true, false, [])
}

/// Hints a host might answer with: a builtin type, unresolved text and `NoneType`.
fn host_hints() -> MontyObject {
    MontyObject::dict([
        (MontyObject::string("url"), MontyObject::type_object(MontyType::Str)),
        (MontyObject::string("retries"), MontyObject::string("int | None")),
        (
            MontyObject::string("return"),
            MontyObject::type_object(MontyType::NoneType),
        ),
    ])
}

/// Starts `code` with `f` bound to `input`.
fn start(code: &str, input: MontyObject) -> RunProgress {
    MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["f".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .start(vec![input], ResourceTracker::default(), PrintWriter::Stdout)
    .unwrap()
}

/// Asserts `progress` is the `get_type_hints` OS call for `target`, then answers it with `hints`.
fn answer_hints(progress: RunProgress, target: &TypeHintsTarget, hints: MontyObject) -> RunProgress {
    let call = progress.into_os_call().expect("expected the get_type_hints OS call");
    assert_eq!(call.function_call.name(), GET_TYPE_HINTS_CALL);
    let OsFunctionCall::GetTypeHints(asked) = &call.function_call else {
        panic!("expected GetTypeHints, got {:?}", call.function_call);
    };
    assert_eq!(asked, target);
    call.resume(hints, PrintWriter::Stdout).unwrap()
}

/// A host function's hints are requested by its name and returned as the host answers them.
#[test]
fn host_function_hints_come_from_the_host() {
    let progress = start(
        "from typing import get_type_hints\nget_type_hints(f)",
        MontyObject::function("fetch", None),
    );
    let target = TypeHintsTarget::Function("fetch".to_owned());
    let result = answer_hints(progress, &target, host_hints()).into_complete().unwrap();
    assert_eq!(result, host_hints());
}

/// A host class's hints are requested by its class id.
#[test]
fn host_class_hints_come_from_the_host() {
    let progress = start("from typing import get_type_hints\nget_type_hints(f)", host_class());
    let target = TypeHintsTarget::Class(CLASS_ID);
    let result = answer_hints(progress, &target, host_hints()).into_complete().unwrap();
    assert_eq!(result, host_hints());
}

/// Without a host to ask, a host function has no hints.
#[test]
fn unserved_host_hints_are_empty() {
    let mut runner = MontyRun::new(
        "from typing import get_type_hints\nget_type_hints(f)".to_owned(),
        "test.py",
        vec!["f".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let result = runner
        .run_no_limits(vec![MontyObject::function("fetch", None)])
        .unwrap();
    assert_eq!(result, MontyObject::dict([]));
}

/// The `get_type_hints` call crosses to a host as `(kind, name or id)` args and reads back.
#[test]
fn type_hints_target_call_args_round_trip() {
    for target in [
        TypeHintsTarget::Function("fetch".to_owned()),
        TypeHintsTarget::Class(CLASS_ID),
    ] {
        let args = OsFunctionCall::GetTypeHints(target.clone()).to_args();
        assert_eq!(TypeHintsTarget::from_call_args(&args), Some(target));
    }
}

/// A session dumped while asking the host for hints restores and resumes,
/// and still evaluates sandbox hints (rebuilding the compiled-hint cache).
#[test]
fn hints_survive_dump_and_restore() {
    let code = "\
from typing import get_type_hints
class Foo:
    pass
def g(a: int, b: 'Foo') -> list['Foo']:
    pass
before = get_type_hints(g)
host = get_type_hints(f)
(before == get_type_hints(g), get_type_hints(g)['b'] is Foo, host)";
    let progress = start(code, MontyObject::function("fetch", None));
    let bytes = dump("test.py", None, &[], SessionRef::Running(&progress)).unwrap();
    let Session::Running(loaded) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped a running session")
    };
    let target = TypeHintsTarget::Function("fetch".to_owned());
    // Both sessions run to completion, so their heaps are released properly.
    for progress in [progress, *loaded] {
        assert_eq!(
            answer_hints(progress, &target, host_hints()).into_complete().unwrap(),
            MontyObject::tuple([MontyObject::bool(true), MontyObject::bool(true), host_hints()]),
        );
    }
}

/// Without namespaces, a class's plain annotations see the class namespace
/// first and its forward references the module globals first, as in CPython.
/// The CPython side of the datatest harness cannot reach module globals from a
/// class, so this lives here; the expected values are CPython 3.14's.
#[test]
fn class_forward_refs_search_module_globals_first() {
    let code = "\
from typing import get_type_hints
X = int
class A:
    X = str
    Y = bytes
    a: 'X'
    b: 'Y'
    c: list['X']
    d: X
h = get_type_hints(A)
(h['a'] is int, h['b'] is bytes, h['c'] == list[int], h['d'] is str)";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(
        runner.run_no_limits(vec![]).unwrap(),
        MontyObject::tuple((0..4).map(|_| MontyObject::bool(true))),
    );
}
