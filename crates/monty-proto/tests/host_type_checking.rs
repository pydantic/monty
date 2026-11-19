//! A worker built with `host-type-checking` (and without `type-checking`) hands
//! each check to its host as `WorkerEvent.host_type_check` and applies the
//! host's `ClientRequest.host_type_check_result`. These tests play the host,
//! answering with the same `TypeChecker` a `type-checking` worker runs.
#![cfg(not(feature = "type-checking"))]

use monty_proto::{
    FrameReader, PROTOCOL_VERSION, WireArena, pb,
    worker::{HandleOutcome, Worker, dispatch_frame},
    write_frame,
};
use monty_type_checking::{SourceFile, TypeCheckContext, TypeChecker};
use monty_types::{
    ExcType, IMPORT_STUBS_CALL_ID, MONTY_VERSION, ModuleStub, MontyException, MontyObject, TypeCheckingConfig, unstable,
};

fn frame_request(kind: pb::client_request::Kind) -> Vec<u8> {
    let mut buf = Vec::new();
    write_frame(
        &mut buf,
        &pb::ClientRequest {
            kind: Some(kind),
            trace_parent: None,
        },
    )
    .expect("framing a request never fails");
    buf
}

/// Sends one request and returns its turn-ending event, checking only `Print`s precede it.
fn turn(worker: &mut Worker, kind: pb::client_request::Kind) -> pb::WorkerEvent {
    let (bytes, outcome) = dispatch_frame(worker, &frame_request(kind));
    assert_eq!(outcome, HandleOutcome::Continue);
    let mut reader = FrameReader::new(bytes.as_slice());
    let mut events = Vec::new();
    while let Some(event) = reader.read::<pb::WorkerEvent>().expect("reply frames decode") {
        events.push(event);
    }
    let last = events.pop().expect("a turn always ends with one event");
    assert!(
        events
            .iter()
            .all(|event| matches!(event.kind, Some(pb::worker_event::Kind::Print(_)))),
        "only Print events precede the terminator"
    );
    last
}

fn configure(worker: &mut Worker, stubs: Option<&str>) {
    let event = turn(
        worker,
        pb::client_request::Kind::Configure(pb::Configure {
            script_name: "main.py".to_owned(),
            type_check: true,
            type_check_stubs: stubs.map(str::to_owned),
            type_check_format: pb::TypeCheckFormat::Concise.into(),
            monty_version: MONTY_VERSION.to_owned(),
            protocol_version: PROTOCOL_VERSION,
            ..Default::default()
        }),
    );
    assert!(
        matches!(event.kind, Some(pb::worker_event::Kind::Ok(_))),
        "got {event:?}"
    );
}

fn feed_request(code: &str, skip_type_check: bool) -> pb::client_request::Kind {
    pb::client_request::Kind::Feed(pb::Feed {
        code: code.to_owned(),
        inputs: vec![].into(),
        values: None,
        skip_type_check,
        cwd: "/".to_owned(),
        async_functions: Vec::new().into(),
        stream_async_calls: false,
    })
}

fn feed(worker: &mut Worker, code: &str) -> pb::WorkerEvent {
    turn(worker, feed_request(code, false))
}

fn expect_check(event: pb::WorkerEvent) -> pb::HostTypeCheck {
    match event.kind {
        Some(pb::worker_event::Kind::HostTypeCheck(check)) => check,
        other => panic!("expected HostTypeCheck, got {other:?}"),
    }
}

fn expect_complete(event: pb::WorkerEvent) -> MontyObject {
    match event.kind {
        Some(pb::worker_event::Kind::Complete(complete)) => {
            MontyObject::try_from(complete).expect("the complete value decodes")
        }
        other => panic!("expected Complete, got {other:?}"),
    }
}

fn expect_typing_error(event: pb::WorkerEvent) -> String {
    assert_eq!(event.feed_suspended, Some(false));
    match event.kind {
        Some(pb::worker_event::Kind::TypingError(typing)) => typing.diagnostics,
        other => panic!("expected TypingError, got {other:?}"),
    }
}

fn expect_error_message(event: pb::WorkerEvent) -> String {
    match event.kind {
        Some(pb::worker_event::Kind::Error(error)) => error.exception.unwrap().message.unwrap(),
        other => panic!("expected Error, got {other:?}"),
    }
}

