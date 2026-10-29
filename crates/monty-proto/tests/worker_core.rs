//! A worker built with `worker-core` alone has no type checker: it serves
//! unchecked sessions and refuses type-checked ones, whether configured or
//! restored from a dump. Run with
//! `cargo test -p monty-proto --features worker-core --test worker_core`.
#![cfg(all(feature = "worker-core", not(feature = "worker")))]

use monty::{MontyRepl, SessionRef, dump};
use monty_proto::{
    PROTOCOL_VERSION, pb,
    worker::{Child, HandleOutcome, dispatch_frame},
    write_frame,
};
use monty_types::{CompileOptions, MONTY_VERSION, ResourceTracker, TypeCheckState, TypeCheckingConfig};

const REFUSED: &str =
    "protocol violation: type checking is not built into this worker (monty-proto built with `worker-core`)";

/// Sends one request and returns the turn-ending event.
fn turn(child: &mut Child, kind: pb::parent_request::Kind) -> pb::child_event::Kind {
    let mut request = Vec::new();
    write_frame(
        &mut request,
        &pb::ParentRequest {
            kind: Some(kind),
            trace_parent: None,
        },
    )
    .unwrap();
    let (bytes, outcome) = dispatch_frame(child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let mut reader = monty_proto::FrameReader::new(bytes.as_slice());
    let mut last = None;
    while let Some(event) = reader.read::<pb::ChildEvent>().unwrap() {
        last = event.kind;
    }
    last.expect("a turn always ends with one event")
}

fn configure(type_check: bool) -> pb::parent_request::Kind {
    pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "main.py".to_owned(),
        type_check,
        monty_version: MONTY_VERSION.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        ..Default::default()
    })
}

fn error_message(event: pb::child_event::Kind) -> String {
    let pb::child_event::Kind::Error(error) = event else {
        panic!("expected an Error event, got {event:?}");
    };
    error.exception.unwrap().message.unwrap()
}

#[test]
fn serves_unchecked_sessions() {
    let mut child = Child::default();
    assert!(matches!(
        turn(&mut child, configure(false)),
        pb::child_event::Kind::Ok(_)
    ));
    let event = turn(
        &mut child,
        pb::parent_request::Kind::Feed(pb::Feed {
            code: "1 + 2".to_owned(),
            cwd: "/".to_owned(),
            ..Default::default()
        }),
    );
    assert!(matches!(event, pb::child_event::Kind::Complete(_)), "got {event:?}");
}

#[test]
fn refuses_type_checked_configure() {
    let mut child = Child::default();
    assert_eq!(error_message(turn(&mut child, configure(true))), REFUSED);
    // the refusal leaves the worker usable for an unchecked session
    assert!(matches!(
        turn(&mut child, configure(false)),
        pb::child_event::Kind::Ok(_)
    ));
}

#[test]
fn refuses_type_checked_dump() {
    let repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    let type_check = TypeCheckState {
        committed_stubs: String::new(),
        pending_snippet: None,
        config: TypeCheckingConfig::default(),
        committed_imports: String::new(),
        pending_imports: String::new(),
        asked_modules: Vec::new(),
    };
    let state = dump("main.py", Some(&type_check), &[], SessionRef::Idle(&repl)).unwrap();
    let mut child = Child::default();
    assert!(matches!(
        turn(&mut child, configure(false)),
        pb::child_event::Kind::Ok(_)
    ));
    let event = turn(
        &mut child,
        pb::parent_request::Kind::Load(pb::Load { state: state.into() }),
    );
    assert_eq!(error_message(event), REFUSED);
}
