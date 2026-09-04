//! Integration tests for `monty subprocess`: spawn the real binary and
//! drive it over the wire protocol, including crash scenarios — the entire
//! point of the subprocess mode is that a dead child is a recoverable event
//! for the parent.

use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use monty_proto::{
    FrameError, FrameReader, MAX_FRAME_LEN, MIN_SUPPORTED_PROTOCOL_VERSION, PROTOCOL_VERSION, WireFunctionCall,
    WireObject, exceeds_max_frame_len, pb, write_frame,
};
use monty_types::MontyObject;

/// How long a death-expecting helper waits for the child to exit. Generous:
/// the regression it guards is "the child never dies", so the only cost of a
/// long wait is how late that failure is reported on a slow CI machine.
const DEATH_TIMEOUT: Duration = Duration::from_secs(20);

/// A spawned `monty subprocess` child with framed pipes.
struct ChildProc {
    child: Child,
    writer: ChildStdin,
    reader: FrameReader<ChildStdout>,
}

impl ChildProc {
    /// Spawns the child with its stderr inherited, so diagnostics show up in
    /// the test output.
    fn spawn() -> Self {
        Self::spawn_with(Stdio::inherit())
    }

    /// Spawns the child with its stderr captured, for tests asserting on the
    /// diagnostics it prints before dying (see [`Self::reap_with_stderr`]).
    fn spawn_stderr_piped() -> Self {
        Self::spawn_with(Stdio::piped())
    }

    fn spawn_with(stderr: Stdio) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_monty"))
            .arg("subprocess")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .expect("failed to spawn monty subprocess");
        let writer = child.stdin.take().expect("child stdin");
        let reader = FrameReader::new(child.stdout.take().expect("child stdout"));
        Self { child, writer, reader }
    }

    fn send(&mut self, kind: pb::parent_request::Kind) {
        write_frame(
            &mut self.writer,
            &pb::ParentRequest {
                kind: Some(kind),
                trace_parent: None,
            },
        )
        .expect("failed to write request");
    }

    /// Reads a single event.
    fn recv(&mut self) -> pb::child_event::Kind {
        self.reader
            .read::<pb::ChildEvent>()
            .expect("failed to read event")
            .expect("unexpected EOF from child")
            .kind
            .expect("event has no kind")
    }

    /// Reads until the turn-ending event, collecting streamed prints.
    fn recv_turn(&mut self) -> (Vec<pb::Print>, pb::child_event::Kind) {
        let mut prints = Vec::new();
        loop {
            match self.recv() {
                pb::child_event::Kind::Print(print) => prints.push(print),
                other => return (prints, other),
            }
        }
    }

    fn create_repl(&mut self) {
        self.create_repl_with(pb::Configure {
            script_name: "main.py".to_owned(),
            limits: None,
            type_check: false,
            type_check_stubs: None,
            monty_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION,
            assert_message_annotations: None,
            ..Default::default()
        });
    }

    fn create_repl_with(&mut self, create: pb::Configure) {
        self.send(pb::parent_request::Kind::Configure(create));
        match self.recv() {
            pb::child_event::Kind::Ok(_) => {}
            other => panic!("expected Ok for Configure, got {other:?}"),
        }
    }

    /// Feeds a snippet and returns `(prints, turn-ending event)`.
    fn feed(&mut self, code: &str) -> (Vec<pb::Print>, pb::child_event::Kind) {
        self.feed_with(code, vec![])
    }

    fn feed_with(&mut self, code: &str, inputs: Vec<pb::NamedValue>) -> (Vec<pb::Print>, pb::child_event::Kind) {
        self.send(pb::parent_request::Kind::Feed(pb::Feed {
            code: code.to_owned(),
            inputs,
            skip_type_check: false,
        }));
        self.recv_turn()
    }

    /// Feeds a snippet and asserts it completes, returning the value.
    #[track_caller]
    fn feed_complete(&mut self, code: &str) -> MontyObject {
        let (_, event) = self.feed(code);
        expect_complete(event)
    }

    fn resume_call(
        &mut self,
        call_id: u32,
        result: pb::ext_function_result::Kind,
    ) -> (Vec<pb::Print>, pb::child_event::Kind) {
        self.send(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id,
            result: Some(pb::ExtFunctionResult { kind: Some(result) }),
        }));
        self.recv_turn()
    }

    /// Feeds a snippet expected to kill the child, asserting no turn-ending
    /// event arrives — EOF (the usual case) or a truncated frame instead.
    #[track_caller]
    fn feed_expecting_death(&mut self, code: &str) {
        self.send(pb::parent_request::Kind::Feed(pb::Feed {
            code: code.to_owned(),
            inputs: vec![],
            skip_type_check: false,
        }));
        self.expect_death();
    }

    /// Writes a bare 200 MiB frame-length prefix — no body — and expects the
    /// child to die buying the buffer: under the wire cap, over any limit a
    /// test applies, and four bytes of writing, so the parent cannot block on a
    /// pipe whose reader has already gone.
    #[track_caller]
    fn oversized_prefix_expecting_death(&mut self) {
        self.writer
            .write_all(&(200u32 * 1024 * 1024).to_le_bytes())
            .expect("failed to write length prefix");
        self.expect_death();
    }

    /// Asserts the child dies without a turn-ending event: EOF (the usual
    /// case) or a truncated frame. Waits for the exit *first* — a surviving
    /// child writes nothing, so reading it would block forever and hang the
    /// suite instead of failing it; once it is dead the read cannot block.
    #[track_caller]
    fn expect_death(&mut self) {
        let deadline = Instant::now() + DEATH_TIMEOUT;
        while self.child.try_wait().expect("failed to poll child").is_none() {
            assert!(
                Instant::now() < deadline,
                "expected the child to die, still alive after {DEATH_TIMEOUT:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        match self.reader.read::<pb::ChildEvent>() {
            Ok(None) | Err(_) => {}
            Ok(Some(event)) => panic!("expected the child to die, got {:?}", event.kind),
        }
    }

    /// Waits for the child and returns its status with everything it wrote to
    /// stderr. Only valid for a child spawned by [`Self::spawn_stderr_piped`].
    fn reap_with_stderr(&mut self) -> (ExitStatus, String) {
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .expect("child stderr must be piped")
            .read_to_string(&mut stderr)
            .expect("failed to read child stderr");
        let status = self.child.wait().expect("failed to wait for child");
        (status, stderr)
    }

    /// Tells the child to shut down and asserts a clean exit.
    fn shutdown(mut self) {
        self.send(pb::parent_request::Kind::Shutdown(pb::Shutdown {}));
        match self.recv() {
            pb::child_event::Kind::Ok(_) => {}
            other => panic!("expected Ok for Shutdown, got {other:?}"),
        }
        let status = self.child.wait().expect("failed to wait for child");
        assert!(status.success(), "child exited with {status:?}");
    }
}

