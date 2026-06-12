//! `monty --subprocess`: protocol child mode.
//!
//! Reads framed [`pb::Request`]s from stdin and writes framed [`pb::Event`]s
//! to stdout (see `monty-proto` for the schema and protocol rules). The child
//! is strictly turn-based: one request in, zero or more streamed `Print`
//! events out, then exactly one turn-ending event.
//!
//! Crash isolation is the entire point of this mode: the parent must treat a
//! child that exits (or EOFs) *without* a `FatalError` event as crashed —
//! stack overflows and allocator aborts produce no final frame.
//!
//! In this mode stdout belongs exclusively to the frame writer; diagnostics
//! go to stderr.

use std::{
    borrow::Cow,
    cell::RefCell,
    io::{self, BufWriter, Stdout},
    mem, panic,
    process::ExitCode,
    rc::Rc,
};

use monty::{
    ExcType, ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRepl, PrintWriter,
    PrintWriterCallback, ReplProgress, ReplStartError, fs::MountTable,
};
use monty_proto::{
    FrameReader, FrameWriter, PROTOCOL_VERSION, build_mount_table, exceeds_max_value_depth, future_results_from_proto,
    pairs_to_proto, pb, values_to_proto,
};
// see main.rs — import style matches the existing rustfmt skip there
#[rustfmt::skip]
use monty_type_checking::{SourceFile, type_check};

/// The child always runs with `LimitedTracker`: an absent/empty limits message
/// behaves like `ResourceLimits::new()`, and a single tracker type keeps the
/// session state enum free of generics.
type Tracker = LimitedTracker;

/// Version tag of the opaque dump envelope produced by `Dump`.
///
/// Wire layout: `[DUMP_VERSION u16 LE][tag u8][session meta][postcard
/// payload]` where tag 0 is a `MontyRepl` (idle session) and tag 1 a
/// `ReplProgress` (suspended). The session meta carries the child-side state
/// that lives *outside* the repl — script name and accumulated type-check
/// stubs — so a `Load`ed session keeps type-check enforcement instead of
/// silently dropping it:
///
/// `[script_name str][type_check u8]` and, when `type_check` is 1,
/// `[committed_stubs str][has_pending u8][pending_snippet str?]`, where each
/// `str` is a `u32 LE` byte length followed by UTF-8 bytes. The payload is
/// monty's postcard format — only a monty child of the same version can
/// restore it.
const DUMP_VERSION: u16 = 1;

/// Runs the subprocess child loop until EOF, `Shutdown`, or a fatal error.
pub(crate) fn run() -> ExitCode {
    install_panic_hook();
    let writer = Rc::new(RefCell::new(FrameWriter::new(BufWriter::new(io::stdout()))));
    let mut reader = FrameReader::new(io::stdin().lock());
    let mut child = Child::new(Rc::clone(&writer));

    loop {
        match reader.read::<pb::Request>() {
            Ok(Some(request)) => match child.handle(request) {
                Ok(Turn::Continue) => {}
                Ok(Turn::Exit(code)) => return code,
                // an oversize event was rejected before any bytes hit the
                // wire, so the stream is still in sync and the parent can
                // receive a parseable last gasp
                Err(monty_proto::FrameError::FrameTooLarge { len, max }) => {
                    child.fatal(&format!("response frame of {len} bytes exceeds maximum of {max} bytes"));
                    return ExitCode::from(2);
                }
                // writing to stdout failed: the parent is gone, nothing left to do
                Err(_) => return ExitCode::from(3),
            },
            // clean EOF at a frame boundary: the parent closed stdin
            Ok(None) => return ExitCode::SUCCESS,
            Err(err) => {
                // the stream is desynchronized — unrecoverable by design
                child.fatal(&format!("malformed request frame: {err}"));
                return ExitCode::from(2);
            }
        }
    }
}

/// What the main loop should do after a request has been handled.
enum Turn {
    Continue,
    Exit(ExitCode),
}

