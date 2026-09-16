//! In-process tests of the buffered per-turn entry point [`dispatch_frame`].
//!
//! This is the exact path a wasm Web Worker drives — framed request in, framed
//! events out — minus the FFI memory marshalling, so it round-trips the whole
//! `Child` state machine over the message-based transport without any wasm
//! toolchain.

use monty::{DUMP_VERSION, MontyRepl, ReplProgress, SessionRef, dump};
use monty_proto::{
    FrameReader, PROTOCOL_VERSION, WireArena, WireFunctionCall, named_values_to_proto, pb,
    worker::{Child, HandleOutcome, dispatch_frame},
    write_frame,
};
use monty_types::{CompileOptions, MONTY_VERSION, MontyObject, NamedValues, PrintWriter, ResourceTracker};

/// Starts a feed with `f` already bound, leaving the worker at its first external call.
fn start_external_call(child: &mut Child, code: &str) -> WireFunctionCall {
    create_repl(child);
    let inputs = NamedValues::from(vec![("f".to_owned(), MontyObject::function("f".to_owned(), None))]);
    let (inputs, values) = named_values_to_proto(inputs);
    let request = frame_request(pb::parent_request::Kind::Feed(pb::Feed {
        code: code.to_owned(),
        inputs,
        values: Some(values),
        skip_type_check: false,
        cwd: "/".to_owned(),
    }));
    let (bytes, _) = dispatch_frame(child, &request);
    let (_, event) = split_turn(&bytes);
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected call, got {event:?}")
    };
    call
}

/// Constructs one settled or malformed future reply without bypassing wire validation.
fn future_reply(call_id: u32, kind: pb::ext_function_result::Kind) -> pb::FutureResult {
    pb::FutureResult {
        call_id,
        result: Some(pb::ExtFunctionResult { kind: Some(kind) }),
    }
}

/// A `ResumeFutures` whose arena holds `value` at index 0, the root every
/// `ReturnValue(0)` reply names.
fn resume_futures(results: Vec<pb::FutureResult>, value: MontyObject) -> pb::ResumeFutures {
    pb::ResumeFutures {
        results,
        values: Some(WireArena::new(value.graph)),
    }
}

/// A one-node arena holding `value` at index 0.
fn arena(value: MontyObject) -> WireArena {
    WireArena::new(value.graph)
}

/// Each eager reply advances directly to the next call or completion.
#[test]
fn allow_eager_await_uses_one_reply_per_call() {
    let mut child = Child::default();
    let mut call = start_external_call(&mut child, "a = await f()\nb = await f()\na + b");
    for n in [10, 20] {
        assert!(call.allow_eager_await);
        let request = frame_request(pb::parent_request::Kind::ResumeFutures(resume_futures(
            vec![future_reply(
                call.call_id,
                pb::ext_function_result::Kind::ReturnValue(0),
            )],
            MontyObject::int(n),
        )));
        let (bytes, outcome) = dispatch_frame(&mut child, &request);
        assert_eq!(outcome, HandleOutcome::Continue);
        let (_, event) = split_turn(&bytes);
        if n == 10 {
            let pb::child_event::Kind::FunctionCall(next) = event else {
                panic!("expected next call, got {event:?}")
            };
            call = next;
        } else {
            assert_eq!(expect_complete(event), MontyObject::int(30));
        }
    }
}

/// Invalid eager replies leave the suspension available for a valid retry.
#[test]
fn allow_eager_await_rejects_malformed_replies() {
    let mut child = Child::default();
    let call = start_external_call(&mut child, "await f()");
    assert!(call.allow_eager_await);
    let value = pb::ext_function_result::Kind::ReturnValue(0);
    for results in [
        vec![],
        vec![future_reply(call.call_id + 1, value.clone())],
        vec![
            future_reply(call.call_id, value.clone()),
            future_reply(call.call_id, value.clone()),
        ],
        vec![future_reply(
            call.call_id,
            pb::ext_function_result::Kind::Future(call.call_id),
        )],
        vec![future_reply(
            call.call_id,
            pb::ext_function_result::Kind::NotFound("f".to_owned()),
        )],
        vec![pb::FutureResult {
            call_id: call.call_id,
            result: None,
        }],
    ] {
        let request = frame_request(pb::parent_request::Kind::ResumeFutures(resume_futures(
            results,
            MontyObject::int(42),
        )));
        let (bytes, outcome) = dispatch_frame(&mut child, &request);
        assert_eq!(outcome, HandleOutcome::Continue);
        assert!(matches!(split_turn(&bytes).1, pb::child_event::Kind::Error(_)));
    }
    let request = frame_request(pb::parent_request::Kind::ResumeFutures(resume_futures(
        vec![future_reply(call.call_id, value)],
        MontyObject::int(42),
    )));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert_eq!(expect_complete(split_turn(&bytes).1), MontyObject::int(42));
}