impl Drop for ChildProc {
    fn drop(&mut self) {
        // don't leak children when a test fails mid-protocol
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[track_caller]
fn expect_complete(event: pb::child_event::Kind) -> MontyObject {
    match event {
        pb::child_event::Kind::Complete(complete) => complete
            .value
            .expect("complete has no value")
            .into_object()
            .expect("invalid complete value"),
        other => panic!("expected Complete, got {other:?}"),
    }
}

#[track_caller]
fn expect_error(event: pb::child_event::Kind) -> pb::RaisedException {
    match event {
        pb::child_event::Kind::Error(error) => error.exception.expect("error has no exception"),
        other => panic!("expected Error, got {other:?}"),
    }
}

fn int_value(i: i64) -> WireObject {
    WireObject::new(MontyObject::Int(i))
}

fn str_value(s: &str) -> WireObject {
    WireObject::new(MontyObject::String(s.to_owned()))
}

// =============================================================================
// Happy path
// =============================================================================

#[test]
fn session_state_persists_across_feeds() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    assert_eq!(child.feed_complete("x = 1 + 2\nx"), MontyObject::Int(3));
    // `x` defined by the first feed is visible to the second
    assert_eq!(child.feed_complete("x * 2"), MontyObject::Int(6));
    child.shutdown();
}

#[test]
fn inputs_are_injected() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let inputs = vec![pb::NamedValue {
        name: "a".to_owned(),
        value: Some(int_value(20)),
    }];
    let (_, event) = child.feed_with("a + 1", inputs);
    assert_eq!(expect_complete(event), MontyObject::Int(21));
    child.shutdown();
}

#[test]
fn print_output_is_streamed_in_order() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (prints, event) = child.feed("print('one')\nprint('two')\nprint('three', end='')\n'done'");
    expect_complete(event);
    let text: String = prints.iter().map(|p| p.text.as_str()).collect();
    // the partial (no-newline) third line must still arrive before the turn ends
    assert_eq!(text, "one\ntwo\nthree");
    assert!(prints.iter().all(|p| p.stream == i32::from(pb::PrintStream::Stdout)));
    child.shutdown();
}

#[test]
fn runtime_error_preserves_session() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    assert_eq!(child.feed_complete("kept = 41"), MontyObject::None);
    let (_, event) = child.feed("1 / 0");
    let error = expect_error(event);
    assert_eq!(error.exc_type, "ZeroDivisionError");
    assert_eq!(error.message.as_deref(), Some("division by zero"));
    assert!(!error.traceback.is_empty(), "traceback frames must cross the wire");
    // the session survives the error, including earlier globals
    assert_eq!(child.feed_complete("kept + 1"), MontyObject::Int(42));
    child.shutdown();
}

// =============================================================================
// Suspensions
// =============================================================================

#[test]
fn external_function_round_trip() {
    let mut child = ChildProc::spawn();
    child.create_repl();

    // calling an unknown name suspends at FunctionCall directly (NameLookup
    // is only emitted for bare name *reads*)
    let (_, event) = child.feed("add(1, 2)");
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(call.function_name, "add");
    assert_eq!(call.object_id, None);
    assert_eq!(call.args, vec![MontyObject::Int(1), MontyObject::Int(2)]);

    let (_, event) = child.resume_call(call.call_id, pb::ext_function_result::Kind::ReturnValue(int_value(3)));
    assert_eq!(expect_complete(event), MontyObject::Int(3));
    child.shutdown();
}

/// `AbortFeed` raises the supplied error uncatchably and keeps the session usable.
#[test]
fn abort_feed_round_trip() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) =
        child.feed("while True:\n    try:\n        open('/etc/passwd')\n    except Exception:\n        pass");
    let pb::child_event::Kind::OsCall(_) = event else {
        panic!("expected OsCall, got {event:?}");
    };
    child.send(pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
        exception: Some(pb::RaisedException {
            exc_type: "RuntimeError".to_owned(),
            message: Some("suspension limit 3 exceeded".to_owned()),
            traceback: vec![],
            data: None,
        }),
    }));
    let (_, event) = child.recv_turn();
    let error = expect_error(event);
    assert_eq!(error.exc_type, "RuntimeError");
    assert_eq!(error.message.as_deref(), Some("suspension limit 3 exceeded"));
    assert_eq!(
        error
            .traceback
            .first()
            .and_then(|frame| frame.start.map(|loc| loc.line)),
        Some(3)
    );
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// A suspension announcement is size-checked *with* its session stamps: one
/// that fits `MAX_FRAME_LEN` only before the stamps are added must be refused
/// as the clean oversized-argument error, not fail at send time and kill the
/// worker (the parent would never learn the resume point).
///
/// Allocates ~256 MiB several times over (sizing here, the sandbox string, its
/// wire copy), so it is memory-heavy; disable it if it proves flaky in CI.
#[test]
fn near_limit_suspension_is_refused_cleanly() {
    let announcement = |arg_len: usize| pb::ChildEvent {
        kind: Some(pb::child_event::Kind::FunctionCall(WireFunctionCall {
            function_name: "f".to_owned(),
            args: vec![MontyObject::String("x".repeat(arg_len))],
            kwargs: vec![],
            call_id: 1,
            object_id: None,
        })),
        ..Default::default()
    };
    // Size the argument so the unstamped announcement is exactly
    // `MAX_FRAME_LEN`: shrink an oversize probe by its excess, then walk up
    // past the length varints that lose a byte as the sizes they describe
    // drop below 2^28 (= `MAX_FRAME_LEN`).
    let probe = MAX_FRAME_LEN as usize + 16;
    let probe_len = exceeds_max_frame_len(&announcement(probe)).expect("probe exceeds the limit") as usize;
    let mut arg_len = probe - (probe_len - MAX_FRAME_LEN as usize);
    while exceeds_max_frame_len(&announcement(arg_len + 1)).is_none() {
        arg_len += 1;
    }
    assert!(exceeds_max_frame_len(&announcement(arg_len)).is_none());

    let mut child = ChildProc::spawn();
    // a configured limit is stamped on every reply, so the sent frame is
    // always larger than the unstamped announcement
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: Some(pb::ResourceLimits {
            max_suspensions: Some(5),
            ..Default::default()
        }),
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        ..Default::default()
    });
    let (_, event) = child.feed(&format!("f('x' * {arg_len})"));
    let error = expect_error(event);
    assert_eq!(error.exc_type, "RuntimeError");
    assert!(
        error
            .message
            .as_deref()
            .is_some_and(|m| m.starts_with("argument frame of ") && m.contains("exceeds the maximum of")),
        "unexpected message: {:?}",
        error.message
    );
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