/// REPL session state of the child.
enum SessionState {
    /// No session; only `Hello` / `ReplCreate` / `Load` / `Reset` / `Shutdown`
    /// are valid.
    Idle,
    /// Session ready for the next `ReplFeed`.
    Ready(Box<MontyRepl<Tracker>>),
    /// Mid-feed, waiting for a resume request. Never holds
    /// `ReplProgress::Complete` — completion ends the turn immediately.
    Suspended(Box<ReplProgress<Tracker>>),
}

/// Per-session type-check state, mirroring `pydantic_monty.MontyRepl`:
/// successfully committed snippets accumulate as stubs so later snippets can
/// reference names defined by earlier ones.
struct TypeCheckState {
    /// User-provided stubs plus every snippet that has completed successfully.
    committed_stubs: String,
    /// The in-flight snippet; committed on `Complete`, discarded on error.
    pending_snippet: Option<String>,
}

/// All child state plus the shared frame writer.
struct Child {
    writer: SharedWriter,
    state: SessionState,
    /// Script name of the current session (used for error and type-check
    /// diagnostics).
    script_name: String,
    /// Mount table for the in-flight feed; rebuilt per feed, dropped when the
    /// feed completes. Not part of dumps — a `Load`ed suspended feed has no
    /// mounts, so its remaining OS calls all bubble to the parent.
    mounts: Option<MountTable>,
    /// `Some` when the session was created with `type_check: true`.
    type_check: Option<TypeCheckState>,
    /// Whether the `Hello` handshake has completed.
    helloed: bool,
}

/// The stdout frame writer, shared between turn-ending writes and the print
/// callback that streams `Print` events mid-execution.
type SharedWriter = Rc<RefCell<FrameWriter<BufWriter<Stdout>>>>;

impl Child {
    fn new(writer: SharedWriter) -> Self {
        Self {
            writer,
            state: SessionState::Idle,
            script_name: String::new(),
            mounts: None,
            type_check: None,
            helloed: false,
        }
    }

    /// Handles one request: emits exactly one turn-ending event and returns
    /// what the main loop should do next. `Err` means stdout is broken.
    fn handle(&mut self, request: pb::Request) -> Result<Turn, monty_proto::FrameError> {
        let Some(kind) = request.kind else {
            self.send(&violation("request has no kind"))?;
            return Ok(Turn::Continue);
        };

        // The handshake must come first, exactly once.
        if let pb::request::Kind::Hello(hello) = kind {
            return self.handle_hello(&hello);
        }
        if !self.helloed {
            self.fatal("first request must be Hello");
            return Ok(Turn::Exit(ExitCode::from(2)));
        }

        let mut event = match kind {
            pb::request::Kind::Hello(_) => unreachable!("handled above"),
            pb::request::Kind::ReplCreate(create) => self.handle_repl_create(create),
            pb::request::Kind::ReplFeed(feed) => self.handle_repl_feed(feed),
            pb::request::Kind::ResumeCall(resume) => self.handle_resume_call(resume),
            pb::request::Kind::ResumeNameLookup(resume) => self.handle_resume_name_lookup(resume),
            pb::request::Kind::ResumeFutures(resume) => self.handle_resume_futures(resume),
            pb::request::Kind::Dump(_) => self.handle_dump(),
            pb::request::Kind::Load(load) => self.handle_load(&load),
            pb::request::Kind::Reset(_) => {
                self.reset();
                ok_event()
            }
            pb::request::Kind::Shutdown(_) => {
                self.send(&ok_event())?;
                return Ok(Turn::Exit(ExitCode::SUCCESS));
            }
        };
        self.stamp_execution_time(&mut event);
        self.send(&event)?;
        Ok(Turn::Continue)
    }

