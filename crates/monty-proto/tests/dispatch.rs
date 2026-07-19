//! In-process tests of the buffered per-turn entry point [`dispatch_frame`].
//!
//! This is the exact path a wasm Web Worker drives — framed request in, framed
//! events out — minus the FFI memory marshalling, so it round-trips the whole
//! `Child` state machine over the message-based transport without any wasm
//! toolchain.

use monty::{CompileOptions, LimitedTracker, MontyObject, MontyRepl, PrintWriter, ReplProgress, ResourceLimits};
use monty_proto::{
    FrameReader, MONTY_VERSION, WireObject, pb,
    worker::{Child, HandleOutcome, dispatch_frame},
    write_frame,
};

/// Frames one request the way a host transport would before posting it.
fn frame_request(kind: pb::parent_request::Kind) -> Vec<u8> {
    let mut buf = Vec::new();
    write_frame(&mut buf, &pb::ParentRequest { kind: Some(kind) }).expect("framing a request never fails");
    buf
}

/// Decodes every framed event in a turn's reply buffer.
fn decode_events(bytes: &[u8]) -> Vec<pb::child_event::Kind> {
    let mut reader = FrameReader::new(bytes);
    let mut events = Vec::new();
    while let Some(event) = reader.read::<pb::ChildEvent>().expect("reply frames decode") {
        events.push(event.kind.expect("event has a kind"));
    }
    events
}

/// Splits a turn's events into the streamed `Print`s and the single
/// turn-ending event.
fn split_turn(bytes: &[u8]) -> (Vec<pb::Print>, pb::child_event::Kind) {
    let mut prints = Vec::new();
    let mut events = decode_events(bytes);
    let last = events.pop().expect("a turn always ends with one event");
    for event in events {
        match event {
            pb::child_event::Kind::Print(print) => prints.push(print),
            other => panic!("expected only Print events before the terminator, got {other:?}"),
        }
    }
    (prints, last)
}

fn create_repl(child: &mut Child) {
    let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: false,
        type_check_stubs: None,
        assert_message_annotations: None,
        monty_version: MONTY_VERSION.to_owned(),
    }));
    let (bytes, outcome) = dispatch_frame(child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    assert!(
        matches!(decode_events(&bytes).as_slice(), [pb::child_event::Kind::Ok(_)]),
        "Configure should answer with a single Ok"
    );
}