#[test]
fn name_lookup_round_trip() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    // a bare name read suspends at NameLookup; the parent supplies the value
    let (_, event) = child.feed("answer + 1");
    let pb::child_event::Kind::NameLookup(lookup) = event else {
        panic!("expected NameLookup, got {event:?}");
    };
    assert_eq!(lookup.name, "answer");
    child.send(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
        kind: Some(pb::resume_name_lookup::Kind::Value(int_value(41))),
    }));
    let (_, event) = child.recv_turn();
    assert_eq!(expect_complete(event), MontyObject::Int(42));
    child.shutdown();
}

/// An `error` answer to a name lookup is raised inside the sandbox where the
/// name was read — catchable there, and reported with a sandbox traceback
/// when it is not — and the session survives it.
#[test]
fn name_lookup_error_raises_in_sandbox() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed("try:\n    secret\nexcept PermissionError as e:\n    caught = str(e)\ncaught");
    let pb::child_event::Kind::NameLookup(lookup) = event else {
        panic!("expected NameLookup, got {event:?}");
    };
    assert_eq!(lookup.name, "secret");
    let exc = pb::RaisedException {
        exc_type: "PermissionError".to_owned(),
        message: Some("secret is off limits".to_owned()),
        traceback: vec![],
        data: None,
    };
    child.send(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
        kind: Some(pb::resume_name_lookup::Kind::Error(exc.clone())),
    }));
    let (_, event) = child.recv_turn();
    assert_eq!(
        expect_complete(event),
        MontyObject::String("secret is off limits".to_owned())
    );

    let (_, event) = child.feed("secret");
    assert!(matches!(event, pb::child_event::Kind::NameLookup(_)));
    child.send(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
        kind: Some(pb::resume_name_lookup::Kind::Error(exc)),
    }));
    let (_, event) = child.recv_turn();
    let error = expect_error(event);
    assert_eq!(error.exc_type, "PermissionError");
    assert_eq!(error.message.as_deref(), Some("secret is off limits"));
    assert!(
        !error.traceback.is_empty(),
        "the sandbox frame must be on the traceback"
    );
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

#[test]
fn external_function_not_found_raises_name_error() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed("undefined_fn()");
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    // the parent has no handler for this name -> Python NameError
    let (_, event) = child.resume_call(
        call.call_id,
        pb::ext_function_result::Kind::NotFound("undefined_fn".to_owned()),
    );
    let error = expect_error(event);
    assert_eq!(error.exc_type, "NameError");
    assert_eq!(error.message.as_deref(), Some("name 'undefined_fn' is not defined"));
    child.shutdown();
}

#[test]
fn os_call_bubbles_to_parent_without_mounts() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed("from pathlib import Path\nPath('/data.txt').read_text()");
    let pb::child_event::Kind::OsCall(call) = event else {
        panic!("expected OsCall, got {event:?}");
    };
    assert_eq!(call.call, Some(pb::os_call::Call::ReadText("/data.txt".to_owned())));

    let (_, event) = child.resume_call(
        call.call_id,
        pb::ext_function_result::Kind::ReturnValue(str_value("hello")),
    );
    assert_eq!(expect_complete(event), MontyObject::String("hello".to_owned()));
    child.shutdown();
}

/// A suspension announcement is *lent* its payload rather than given a copy of
/// it, so the child must have taken it back by the time anything else can see
/// the suspension. Dumping straight after the announcement is what proves it:
/// the dump is built from the stored call, so a payload left behind in the
/// event would come back from `Load` as an argument-less call.
#[test]
fn suspended_call_keeps_its_arguments_for_a_dump() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed("ext('hello', 1, key='value')");
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(
        call.args,
        vec![MontyObject::String("hello".to_owned()), MontyObject::Int(1)]
    );

    child.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = child.recv() else {
        panic!("expected DumpResult");
    };
    drop(child);

    let mut fresh = ChildProc::spawn();
    fresh.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let (_, event) = fresh.recv_turn();
    let pb::child_event::Kind::FunctionCall(restored) = event else {
        panic!("expected re-emitted FunctionCall after Load, got {event:?}");
    };
    assert_eq!(restored.args, call.args);
    assert_eq!(restored.kwargs, call.kwargs);
    assert_eq!(restored.function_name, "ext");
    fresh.shutdown();
}

/// The same loan applies to an `OsCall`'s payload, which is swapped out for a
/// `GetEnviron` placeholder while the announcement is on the wire — a lost
/// payload would come back from `Load` as that placeholder rather than the
/// write.
#[test]
fn suspended_os_call_keeps_its_payload_for_a_dump() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed("from pathlib import Path\nPath('/data.txt').write_text('contents')");
    let pb::child_event::Kind::OsCall(call) = event else {
        panic!("expected OsCall, got {event:?}");
    };

    child.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = child.recv() else {
        panic!("expected DumpResult");
    };
    drop(child);

    let mut fresh = ChildProc::spawn();
    fresh.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let (_, event) = fresh.recv_turn();
    let pb::child_event::Kind::OsCall(restored) = event else {
        panic!("expected re-emitted OsCall after Load, got {event:?}");
    };
    assert_eq!(restored.call, call.call);
    assert_eq!(
        restored.call,
        Some(pb::os_call::Call::WriteText(pb::os_call::TextWrite {
            path: "/data.txt".to_owned(),
            data: "contents".to_owned(),
        }))
    );
    fresh.shutdown();
}

#[test]
fn os_call_error_resume_carries_exception() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed("from pathlib import Path\nPath('/nope.txt').read_text()");
    let pb::child_event::Kind::OsCall(call) = event else {
        panic!("expected OsCall, got {event:?}");
    };
    let exc = pb::RaisedException {
        exc_type: "FileNotFoundError".to_owned(),
        message: Some("No such file or directory: '/nope.txt'".to_owned()),
        traceback: vec![],
        data: None,
    };
    let (_, event) = child.resume_call(call.call_id, pb::ext_function_result::Kind::Error(exc));
    let error = expect_error(event);
    assert_eq!(error.exc_type, "FileNotFoundError");
    // the child's VM raised the exception inside the sandbox, so the
    // traceback now includes the sandbox frame
    assert!(!error.traceback.is_empty());
    child.shutdown();
}

// =============================================================================
// Resource limits
// =============================================================================