/// Older hosts can ignore the hint; calls without the hint reject the new reply sequence.
#[test]
fn allow_eager_await_preserves_legacy_replies() {
    let mut child = Child::default();
    let call = start_external_call(&mut child, "await f()");
    let request = frame_request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
        call_id: call.call_id,
        result: Some(pb::ExtFunctionResult {
            kind: Some(pb::ext_function_result::Kind::Future(call.call_id)),
        }),
        values: None,
    }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert!(matches!(split_turn(&bytes).1, pb::child_event::Kind::ResolveFutures(_)));
    let value = pb::ext_function_result::Kind::ReturnValue(0);
    let request = frame_request(pb::parent_request::Kind::ResumeFutures(resume_futures(
        vec![future_reply(call.call_id, value.clone())],
        MontyObject::int(42),
    )));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert_eq!(expect_complete(split_turn(&bytes).1), MontyObject::int(42));

    let mut child = Child::default();
    let call = start_external_call(&mut child, "f()");
    assert!(!call.allow_eager_await);
    let request = frame_request(pb::parent_request::Kind::ResumeFutures(resume_futures(
        vec![future_reply(call.call_id, value.clone())],
        MontyObject::int(42),
    )));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert!(matches!(split_turn(&bytes).1, pb::child_event::Kind::Error(_)));
    let request = frame_request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
        call_id: call.call_id,
        result: Some(pb::ExtFunctionResult { kind: Some(value) }),
        values: Some(arena(MontyObject::int(42))),
    }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert_eq!(expect_complete(split_turn(&bytes).1), MontyObject::int(42));
}

/// Frames one request the way a host transport would before posting it.
fn frame_request(kind: pb::parent_request::Kind) -> Vec<u8> {
    let mut buf = Vec::new();
    write_frame(
        &mut buf,
        &pb::ParentRequest {
            kind: Some(kind),
            trace_parent: None,
        },
    )
    .expect("framing a request never fails");
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
    create_repl_with_flush_interval(child, None);
}

/// `create_repl`, naming the print flush interval the session runs with.
fn create_repl_with_flush_interval(child: &mut Child, print_flush_interval_ms: Option<u32>) {
    let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: false,
        type_check_stubs: None,
        assert_message_annotations: None,
        monty_version: MONTY_VERSION.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        print_flush_interval_ms,
        ..Default::default()
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
        values: None,
        skip_type_check: false,
        cwd: "/".to_owned(),
    }));
    let (bytes, outcome) = dispatch_frame(child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    split_turn(&bytes)
}

/// The runs each `Print` event carried, as `(stream, text)` per event, so a
/// test can assert both what was batched together and in what order.
fn segments_per_event(prints: &[pb::Print]) -> Vec<Vec<(pb::PrintStream, &str)>> {
    prints
        .iter()
        .map(|print| {
            print
                .segments
                .iter()
                .map(|segment| (segment.stream(), segment.text.as_str()))
                .collect()
        })
        .collect()
}

fn expect_complete(event: pb::child_event::Kind) -> MontyObject {
    match event {
        pb::child_event::Kind::Complete(complete) => {
            MontyObject::try_from(complete).expect("the complete value decodes")
        }
        other => panic!("expected Complete, got {other:?}"),
    }
}

#[test]
fn feed_round_trips_a_value() {
    let mut child = Child::default();
    create_repl(&mut child);

    let (_, event) = feed(&mut child, "1 + 2");
    assert_eq!(expect_complete(event), MontyObject::int(3));
}

#[test]
fn session_state_persists_across_feeds() {
    let mut child = Child::default();
    create_repl(&mut child);

    let (_, first) = feed(&mut child, "x = 21");
    assert_eq!(expect_complete(first), MontyObject::none());

    let (_, second) = feed(&mut child, "x * 2");
    assert_eq!(expect_complete(second), MontyObject::int(42));
}

#[test]
fn print_output_is_streamed_before_the_terminator() {
    let mut child = Child::default();
    create_repl(&mut child);

    let (prints, event) = feed(&mut child, "print('hello'); print('world')");
    let streamed: String = prints
        .into_iter()
        .flat_map(|print| print.segments)
        .map(|segment| segment.text)
        .collect();
    assert_eq!(streamed, "hello\nworld\n");
    assert_eq!(expect_complete(event), MontyObject::none());
}