    /// Stamps the session's cumulative execution time and `max_duration`
    /// budget onto a turn-ending event, making the child the single source of
    /// truth for timing: the parent derives its watchdog backstop from these
    /// fields instead of keeping a second clock. Left zero/absent when no
    /// session exists (handshake, post-`Reset`, failed create/load).
    fn stamp_execution_time(&self, event: &mut pb::Event) {
        let tracker = match &self.state {
            SessionState::Ready(repl) => repl.tracker(),
            SessionState::Suspended(progress) => progress.tracker(),
            SessionState::Idle => return,
        };
        event.total_execution_micros = u64::try_from(tracker.elapsed().as_micros()).unwrap_or(u64::MAX);
        event.max_duration_micros = tracker
            .max_duration()
            .map(|max| u64::try_from(max.as_micros()).unwrap_or(u64::MAX));
    }

    fn handle_hello(&mut self, hello: &pb::Hello) -> Result<Turn, monty_proto::FrameError> {
        if self.helloed {
            self.send(&violation("Hello after the handshake has completed"))?;
            return Ok(Turn::Continue);
        }
        if hello.protocol_version > PROTOCOL_VERSION {
            self.fatal(&format!(
                "unsupported protocol version {} (this child speaks {PROTOCOL_VERSION})",
                hello.protocol_version
            ));
            return Ok(Turn::Exit(ExitCode::from(2)));
        }
        self.helloed = true;
        self.send(&event(pb::event::Kind::HelloReply(pb::HelloReply {
            protocol_version: PROTOCOL_VERSION,
            monty_version: env!("CARGO_PKG_VERSION").to_owned(),
        })))?;
        Ok(Turn::Continue)
    }

    fn handle_repl_create(&mut self, create: pb::ReplCreate) -> pb::Event {
        if !matches!(self.state, SessionState::Idle) {
            return violation("ReplCreate while a session already exists");
        }
        let limits = match create.limits.unwrap_or_default().try_into() {
            Ok(limits) => limits,
            Err(err) => return violation(&format!("invalid limits: {err}")),
        };
        self.script_name = create.script_name;
        self.type_check = create.type_check.then(|| TypeCheckState {
            committed_stubs: create.type_check_stubs.unwrap_or_default(),
            pending_snippet: None,
        });
        self.state = SessionState::Ready(Box::new(MontyRepl::new(&self.script_name, LimitedTracker::new(limits))));
        ok_event()
    }

    fn handle_repl_feed(&mut self, feed: pb::ReplFeed) -> pb::Event {
        let SessionState::Ready(_) = &self.state else {
            return violation("ReplFeed without a session ready for input");
        };
        if !feed.skip_type_check
            && let Some(event) = self.type_check_feed(&feed.code)
        {
            return event;
        }
        self.mounts = match build_mount_table(feed.mounts) {
            Ok(mounts) => mounts,
            Err(err) => return violation(&format!("invalid mounts: {err}")),
        };
        let inputs = match named_inputs(feed.inputs) {
            Ok(inputs) => inputs,
            Err(event) => return *event,
        };
        let SessionState::Ready(repl) = mem::replace(&mut self.state, SessionState::Idle) else {
            unreachable!("checked above");
        };
        // snippets fed with skip_type_check never become type-check context:
        // the caller explicitly excluded them from checking, so later snippets
        // must not be checked against their (unchecked) bindings either
        if !feed.skip_type_check
            && let Some(state) = &mut self.type_check
        {
            state.pending_snippet = Some(feed.code.clone());
        }
        let mut print = ProtoPrint::new(Rc::clone(&self.writer));
        let result = repl.feed_start(&feed.code, inputs, PrintWriter::Callback(&mut print));
        let event = self.drive(result, &mut print);
        print.drain();
        event
    }