#[test]
fn child_enforces_time_limit() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: Some(pb::ResourceLimits {
            max_duration_micros: Some(100_000), // 100ms
            ..Default::default()
        }),
        type_check: false,
        type_check_stubs: None,
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        assert_message_annotations: None,
        ..Default::default()
    });
    let (_, event) = child.feed("while True:\n    pass");
    let error = expect_error(event);
    assert_eq!(error.exc_type, "TimeoutError");
    // resource exhaustion is terminal for the SESSION (the tracker stays
    // exhausted) but not for the child process: Reset + Configure reuses it
    let (_, event) = child.feed("1 + 1");
    assert_eq!(expect_error(event).exc_type, "TimeoutError");
    child.send(pb::parent_request::Kind::Reset(pb::Reset {}));
    let pb::child_event::Kind::Ok(_) = child.recv() else {
        panic!("expected Ok for Reset");
    };
    child.create_repl();
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// A session's `max_memory` must not disturb work that stays inside it. This
/// small budget includes the real allocations needed to compile and run a feed.
#[test]
fn small_memory_limit_leaves_normal_work_alone() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(64 * 1024));
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// Crossing the allocator's soft limit raises an ordinary session error rather
/// than killing the worker, and unwinding releases the incomplete result.
#[test]
fn exceeding_the_soft_memory_limit_preserves_the_worker() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(8 * 1024 * 1024));
    let (_, event) = child.feed("[str(i) for i in range(131_072)]");
    assert_eq!(expect_error(event).exc_type, "MemoryError");
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// Async scheduler state is allocator-accounted even though it lives outside
/// Monty's object heap, so recursive gathers reach the soft limit safely.
#[test]
fn async_accumulation_reaches_the_soft_limit() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(1024 * 1024));
    let code = "import asyncio\nasync def f():\n    return await asyncio.gather(f())\nasyncio.run(f())";
    let (_, event) = child.feed(code);
    assert_eq!(expect_error(event).exc_type, "MemoryError");
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// Gathers nested as *items* of one another (`g = asyncio.gather(g)`) cost no
/// Python frames, so nothing but `max_memory` bounds how deep a nest gets built.
/// Building one too large for the limit must end the run with a `MemoryError`,
/// and the worker must survive it.
#[test]
fn building_a_deep_gather_nest_reaches_the_soft_limit() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(1024 * 1024));
    // Built inside a function so unwinding releases the partial nest — a nest
    // left bound at module level keeps the session over its limit.
    let code = "import asyncio\nasync def leaf():\n    return 1\ndef build():\n    g = leaf()\n    for _ in range(50_000):\n        g = asyncio.gather(g)\n    return g\nbuild()";
    let (_, event) = child.feed(code);
    assert_eq!(expect_error(event).exc_type, "MemoryError");
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// Committing a nest costs a walk frame per level on the way down and a result
/// list per level on the way back up, none of it between bytecode instructions.
/// So a nest that *fits* under `max_memory` can still exceed it when awaited,
/// and that has to arrive as a `MemoryError` rather than as a dead worker: the
/// walk polls the limit as it goes, and preflights its own reallocations.
///
/// Both depths build inside the limit. The small one crosses it during the walk;
/// the large one is where a single `Vec` growth of the walk's stack used to jump
/// clear over the allocator's hard ceiling in one allocation.
#[test]
fn committing_a_deep_gather_nest_reaches_the_soft_limit() {
    for (limit, depth) in [(1024 * 1024, 3_500), (32 * 1024 * 1024, 100_000)] {
        let mut child = ChildProc::spawn();
        child.create_repl_with(configure_with_max_memory(limit));
        let build = format!(
            "import asyncio\nasync def leaf():\n    return 1\ng = leaf()\nfor _ in range({depth}):\n    g = asyncio.gather(g)\n1"
        );
        assert_eq!(child.feed_complete(&build), MontyObject::Int(1), "depth {depth}");

        let (_, event) = child.feed("await g");
        assert_eq!(expect_error(event).exc_type, "MemoryError", "depth {depth}");
        // Dropping the nest brings the session back under its limit, which it
        // could not do if the worker had died on the hard ceiling instead.
        assert_eq!(
            child.feed_complete("g = None\n1 + 1"),
            MontyObject::Int(2),
            "depth {depth}"
        );
        child.shutdown();
    }
}

/// Known large results are rejected against allocator usage before they can
/// jump from below the soft limit past the hard ceiling. The reported figure is
/// what each result really costs, so it pins down that the refusal accounted for
/// the whole allocation rather than tripping on some smaller intermediate.
#[test]
fn large_allocations_are_rejected_before_the_hard_limit() {
    // each case with the allocator usage it should be refused at
    let cases = [
        ("'x' * 10_000_000", 10_031_137),
        ("b'x' * 10_000_000", 10_031_269),
        ("[None] * 1_000_000", 16_031_391),
        ("2 ** 10_000_000", 10_031_230),
        ("1 << 10_000_000", 1_281_231),
        ("('a' * 1000).replace('a', 'b' * 2000)", 2_034_769),
        // Bulk container clones: `+=` preflights the temp clone plus the target
        // growth, `+` preflights each side's clone.
        ("x = [None] * 40_000\nx += x", 1_951_835),
        ("t = (None,) * 40_000\nt + t", 1_311_835),
        ("x = [None] * 40_000\nx.copy()", 1_311_585),
        // A partial re-clones its bound arguments on every call, so that clone
        // is preflighted like any other bulk container copy.
        (
            "import functools\ndef f(*a):\n    return 0\np = functools.partial(f, *range(20_000))\njunk = [None] * 40_000\np()",
            1_314_563,
        ),
        // Reading `p.args` / `p.keywords` rebuilds them in full, so both are
        // preflighted like any other bulk container copy.
        (
            "import functools\ndef f(*a):\n    return 0\np = functools.partial(f, *range(20_000))\njunk = [0] * 40_000\np.args",
            1_314_563,
        ),
        (
            "import functools\ndef f(**k):\n    return 0\np = functools.partial(f, **{str(i): i for i in range(6_000)})\njunk = [0] * 30_000\np.keywords",
            1_071_419,
        ),
        // `deque.extend` preflights exact-hint iterators up front.
        (
            "from collections import deque\nd = deque()\nd.extend(range(1_000_000))",
            16_031_971,
        ),
    ];

    for (code, expected) in cases {
        let mut child = ChildProc::spawn();
        child.create_repl_with(configure_with_max_memory(1024 * 1024));
        let (_, event) = child.feed(code);
        let error = expect_error(event);
        assert_eq!(error.exc_type, "MemoryError", "{code}");
        let message = error.message.expect("MemoryError should have a message");
        assert_reported_usage(&message, expected, code);
        assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2), "{code}");
        child.shutdown();
    }
}

