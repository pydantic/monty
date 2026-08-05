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

use monty_proto::{FrameError, FrameReader, WireObject, pb, write_frame};
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
            assert_message_annotations: None,
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
    assert!(!call.method_call);
    assert_eq!(call.args, vec![MontyObject::Int(1), MontyObject::Int(2)]);

    let (_, event) = child.resume_call(call.call_id, pb::ext_function_result::Kind::ReturnValue(int_value(3)));
    assert_eq!(expect_complete(event), MontyObject::Int(3));
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
        assert_message_annotations: None,
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

/// Known large results are rejected against allocator usage before they can
/// jump from below the soft limit past the hard ceiling.
#[test]
fn large_allocations_are_rejected_before_the_hard_limit() {
    let cases = [
        "'x' * 10_000_000",
        "b'x' * 10_000_000",
        "[None] * 1_000_000",
        "2 ** 10_000_000",
        "1 << 10_000_000",
        "('a' * 1000).replace('a', 'b' * 2000)",
    ];
    let mut messages = Vec::new();

    for code in cases {
        let mut child = ChildProc::spawn();
        child.create_repl_with(configure_with_max_memory(1024 * 1024));
        let (_, event) = child.feed(code);
        let error = expect_error(event);
        assert_eq!(error.exc_type, "MemoryError", "{code}");
        messages.push(error.message.expect("MemoryError should have a message"));
        assert_eq!(child.feed_complete("1 + 1"), MontyObject::Int(2), "{code}");
        child.shutdown();
    }

    assert_eq!(
        messages,
        [
            "memory limit exceeded: 10030889 bytes > 1048576 bytes",
            "memory limit exceeded: 10031021 bytes > 1048576 bytes",
            "memory limit exceeded: 16031143 bytes > 1048576 bytes",
            "memory limit exceeded: 10030982 bytes > 1048576 bytes",
            "memory limit exceeded: 1280983 bytes > 1048576 bytes",
            "memory limit exceeded: 2034521 bytes > 1048576 bytes",
        ]
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
    assert_eq!(status.code(), Some(monty_proto::OOM_EXIT_CODE), "got {status:?}");
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
    assert_eq!(status.code(), Some(monty_proto::OOM_EXIT_CODE), "got {status:?}");
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
    assert_eq!(status.code(), Some(monty_proto::OOM_EXIT_CODE), "got {status:?}");
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
        assert_message_annotations: None,
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
        assert_message_annotations: None,
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
        assert_message_annotations: None,
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
        // 0 = annotations off on the wire.
        assert_message_annotations: Some(0),
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
        // Non-zero = annotations on, truncating operand reprs to N chars.
        assert_message_annotations: Some(6),
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
        assert_message_annotations: None,
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

#[test]
fn version_skew_on_create_is_a_fatal_error() {
    let mut child = ChildProc::spawn();
    // A parent built against a different monty version: the child must reject
    // the session with a FatalError and exit non-zero rather than risk a wire
    // desync from a mismatched frame layout.
    child.send(pb::parent_request::Kind::Configure(pb::Configure {
        script_name: "main.py".to_owned(),
        limits: None,
        type_check: false,
        type_check_stubs: None,
        monty_version: "0.0.0-not-a-real-version".to_owned(),
        assert_message_annotations: None,
    }));
    match child.recv() {
        pb::child_event::Kind::FatalError(fatal) => assert!(fatal.message.contains("version skew")),
        other => panic!("expected FatalError, got {other:?}"),
    }
    let status = child.child.wait().expect("wait");
    assert_eq!(status.code(), Some(4));
    // disarm Drop's kill — already exited
    let _ = child.child.kill();
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