    fn handle_resume_call(&mut self, resume: pb::ResumeCall) -> pb::Event {
        let expected_call_id = match &self.state {
            SessionState::Suspended(progress) => match progress.as_ref() {
                ReplProgress::FunctionCall(call) => Some(call.call_id),
                ReplProgress::OsCall(call) => Some(call.call_id),
                _ => None,
            },
            _ => None,
        };
        let Some(call_id) = expected_call_id else {
            return violation("ResumeCall without a suspended function/OS call");
        };
        if resume.call_id != call_id {
            return violation(&format!(
                "ResumeCall call_id {} does not match {call_id}",
                resume.call_id
            ));
        }
        let result: ExtFunctionResult = match resume.result {
            Some(result) => match result.try_into() {
                Ok(result) => result,
                Err(err) => return violation(&format!("invalid result: {err}")),
            },
            None => return violation("ResumeCall has no result"),
        };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Idle) else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(Rc::clone(&self.writer));
        let outcome = match *progress {
            ReplProgress::FunctionCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            ReplProgress::OsCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            _ => unreachable!("checked above"),
        };
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    fn handle_resume_name_lookup(&mut self, resume: pb::ResumeNameLookup) -> pb::Event {
        let SessionState::Suspended(progress) = &self.state else {
            return violation("ResumeNameLookup without a suspended name lookup");
        };
        if !matches!(progress.as_ref(), ReplProgress::NameLookup(_)) {
            return violation("ResumeNameLookup without a suspended name lookup");
        }
        let result = match resume.try_into() {
            Ok(result) => result,
            Err(err) => return violation(&format!("invalid result: {err}")),
        };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Idle) else {
            unreachable!("checked above");
        };
        let ReplProgress::NameLookup(lookup) = *progress else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(Rc::clone(&self.writer));
        let outcome = lookup.resume(result, PrintWriter::Callback(&mut print));
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    fn handle_resume_futures(&mut self, resume: pb::ResumeFutures) -> pb::Event {
        let SessionState::Suspended(progress) = &self.state else {
            return violation("ResumeFutures without suspended futures");
        };
        if !matches!(progress.as_ref(), ReplProgress::ResolveFutures(_)) {
            return violation("ResumeFutures without suspended futures");
        }
        let results = match future_results_from_proto(resume.results) {
            Ok(results) => results,
            Err(err) => return violation(&format!("invalid results: {err}")),
        };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Idle) else {
            unreachable!("checked above");
        };
        let ReplProgress::ResolveFutures(state) = *progress else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(Rc::clone(&self.writer));
        let outcome = state.resume(results, PrintWriter::Callback(&mut print));
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    /// Serializes the current session into the opaque dump envelope. The
    /// session stays live — dumping is read-only.
    fn handle_dump(&mut self) -> pb::Event {
        let dumped = match &self.state {
            SessionState::Ready(repl) => repl.dump().map(|bytes| (0u8, bytes)),
            SessionState::Suspended(progress) => progress.dump().map(|bytes| (1u8, bytes)),
            SessionState::Idle => return violation("Dump without a session"),
        };
        match dumped {
            Ok((tag, payload)) => {
                let mut state = Vec::with_capacity(payload.len() + 64);
                state.extend_from_slice(&DUMP_VERSION.to_le_bytes());
                state.push(tag);
                push_str_field(&mut state, &self.script_name);
                match &self.type_check {
                    Some(tc) => {
                        state.push(1);
                        push_str_field(&mut state, &tc.committed_stubs);
                        match &tc.pending_snippet {
                            Some(snippet) => {
                                state.push(1);
                                push_str_field(&mut state, snippet);
                            }
                            None => state.push(0),
                        }
                    }
                    None => state.push(0),
                }
                state.extend_from_slice(&payload);
                event(pb::event::Kind::DumpResult(pb::DumpResult { state }))
            }
            Err(err) => violation(&format!("dump failed: {err}")),
        }
    }

