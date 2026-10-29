//! In-process tests of the buffered per-turn entry point [`dispatch_frame`].
//!
//! This is the exact path a wasm Web Worker drives — framed request in, framed
//! events out — minus the FFI memory marshalling, so it round-trips the whole
//! `Child` state machine over the message-based transport without any wasm
//! toolchain.

use std::io;

use monty::{Dump, MIN_SUPPORTED_DUMP_VERSION, MontyRepl, ReplProgress, SessionRef, dump};
use monty_proto::{
    FrameError, FrameReader, PROTOCOL_VERSION, WireArena, WireFunctionCall, named_values_to_proto, pb,
    worker::{Child, EventSink, HandleOutcome, dispatch_frame},
    write_frame,
};
use monty_types::{
    CompileOptions, ExcType, IMPORT_STUBS_CALL_ID, MONTY_VERSION, MontyException, MontyObject, MontyUuid, NamedValues,
    PrintWriter, ResourceLimits, ResourceTracker, unstable,
};

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
        results: results.into(),
        values: Some(WireArena::new(unstable::into_graph_parts(value).0)),
    }
}

/// A one-node arena holding `value` at index 0.
fn arena(value: MontyObject) -> WireArena {
    WireArena::new(unstable::into_graph_parts(value).0)
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
    feed_with_cwd(child, code, "/")
}

/// [`feed`] with the wire `cwd`, where `""` keeps the session's working directory.
fn feed_with_cwd(child: &mut Child, code: &str, cwd: &str) -> (Vec<pb::Print>, pb::child_event::Kind) {
    let request = frame_request(pb::parent_request::Kind::Feed(pb::Feed {
        code: code.to_owned(),
        inputs: vec![].into(),
        values: None,
        skip_type_check: false,
        cwd: cwd.to_owned(),
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

/// A dump below `MIN_SUPPORTED_DUMP_VERSION` is rejected, and the error names
/// the bound it missed so a host can tell a stale snapshot from a corrupt one.
#[test]
fn load_rejects_old_dump_version() {
    // a real dump rewound to the previous version, so only the version is wrong
    let repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    let mut state = dump("main.py", None, &[], SessionRef::Idle(&repl)).expect("dumping an idle repl succeeds");
    state[6..8].copy_from_slice(&(MIN_SUPPORTED_DUMP_VERSION - 1).to_le_bytes());

    let mut child = Child::default();
    create_repl(&mut child);
    let request = frame_request(pb::parent_request::Kind::Load(pb::Load { state: state.into() }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let (_, event) = split_turn(&bytes);
    let pb::child_event::Kind::Error(error) = event else {
        panic!("expected an Error event, got {event:?}");
    };
    assert_eq!(
        error.exception.unwrap().message.unwrap(),
        format!(
            "protocol violation: failed to load session: dump format version {} is older than \
             {MIN_SUPPORTED_DUMP_VERSION}, the oldest this build reads",
            MIN_SUPPORTED_DUMP_VERSION - 1
        )
    );
}

/// A suspended dump whose call argument nests deeply is re-announced as-is on
/// `Load`: the arena carries any depth, so nothing bounds it on the wire.
#[test]
fn load_re_announces_deep_suspension_args() {
    let repl = MontyRepl::new("main.py", ResourceTracker::default(), CompileOptions::default());
    // nested 100 lists deep; the arena is flat, so decoding never recurses
    let code = "x = []\nfor _ in range(100):\n    x = [x]\nf(x)";
    let progress = repl
        .feed_start(code, vec![], PrintWriter::Stdout)
        .expect("feed_start suspends");
    assert!(
        matches!(progress, ReplProgress::FunctionCall(_)),
        "expected a FunctionCall suspension"
    );
    let state = dump("main.py", None, &[], SessionRef::Suspended(&progress)).expect("suspended dump");

    let mut child = Child::default();
    create_repl(&mut child);
    let request = frame_request(pb::parent_request::Kind::Load(pb::Load { state: state.into() }));
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

/// Restore caps both idle and suspended trackers before reporting the session budget.
#[test]
fn restore_caps_idle_and_suspended_resource_limits() {
    for suspended in [false, true] {
        let repl = MontyRepl::new(
            "saved.py",
            ResourceTracker::new(
                ResourceLimits::default()
                    .max_memory(64 * 1024 * 1024)
                    .max_suspensions(20),
            ),
            CompileOptions::default(),
        );
        let progress = repl
            .feed_start(
                if suspended { "x = 42\nf(x)" } else { "x = 42" },
                vec![("f".to_owned(), MontyObject::function("f".to_owned(), None))],
                PrintWriter::Stdout,
            )
            .unwrap();
        let session = match &progress {
            ReplProgress::Complete { repl, .. } => SessionRef::Idle(repl),
            progress => SessionRef::Suspended(progress),
        };
        let state = dump("saved.py", None, &[], session).unwrap();
        let mut child = Child::default();
        let configure = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
            protocol_version: PROTOCOL_VERSION,
            monty_version: MONTY_VERSION.to_owned(),
            limits: Some(
                (&ResourceLimits::default()
                    .max_memory(10 * 1024 * 1024)
                    .max_suspensions(3))
                    .into(),
            ),
            ..Default::default()
        }));
        dispatch_frame(&mut child, &configure);
        let request = frame_request(pb::parent_request::Kind::Load(pb::Load { state: state.into() }));
        let (bytes, outcome) = dispatch_frame(&mut child, &request);
        assert_eq!(outcome, HandleOutcome::Continue);
        let events = decode_full_events(&bytes);
        assert_eq!(child.session_budget().max_memory, Some(10 * 1024 * 1024));
        assert_eq!(child.session_budget().max_suspensions, Some(3));
        assert_eq!(events.last().unwrap().max_suspensions, Some(3));
        assert_eq!(events.last().unwrap().restored_script_name.as_deref(), Some("saved.py"));
        if suspended {
            assert!(matches!(
                events.last().unwrap().kind,
                Some(pb::child_event::Kind::FunctionCall(_))
            ));
        } else {
            assert_eq!(expect_complete(feed(&mut child, "x").1), MontyObject::int(42));
        }
    }
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
            traceback: vec![].into(),
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
            traceback: vec![].into(),
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
        inputs: vec![].into(),
        values: None,
        skip_type_check: false,
        cwd: "/".to_owned(),
    }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    let events = decode_full_events(&bytes);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].max_suspensions, Some(3));
}

// ---------------------------------------------------------------------------
// Host modules: `import` and `import_stubs`
// ---------------------------------------------------------------------------

fn configure(child: &mut Child, type_check: bool) {
    let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "main.py".to_owned(),
        type_check,
        type_check_format: pb::TypeCheckFormat::Concise.into(),
        monty_version: MONTY_VERSION.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        ..Default::default()
    }));
    let (bytes, outcome) = dispatch_frame(child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    assert!(matches!(split_turn(&bytes).1, pb::child_event::Kind::Ok(_)));
}