/// Announcing a suspension must not cost extra copies of the value being
/// announced. A host-call argument sized as a *fraction of the limit* is what
/// makes this a regression test for that amplification rather than for one
/// absolute number: at three copies (interpreter value, converted args, encode
/// buffer) a 3/8 argument fits, at four it crossed the allocator's hard ceiling
/// and the worker was killed mid-announcement.
#[test]
fn large_host_call_arguments_survive_being_announced() {
    const LIMIT: usize = 8 * 1024 * 1024;
    const ARG: usize = LIMIT * 3 / 8;

    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(LIMIT as u64));
    let (_, event) = child.feed(&format!("s = 'A' * {ARG}\nfoobar(s)"));
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(call.args.len(), 1);
    assert_eq!(call.args[0], MontyObject::String("A".repeat(ARG)));

    // the session is still usable afterwards, i.e. nothing overshot into a
    // soft-limit `MemoryError` on the next checkpoint either
    let (_, event) = child.resume_call(call.call_id, pb::ext_function_result::Kind::ReturnValue(int_value(7)));
    assert_eq!(expect_complete(event), MontyObject::Int(7));
    assert_eq!(
        child.feed_complete("len(s)"),
        MontyObject::Int(i64::try_from(ARG).unwrap())
    );
    child.shutdown();
}

/// The asymmetry the amplification produced: a value small enough to *return*
/// to the host was not necessarily small enough to *pass* to a host function,
/// because only the announcement path cloned it. Both directions now cost the
/// same, so one limit governs both.
#[test]
fn a_returnable_value_can_also_be_passed_to_a_host_function() {
    const LIMIT: usize = 8 * 1024 * 1024;
    const ARG: usize = LIMIT * 3 / 8;

    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(LIMIT as u64));
    child.feed_complete(&format!("s = 'A' * {ARG}"));
    // the same binding, first returned to the host...
    assert_eq!(string_len(&child.feed_complete("s")), ARG);

    // ...then passed to a host function, which must cost no more
    let (_, event) = child.feed("foobar(s)");
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(string_len(&call.args[0]), ARG);
    child.shutdown();
}

/// The length of a `str` value, for assertions that care only about its size —
/// printing a multi-megabyte string on failure helps nobody.
#[track_caller]
fn string_len(value: &MontyObject) -> usize {
    match value {
        MontyObject::String(s) => s.len(),
        other => panic!("expected a string, got {other:?}"),
    }
}

/// The fix must not have quietly stopped enforcing: an argument that genuinely
/// does not fit still fails as a recoverable session error rather than taking
/// the worker with it.
#[test]
fn host_call_arguments_over_the_limit_still_fail_gracefully() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(8 * 1024 * 1024));
    let (_, event) = child.feed("foobar('A' * (16 * 1024 * 1024))");
    assert_eq!(expect_error(event).exc_type, "MemoryError");
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// Reading `p.args` off a widely bound partial must raise `MemoryError` and
/// leave the session usable, not kill the worker.
///
/// The materialization is a single 16 MiB burst, four times the allocator's
/// hard-limit headroom, so before the preflight in `check_clone_slots` this
/// exited with `OOM_EXIT_CODE` mid-turn and the pool had to replace the child.
#[test]
fn reading_partial_args_cannot_kill_the_worker() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(64 * 1024 * 1024));
    // Sized to sit just under the soft limit, so the burst would cross the hard
    // ceiling rather than merely exceeding what a checkpoint would have caught.
    let build = "import functools\n\
                 def f(*a):\n    return 0\n\
                 p = functools.partial(f, *range(1_000_000))\n\
                 junk = [0] * 2_800_000";
    assert_eq!(child.feed_complete(build), MontyObject::None);

    let (_, event) = child.feed("p.args");
    let error = expect_error(event);
    assert_eq!(error.exc_type, "MemoryError");
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

/// A bounded deque retains at most `maxlen` items, so extending it from a huge
/// exact-hint iterator (the sliding-window pattern) must not trip the
/// `deque.extend` preflight — the memory really is capped at `maxlen`.
#[test]
fn bounded_deque_extend_is_not_preflighted() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(configure_with_max_memory(1024 * 1024));
    let code = "from collections import deque\nd = deque(maxlen=8)\nd.extend(range(500_000))\nlen(d)";
    assert_eq!(child.feed_complete(code), MontyObject::Int(8));
    child.shutdown();
}

/// Assert a `memory limit exceeded` message reports roughly `expected` bytes
/// used against a 1 MiB limit.
///
/// Exact equality is not usable: the figure is real allocator bytes, so the
/// baseline the session starts from varies by a few dozen bytes between
/// platforms (macOS runs consistently below Linux and Windows). The tolerance is
/// far below what a mis-accounted allocation would move the number by.
fn assert_reported_usage(message: &str, expected: u64, code: &str) {
    const TOLERANCE: u64 = 1024;

    let used: u64 = message
        .strip_prefix("memory limit exceeded: ")
        .and_then(|rest| rest.strip_suffix(" bytes > 1048576 bytes"))
        .unwrap_or_else(|| panic!("{code}: unexpected message {message:?}"))
        .parse()
        .unwrap_or_else(|_| panic!("{code}: unexpected message {message:?}"));
    assert!(
        used.abs_diff(expected) <= TOLERANCE,
        "{code}: reported {used} bytes, expected within {TOLERANCE} of {expected}"
    );
}

/// A refused allocation must leave the parent something it can classify: the
/// dedicated exit code, not the `SIGABRT` Rust's allocation-error handler would
/// raise (which a stack overflow also produces). Needs no limit: 1 EiB is
/// thousands of times the usable address space on any 64-bit host, so `mmap`
/// fails on the address-space check before overcommit policy is consulted —
/// deterministic, and no page is ever touched.
#[test]
fn refused_allocation_exits_with_the_oom_code() {
    let mut child = ChildProc::spawn_stderr_piped();
    child.create_repl();
    // no `max_memory`, so the sandbox tracker permits this outright
    child.feed_expecting_death("x = ' ' * (1 << 60)");
    let (status, stderr) = child.reap_with_stderr();
    assert_eq!(status.code(), Some(monty_types::OOM_EXIT_CODE), "got {status:?}");
    assert!(
        stderr.contains("allocation of 1152921504606846976 bytes failed"),
        "{stderr}"
    );
}