    /// Restores a dump produced by [`Self::handle_dump`] into this (idle)
    /// child. A restored suspension re-emits its suspension event so the
    /// parent learns the resume point.
    fn handle_load(&mut self, load: &pb::Load) -> pb::Event {
        if !matches!(self.state, SessionState::Idle) {
            return violation("Load while a session already exists");
        }
        let Some((version_bytes, rest)) = load.state.split_at_checked(2) else {
            return violation("dump state too short");
        };
        let version = u16::from_le_bytes([version_bytes[0], version_bytes[1]]);
        if version != DUMP_VERSION {
            return violation(&format!("unsupported dump version {version} (expected {DUMP_VERSION})"));
        }
        let Some((&tag, rest)) = rest.split_first() else {
            return violation("dump state too short");
        };
        let Some((script_name, type_check, payload)) = take_session_meta(rest) else {
            return violation("malformed dump session metadata");
        };
        let event = match tag {
            0 => match MontyRepl::load(payload) {
                Ok(repl) => {
                    self.state = SessionState::Ready(Box::new(repl));
                    ok_event()
                }
                Err(err) => violation(&format!("failed to load session: {err}")),
            },
            1 => match ReplProgress::load(payload) {
                Ok(ReplProgress::Complete { repl, value }) => {
                    // a dump is never taken at Complete, but a forged/legacy
                    // one could contain it; surface the value rather than fail
                    self.state = SessionState::Ready(Box::new(repl));
                    complete_event(&value)
                }
                Ok(progress) => {
                    let event = suspension_event(&progress);
                    self.state = SessionState::Suspended(Box::new(progress));
                    event
                }
                Err(err) => violation(&format!("failed to load suspended session: {err}")),
            },
            other => violation(&format!("unknown dump tag {other}")),
        };
        // adopt the restored metadata only once the payload actually loaded —
        // a failed load must leave the child fully idle
        if !matches!(self.state, SessionState::Idle) {
            self.script_name = script_name;
            self.type_check = type_check;
        }
        event
    }

    /// Drives execution until it needs the parent: handles mount-covered OS
    /// calls locally and returns the turn-ending event for everything else.
    fn drive(
        &mut self,
        mut result: Result<ReplProgress<Tracker>, Box<ReplStartError<Tracker>>>,
        print: &mut ProtoPrint,
    ) -> pb::Event {
        loop {
            match result {
                Ok(ReplProgress::Complete { repl, value }) => {
                    self.state = SessionState::Ready(Box::new(repl));
                    self.mounts = None;
                    if let Some(state) = &mut self.type_check
                        && let Some(snippet) = state.pending_snippet.take()
                    {
                        state.committed_stubs.push('\n');
                        state.committed_stubs.push_str(&snippet);
                    }
                    // a value too deep for the wire must fail cleanly here —
                    // shipping it would be an undecodable frame, which the
                    // parent has to treat as a worker crash
                    if exceeds_max_value_depth(&value) {
                        return error_event(ExcType::RuntimeError, "Max output depth exceeded");
                    }
                    return complete_event(&value);
                }
                Ok(ReplProgress::OsCall(mut call)) => {
                    // mount-covered OS calls are handled locally; the parent
                    // never sees them
                    let handled = self
                        .mounts
                        .as_mut()
                        .and_then(|mounts| mounts.handle_os_call(&call.function_call));
                    if let Some(outcome) = handled {
                        let ext: ExtFunctionResult = match outcome {
                            Ok(obj) => obj.into(),
                            Err(err) => err.into_exception().into(),
                        };
                        result = call.resume(ext, PrintWriter::Callback(print));
                        continue;
                    }
                    let function_call = call.take_function_call();
                    let name = function_call.name();
                    // only the child knows per-call no-handler semantics, so
                    // the event carries the error a handler-less parent
                    // should answer with
                    let not_handled_error = function_call.on_no_handler();
                    let call_id = call.call_id;
                    let (args, kwargs) = function_call.to_args();
                    if args.iter().any(exceeds_max_value_depth)
                        || kwargs
                            .iter()
                            .any(|(k, v)| exceeds_max_value_depth(k) || exceeds_max_value_depth(v))
                    {
                        let err =
                            MontyException::new(ExcType::RuntimeError, Some("Max argument depth exceeded".to_owned()));
                        result = call.resume(ExtFunctionResult::Error(err), PrintWriter::Callback(print));
                        continue;
                    }
                    self.state = SessionState::Suspended(Box::new(ReplProgress::OsCall(call)));
                    return event(pb::event::Kind::OsCall(pb::OsCall {
                        function_name: name.to_owned(),
                        args: values_to_proto(&args),
                        kwargs: pairs_to_proto(&kwargs),
                        call_id,
                        not_handled_error: Some((&not_handled_error).into()),
                    }));
                }
                Ok(ReplProgress::FunctionCall(call)) => {
                    // arguments too deep for the wire resume the call with a
                    // catchable error instead of corrupting the protocol
                    if call.args.iter().any(exceeds_max_value_depth)
                        || call
                            .kwargs
                            .iter()
                            .any(|(k, v)| exceeds_max_value_depth(k) || exceeds_max_value_depth(v))
                    {
                        let err =
                            MontyException::new(ExcType::RuntimeError, Some("Max argument depth exceeded".to_owned()));
                        result = call.resume(ExtFunctionResult::Error(err), PrintWriter::Callback(print));
                        continue;
                    }
                    let event = suspension_event_function_call(&call);
                    self.state = SessionState::Suspended(Box::new(ReplProgress::FunctionCall(call)));
                    return event;
                }
                Ok(progress) => {
                    let event = suspension_event(&progress);
                    self.state = SessionState::Suspended(Box::new(progress));
                    return event;
                }
                Err(err) => {
                    // Python-level failure: the session always survives
                    self.state = SessionState::Ready(Box::new(err.repl));
                    self.mounts = None;
                    if let Some(state) = &mut self.type_check {
                        state.pending_snippet = None;
                    }
                    return event(pb::event::Kind::Error(pb::Error {
                        exception: Some((&err.error).into()),
                    }));
                }
            }
        }
    }