fn expect_error_message(event: pb::child_event::Kind) -> String {
    let pb::child_event::Kind::Error(error) = event else {
        panic!("expected an Error event, got {event:?}");
    };
    error.exception.unwrap().message.unwrap()
}

fn expect_typing_error(event: pb::child_event::Kind) -> String {
    let pb::child_event::Kind::TypingError(typing) = event else {
        panic!("expected a TypingError event, got {event:?}");
    };
    typing.diagnostics
}

fn expect_import_stubs(event: pb::child_event::Kind, position: (&str, u32, u32)) -> Vec<String> {
    let pb::child_event::Kind::OsCall(call) = event else {
        panic!("expected an import_stubs ask, got {event:?}");
    };
    assert_eq!(call.call_id, IMPORT_STUBS_CALL_ID);
    assert!(!call.allow_eager_await);
    let range = call.position.expect("the ask names the first import");
    assert_eq!((range.filename.as_str(), range.start, range.end), position);
    let Some(pb::os_call::Call::ImportStubs(ask)) = call.call else {
        panic!("expected an import_stubs ask, got {:?}", call.call);
    };
    ask.modules.into_iter().collect()
}

fn expect_import(event: pb::child_event::Kind) -> String {
    let pb::child_event::Kind::OsCall(call) = event else {
        panic!("expected an import, got {event:?}");
    };
    let Some(pb::os_call::Call::Import(import)) = call.call else {
        panic!("expected an import, got {:?}", call.call);
    };
    import.module
}

fn answer_stubs(
    child: &mut Child,
    result: pb::ext_function_result::Kind,
    values: Option<WireArena>,
) -> pb::child_event::Kind {
    let request = frame_request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
        call_id: IMPORT_STUBS_CALL_ID,
        result: Some(pb::ExtFunctionResult { kind: Some(result) }),
        values,
    }));
    let (bytes, _) = dispatch_frame(child, &request);
    split_turn(&bytes).1
}

fn answer_stubs_with(child: &mut Child, stubs: &[(&str, Option<&str>)]) -> pb::child_event::Kind {
    let answer = MontyObject::dict(stubs.iter().map(|(module, source)| {
        (
            MontyObject::string(*module),
            source.map_or_else(MontyObject::none, MontyObject::string),
        )
    }));
    let (graph, root) = unstable::into_graph_parts(answer);
    answer_stubs(
        child,
        pb::ext_function_result::Kind::ReturnValue(root.0),
        Some(WireArena::new(graph)),
    )
}