/// Memory allocated outside interpreter checkpoints must still hit the hard
/// ceiling rather than grow the host without bound. The allocation here comes
/// from the frame reader — a bare length
/// prefix, under the wire cap and over the limit, buys a 200 MiB buffer with
/// four bytes. Same exit code as a refused allocation; the limit only changes
/// *where* refusal starts.
#[test]
fn exceeding_the_memory_limit_exits_with_the_oom_code() {
    let mut child = ChildProc::spawn_stderr_piped();
    child.create_repl_with(configure_with_max_memory(1024));
    child.oversized_prefix_expecting_death();
    let (status, stderr) = child.reap_with_stderr();
    assert_eq!(status.code(), Some(monty_types::OOM_EXIT_CODE), "got {status:?}");
    assert!(
        stderr.contains("allocation of 209715200 bytes exceeds the memory limit"),
        "{stderr}"
    );
}

/// A dump carries its own limits, so restoring one must re-apply them: this
/// `Load` lands on a child that was never configured with a limit, and the
/// restored session's `max_memory` is all there is to bound it.
#[test]
fn loading_a_dump_applies_its_own_memory_limit() {
    let mut source = ChildProc::spawn();
    source.create_repl_with(configure_with_max_memory(64 * 1024));
    assert_eq!(source.feed_complete("x = 1"), MontyObject::None);
    source.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = source.recv() else {
        panic!("expected DumpResult");
    };
    source.shutdown();

    let mut restored = ChildProc::spawn_stderr_piped();
    restored.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let pb::child_event::Kind::Ok(_) = restored.recv() else {
        panic!("expected Ok for Load");
    };
    restored.oversized_prefix_expecting_death();
    let (status, stderr) = restored.reap_with_stderr();
    assert_eq!(status.code(), Some(monty_types::OOM_EXIT_CODE), "got {status:?}");
    assert!(
        stderr.contains("allocation of 209715200 bytes exceeds the memory limit"),
        "{stderr}"
    );
}

/// A `Configure` carrying `max_memory`, which is what limits the worker.
fn configure_with_max_memory(bytes: u64) -> pb::Configure {
    pb::Configure {
        script_name: "main.py".to_owned(),
        limits: Some(pb::ResourceLimits {
            max_memory_bytes: Some(bytes),
            ..Default::default()
        }),
        type_check: false,
        type_check_stubs: None,
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        assert_message_annotations: None,
        ..Default::default()
    }
}

#[test]
fn install_dependencies_is_rejected_but_session_survives() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    // The Monty sandbox has no host interpreter to install packages for, so it
    // refuses `InstallDependencies` with a recoverable error.
    child.send(pb::parent_request::Kind::InstallDependencies(pb::InstallDependencies {
        requirements: vec!["numpy".to_owned()],
    }));
    let error = expect_error(child.recv());
    assert_eq!(error.exc_type, "RuntimeError");
    assert_eq!(
        error.message.as_deref(),
        Some("dependency installation is only supported by the CPython worker")
    );
    // The session is intact: subsequent feeds still work.
    assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2));
    child.shutdown();
}

// =============================================================================
// Type checking
// =============================================================================

#[test]
fn type_checked_session_rejects_bad_snippets_and_remembers_good_ones() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: true,
        type_check_stubs: None,
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        assert_message_annotations: None,
        ..Default::default()
    });

    let (_, event) = child.feed("x: int = 'not an int'");
    let pb::child_event::Kind::TypingError(typing) = event else {
        panic!("expected TypingError, got {event:?}");
    };
    assert!(
        typing.diagnostics.contains("invalid-assignment"),
        "{}",
        typing.diagnostics
    );

    // a committed snippet becomes visible to later type checks
    assert_eq!(child.feed_complete("y = 1"), MontyObject::None);
    assert_eq!(child.feed_complete("y + 1"), MontyObject::Int(2));

    // ... and the rejected snippet was never committed
    let (_, event) = child.feed("x");
    let pb::child_event::Kind::TypingError(_) = event else {
        panic!("expected TypingError for undefined x, got {event:?}");
    };
    child.shutdown();
}

/// The format is chosen on `Configure` because rendering happens in the child
/// — only the rendered text crosses the wire, so a parent that wants anything
/// other than `full` has to ask before the check runs.
#[test]
fn type_check_format_selects_the_rendering() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        type_check: true,
        type_check_format: pb::TypeCheckFormat::Concise.into(),
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        ..Default::default()
    });

    let (_, event) = child.feed("x: int = 'not an int'");
    let pb::child_event::Kind::TypingError(typing) = event else {
        panic!("expected TypingError, got {event:?}");
    };
    // one line per diagnostic, with no `-->` source snippet as `full` has
    assert_eq!(
        typing.diagnostics,
        "main.py:1:10: error[invalid-assignment] Object of type `Literal[\"not an int\"]` is not assignable to `int`\n"
    );
    child.shutdown();
}

/// Security-critical: `Reset` must scrub every file a session wrote into the
/// type checker — its script (wherever `script_name` placed it, including
/// nested directories and `..`/absolute forms) and its stubs — so the next
/// session served by the SAME process cannot resolve any of them. This runs
/// against one child by construction, so unlike a pool test it cannot pass
/// vacuously on a fresh worker.
#[test]
fn reset_scrubs_type_check_state_from_the_next_session() {
    // (script_name of session A, module path session B tries to import)
    let cases = [
        ("a.py", "a"),
        ("sub/nested.py", "sub.nested"),
        ("../escape.py", "escape"),
        ("/abs.py", "abs"),
    ];
    let mut child = ChildProc::spawn();
    for (script_name, module) in cases {
        // Session A: commits one snippet and carries stubs.
        child.create_repl_with(pb::Configure {
            script_name: script_name.to_owned(),
            type_check: true,
            type_check_stubs: Some("STUB_SECRET: int = 0".to_owned()),
            type_check_format: pb::TypeCheckFormat::Concise.into(),
            monty_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION,
            ..Default::default()
        });
        assert_eq!(child.feed_complete("LEAKY = 'hunter2'"), MontyObject::None);

        child.send(pb::parent_request::Kind::Reset(pb::Reset {}));
        let pb::child_event::Kind::Ok(_) = child.recv() else {
            panic!("expected Ok for Reset");
        };

        // Session B, same process: everything session A wrote must be gone.
        child.create_repl_with(pb::Configure {
            script_name: "b.py".to_owned(),
            type_check: true,
            type_check_format: pb::TypeCheckFormat::Concise.into(),
            monty_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION,
            ..Default::default()
        });
        let mut probe = |code: String| {
            let (_, event) = child.feed(&code);
            let pb::child_event::Kind::TypingError(typing) = event else {
                panic!("expected TypingError for {code:?} after {script_name:?}, got {event:?}");
            };
            typing.diagnostics
        };
        assert_eq!(
            probe(format!("from {module} import LEAKY")),
            format!("b.py:1:6: error[unresolved-import] Cannot resolve imported module `{module}`\n"),
        );
        assert_eq!(
            probe("from repl_type_stubs import STUB_SECRET".to_owned()),
            "b.py:1:6: error[unresolved-import] Cannot resolve imported module `repl_type_stubs`\n",
        );
        // the scrub keeps SRC_ROOT itself intact — fresh checks still work
        assert_eq!(child.feed_complete("x: int = 1\nx"), MontyObject::Int(1));

        // back to unconfigured for the next case
        child.send(pb::parent_request::Kind::Reset(pb::Reset {}));
        let pb::child_event::Kind::Ok(_) = child.recv() else {
            panic!("expected Ok for the trailing Reset");
        };
    }
    child.shutdown();
}