/// Runs the check as a host would: the request carries the whole context.
fn check(checker: &mut TypeChecker, request: &pb::HostTypeCheck) -> pb::host_type_check_result::Outcome {
    let stubs = request
        .stubs
        .as_deref()
        .map(|stubs| SourceFile::new(stubs, "repl_type_stubs.pyi"));
    let module_stubs: Vec<ModuleStub> = request
        .module_stubs
        .iter()
        .map(|stub| ModuleStub::new(stub.module.as_str(), stub.source.as_str()).expect("the worker validated it"))
        .collect();
    let context = TypeCheckContext {
        stubs: stubs.as_ref(),
        module_stubs: &module_stubs,
        prelude: &request.prelude,
    };
    let config = TypeCheckingConfig {
        format: request.format().into(),
        color: request.color,
    };
    match checker.run_with(&SourceFile::new(&request.code, &request.script_name), &context, config) {
        Ok(None) => pb::host_type_check_result::Outcome::Clean(pb::host_type_check_result::Clean {}),
        Ok(Some(diagnostics)) => pb::host_type_check_result::Outcome::Diagnostics(diagnostics.to_string()),
        Err(err) => pb::host_type_check_result::Outcome::Error(err),
    }
}

fn answer(worker: &mut Worker, outcome: pb::host_type_check_result::Outcome) -> pb::WorkerEvent {
    turn(
        worker,
        pb::client_request::Kind::HostTypeCheckResult(pb::HostTypeCheckResult { outcome: Some(outcome) }),
    )
}

/// Feeds `code`, answers its check with `checker`, and returns the feed's outcome.
fn checked_feed(worker: &mut Worker, checker: &mut TypeChecker, code: &str) -> pb::WorkerEvent {
    let request = expect_check(feed(worker, code));
    let outcome = check(checker, &request);
    answer(worker, outcome)
}

fn clean() -> pb::host_type_check_result::Outcome {
    pb::host_type_check_result::Outcome::Clean(pb::host_type_check_result::Clean {})
}

#[test]
fn a_type_checked_session_is_served_and_each_check_carries_its_context() {
    let mut worker = Worker::default();
    configure(&mut worker, Some("def double(x: int) -> int: ..."));
    let request = expect_check(feed(&mut worker, "import math\nn: int = 2\nn"));
    assert_eq!(request.code, "import math\nn: int = 2\nn");
    assert_eq!(request.script_name, "main.py");
    assert_eq!(request.stubs.as_deref(), Some("def double(x: int) -> int: ..."));
    assert_eq!(request.prelude, "");
    assert_eq!(request.format(), pb::TypeCheckFormat::Concise);
    let event = answer(&mut worker, check(&mut TypeChecker::default(), &request));
    assert_eq!(expect_complete(event), MontyObject::int(2));
    // a committed snippet joins the stubs, its imports the prelude
    let request = expect_check(feed(&mut worker, "n + math.floor(1.5)"));
    assert_eq!(
        request.stubs.as_deref(),
        Some("def double(x: int) -> int: ...\nimport math\nn: int = 2\nn")
    );
    assert_eq!(request.prelude, "import math\n");
    assert_eq!(
        answer(&mut worker, check(&mut TypeChecker::default(), &request)).kind,
        expect_complete_kind(3)
    );
}

fn expect_complete_kind(value: i64) -> Option<pb::worker_event::Kind> {
    let (graph, root) = unstable::into_graph_parts(MontyObject::int(value));
    Some(pb::worker_event::Kind::Complete(pb::Complete {
        value: root.0,
        values: Some(WireArena::new(graph)),
    }))
}