    /// Type-checks a snippet against the accumulated session stubs. Returns
    /// the turn-ending event if the check fails (or errors), `None` to
    /// proceed with execution.
    fn type_check_feed(&mut self, code: &str) -> Option<pb::Event> {
        let state = self.type_check.as_ref()?;
        let stubs =
            (!state.committed_stubs.is_empty()).then(|| SourceFile::new(&state.committed_stubs, "repl_type_stubs.pyi"));
        match type_check(&SourceFile::new(code, &self.script_name), stubs.as_ref()) {
            Ok(None) => None,
            Ok(Some(diagnostics)) => Some(event(pb::event::Kind::TypingError(pb::TypingError {
                diagnostics: diagnostics.to_string(),
            }))),
            Err(err) => Some(violation(&format!("type checker failed: {err}"))),
        }
    }

    /// Drops all session state, returning to `Idle`.
    fn reset(&mut self) {
        self.state = SessionState::Idle;
        self.mounts = None;
        self.type_check = None;
        self.script_name = String::new();
    }

    fn send(&self, event: &pb::Event) -> Result<(), monty_proto::FrameError> {
        self.writer.borrow_mut().write(event)
    }

    /// Best-effort `FatalError` event, duplicated to stderr. Used only for
    /// unrecoverable conditions — the child exits right after.
    fn fatal(&self, message: &str) {
        eprintln!("monty --subprocess fatal error: {message}");
        let _ = self.send(&event(pb::event::Kind::FatalError(pb::FatalError {
            message: message.to_owned(),
        })));
    }
}

/// Wraps an event kind into an `Event` with zeroed timing fields —
/// `Child::handle` stamps `total_execution_micros`/`max_duration_micros`
/// onto every turn-ending event just before it is sent.
fn event(kind: pb::event::Kind) -> pb::Event {
    pb::Event {
        kind: Some(kind),
        ..Default::default()
    }
}

/// Builds the turn-ending event for a recoverable protocol violation (wrong
/// state, bad call id, invalid payload). The child's state is unchanged.
fn violation(message: &str) -> pb::Event {
    event(pb::event::Kind::Error(pb::Error {
        exception: Some(pb::MontyError {
            exc_type: ExcType::RuntimeError.to_string(),
            message: Some(format!("protocol violation: {message}")),
            traceback: vec![],
        }),
    }))
}