/// The request names the first import; dotted answer keys define package stubs.
#[test]
fn a_type_checked_feed_asks_for_module_stubs_once() {
    let mut child = Child::default();
    configure(&mut child, true);
    let (_, event) = feed(
        &mut child,
        "x = 1\nimport tools\nfrom tools.sub import add, VERSION\nadd('x', 2)",
    );
    assert_eq!(
        expect_import_stubs(event, ("<python-input-0>", 6, 18)),
        vec!["tools".to_owned()]
    );
    let event = answer_stubs_with(
        &mut child,
        &[
            ("tools", Some("x: int\n")),
            ("tools.sub", Some("VERSION: int\ndef add(a: int, b: int) -> int: ...\n")),
        ],
    );
    insta::assert_snapshot!(expect_typing_error(event), @r#"main.py:4:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`"#);
    // the stubs are kept, so the retry asks nothing and the import suspends
    let (_, event) = feed(&mut child, "import tools\ntools.x");
    assert_eq!(expect_import(event), "tools");
}

/// Missing stubs leave imports unresolved for the rest of the session.
#[test]
fn a_module_without_a_stub_is_asked_for_once() {
    for answer in [
        None,
        Some(pb::ext_function_result::Kind::NotFound("import_stubs".to_owned())),
        Some(pb::ext_function_result::Kind::NotHandled(pb::Unit {})),
    ] {
        let mut child = Child::default();
        configure(&mut child, true);
        let (_, event) = feed(&mut child, "import tools, other");
        assert_eq!(
            expect_import_stubs(event, ("<python-input-0>", 0, 19)),
            vec!["tools".to_owned(), "other".to_owned()]
        );
        let event = match answer {
            None => answer_stubs_with(&mut child, &[("tools", None)]),
            Some(kind) => answer_stubs(&mut child, kind, None),
        };
        assert_eq!(
            expect_typing_error(event),
            "main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`\n\
             main.py:1:15: error[unresolved-import] Cannot resolve imported module `other`\n"
        );
        let (_, event) = feed(&mut child, "import tools");
        assert_eq!(
            expect_typing_error(event),
            "main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`\n"
        );
    }
}

/// Built-in and checker-provided modules need no host stubs; unchecked feeds request none.
#[test]
fn only_host_modules_of_a_type_checked_feed_are_asked_for() {
    let mut child = Child::default();
    configure(&mut child, true);
    let (_, event) = feed(&mut child, "import json, abc\nimport tools");
    assert_eq!(
        expect_import_stubs(event, ("<python-input-0>", 17, 29)),
        vec!["tools".to_owned()]
    );
    let event = answer_stubs_with(&mut child, &[("tools", Some(""))]);
    assert_eq!(expect_import(event), "abc");

    let mut child = Child::default();
    configure(&mut child, true);
    let request = frame_request(pb::parent_request::Kind::Feed(pb::Feed {
        code: "import tools".to_owned(),
        inputs: vec![].into(),
        values: None,
        skip_type_check: true,
        cwd: String::new(),
    }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert_eq!(expect_import(split_turn(&bytes).1), "tools");

    let mut child = Child::default();
    configure(&mut child, false);
    let (_, event) = feed(&mut child, "import tools");
    assert_eq!(expect_import(event), "tools");
}

/// The parent counts a sent directory as adopted, so the worker must switch
/// before the stubs ask, even when the resumed check then rejects the feed.
#[test]
fn a_directory_switch_survives_a_typing_rejection_after_the_stubs_ask() {
    let mut child = Child::default();
    configure(&mut child, true);
    let (_, event) = feed_with_cwd(&mut child, "import tools\ntools.add('x', 2)", "/work");
    assert_eq!(
        expect_import_stubs(event, ("<python-input-0>", 0, 12)),
        vec!["tools".to_owned()]
    );
    let event = answer_stubs_with(&mut child, &[("tools", Some("def add(a: int, b: int) -> int: ...\n"))]);
    insta::assert_snapshot!(expect_typing_error(event), @r#"main.py:2:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`"#);
    let (_, event) = feed_with_cwd(&mut child, "import os\nos.getcwd()", "");
    assert_eq!(expect_complete(event), MontyObject::string("/work"));
}

/// Malformed answers leave the request pending; exception answers end it before execution.
#[test]
fn a_bad_stubs_answer_is_refused_and_an_exception_ends_the_feed() {
    let mut child = Child::default();
    configure(&mut child, true);
    let (_, event) = feed(&mut child, "import tools");
    assert_eq!(
        expect_import_stubs(event, ("<python-input-0>", 0, 12)),
        vec!["tools".to_owned()]
    );
    let (graph, root) = unstable::into_graph_parts(MontyObject::int(1));
    let event = answer_stubs(
        &mut child,
        pb::ext_function_result::Kind::ReturnValue(root.0),
        Some(WireArena::new(graph)),
    );
    insta::assert_snapshot!(expect_error_message(event), @"protocol violation: import_stubs must be answered with a dict of module to stub source");
    let event = answer_stubs_with(&mut child, &[("other", Some(""))]);
    insta::assert_snapshot!(expect_error_message(event), @r#"protocol violation: import_stubs answered for "other", which was not asked for"#);
    let event = answer_stubs_with(&mut child, &[("tools.1sub", Some(""))]);
    insta::assert_snapshot!(expect_error_message(event), @r#"protocol violation: import_stubs: module name "tools.1sub" is not a valid identifier"#);
    let event = answer_stubs(
        &mut child,
        pb::ext_function_result::Kind::Future(IMPORT_STUBS_CALL_ID),
        None,
    );
    insta::assert_snapshot!(expect_error_message(event), @"protocol violation: import_stubs cannot be answered with a future");
    // a dump cannot carry the held feed
    let request = frame_request(pb::parent_request::Kind::Dump(pb::Dump {}));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    insta::assert_snapshot!(expect_error_message(split_turn(&bytes).1), @"cannot dump while a feed awaits its module stubs");
    let boom = MontyException::new(ExcType::ImportError, Some("boom".to_owned()));
    let event = answer_stubs(&mut child, pb::ext_function_result::Kind::Error((&boom).into()), None);
    insta::assert_snapshot!(expect_error_message(event), @"boom");
    // the session is ready, and the feed never became type-check context
    let (_, event) = feed(&mut child, "import tools");
    assert_eq!(
        expect_import_stubs(event, ("<python-input-0>", 0, 12)),
        vec!["tools".to_owned()]
    );
}

#[test]
fn abort_during_the_stubs_ask_ends_the_feed() {
    let mut child = Child::default();
    configure(&mut child, true);
    let (_, event) = feed(&mut child, "import tools");
    assert_eq!(
        expect_import_stubs(event, ("<python-input-0>", 0, 12)),
        vec!["tools".to_owned()]
    );
    let stop = MontyException::new(ExcType::RuntimeError, Some("stopped".to_owned()));
    let request = frame_request(pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
        exception: Some((&stop).into()),
    }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    insta::assert_snapshot!(expect_error_message(split_turn(&bytes).1), @"stopped");
    let (_, event) = feed(&mut child, "1 + 1");
    assert_eq!(expect_complete(event), MontyObject::int(2));
}

/// Restoring a suspended import re-announces it and accepts a module value.
#[test]
fn load_restores_a_suspended_import() {
    let mut child = Child::default();
    create_repl(&mut child);
    let (_, event) = feed(&mut child, "import tools\ntools.x");
    assert_eq!(expect_import(event), "tools");
    let request = frame_request(pb::parent_request::Kind::Dump(pb::Dump {}));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    let pb::child_event::Kind::DumpResult(result) = split_turn(&bytes).1 else {
        panic!("expected DumpResult");
    };

    let mut child = Child::default();
    create_repl(&mut child);
    let request = frame_request(pb::parent_request::Kind::Load(pb::Load { state: result.state }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    let pb::child_event::Kind::OsCall(call) = split_turn(&bytes).1 else {
        panic!("expected the re-announced import");
    };
    let call_id = call.call_id;
    assert!(matches!(call.call, Some(pb::os_call::Call::Import(pb::os_call::Import { module })) if module == "tools"));

    let tools = MontyObject::class_instance(
        MontyObject::class_type("tools", MontyUuid::from_u128(1), true, true, []),
        MontyUuid::from_u128(2),
        [(MontyObject::string("x"), MontyObject::int(41))],
    );
    // post-order: the root is the last node, not index 0
    let (graph, root) = unstable::into_graph_parts(tools);
    let request = frame_request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
        call_id,
        result: Some(pb::ExtFunctionResult {
            kind: Some(pb::ext_function_result::Kind::ReturnValue(root.0)),
        }),
        values: Some(WireArena::new(graph)),
    }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert_eq!(expect_complete(split_turn(&bytes).1), MontyObject::int(41));
}

/// Dumps preserve both received stubs and modules answered without stubs.
#[test]
fn load_restores_the_received_module_stubs() {
    let mut child = Child::default();
    configure(&mut child, true);
    let (_, event) = feed(&mut child, "import tools, other");
    assert_eq!(
        expect_import_stubs(event, ("<python-input-0>", 0, 19)),
        vec!["tools".to_owned(), "other".to_owned()]
    );
    let event = answer_stubs_with(&mut child, &[("tools", Some("x: int\n"))]);
    insta::assert_snapshot!(expect_typing_error(event), @"main.py:1:15: error[unresolved-import] Cannot resolve imported module `other`");
    let request = frame_request(pb::parent_request::Kind::Dump(pb::Dump {}));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    let pb::child_event::Kind::DumpResult(result) = split_turn(&bytes).1 else {
        panic!("expected DumpResult");
    };

    let mut child = Child::default();
    configure(&mut child, true);
    let request = frame_request(pb::parent_request::Kind::Load(pb::Load { state: result.state }));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    assert!(matches!(split_turn(&bytes).1, pb::child_event::Kind::Ok(_)));
    let (_, event) = feed(&mut child, "import other");
    insta::assert_snapshot!(expect_typing_error(event), @"main.py:1:8: error[unresolved-import] Cannot resolve imported module `other`");
    let (_, event) = feed(&mut child, "import tools\ntools.x");
    assert_eq!(expect_import(event), "tools");
}

/// Deduplication bounds the prelude across repeated feeds while preserving import order.
#[test]
fn a_repeated_import_is_carried_once() {
    let mut child = Child::default();
    configure(&mut child, true);
    for code in ["import math", "import math\nimport json", "import math"] {
        let (_, event) = feed(&mut child, code);
        assert!(matches!(event, pb::child_event::Kind::Complete(_)), "{event:?}");
    }
    let request = frame_request(pb::parent_request::Kind::Dump(pb::Dump {}));
    let (bytes, _) = dispatch_frame(&mut child, &request);
    let pb::child_event::Kind::DumpResult(result) = split_turn(&bytes).1 else {
        panic!("expected DumpResult");
    };
    let state = Dump::load(&result.state.into_inner()).expect("a dump this build wrote loads");
    assert_eq!(
        state.type_check.unwrap().committed_imports,
        "import json\nimport math\n"
    );
}

/// The latest import binding determines the type seen by later feeds.
#[test]
fn a_repeated_import_rebinds_its_name() {
    let mut child = Child::default();
    configure(&mut child, true);
    for code in ["import math as m", "import json as m", "import math as m"] {
        let (_, event) = feed(&mut child, code);
        assert!(matches!(event, pb::child_event::Kind::Complete(_)), "{event:?}");
    }
    let (_, event) = feed(&mut child, "m.sqrt(4)");
    assert!(matches!(event, pb::child_event::Kind::Complete(_)), "{event:?}");
    let (_, event) = feed(&mut child, "m.loads('4')");
    insta::assert_snapshot!(expect_typing_error(event), @"main.py:1:1: error[unresolved-attribute] Module `math` has no member `loads`");
}

#[test]
fn a_committed_import_carries_into_the_next_feed() {
    let mut child = Child::default();
    configure(&mut child, true);
    let (_, event) = feed(&mut child, "import math as m");
    assert!(matches!(event, pb::child_event::Kind::Complete(_)), "{event:?}");
    let (_, event) = feed(&mut child, "m.sqrt('4')");
    insta::assert_snapshot!(expect_typing_error(event), @r#"main.py:1:8: error[invalid-argument-type] Argument to function `sqrt` is incorrect: Expected `SupportsFloat | SupportsIndex`, found `Literal["4"]`"#);
}

/// Offers the real receiver model, with optional changes for negotiation tests.
fn configure_profile(
    child: &mut Child,
    profile: Option<pb::ReceiverAllocationProfile>,
    version: u32,
    type_check: bool,
) -> pb::ChildEvent {
    let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "main.py".to_owned(),
        protocol_version: version,
        receiver_allocation_profile: profile,
        type_check,
        print_flush_interval_ms: Some(60_000),
        ..Default::default()
    }));
    let (bytes, outcome) = dispatch_frame(child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let mut reader = FrameReader::new(bytes.as_slice());
    let event = reader.read().unwrap().unwrap();
    assert!(reader.read::<pb::ChildEvent>().unwrap().is_none());
    event
}

/// Uses the actual receiver budget on every frame, with exactly one final event.
fn admitted_turn(child: &mut Child, kind: pb::parent_request::Kind, budget: usize) -> Vec<pb::ChildEvent> {
    let request = frame_request(kind);
    let (bytes, outcome) = dispatch_frame(child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    let mut rest = bytes.as_slice();
    let mut events = Vec::new();
    while !rest.is_empty() {
        let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        events.push(monty_proto::decode_frame_with_budget::<pb::ChildEvent>(&rest[4..4 + len], budget).unwrap());
        rest = &rest[4 + len..];
    }
    assert_ne!(events, []);
    assert!(
        events[..events.len() - 1]
            .iter()
            .all(|event| matches!(event.kind, Some(pb::child_event::Kind::Print(_))))
    );
    assert!(!matches!(
        events.last().unwrap().kind,
        Some(pb::child_event::Kind::Print(_))
    ));
    events
}

/// Builds a feed without input values so the reply alone determines admission.
fn admission_feed(code: &str) -> pb::parent_request::Kind {
    pb::parent_request::Kind::Feed(pb::Feed {
        code: code.to_owned(),
        cwd: "/".to_owned(),
        ..Default::default()
    })
}

/// Requires the bounded replacement rather than a transport or decoder error.
fn assert_budget_replacement(event: &pb::ChildEvent) {
    assert_budget_error(event);
    let Some(pb::child_event::Kind::Error(error)) = &event.kind else {
        unreachable!()
    };
    assert!(error.exception.as_ref().unwrap().traceback.is_empty());
    assert!(event.restored_script_name.is_none());
}

/// Print failures may retain their ordinary traceback when the final error fits.
fn assert_budget_error(event: &pb::ChildEvent) {
    let Some(pb::child_event::Kind::Error(error)) = &event.kind else {
        panic!("expected Error: {event:?}")
    };
    let exception = error.exception.as_ref().unwrap();
    assert_eq!(exception.exc_type, "RuntimeError");
    assert_eq!(
        exception.message.as_deref(),
        Some("response exceeds host decode budget")
    );
    assert!(exception.data.is_none());
}

/// Negotiation is explicit and independent of configuration consumption and resets.
#[test]
fn allocation_profiles_are_acknowledged_only_when_usable() {
    let profile = monty_proto::receiver_allocation_profile(4096);
    let mut unknown = profile;
    unknown.model_version = 2;
    let mut invalid = profile;
    invalid.reference_slot_bytes = 0;
    let mut huge = profile;
    huge.max_decode_bytes = u64::MAX;
    for (offer, version, active) in [
        (Some(profile), PROTOCOL_VERSION, true),
        (None, PROTOCOL_VERSION, false),
        (Some(unknown), PROTOCOL_VERSION, false),
        (Some(invalid), PROTOCOL_VERSION, false),
        (Some(huge), PROTOCOL_VERSION, false),
        (Some(profile), 6, false),
        (Some(profile), 5, false),
        (
            Some(monty_proto::receiver_allocation_profile(46)),
            PROTOCOL_VERSION,
            false,
        ),
        (
            Some(monty_proto::receiver_allocation_profile(0)),
            PROTOCOL_VERSION,
            false,
        ),
    ] {
        let mut child = Child::default();
        let event = configure_profile(&mut child, offer, version, false);
        assert!(matches!(event.kind, Some(pb::child_event::Kind::Ok(_))));
        assert_eq!(event.accepted_receiver_allocation_profile_version, active.then_some(1));
        let events = admitted_turn(&mut child, admission_feed("1"), 4096);
        assert!(events[0].accepted_receiver_allocation_profile_version.is_none());
        assert!(matches!(events[0].kind, Some(pb::child_event::Kind::Complete(_))));
        let events = admitted_turn(&mut child, pb::parent_request::Kind::Reset(pb::Reset {}), 4096);
        assert!(events[0].accepted_receiver_allocation_profile_version.is_none());
        assert!(
            configure_profile(&mut child, None, PROTOCOL_VERSION, false)
                .accepted_receiver_allocation_profile_version
                .is_none()
        );
        let events = admitted_turn(
            &mut child,
            admission_feed("list(range(128))"),
            monty_proto::DEFAULT_MAX_DECODE_BYTES,
        );
        assert!(matches!(events[0].kind, Some(pb::child_event::Kind::Complete(_))));
    }
    let mut child = Child::default();
    assert_eq!(
        configure_profile(
            &mut child,
            Some(monty_proto::receiver_allocation_profile(47)),
            PROTOCOL_VERSION,
            false
        )
        .accepted_receiver_allocation_profile_version,
        Some(1)
    );
    assert_budget_replacement(&admitted_turn(&mut child, admission_feed("1"), 47)[0]);
}

/// Exact-charge acceptance and one-byte rejection preserve committed globals.
#[test]
fn admitted_complete_boundary_preserves_the_repl() {
    let values = MontyObject::list((0..16).map(MontyObject::int));
    let complete: pb::Complete = values.into();
    let event = pb::ChildEvent {
        kind: Some(pb::child_event::Kind::Complete(complete)),
        ..Default::default()
    };
    let profile = monty_proto::receiver_allocation_profile(4096);
    let charge = usize::try_from(monty_proto::canonical_event_decode_bytes(&event, &profile).unwrap()).unwrap();
    for budget in [charge, charge - 1] {
        let mut child = Child::default();
        configure_profile(
            &mut child,
            Some(monty_proto::receiver_allocation_profile(budget)),
            PROTOCOL_VERSION,
            false,
        );
        admitted_turn(&mut child, admission_feed("kept = 41"), budget);
        let events = admitted_turn(&mut child, admission_feed("list(range(16))"), budget);
        if budget == charge {
            assert_eq!(
                expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
                MontyObject::list((0..16).map(MontyObject::int))
            );
        } else {
            assert_budget_replacement(&events[0]);
        }
        let events = admitted_turn(&mut child, admission_feed("kept + 1"), budget);
        assert_eq!(
            expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
            MontyObject::int(42)
        );
    }
}

/// A rejected suspension never leaves an unannounced resume point in the child.
#[test]
fn allocation_rejection_recovers_fresh_suspensions_and_stub_requests() {
    for (code, type_check) in [
        ("missing_name_whose_length_exceeds_the_offered_allocation_budget", false),
        ("import os\nos.getenv('x' * 200)", false),
        ("import extraordinarily_long_module_name_for_stubs", true),
    ] {
        let mut child = Child::default();
        let mut profile = monty_proto::receiver_allocation_profile(4096);
        // A modeled receiver with expensive node and repeated-string slots.
        profile.string_slot_bytes = 4096;
        profile.node_slot_bytes = 4097;
        if !type_check {
            profile.max_decode_bytes = 47;
        }
        configure_profile(&mut child, Some(profile), PROTOCOL_VERSION, type_check);
        let events = admitted_turn(
            &mut child,
            admission_feed(code),
            usize::try_from(profile.max_decode_bytes).unwrap(),
        );
        assert_budget_replacement(&events[0]);
        // A fresh Feed must be accepted as a state transition, even if its value is too large.
        let events = admitted_turn(
            &mut child,
            admission_feed("1"),
            usize::try_from(profile.max_decode_bytes).unwrap(),
        );
        assert_budget_replacement(&events[0]);
    }
}

/// Final drain rejection must recover a suspension that was computed after the print.
#[test]
fn print_admission_and_final_drain_end_the_feed_once() {
    for code in [
        "kept = 41\nprint('x' * 700, end='')",
        "kept = 41\nprint('x' * 700, end='')\nmissing_name",
        "kept = 41\nprint('x' * 9000)",
    ] {
        let mut child = Child::default();
        configure_profile(
            &mut child,
            Some(monty_proto::receiver_allocation_profile(512)),
            PROTOCOL_VERSION,
            false,
        );
        let events = admitted_turn(&mut child, admission_feed(code), 512);
        assert_eq!(events.len(), 1);
        assert_budget_error(&events[0]);
        let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
        assert_eq!(
            expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
            MontyObject::int(42)
        );
    }
}

/// Binds a host function without a preliminary name-lookup suspension.
fn admission_call_feed(code: &str) -> pb::parent_request::Kind {
    let (inputs, values) = named_values_to_proto(NamedValues::from(vec![(
        "f".to_owned(),
        MontyObject::function("f", None),
    )]));
    pb::parent_request::Kind::Feed(pb::Feed {
        code: code.to_owned(),
        inputs,
        values: Some(values),
        cwd: "/".to_owned(),
        ..Default::default()
    })
}

/// A dump rejection preserves an existing suspension, including its argument loan.
#[test]
fn rejected_suspended_dump_keeps_the_call_resumable() {
    let mut child = Child::default();
    configure_profile(
        &mut child,
        Some(monty_proto::receiver_allocation_profile(512)),
        PROTOCOL_VERSION,
        false,
    );
    let events = admitted_turn(&mut child, admission_call_feed("kept = 41\nf(1)"), 512);
    let Some(pb::child_event::Kind::FunctionCall(call)) = &events[0].kind else {
        panic!("expected call: {events:?}")
    };
    let call_id = call.call_id;
    assert_budget_replacement(&admitted_turn(&mut child, pb::parent_request::Kind::Dump(pb::Dump {}), 512)[0]);
    let events = admitted_turn(
        &mut child,
        pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id,
            result: Some(pb::ExtFunctionResult {
                kind: Some(pb::ext_function_result::Kind::ReturnValue(0)),
            }),
            values: Some(arena(MontyObject::int(42))),
        }),
        512,
    );
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
    let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
}

/// Rejecting a function announcement or final drain leaves the REPL ready.
#[test]
fn rejected_function_payload_and_print_drain_restore_the_ready_repl() {
    for code in ["kept = 41\nf('x' * 1000)", "kept = 41\nprint('x' * 700, end='')\nf(1)"] {
        let mut child = Child::default();
        configure_profile(
            &mut child,
            Some(monty_proto::receiver_allocation_profile(512)),
            PROTOCOL_VERSION,
            false,
        );
        let events = admitted_turn(&mut child, admission_call_feed(code), 512);
        assert_eq!(events.len(), 1);
        assert_budget_replacement(&events[0]);
        let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
        assert_eq!(
            expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
            MontyObject::int(42)
        );
    }
}

/// Keeps earlier prints in order while rejecting a later frame before its sink write.
#[test]
fn print_rejection_preserves_prior_delivered_output() {
    let mut child = Child::default();
    let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
        protocol_version: PROTOCOL_VERSION,
        receiver_allocation_profile: Some(monty_proto::receiver_allocation_profile(512)),
        print_flush_interval_ms: Some(0),
        ..Default::default()
    }));
    dispatch_frame(&mut child, &request);
    let events = admitted_turn(&mut child, admission_feed("print('prior')\nprint('x' * 700)"), 512);
    assert_eq!(events.len(), 2);
    let Some(pb::child_event::Kind::Print(print)) = &events[0].kind else {
        panic!("expected print")
    };
    assert_eq!(print.segments.len(), 1);
    assert_eq!(print.segments[0].text, "prior\n");
    assert_budget_error(&events[1]);
}

/// An excessive restored envelope is refused before the previous configuration is consumed.
#[test]
fn rejected_loads_are_atomic_including_restored_script_metadata() {
    let mut idle = Child::default();
    create_repl(&mut idle);
    feed(&mut idle, "kept = 41");
    let (bytes, _) = dispatch_frame(&mut idle, &frame_request(pb::parent_request::Kind::Dump(pb::Dump {})));
    let (_, pb::child_event::Kind::DumpResult(idle_dump)) = split_turn(&bytes) else {
        panic!("expected dump")
    };
    for large_metadata in [false, true] {
        let mut source = Child::default();
        let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
            script_name: if large_metadata {
                "s".repeat(600)
            } else {
                "main.py".to_owned()
            },
            protocol_version: PROTOCOL_VERSION,
            ..Default::default()
        }));
        dispatch_frame(&mut source, &request);
        let kind = if large_metadata {
            admission_feed("kept = 41")
        } else {
            admission_call_feed("kept = 41\nf('x' * 1000)")
        };
        dispatch_frame(&mut source, &frame_request(kind));
        let (bytes, _) = dispatch_frame(&mut source, &frame_request(pb::parent_request::Kind::Dump(pb::Dump {})));
        let (_, pb::child_event::Kind::DumpResult(large_dump)) = split_turn(&bytes) else {
            panic!("expected dump")
        };
        let mut child = Child::default();
        configure_profile(
            &mut child,
            Some(monty_proto::receiver_allocation_profile(512)),
            PROTOCOL_VERSION,
            false,
        );
        let events = admitted_turn(
            &mut child,
            pb::parent_request::Kind::Load(pb::Load {
                state: large_dump.state,
            }),
            512,
        );
        assert_budget_replacement(&events[0]);
        let events = admitted_turn(
            &mut child,
            pb::parent_request::Kind::Load(pb::Load {
                state: idle_dump.state.clone(),
            }),
            512,
        );
        assert!(matches!(events[0].kind, Some(pb::child_event::Kind::Ok(_))));
        assert_eq!(events[0].restored_script_name.as_deref(), Some("main.py"));
        let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
        assert_eq!(
            expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
            MontyObject::int(42)
        );
        assert_budget_replacement(&admitted_turn(&mut child, admission_feed("'x' * 1000"), 512)[0]);
    }
}