#[test]
fn a_rejected_snippet_does_not_run_or_join_the_context() {
    let mut worker = Worker::default();
    configure(&mut worker, None);
    let mut checker = TypeChecker::default();
    let event = checked_feed(&mut worker, &mut checker, "x: int = 'a'\nprint('ran')");
    insta::assert_snapshot!(expect_typing_error(event), @r#"main.py:1:10: error[invalid-assignment] Object of type `Literal["a"]` is not assignable to `int`"#);
    let request = expect_check(feed(&mut worker, "1"));
    assert_eq!(request.stubs, None);
    assert_eq!(expect_complete(answer(&mut worker, clean())), MontyObject::int(1));
}

#[test]
fn module_stubs_reach_the_host_after_the_import_stubs_ask() {
    let mut worker = Worker::default();
    configure(&mut worker, None);
    let event = feed(&mut worker, "import tools\ntools.add('x', 2)");
    let Some(pb::worker_event::Kind::OsCall(call)) = event.kind else {
        panic!("expected the import_stubs ask, got {event:?}");
    };
    assert_eq!(call.call_id, IMPORT_STUBS_CALL_ID);
    let stubs = MontyObject::dict([(
        MontyObject::string("tools"),
        MontyObject::string("def add(a: int, b: int) -> int: ...\n"),
    )]);
    let (graph, root) = unstable::into_graph_parts(stubs);
    let event = turn(
        &mut worker,
        pb::client_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: IMPORT_STUBS_CALL_ID,
            result: Some(pb::ExtFunctionResult {
                kind: Some(pb::ext_function_result::Kind::ReturnValue(root.0)),
            }),
            values: Some(WireArena::new(graph)),
        }),
    );
    let request = expect_check(event);
    assert_eq!(request.module_stubs.len(), 1);
    assert_eq!(request.module_stubs[0].module, "tools");
    let event = answer(&mut worker, check(&mut TypeChecker::default(), &request));
    insta::assert_snapshot!(expect_typing_error(event), @r#"main.py:2:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`"#);
}

#[test]
fn a_feed_awaiting_its_check_holds_until_answered_or_aborted() {
    let mut worker = Worker::default();
    configure(&mut worker, None);
    // an answer with nothing awaiting it
    let event = answer(&mut worker, clean());
    insta::assert_snapshot!(expect_error_message(event), @"protocol violation: HostTypeCheckResult without a feed awaiting its type check");

    expect_check(feed(&mut worker, "1"));
    let event = turn(&mut worker, pb::client_request::Kind::Dump(pb::Dump {}));
    insta::assert_snapshot!(expect_error_message(event), @"cannot dump while a feed awaits its type check");
    let event = feed(&mut worker, "2");
    insta::assert_snapshot!(expect_error_message(event), @"protocol violation: Feed without a session ready for input");
    let event = turn(
        &mut worker,
        pb::client_request::Kind::HostTypeCheckResult(pb::HostTypeCheckResult { outcome: None }),
    );
    insta::assert_snapshot!(expect_error_message(event), @"protocol violation: HostTypeCheckResult has no outcome");
    let event = answer(
        &mut worker,
        pb::host_type_check_result::Outcome::Error("checker crashed".to_owned()),
    );
    assert_eq!(event.feed_suspended, Some(false));
    insta::assert_snapshot!(expect_error_message(event), @"protocol violation: type checker failed: checker crashed");

    expect_check(feed(&mut worker, "3"));
    let stop = MontyException::new(ExcType::RuntimeError, Some("stopped".to_owned()));
    let event = turn(
        &mut worker,
        pb::client_request::Kind::AbortFeed(pb::AbortFeed {
            exception: Some((&stop).into()),
        }),
    );
    insta::assert_snapshot!(expect_error_message(event), @"stopped");
    expect_check(feed(&mut worker, "4"));
    assert_eq!(expect_complete(answer(&mut worker, clean())), MontyObject::int(4));
}

#[test]
fn skipped_feeds_are_not_checked() {
    let mut worker = Worker::default();
    configure(&mut worker, None);
    let event = turn(&mut worker, feed_request("x: int = 'a'\n5", true));
    assert_eq!(expect_complete(event), MontyObject::int(5));
}

#[test]
fn a_type_checked_dump_loads_with_its_context() {
    let mut worker = Worker::default();
    configure(&mut worker, None);
    let event = checked_feed(&mut worker, &mut TypeChecker::default(), "y: str = 'a'");
    assert!(matches!(event.kind, Some(pb::worker_event::Kind::Complete(_))));
    let event = turn(&mut worker, pb::client_request::Kind::Dump(pb::Dump {}));
    let Some(pb::worker_event::Kind::DumpResult(result)) = event.kind else {
        panic!("expected DumpResult, got {event:?}");
    };

    let mut worker = Worker::default();
    configure(&mut worker, None);
    let event = turn(
        &mut worker,
        pb::client_request::Kind::Load(pb::Load { state: result.state }),
    );
    assert!(
        matches!(event.kind, Some(pb::worker_event::Kind::Ok(_))),
        "got {event:?}"
    );
    let event = checked_feed(&mut worker, &mut TypeChecker::default(), "y + 1");
    insta::assert_snapshot!(expect_typing_error(event), @"main.py:1:1: error[unsupported-operator] Operator `+` is not supported between objects of type `str` and `Literal[1]`");
}