/// Output alternating between the streams batches into one event, holding a
/// segment per run: the worker never has to flush to change stream, so the
/// debounce applies to mixed output as much as to plain stdout.
#[test]
fn alternating_streams_batch_into_one_event() {
    let mut child = Child::default();
    // an interval no test will wait out leaves the flush to `drain`, so this
    // asserts how output is segmented rather than how the timer fires
    create_repl_with_flush_interval(&mut child, Some(60_000));

    let (prints, event) = feed(
        &mut child,
        "import sys\nprint('a')\nprint('b', file=sys.stderr)\nprint('c')",
    );
    assert_eq!(expect_complete(event), MontyObject::none());
    assert_eq!(
        segments_per_event(&prints),
        vec![vec![
            (pb::PrintStream::Stdout, "a\n"),
            (pb::PrintStream::Stderr, "b\n"),
            (pb::PrintStream::Stdout, "c\n"),
        ]]
    );
}

/// With the timer off, a completed line still ends the event whichever stream
/// wrote it, so a host that asked for line buffering gets one event per line.
#[test]
fn line_buffering_gives_each_line_its_own_event() {
    let mut child = Child::default();
    create_repl_with_flush_interval(&mut child, Some(0));

    let (prints, event) = feed(&mut child, "import sys\nprint('a')\nprint('b', file=sys.stderr)");
    assert_eq!(expect_complete(event), MontyObject::none());
    assert_eq!(
        segments_per_event(&prints),
        vec![
            vec![(pb::PrintStream::Stdout, "a\n")],
            vec![(pb::PrintStream::Stderr, "b\n")],
        ]
    );
}

/// A line the sandbox wrote across both streams is one line, so line buffering
/// ships the runs it spans in one event rather than cutting at the switch. The
/// trailing partial line waits for the drain at the end of the turn.
#[test]
fn line_buffering_keeps_a_line_spanning_the_streams_together() {
    let mut child = Child::default();
    create_repl_with_flush_interval(&mut child, Some(0));

    let (prints, event) = feed(
        &mut child,
        "import sys\nprint('out', end='')\nprint('err', file=sys.stderr)\nprint('next', end='')",
    );
    assert_eq!(expect_complete(event), MontyObject::none());
    assert_eq!(
        segments_per_event(&prints),
        vec![
            vec![(pb::PrintStream::Stdout, "out"), (pb::PrintStream::Stderr, "err\n")],
            vec![(pb::PrintStream::Stdout, "next")],
        ]
    );
}

#[test]
fn inputs_are_injected() {
    let mut child = Child::default();
    create_repl(&mut child);

    let (inputs, values) = named_values_to_proto(NamedValues::from(vec![("n".to_owned(), MontyObject::int(41))]));
    let request = frame_request(pb::parent_request::Kind::Feed(pb::Feed {
        code: "n + 1".to_owned(),
        inputs,
        values: Some(values),
        skip_type_check: false,
        cwd: "/".to_owned(),
    }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let (_, event) = split_turn(&bytes);
    assert_eq!(expect_complete(event), MontyObject::int(42));
}

#[test]
fn malformed_request_frame_is_recoverable() {
    let mut child = Child::default();
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
    let mut child = Child::default();
    create_repl(&mut child);

    let request = frame_request(pb::parent_request::Kind::Shutdown(pb::Shutdown {}));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Shutdown);
    assert!(
        matches!(decode_events(&bytes).as_slice(), [pb::child_event::Kind::Ok(_)]),
        "Shutdown answers with a single Ok"
    );
}

/// A dump written by a different `DUMP_VERSION` is rejected, and the error
/// names both versions so a host can tell a stale snapshot from a corrupt one.
#[test]
fn load_rejects_old_dump_version() {
    // a real dump rewound to the previous version, so only the version is wrong
    let repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    let mut state = dump("main.py", None, SessionRef::Idle(&repl)).expect("dumping an idle repl succeeds");
    state[6..8].copy_from_slice(&(DUMP_VERSION - 1).to_le_bytes());

    let mut child = Child::default();
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
        format!(
            "protocol violation: failed to load session: dump format version {}, this build reads {DUMP_VERSION}",
            DUMP_VERSION - 1
        )
    );
}