fn ok_event() -> pb::Event {
    event(pb::event::Kind::Ok(pb::Ok {}))
}

/// Builds a turn-ending `Error` event from an exception type and message.
fn error_event(exc_type: ExcType, message: &str) -> pb::Event {
    event(pb::event::Kind::Error(pb::Error {
        exception: Some(pb::MontyError {
            exc_type: exc_type.to_string(),
            message: Some(message.to_owned()),
            traceback: vec![],
        }),
    }))
}

/// Builds the suspension event for a fresh `FunctionCall` (depth-checked by
/// the caller).
fn suspension_event_function_call(call: &monty::ReplFunctionCall<Tracker>) -> pb::Event {
    event(pb::event::Kind::FunctionCall(pb::FunctionCall {
        function_name: call.function_name.clone(),
        args: values_to_proto(&call.args),
        kwargs: pairs_to_proto(&call.kwargs),
        call_id: call.call_id,
        method_call: call.method_call,
    }))
}

fn complete_event(value: &MontyObject) -> pb::Event {
    event(pb::event::Kind::Complete(pb::Complete {
        value: Some(value.into()),
    }))
}

/// Builds the suspension event for a non-`Complete`, non-`OsCall` progress
/// state (OS calls are special-cased in `drive` because emitting them consumes
/// the call's argument payload).
fn suspension_event(progress: &ReplProgress<Tracker>) -> pb::Event {
    let kind = match progress {
        ReplProgress::FunctionCall(call) => pb::event::Kind::FunctionCall(pb::FunctionCall {
            function_name: call.function_name.clone(),
            args: values_to_proto(&call.args),
            kwargs: pairs_to_proto(&call.kwargs),
            call_id: call.call_id,
            method_call: call.method_call,
        }),
        ReplProgress::OsCall(call) => {
            // reached only on `Load` of a dumped OsCall suspension, where the
            // payload was already consumed by `take_function_call` (leaving
            // `Used`, whose `name()` would panic) — the parent re-learns the
            // name/args from its own records; a fresh suspension goes through
            // `drive` instead
            let has_payload = !matches!(call.function_call, monty::OsFunctionCall::Used);
            pb::event::Kind::OsCall(pb::OsCall {
                function_name: if has_payload {
                    call.function_call.name().to_owned()
                } else {
                    String::new()
                },
                args: vec![],
                kwargs: vec![],
                call_id: call.call_id,
                not_handled_error: has_payload.then(|| (&call.function_call.on_no_handler()).into()),
            })
        }
        ReplProgress::NameLookup(lookup) => pb::event::Kind::NameLookup(pb::NameLookup {
            name: lookup.name.clone(),
        }),
        ReplProgress::ResolveFutures(state) => pb::event::Kind::ResolveFutures(pb::ResolveFutures {
            pending_call_ids: state.pending_call_ids().to_vec(),
        }),
        ReplProgress::Complete { .. } => unreachable!("Complete is handled before suspension_event"),
    };
    event(kind)
}

