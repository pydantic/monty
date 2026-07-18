//! Integration tests for `monty subprocess`: spawn the real binary and
//! drive it over the wire protocol, including crash scenarios — the entire
//! point of the subprocess mode is that a dead child is a recoverable event
//! for the parent.

use std::{
    fs,
    io::Write,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::Duration,
};

use monty::MontyObject;
use monty_proto::{FrameError, FrameReader, WireObject, pb, write_frame};

/// A spawned `monty subprocess` child with framed pipes.
struct ChildProc {
    child: Child,
    writer: ChildStdin,
    reader: FrameReader<ChildStdout>,
}

impl ChildProc {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_monty"))
            .arg("subprocess")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn monty subprocess");
        let writer = child.stdin.take().expect("child stdin");
        let reader = FrameReader::new(child.stdout.take().expect("child stdout"));
        Self { child, writer, reader }
    }

    fn send(&mut self, kind: pb::parent_request::Kind) {
        write_frame(&mut self.writer, &pb::ParentRequest { kind: Some(kind) }).expect("failed to write request");
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
        self.feed_with(code, vec![], vec![])
    }

    fn feed_with(
        &mut self,
        code: &str,
        inputs: Vec<pb::NamedValue>,
        mounts: Vec<pb::Mount>,
    ) -> (Vec<pb::Print>, pb::child_event::Kind) {
        self.send(pb::parent_request::Kind::Feed(pb::Feed {
            code: code.to_owned(),
            inputs,
            mounts,
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
    let (_, event) = child.feed_with("a + 1", inputs, vec![]);
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
    assert_eq!(call.function_name, "Path.read_text");
    assert_eq!(call.args, vec![MontyObject::Path("/data.txt".to_owned())]);

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
// Mounts (child-local filesystem)
// =============================================================================

#[test]
fn mounted_reads_are_handled_locally() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "mounted!").unwrap();
    let mount = pb::Mount {
        virtual_path: "/mnt".to_owned(),
        host_path: dir.path().to_string_lossy().into_owned(),
        mode: pb::MountMode::ReadOnly.into(),
        write_bytes_limit: None,
    };

    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed_with(
        "from pathlib import Path\nPath('/mnt/data.txt').read_text()",
        vec![],
        vec![mount],
    );
    // no OsCall event on the wire — the mount handled it inside the child
    assert_eq!(expect_complete(event), MontyObject::String("mounted!".to_owned()));
    child.shutdown();
}

#[test]
fn read_only_mount_write_raises_inside_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let mount = pb::Mount {
        virtual_path: "/mnt".to_owned(),
        host_path: dir.path().to_string_lossy().into_owned(),
        mode: pb::MountMode::ReadOnly.into(),
        write_bytes_limit: None,
    };

    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed_with(
        "from pathlib import Path\nPath('/mnt/new.txt').write_text('x')",
        vec![],
        vec![mount],
    );
    let error = expect_error(event);
    assert_eq!(error.exc_type, "PermissionError");
    child.shutdown();
}

#[test]
fn overlay_mount_discards_writes_at_feed_end() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data.txt"), "original").unwrap();
    let mount = pb::Mount {
        virtual_path: "/mnt".to_owned(),
        host_path: dir.path().to_string_lossy().into_owned(),
        mode: pb::MountMode::Overlay.into(),
        write_bytes_limit: None,
    };

    let mut child = ChildProc::spawn();
    child.create_repl();
    let (_, event) = child.feed_with(
        "from pathlib import Path\nPath('/mnt/data.txt').write_text('changed')\nPath('/mnt/data.txt').read_text()",
        vec![],
        vec![mount.clone()],
    );
    assert_eq!(expect_complete(event), MontyObject::String("changed".to_owned()));
    // the host file is untouched and the overlay does not persist to the next feed
    assert_eq!(fs::read_to_string(dir.path().join("data.txt")).unwrap(), "original");
    let (_, event) = child.feed_with("Path('/mnt/data.txt').read_text()", vec![], vec![mount]);
    assert_eq!(expect_complete(event), MontyObject::String("original".to_owned()));
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
    fresh.send(pb::parent_request::Kind::Load(pb::Load {
        state: dump.state,
        mounts: vec![],
    }));
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
    fresh.send(pb::parent_request::Kind::Load(pb::Load {
        state: dump.state,
        mounts: vec![],
    }));
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
    fresh.send(pb::parent_request::Kind::Load(pb::Load {
        state: dump.state,
        mounts: vec![],
    }));
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
    fresh.send(pb::parent_request::Kind::Load(pb::Load {
        state: dump.state,
        mounts: vec![],
    }));
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
    // mangled frame and must bail out with FatalError + exit code 2
    let raw = &mut child.writer;
    raw.write_all(&[0xFF, 0xFF, 0xFF, 0x7F]).unwrap();
    raw.flush().unwrap();
    drop_stdin(&mut child);

    match child.recv() {
        pb::child_event::Kind::FatalError(fatal) => assert!(fatal.message.contains("malformed request frame")),
        other => panic!("expected FatalError, got {other:?}"),
    }
    let status = child.child.wait().expect("wait");
    assert_eq!(status.code(), Some(2));
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
        mounts: vec![],
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