/// A suspended dump whose call argument nests deeply is re-announced as-is on
/// `Load`: the arena carries any depth, so nothing bounds it on the wire.
#[test]
fn load_re_announces_deep_suspension_args() {
    let repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    // nested 100 lists deep: far past any bound the wire used to impose, and
    // shallow enough that postcard's recursive deserialize fits the test stack
    let code = "x = []\nfor _ in range(100):\n    x = [x]\nf(x)";
    let progress = repl
        .feed_start(code, vec![], PrintWriter::Stdout)
        .expect("feed_start suspends");
    assert!(
        matches!(progress, ReplProgress::FunctionCall(_)),
        "expected a FunctionCall suspension"
    );
    let state = dump("main.py", None, SessionRef::Suspended(&progress)).expect("suspended dump");

    let mut child = Child::default();
    create_repl(&mut child);
    let request = frame_request(pb::parent_request::Kind::Load(pb::Load { state }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let (_, event) = split_turn(&bytes);
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected the re-announced FunctionCall, got {event:?}");
    };
    assert_eq!(call.function_name, "f");
    // the empty innermost list, 100 wrappers, and nothing else
    assert_eq!(call.values.0.len(), 101);
}

/// Decodes complete events, including their session budget fields.
fn decode_full_events(bytes: &[u8]) -> Vec<pb::ChildEvent> {
    let mut reader = FrameReader::new(bytes);
    let mut events = Vec::new();
    while let Some(event) = reader.read::<pb::ChildEvent>().expect("reply frames decode") {
        events.push(event);
    }
    events
}

/// `AbortFeed` raises the supplied error uncatchably and leaves the session usable.
#[test]
fn abort_feed_ends_a_suspended_feed_uncatchably() {
    let mut child = Child::default();
    create_repl(&mut child);
    let (_, event) = feed(
        &mut child,
        "x = 41\ntry:\n    fetch('x')\nexcept Exception:\n    x = 0\n",
    );
    let pb::child_event::Kind::FunctionCall(_) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    let request = frame_request(pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
        exception: Some(pb::RaisedException {
            exc_type: "RuntimeError".to_owned(),
            message: Some("suspension limit 3 exceeded".to_owned()),
            traceback: vec![],
            data: None,
        }),
    }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let (_, event) = split_turn(&bytes);
    let pb::child_event::Kind::Error(error) = event else {
        panic!("expected Error, got {event:?}");
    };
    let exception = error.exception.expect("error carries the exception");
    assert_eq!(exception.exc_type, "RuntimeError");
    assert_eq!(exception.message.as_deref(), Some("suspension limit 3 exceeded"));
    // raised where the feed stopped: the traceback names the call's line
    assert_eq!(exception.traceback.len(), 1);
    assert_eq!(exception.traceback[0].start.map(|loc| loc.line), Some(3));
    // the session survives with the globals from before the suspension
    let (_, event) = feed(&mut child, "x");
    assert_eq!(expect_complete(event), MontyObject::int(41));
}

/// `AbortFeed` is only meaningful while a feed is suspended.
#[test]
fn abort_feed_without_a_suspension_is_a_protocol_violation() {
    let mut child = Child::default();
    create_repl(&mut child);
    let request = frame_request(pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
        exception: Some(pb::RaisedException {
            exc_type: "RuntimeError".to_owned(),
            message: None,
            traceback: vec![],
            data: None,
        }),
    }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let (_, event) = split_turn(&bytes);
    let pb::child_event::Kind::Error(error) = event else {
        panic!("expected Error, got {event:?}");
    };
    let exception = error.exception.expect("error carries the exception");
    assert_eq!(
        exception.message.as_deref(),
        Some("protocol violation: AbortFeed without a suspended feed")
    );
    // the session is untouched
    let (_, event) = feed(&mut child, "1 + 1");
    assert_eq!(expect_complete(event), MontyObject::int(2));
}

/// The child echoes the session's `max_suspensions` on every turn-ending
/// event, so a parent restoring a dump learns the budget it must enforce.
#[test]
fn turn_events_carry_the_suspension_budget() {
    let mut child = Child::default();
    let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: Some(pb::ResourceLimits {
            max_suspensions: Some(3),
            ..Default::default()
        }),
        monty_version: MONTY_VERSION.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        ..Default::default()
    }));
    let (_, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let request = frame_request(pb::parent_request::Kind::Feed(pb::Feed {
        code: "1 + 1".to_owned(),
        inputs: vec![],
        values: None,
        skip_type_check: false,
        cwd: "/".to_owned(),
    }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    let events = decode_full_events(&bytes);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].max_suspensions, Some(3));
}