/// Abort reply replacement preserves its normal transition back to Ready.
#[test]
fn excessive_abort_reply_keeps_the_repl_ready() {
    let mut child = Child::default();
    configure_profile(
        &mut child,
        Some(monty_proto::receiver_allocation_profile(512)),
        PROTOCOL_VERSION,
        false,
    );
    admitted_turn(&mut child, admission_call_feed("kept = 41\nf(1)"), 512);
    let events = admitted_turn(
        &mut child,
        pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
            exception: Some(pb::RaisedException {
                exc_type: "RuntimeError".to_owned(),
                message: Some("x".repeat(1000)),
                ..Default::default()
            }),
        }),
        512,
    );
    assert_budget_replacement(&events[0]);
    let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
}

/// A futures announcement is admitted before its resume point becomes persistent.
#[test]
fn rejected_futures_announcement_recovers_the_ready_repl() {
    let mut child = Child::default();
    configure_profile(
        &mut child,
        Some(monty_proto::receiver_allocation_profile(512)),
        PROTOCOL_VERSION,
        false,
    );
    let code = "import asyncio\nkept = 41\nasync def main():\n    return await asyncio.gather(*[f() for _ in range(65)])\nasyncio.run(main())";
    let mut events = admitted_turn(&mut child, admission_call_feed(code), 512);
    for _ in 0..65 {
        let Some(pb::child_event::Kind::FunctionCall(call)) = &events[0].kind else {
            panic!("expected call: {events:?}")
        };
        events = admitted_turn(
            &mut child,
            pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
                call_id: call.call_id,
                result: Some(pb::ExtFunctionResult {
                    kind: Some(pb::ext_function_result::Kind::Future(call.call_id)),
                }),
                values: None,
            }),
            512,
        );
    }
    assert_budget_replacement(&events[0]);
    let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
}