fn feed(child: &mut Child, code: &str) -> (Vec<pb::Print>, pb::child_event::Kind) {
    let request = frame_request(pb::parent_request::Kind::Feed(pb::Feed {
        code: code.to_owned(),
        inputs: vec![],
        skip_type_check: false,
    }));
    let (bytes, outcome) = dispatch_frame(child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    split_turn(&bytes)
}

fn expect_complete(event: pb::child_event::Kind) -> MontyObject {
    match event {
        pb::child_event::Kind::Complete(complete) => complete
            .value
            .expect("complete carries a value")
            .into_object()
            .expect("the complete value decodes"),
        other => panic!("expected Complete, got {other:?}"),
    }
}

#[test]
fn feed_round_trips_a_value() {
    let mut child = Child::new();
    create_repl(&mut child);

    let (_, event) = feed(&mut child, "1 + 2");
    assert_eq!(expect_complete(event), MontyObject::Int(3));
}

#[test]
fn session_state_persists_across_feeds() {
    let mut child = Child::new();
    create_repl(&mut child);

    let (_, first) = feed(&mut child, "x = 21");
    assert_eq!(expect_complete(first), MontyObject::None);

    let (_, second) = feed(&mut child, "x * 2");
    assert_eq!(expect_complete(second), MontyObject::Int(42));
}

#[test]
fn print_output_is_streamed_before_the_terminator() {
    let mut child = Child::new();
    create_repl(&mut child);

    let (prints, event) = feed(&mut child, "print('hello'); print('world')");
    let streamed: String = prints.into_iter().map(|print| print.text).collect();
    assert_eq!(streamed, "hello\nworld\n");
    assert_eq!(expect_complete(event), MontyObject::None);
}

#[test]
fn inputs_are_injected() {
    let mut child = Child::new();
    create_repl(&mut child);

    let request = frame_request(pb::parent_request::Kind::Feed(pb::Feed {
        code: "n + 1".to_owned(),
        inputs: vec![pb::NamedValue {
            name: "n".to_owned(),
            value: Some(WireObject::new(MontyObject::Int(41))),
        }],
        skip_type_check: false,
    }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let (_, event) = split_turn(&bytes);
    assert_eq!(expect_complete(event), MontyObject::Int(42));
}

#[test]
fn malformed_request_frame_is_recoverable() {
    let mut child = Child::new();
    // a length prefix claiming bytes that aren't there: structurally broken
    // framing, not a decode error
    let (bytes, outcome) = dispatch_frame(&mut child, &[0xff, 0xff, 0xff, 0x7f]);
    assert_eq!(outcome, HandleOutcome::Shutdown);
    assert!(
        matches!(decode_events(&bytes).as_slice(), [pb::child_event::Kind::FatalError(_)]),
        "a framing desync ends the worker with a FatalError"
    );
}

#[test]
fn shutdown_request_reports_shutdown() {
    let mut child = Child::new();
    create_repl(&mut child);

    let request = frame_request(pb::parent_request::Kind::Shutdown(pb::Shutdown {}));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Shutdown);
    assert!(
        matches!(decode_events(&bytes).as_slice(), [pb::child_event::Kind::Ok(_)]),
        "Shutdown answers with a single Ok"
    );
}

/// A forged suspended dump whose call arguments nest deeper than the wire
/// depth bound must be rejected at `Load` with a protocol violation — not
/// re-announced as an event the parent cannot decode.
#[test]
fn load_rejects_dump_with_over_deep_suspension_args() {
    // suspend in-process (no wire depth bound) at `f(x)` with x nested 100
    // lists deep — over the ~48 wire bound, shallow enough that postcard's
    // recursive deserialize doesn't overflow the test stack
    let repl = MontyRepl::new(
        "main.py",
        LimitedTracker::new(ResourceLimits::new()),
        CompileOptions::default(),
    );
    let code = "x = []\nfor _ in range(100):\n    x = [x]\nf(x)";
    let progress = repl
        .feed_start(code, vec![], PrintWriter::Stdout)
        .expect("feed_start suspends");
    assert!(
        matches!(progress, ReplProgress::FunctionCall(_)),
        "expected a FunctionCall suspension"
    );
    let payload = progress.dump().expect("in-process dump has no depth bound");

    // Assemble the dump envelope the way `Dump` does: [version u16 LE]
    // [tag u8 = 1 (suspended)][script_name str][type_check u8 = 0][payload].
    let mut state = 5u16.to_le_bytes().to_vec();
    state.push(1);
    let script_name = b"main.py";
    state.extend_from_slice(&u32::try_from(script_name.len()).unwrap().to_le_bytes());
    state.extend_from_slice(script_name);
    state.push(0);
    state.extend_from_slice(&payload);

    let mut child = Child::new();
    create_repl(&mut child);
    let request = frame_request(pb::parent_request::Kind::Load(pb::Load { state }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let (_, event) = split_turn(&bytes);
    let pb::child_event::Kind::Error(error) = event else {
        panic!("expected an Error event, got {event:?}");
    };
    assert_eq!(
        error.exception.unwrap().message.unwrap(),
        "protocol violation: dump suspension arguments exceed the maximum wire depth"
    );

    // the rejected load adopted nothing: the child is still fresh and usable
    let (_, event) = feed(&mut child, "1 + 1");
    assert_eq!(expect_complete(event), MontyObject::Int(2));
}