/// The rendering choice lives in the dump envelope, so a session restored into
/// a fresh worker keeps reporting diagnostics the way its parent asked for.
#[test]
fn type_check_format_survives_dump_and_load() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        type_check: true,
        type_check_format: pb::TypeCheckFormat::Concise.into(),
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        ..Default::default()
    });
    assert_eq!(child.feed_complete("y = 1"), MontyObject::None);
    child.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = child.recv() else {
        panic!("expected DumpResult");
    };
    drop(child);

    let mut fresh = ChildProc::spawn();
    fresh.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let pb::child_event::Kind::Ok(_) = fresh.recv() else {
        panic!("expected Ok for Load");
    };
    let (_, event) = fresh.feed("x: int = 'not an int'");
    let pb::child_event::Kind::TypingError(typing) = event else {
        panic!("expected TypingError after Load, got {event:?}");
    };
    assert_eq!(
        typing.diagnostics,
        "main.py:1:10: error[invalid-assignment] Object of type `Literal[\"not an int\"]` is not assignable to `int`\n"
    );
    fresh.shutdown();
}

// =============================================================================
// Dump / Load (cross-process resume)
// =============================================================================

#[test]
fn dump_then_load_into_fresh_child_resumes() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    assert_eq!(child.feed_complete("base = 40"), MontyObject::None);

    // suspend at an external function call
    let (_, event) = child.feed("ext()");
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(call.function_name, "ext");

    // dump the suspended state, then kill this child outright
    child.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = child.recv() else {
        panic!("expected DumpResult");
    };
    assert!(!dump.state.is_empty());
    drop(child); // SIGKILL via Drop

    // a fresh child restores the dump and re-announces the suspension
    let mut fresh = ChildProc::spawn();
    fresh.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let (_, event) = fresh.recv_turn();
    let pb::child_event::Kind::FunctionCall(restored) = event else {
        panic!("expected re-emitted FunctionCall after Load, got {event:?}");
    };
    assert_eq!(restored.function_name, "ext");
    assert_eq!(restored.call_id, call.call_id);

    let (_, event) = fresh.resume_call(
        restored.call_id,
        pb::ext_function_result::Kind::ReturnValue(int_value(2)),
    );
    assert_eq!(expect_complete(event), MontyObject::Int(2));
    // session globals survived the round trip through the dump
    assert_eq!(fresh.feed_complete("base + 2"), MontyObject::Int(42));
    fresh.shutdown();
}

#[test]
fn type_check_state_survives_dump_and_load() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: true,
        type_check_stubs: None,
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        assert_message_annotations: None,
        ..Default::default()
    });
    // a committed snippet that later feeds must see through the dump
    assert_eq!(child.feed_complete("y = 1"), MontyObject::None);
    child.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = child.recv() else {
        panic!("expected DumpResult");
    };
    drop(child);

    let mut fresh = ChildProc::spawn();
    fresh.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let pb::child_event::Kind::Ok(_) = fresh.recv() else {
        panic!("expected Ok for Load");
    };
    // type-check enforcement survived the dump...
    let (_, event) = fresh.feed("x: int = 'not an int'");
    let pb::child_event::Kind::TypingError(_) = event else {
        panic!("expected TypingError after Load, got {event:?}");
    };
    // ... and so did the stubs committed before it
    assert_eq!(fresh.feed_complete("y + 1"), MontyObject::Int(2));
    fresh.shutdown();
}

#[test]
fn assert_annotation_option_survives_dump_and_load() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: false,
        type_check_stubs: None,
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        // 0 = annotations off on the wire.
        assert_message_annotations: Some(0),
        ..Default::default()
    });
    child.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = child.recv() else {
        panic!("expected DumpResult");
    };
    drop(child);

    let mut fresh = ChildProc::spawn();
    fresh.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let pb::child_event::Kind::Ok(_) = fresh.recv() else {
        panic!("expected Ok for Load");
    };

    let (_, event) = fresh.feed("assert 1 == 2");
    let error = expect_error(event);
    assert_eq!(error.exc_type, "AssertionError");
    assert_eq!(error.message, None);
    fresh.shutdown();
}

#[test]
fn assert_annotation_custom_limit_survives_dump_and_load() {
    let mut child = ChildProc::spawn();
    child.create_repl_with(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: false,
        type_check_stubs: None,
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        // Non-zero = annotations on, truncating operand reprs to N chars.
        assert_message_annotations: Some(6),
        ..Default::default()
    });
    child.send(pb::parent_request::Kind::Dump(pb::Dump {}));
    let pb::child_event::Kind::DumpResult(dump) = child.recv() else {
        panic!("expected DumpResult");
    };
    drop(child);

    let mut fresh = ChildProc::spawn();
    fresh.send(pb::parent_request::Kind::Load(pb::Load { state: dump.state }));
    let pb::child_event::Kind::Ok(_) = fresh.recv() else {
        panic!("expected Ok for Load");
    };

    let (_, event) = fresh.feed("assert 'abcdefghij' == ''");
    let error = expect_error(event);
    assert_eq!(error.exc_type, "AssertionError");
    assert_eq!(error.message.as_deref(), Some("assert 'abcde… == ''"));
    fresh.shutdown();
}

// =============================================================================
// Protocol violations and crashes
// =============================================================================

#[test]
fn protocol_violations_keep_the_child_alive() {
    let mut child = ChildProc::spawn();

    // feed without a session
    let (_, event) = child.feed("1 + 1");
    let error = expect_error(event);
    assert_eq!(error.exc_type, "RuntimeError");
    assert!(error.message.unwrap().starts_with("protocol violation"));

    // the child is still usable
    child.create_repl();

    // double create
    child.send(pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "again.py".to_owned(),
        limits: None,
        type_check: false,
        type_check_stubs: None,
        monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: PROTOCOL_VERSION,
        assert_message_annotations: None,
        ..Default::default()
    }));
    let error = expect_error(child.recv());
    assert!(error.message.unwrap().contains("already exists"));

    // resume with a bogus call id while suspended
    let (_, event) = child.feed("missing()");
    let pb::child_event::Kind::FunctionCall(call) = event else {
        panic!("expected FunctionCall, got {event:?}");
    };
    let (_, event) = child.resume_call(
        call.call_id + 1,
        pb::ext_function_result::Kind::ReturnValue(int_value(0)),
    );
    let error = expect_error(event);
    assert!(error.message.unwrap().starts_with("protocol violation"));

    // ... and the suspension is still resumable correctly
    let (_, event) = child.resume_call(
        call.call_id,
        pb::ext_function_result::Kind::NotFound("missing".to_owned()),
    );
    assert_eq!(expect_error(event).exc_type, "NameError");
    child.shutdown();
}