/// Completion commits type-check context even when the returned value is rejected.
#[test]
fn rejected_complete_retains_committed_type_check_context() {
    let mut child = Child::default();
    configure_profile(
        &mut child,
        Some(monty_proto::receiver_allocation_profile(4096)),
        PROTOCOL_VERSION,
        true,
    );
    let events = admitted_turn(&mut child, admission_feed("kept: int = 41\nlist(range(100))"), 4096);
    assert_budget_replacement(&events[0]);
    let events = admitted_turn(&mut child, admission_feed("kept + 1"), 4096);
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
    let events = admitted_turn(&mut child, admission_feed("kept = 'wrong type'"), 4096);
    assert!(matches!(events[0].kind, Some(pb::child_event::Kind::TypingError(_))));
}

/// A restored suspension retains the destination profile after its configuration is consumed.
#[test]
fn accepted_suspended_load_keeps_the_destination_profile() {
    let mut source = Child::default();
    create_repl(&mut source);
    dispatch_frame(&mut source, &frame_request(admission_call_feed("kept = 41\nf(1)")));
    let (bytes, _) = dispatch_frame(&mut source, &frame_request(pb::parent_request::Kind::Dump(pb::Dump {})));
    let (_, pb::child_event::Kind::DumpResult(dump)) = split_turn(&bytes) else {
        panic!("expected dump")
    };
    let mut child = Child::default();
    configure_profile(
        &mut child,
        Some(monty_proto::receiver_allocation_profile(512)),
        PROTOCOL_VERSION,
        false,
    );
    let events = admitted_turn(
        &mut child,
        pb::parent_request::Kind::Load(pb::Load { state: dump.state }),
        512,
    );
    let Some(pb::child_event::Kind::FunctionCall(call)) = &events[0].kind else {
        panic!("expected restored call: {events:?}")
    };
    let events = admitted_turn(
        &mut child,
        pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: call.call_id,
            result: Some(pb::ExtFunctionResult {
                kind: Some(pb::ext_function_result::Kind::ReturnValue(0)),
            }),
            values: Some(arena(MontyObject::string("x".repeat(1000)))),
        }),
        512,
    );
    assert_budget_replacement(&events[0]);
    let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
}