/// Appends a `u32 LE`-length-prefixed string field to a dump envelope.
fn push_str_field(buf: &mut Vec<u8>, s: &str) {
    // dump fields originate from ≤256 MiB protocol frames, so the length
    // always fits in u32
    let len = u32::try_from(s.len()).expect("dump field exceeds u32::MAX bytes");
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// Splits the session metadata (script name + type-check state, see
/// [`DUMP_VERSION`]) off the front of a dump envelope, returning it together
/// with the remaining postcard payload. `None` means the envelope is
/// malformed.
fn take_session_meta(bytes: &[u8]) -> Option<(String, Option<TypeCheckState>, &[u8])> {
    let (script_name, rest) = take_str_field(bytes)?;
    let (&type_check_flag, rest) = rest.split_first()?;
    match type_check_flag {
        0 => Some((script_name, None, rest)),
        1 => {
            let (committed_stubs, rest) = take_str_field(rest)?;
            let (&pending_flag, rest) = rest.split_first()?;
            let (pending_snippet, rest) = match pending_flag {
                0 => (None, rest),
                1 => take_str_field(rest).map(|(snippet, rest)| (Some(snippet), rest))?,
                _ => return None,
            };
            Some((
                script_name,
                Some(TypeCheckState {
                    committed_stubs,
                    pending_snippet,
                }),
                rest,
            ))
        }
        _ => None,
    }
}

/// Splits a `u32 LE`-length-prefixed string field off the front of a dump
/// envelope.
fn take_str_field(bytes: &[u8]) -> Option<(String, &[u8])> {
    let (len_bytes, rest) = bytes.split_at_checked(4)?;
    let len = u32::from_le_bytes(len_bytes.try_into().ok()?) as usize;
    let (field, rest) = rest.split_at_checked(len)?;
    Some((String::from_utf8(field.to_vec()).ok()?, rest))
}

/// Converts wire named inputs into `(name, value)` pairs for `feed_start`.
fn named_inputs(inputs: Vec<pb::NamedValue>) -> Result<Vec<(String, MontyObject)>, Box<pb::Event>> {
    inputs
        .into_iter()
        .map(|input| {
            let value = input
                .value
                .ok_or_else(|| Box::new(violation(&format!("input {:?} has no value", input.name))))?;
            let value = MontyObject::try_from(value)
                .map_err(|err| Box::new(violation(&format!("invalid input {:?}: {err}", input.name))))?;
            Ok((input.name, value))
        })
        .collect()
}

/// Streams sandbox `print()` output as `Print` events.
///
/// Line-buffered: a frame is written when the buffer ends with a newline or
/// exceeds [`Self::FLUSH_BYTES`], and [`Self::drain`] flushes any partial
/// line before the turn-ending event so ordering is exact.
struct ProtoPrint {
    writer: SharedWriter,
    buf: String,
}

impl ProtoPrint {
    /// Flush threshold for output that never produces a newline.
    const FLUSH_BYTES: usize = 8 * 1024;

    fn new(writer: SharedWriter) -> Self {
        Self {
            writer,
            buf: String::new(),
        }
    }

    /// Writes the buffer (if any) as one `Print` event.
    fn flush(&mut self) -> Result<(), MontyException> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let event = event(pb::event::Kind::Print(pb::Print {
            stream: pb::PrintStream::Stdout.into(),
            text: mem::take(&mut self.buf),
        }));
        self.writer.borrow_mut().write(&event).map_err(|err| {
            MontyException::new(
                ExcType::RuntimeError,
                Some(format!("failed to stream print output: {err}")),
            )
        })
    }

    fn maybe_flush(&mut self) -> Result<(), MontyException> {
        if self.buf.ends_with('\n') || self.buf.len() >= Self::FLUSH_BYTES {
            self.flush()
        } else {
            Ok(())
        }
    }

    /// Flushes any trailing partial line; called before every turn-ending
    /// event. Errors are ignored — if stdout is broken the turn-ending write
    /// fails anyway.
    fn drain(&mut self) {
        let _ = self.flush();
    }
}

impl PrintWriterCallback for ProtoPrint {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.buf.push_str(&output);
        self.maybe_flush()
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.buf.push(end);
        self.maybe_flush()
    }
}

/// Installs a panic hook that emits a best-effort `FatalError` frame before
/// the default unwind, giving the parent a parseable last gasp for ordinary
/// panics. Hard crashes (stack overflow, allocator abort) bypass this — the
/// parent's contract is "exit without FatalError == crash".
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // a fresh handle: stdout's lock is reentrant on the same thread, and
        // the shared BufWriter may hold a partial frame we cannot complete —
        // a corrupt tail is fine, the parent already treats it as a crash
        let mut writer = FrameWriter::new(io::stdout());
        let _ = writer.write(&event(pb::event::Kind::FatalError(pb::FatalError {
            message: format!("child panicked: {info}"),
        })));
        default_hook(info);
    }));
}