/// Builds a `Configure` with an explicit protocol version, for the version
/// checks below.
fn configure_with_protocol_version(protocol_version: u32, monty_version: &str) -> pb::Configure {
    pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: false,
        type_check_stubs: None,
        monty_version: monty_version.to_owned(),
        protocol_version,
        assert_message_annotations: None,
        ..Default::default()
    }
}

/// Asserts the child rejected the session and exited non-zero, returning the
/// fatal message.
fn expect_fatal_exit(mut child: ChildProc) -> String {
    let message = match child.recv() {
        pb::child_event::Kind::FatalError(fatal) => fatal.message,
        other => panic!("expected FatalError, got {other:?}"),
    };
    let status = child.child.wait().expect("wait");
    assert_eq!(status.code(), Some(4));
    // disarm Drop's kill — already exited
    let _ = child.child.kill();
    message
}

/// A parent speaking a protocol this build does not serve must be rejected
/// before any session exists, and told the range so it can downgrade — there
/// is no handshake to discover it from.
#[test]
fn unsupported_protocol_version_on_create_is_a_fatal_error() {
    let mut child = ChildProc::spawn();
    child.send(pb::parent_request::Kind::Configure(configure_with_protocol_version(
        PROTOCOL_VERSION + 1,
        env!("CARGO_PKG_VERSION"),
    )));
    let message = expect_fatal_exit(child);
    assert!(
        message.contains(&format!("unsupported protocol version {}", PROTOCOL_VERSION + 1)),
        "message should name the rejected version: {message}"
    );
    // Spelled out rather than taken from `check_protocol_version`, so rewording
    // the refusal a parent actually reads fails here.
    let supported = if MIN_SUPPORTED_PROTOCOL_VERSION == PROTOCOL_VERSION {
        format!("server supports protocol version {PROTOCOL_VERSION}")
    } else {
        format!("server supports protocol versions {MIN_SUPPORTED_PROTOCOL_VERSION} to {PROTOCOL_VERSION}")
    };
    assert!(
        message.contains(&supported),
        "message should name the supported range: {message}"
    );
    // Ahead of the range, so the client is on the wrong version rather than behind.
    assert!(
        message.contains("make sure you are using the correct client version"),
        "message should point at the client version: {message}"
    );
}

/// Zero means the parent declared nothing — it predates the field, or is not a
/// monty parent. Without in-band negotiation it cannot be assumed compatible.
#[test]
fn undeclared_protocol_version_is_a_fatal_error() {
    let mut child = ChildProc::spawn();
    child.send(pb::parent_request::Kind::Configure(configure_with_protocol_version(
        0,
        env!("CARGO_PKG_VERSION"),
    )));
    let message = expect_fatal_exit(child);
    assert!(
        message.contains("unsupported protocol version 0"),
        "message should name the rejected version: {message}"
    );
    // Below the range, so the client needs to move forward.
    assert!(
        message.contains("try updating to a newer client version"),
        "message should tell the client to update: {message}"
    );
}

/// The package version is informational: a parent from a different build is
/// served as long as its protocol version is one this build speaks.
#[test]
fn differing_package_version_is_accepted() {
    let mut child = ChildProc::spawn();
    child.send(pb::parent_request::Kind::Configure(configure_with_protocol_version(
        PROTOCOL_VERSION,
        "0.0.0-not-a-real-version",
    )));
    assert!(
        matches!(child.recv(), pb::child_event::Kind::Ok(_)),
        "a mismatched package version must not end the session"
    );
    child.shutdown();
}

#[test]
fn garbage_stdin_is_a_fatal_error() {
    let mut child = ChildProc::spawn();
    // valid length prefix followed by a truncated stream: the child reads a
    // mangled frame and must bail out with FatalError + EX_PROTOCOL
    let raw = &mut child.writer;
    raw.write_all(&[0xFF, 0xFF, 0xFF, 0x7F]).unwrap();
    raw.flush().unwrap();
    drop_stdin(&mut child);

    match child.recv() {
        pb::child_event::Kind::FatalError(fatal) => assert!(fatal.message.contains("malformed request frame")),
        other => panic!("expected FatalError, got {other:?}"),
    }
    let status = child.child.wait().expect("wait");
    assert_eq!(status.code(), Some(76)); // EX_PROTOCOL
    // disarm Drop's kill — already exited
    let _ = child.child.kill();
}

#[test]
fn killed_child_is_detected_as_eof() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    // run forever (no limits), then kill the child mid-execution
    child.send(pb::parent_request::Kind::Feed(pb::Feed {
        code: "while True:\n    pass".to_owned(),
        inputs: vec![],
        skip_type_check: false,
    }));
    thread::sleep(Duration::from_millis(200));
    child.child.kill().expect("kill");

    // the parent observes EOF (or a truncated frame), never a hang
    match child.reader.read::<pb::ChildEvent>() {
        Ok(None) | Err(FrameError::Truncated | FrameError::Io(_)) => {}
        other => panic!("expected EOF after kill, got {other:?}"),
    }
    let status = child.child.wait().expect("wait");
    assert!(!status.success());
}

#[test]
fn reset_returns_child_to_idle_for_reuse() {
    let mut child = ChildProc::spawn();
    child.create_repl();
    assert_eq!(child.feed_complete("x = 1"), MontyObject::None);
    child.send(pb::parent_request::Kind::Reset(pb::Reset {}));
    let pb::child_event::Kind::Ok(_) = child.recv() else {
        panic!("expected Ok for Reset");
    };
    // a fresh session has none of the previous session's state
    child.create_repl();
    let (_, event) = child.feed("x");
    let pb::child_event::Kind::NameLookup(lookup) = event else {
        panic!("expected NameLookup for undefined x, got {event:?}");
    };
    assert_eq!(lookup.name, "x");
    child.shutdown();
}

/// Closes the child's stdin without dropping the rest of the harness.
fn drop_stdin(_child: &mut ChildProc) {
    // ChildProc owns ChildStdin; nothing to do — the test just stops
    // writing. Present for readability at call sites.
}