/// A sink can fail once, so a later successful send must not hide the transport failure.
struct FailingPrintSink {
    sent: Vec<pb::ChildEvent>,
}

impl EventSink for FailingPrintSink {
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), FrameError> {
        if matches!(event.kind, Some(pb::child_event::Kind::Print(_))) {
            Err(FrameError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "print transport failed",
            )))
        } else {
            self.sent.push(event.clone());
            Ok(())
        }
    }
}

/// A print transport failure remains a transport error rather than a graceful budget rejection.
#[test]
fn print_transport_failure_is_not_classified_as_local_admission() {
    let mut child = Child::default();
    configure_profile(
        &mut child,
        Some(monty_proto::receiver_allocation_profile(4096)),
        PROTOCOL_VERSION,
        false,
    );
    let pb::parent_request::Kind::Feed(feed) = admission_feed("print('data')") else {
        unreachable!()
    };
    let mut sink = FailingPrintSink { sent: Vec::new() };
    let error = child
        .handle(
            pb::ParentRequest {
                kind: Some(pb::parent_request::Kind::Feed(feed)),
                trace_parent: None,
            },
            &mut sink,
        )
        .unwrap_err();
    assert!(matches!(error, FrameError::Io(error) if error.kind() == io::ErrorKind::BrokenPipe));
    assert_eq!(sink.sent, []);
}

/// Uses line buffering so each completed line has its own admission decision.
fn line_buffered_admission_child() -> Child {
    let mut child = Child::default();
    let request = frame_request(pb::parent_request::Kind::Configure(pb::Configure {
        protocol_version: PROTOCOL_VERSION,
        receiver_allocation_profile: Some(monty_proto::receiver_allocation_profile(512)),
        print_flush_interval_ms: Some(0),
        ..Default::default()
    }));
    let (bytes, outcome) = dispatch_frame(&mut child, &request);
    assert_eq!(outcome, HandleOutcome::Continue);
    assert!(matches!(
        decode_events(&bytes).as_slice(),
        [pb::child_event::Kind::Ok(_)]
    ));
    child
}

/// The failed write's suffix must not escape through the final drain.
#[test]
fn multiline_print_rejection_discards_the_unsent_suffix() {
    let mut child = line_buffered_admission_child();
    let events = admitted_turn(
        &mut child,
        admission_feed("kept = 41\nprint('prior\\n' + 'x' * 700 + '\\ntail\\n', end='')"),
        512,
    );
    assert_eq!(events.len(), 2);
    let Some(pb::child_event::Kind::Print(print)) = &events[0].kind else {
        panic!("expected prior output")
    };
    assert_eq!(print.segments.len(), 1);
    assert_eq!(print.segments[0].text, "prior\n");
    assert_budget_error(&events[1]);
    let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
    assert_eq!(events.len(), 1);
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
}

/// A caught print failure leaves a clean buffer for later valid output.
#[test]
fn caught_multiline_print_rejection_preserves_only_the_later_marker() {
    let mut child = line_buffered_admission_child();
    let code = "kept = 41\ntry:\n    print('prior\\n' + 'x' * 700 + '\\ntail\\n', end='')\nexcept RuntimeError:\n    print('caught')\nkept + 1";
    let events = admitted_turn(&mut child, admission_feed(code), 512);
    assert_eq!(events.len(), 3);
    for (event, expected) in events[..2].iter().zip(["prior\n", "caught\n"]) {
        let Some(pb::child_event::Kind::Print(print)) = &event.kind else {
            panic!("expected streamed output")
        };
        assert_eq!(print.segments.len(), 1);
        assert_eq!(print.segments[0].text, expected);
    }
    assert_eq!(
        expect_complete(events.into_iter().last().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
    let events = admitted_turn(&mut child, admission_feed("kept + 1"), 512);
    assert_eq!(events.len(), 1);
    assert_eq!(
        expect_complete(events.into_iter().next().unwrap().kind.unwrap()),
        MontyObject::int(42)
    );
}
