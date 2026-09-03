//! The transport-agnostic Monty protocol-child state machine.
//!
//! [`Child`] is the REPL session worker that both `monty subprocess` (native,
//! over stdio pipes) and the browser wasm worker (over `postMessage`) drive. It
//! consumes [`pb::ParentRequest`]s and emits [`pb::ChildEvent`]s through an
//! [`EventSink`], so the same turn logic serves any byte channel — the only
//! difference between transports is the sink and how requests are read.
//!
//! The child is strictly turn-based: one request in, zero or more streamed
//! `Print` events out, then exactly one turn-ending event (see `monty-proto`
//! for the schema and protocol rules).
//!
//! Crash isolation is the entire point: a host must treat a child that exits
//! (or EOFs) *without* a `FatalError` event as crashed — stack overflows and
//! allocator aborts produce no final frame. This crate has no opinion on how
//! the host transport surfaces that; it only ensures every *graceful* turn ends
//! with exactly one turn-ending event.

use std::{borrow::Cow, mem};

use monty::{Dump, MontyRepl, ReplProgress, ReplStartError, Session, SessionRef, dump};
use monty_type_checking::{SourceFile, TypeChecker};
use monty_types::{
    AssertMessageAnnotations, CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, OsFunctionCall,
    PrintWriter, PrintWriterCallback, ResourceLimits, ResourceTracker, TypeCheckState, TypeCheckingConfig,
};

use super::{
    FrameError, FrameReader, MAX_FRAME_LEN, ProtoConvertError, WireFunctionCall, check_protocol_version,
    exceeds_max_frame_len, exceeds_max_value_depth, future_results_from_proto, pb, write_frame,
};
use crate::wire::uuid_to_pb;

/// A sink for framed [`pb::ChildEvent`]s, decoupling the child from its
/// transport.
///
/// The native subprocess implements this over stdout; the wasm worker buffers
/// frames for the host to read (see [`VecEventSink`]). `send` frames the event
/// (4-byte LE length prefix + protobuf) exactly as `monty-proto`'s
/// [`write_frame`] does.
///
/// `Err` is a transport failure the caller treats as terminal: a broken pipe
/// (the parent is gone) for stdout, or — for an in-memory buffer that cannot
/// fail on I/O — only an oversize frame, which [`write_frame`] rejects *before*
/// buffering any bytes, so the stream stays in sync and the child can recover.
pub trait EventSink {
    /// Frames and emits one event.
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), FrameError>;
}

/// An [`EventSink`] that appends framed events to an in-memory buffer.
///
/// Used by the wasm worker, which collects a turn's frames and hands the whole
/// buffer back to the host in one `postMessage`, and by tests that drive
/// [`Child`] in-process. [`Self::take`] yields the accumulated frames and
/// resets the buffer for the next turn.
#[derive(Default)]
pub struct VecEventSink {
    frames: Vec<u8>,
}

impl VecEventSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the frames buffered since the last call and clears the buffer.
    pub fn take(&mut self) -> Vec<u8> {
        mem::take(&mut self.frames)
    }
}

impl EventSink for VecEventSink {
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), FrameError> {
        // `Vec<u8>: io::Write` never fails on I/O, so the only error this can
        // surface is `FrameTooLarge`, which `write_frame` raises before
        // appending anything.
        write_frame(&mut self.frames, event)
    }
}

/// What the host loop should do after [`Child::handle`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleOutcome {
    /// Keep serving the next request.
    Continue,
    /// The child received `Shutdown` and should exit cleanly.
    Shutdown,
    /// The child emitted a `FatalError` (e.g. parent/child version skew) and
    /// must terminate. Distinct from `Shutdown` so a native host can exit with
    /// a non-zero status; a message-based host treats it like `Shutdown`.
    Fatal,
}

/// Runs one buffered turn: reads exactly one framed `ParentRequest` from
/// `request_frame`, handles it on `child`, and returns the concatenated framed
/// events (zero or more `Print`s then one turn-ending event) plus what the host
/// should do next.
///
/// This is the per-turn entry point for a message-based transport such as a
/// wasm Web Worker, where each `postMessage` carries one request frame and the
/// reply carries that turn's frames. It mirrors the native shell's stdio loop
/// body — malformed frames become a `protocol_violation` (recoverable) or a
/// `FatalError` (desync), and an unrecoverable oversize event becomes a
/// `FatalError` — but writes to an in-memory buffer instead of a pipe.
///
/// Unlike a streaming transport, `Print` events are buffered for the whole turn
/// and returned together rather than delivered incrementally.
pub fn dispatch_frame(child: &mut Child, request_frame: &[u8]) -> (Vec<u8>, HandleOutcome) {
    let mut sink = VecEventSink::new();
    let outcome = dispatch_into(child, request_frame, &mut sink);
    (sink.take(), outcome)
}

/// Decodes and handles one request frame, sending all resulting frames to
/// `sink`. Factored out of [`dispatch_frame`] so the framing/recovery decisions
/// stay separate from buffer ownership.
fn dispatch_into(child: &mut Child, request_frame: &[u8], sink: &mut VecEventSink) -> HandleOutcome {
    let mut reader = FrameReader::new(request_frame);
    match reader.read::<pb::ParentRequest>() {
        Ok(Some(request)) => match child.handle(request, sink) {
            Ok(outcome) => outcome,
            // an oversize turn-ending event was rejected before any bytes were
            // buffered, so the reply is still parseable — but an oversize
            // suspension (or any unrecoverable error) leaves no resume point,
            // so emit a fatal last gasp and stop the worker
            Err(FrameError::FrameTooLarge { len, max }) => {
                let _ = sink
                    .send(&child.fatal_event(&format!("response frame of {len} bytes exceeds maximum of {max} bytes")));
                HandleOutcome::Shutdown
            }
            // `VecEventSink` cannot fail on I/O, so this is unreachable in
            // practice; treat any other transport error as terminal anyway
            Err(_) => HandleOutcome::Shutdown,
        },
        // an empty buffer carries no request — nothing to do
        Ok(None) => HandleOutcome::Continue,
        // the frame decoded structurally but its payload was invalid (bad
        // dates, unknown enum names); the buffer is in sync, so answer with a
        // recoverable violation and keep serving
        Err(FrameError::Decode(err)) => {
            let _ = sink.send(&protocol_violation(&format!("malformed request: {err}")));
            HandleOutcome::Continue
        }
        // framing itself is broken — unrecoverable by design
        Err(err) => {
            let _ = sink.send(&child.fatal_event(&format!("malformed request frame: {err}")));
            HandleOutcome::Shutdown
        }
    }
}

/// The current sandbox budget visible to an external host.
///
/// Hosts use the memory fields to arm their allocator and `max_suspensions` to
/// restore their accounting.
#[derive(Debug, Default, Clone, Copy)]
pub struct SessionBudget {
    /// `max_memory` in bytes; `None` when unlimited, or when no session exists.
    pub max_memory: Option<usize>,
    /// Whether the session type checks each fed snippet.
    pub type_check: bool,
    /// Maximum suspensions the host may service; enforced outside the child.
    /// `None` only when no session exists.
    pub max_suspensions: Option<usize>,
}

/// REPL session state of the child.
enum SessionState {
    /// No repl materialized yet. `Some` once `Configure` has stored the config
    /// (the repl is built lazily on the first `Feed` / `Dump`); `None` on a
    /// freshly spawned or just-`Reset` worker, before `Configure`. `Load` is
    /// valid only from here — it cannot clobber a started session.
    Configured(Option<Box<pb::Configure>>),
    /// Session ready for the next `Feed`.
    Ready(Box<MontyRepl>),
    /// Mid-feed, waiting for a resume request. Never holds
    /// `ReplProgress::Complete` — completion ends the turn immediately.
    Suspended(Box<ReplProgress>),
}

/// All state of one protocol child: the current REPL session plus the
/// per-session metadata (script name, type-check context) that lives outside
/// the repl.
///
/// The child performs no filesystem I/O: mounts are host configuration the
/// parent handles entirely by servicing filesystem `OsCall` events itself, so
/// no mount state (or host path) ever reaches the child.
///
/// Drive it by reading framed [`pb::ParentRequest`]s from the host transport
/// and passing each to [`Self::handle`] along with an [`EventSink`]; the child
/// streams `Print` events and one turn-ending event per request.
pub struct Child {
    state: SessionState,
    /// Script name of the current session (used for error and type-check
    /// diagnostics).
    script_name: String,
    type_checker: TypeChecker,
    /// `Some` when the session was created with `type_check: true`.
    type_check: Option<TypeCheckState>,
}

impl Default for Child {
    fn default() -> Self {
        Self {
            state: SessionState::Configured(None),
            script_name: String::new(),
            type_checker: TypeChecker::default(),
            type_check: None,
        }
    }
}

impl Child {
    /// Handles one request: streams any `Print` events and emits exactly one
    /// turn-ending event through `sink`, then reports what the host loop should
    /// do next. `Err` means the sink is broken (for stdout, the parent is
    /// gone).
    pub fn handle(
        &mut self,
        request: pb::ParentRequest,
        sink: &mut dyn EventSink,
    ) -> Result<HandleOutcome, FrameError> {
        let Some(kind) = request.kind else {
            sink.send(&protocol_violation("request has no kind"))?;
            return Ok(HandleOutcome::Continue);
        };

        let mut event = match kind {
            pb::parent_request::Kind::Configure(configure) => {
                // An unsupported protocol version is fatal: the parent may frame
                // or interpret later messages differently, so serving it risks a
                // silent desync. Emit the fatal last gasp and stop the child.
                if let Err(refusal) = check_protocol_version(configure.protocol_version) {
                    sink.send(&self.fatal_event(&refusal))?;
                    return Ok(HandleOutcome::Fatal);
                }
                self.handle_configure(configure)
            }
            pb::parent_request::Kind::Feed(feed) => self.handle_repl_feed(feed, sink),
            // The Monty sandbox has no host interpreter to install packages for;
            // dependency installation is only supported by the CPython worker.
            // Answer with a session-preserving error rather than a hard failure.
            pb::parent_request::Kind::InstallDependencies(_) => error_event(
                ExcType::RuntimeError,
                "dependency installation is only supported by the CPython worker",
            ),
            pb::parent_request::Kind::ResumeCall(resume) => self.handle_resume_call(resume, sink),
            pb::parent_request::Kind::ResumeNameLookup(resume) => self.handle_resume_name_lookup(resume, sink),
            pb::parent_request::Kind::ResumeFutures(resume) => self.handle_resume_futures(resume, sink),
            pb::parent_request::Kind::AbortFeed(abort) => self.handle_abort_feed(abort, sink),
            pb::parent_request::Kind::Dump(_) => self.handle_dump(),
            pb::parent_request::Kind::Load(load) => self.handle_load(&load),
            pb::parent_request::Kind::Reset(_) => match self.reset() {
                Ok(()) => ok_event(),
                // A failed scrub leaves the finished session's files in the
                // type checker, so this worker must never serve another one:
                // the next session could resolve the previous session's
                // modules. Die with an explanation the parent can log rather
                // than carry on — or panic, which it would only see as a crash.
                Err(err) => {
                    sink.send(&self.fatal_event(&format!("type-check cleanup failed: {err}")))?;
                    return Ok(HandleOutcome::Fatal);
                }
            },
            pb::parent_request::Kind::Shutdown(_) => {
                sink.send(&ok_event())?;
                return Ok(HandleOutcome::Shutdown);
            }
        };
        self.stamp_session_budget(&mut event);
        let sent = sink.send(&event);
        // a suspension announcement was *lent* the payload it announces, so
        // take it back before anything can observe the stored suspension
        // without it — on the failed-send path too, since the session may yet
        // be dumped or answered
        if let Err(err) = self.reclaim_suspension_payload(&mut event) {
            sink.send(&self.fatal_event(&format!("suspension payload could not be restored: {err}")))?;
            return Ok(HandleOutcome::Fatal);
        }
        if let Err(err) = sent {
            self.recover_send_error(&event, err, sink)?;
        }
        Ok(HandleOutcome::Continue)
    }

    /// Moves a suspension announcement's payload back into the suspension it
    /// announces, undoing the loan taken by [`suspension_event_function_call`]
    /// / [`suspension_event_os_call`]. A no-op for every other event.
    ///
    /// The gap this closes is short by construction: the payload is lent as the
    /// event is built and reclaimed as soon as [`Self::handle`] has sent it,
    /// with only timing stamps and print draining in between.
    ///
    /// `Err` means the wire arms and [`OsFunctionCall`] have drifted apart —
    /// the conversion back is total for a payload this child just produced, so
    /// a failure would leave a suspension the parent's answer can no longer be
    /// applied to. The caller makes that fatal rather than serving on.
    fn reclaim_suspension_payload(&mut self, event: &mut pb::ChildEvent) -> Result<(), ProtoConvertError> {
        let SessionState::Suspended(progress) = &mut self.state else {
            return Ok(());
        };
        match (progress.as_mut(), &mut event.kind) {
            (ReplProgress::FunctionCall(call), Some(pb::child_event::Kind::FunctionCall(announced))) => {
                call.args = mem::take(&mut announced.args);
                call.kwargs = mem::take(&mut announced.kwargs);
            }
            (ReplProgress::OsCall(call), Some(pb::child_event::Kind::OsCall(announced))) => {
                if let Some(announced) = announced.call.take() {
                    call.function_call = announced.try_into()?;
                }
            }
            // any other pairing is an event that borrowed nothing
            _ => {}
        }
        Ok(())
    }

    /// What the session the child is *currently* holding would run under.
    ///
    /// A host that bounds the worker process from outside the interpreter (the
    /// subprocess shell caps its own allocator) sizes that bound from this,
    /// after every request: the budget changes when a session is configured,
    /// restored from a dump — which brings its own limits, not the
    /// `Configure`'s — or ended by `Reset`.
    #[must_use]
    pub fn session_budget(&self) -> SessionBudget {
        match &self.state {
            SessionState::Configured(Some(config)) => SessionBudget {
                max_memory: config
                    .limits
                    .as_ref()
                    .and_then(|limits| limits.max_memory_bytes)
                    .map(|v| usize::try_from(v).unwrap_or(usize::MAX)),
                type_check: config.type_check,
                // the wire default applies before the repl exists too
                max_suspensions: Some(ResourceLimits::from(config.limits.unwrap_or_default()).max_suspensions),
            },
            SessionState::Configured(None) => SessionBudget::default(),
            SessionState::Ready(repl) => self.tracker_budget(repl.tracker()),
            SessionState::Suspended(progress) => self.tracker_budget(progress.tracker()),
        }
    }

    /// The budget of a materialized session, whose limits live in its tracker.
    fn tracker_budget(&self, tracker: &ResourceTracker) -> SessionBudget {
        SessionBudget {
            max_memory: tracker.max_memory(),
            type_check: self.type_check.is_some(),
            max_suspensions: Some(tracker.max_suspensions()),
        }
    }

    /// Builds a timing-stamped `FatalError` event for an unrecoverable
    /// condition the host detected (frame desync, oversize request). The host
    /// sends it and exits right after; it is the child's parseable last gasp.
    #[must_use]
    pub fn fatal_event(&self, message: &str) -> pb::ChildEvent {
        let mut event = fatal_error_event(message);
        // fatal paths bypass `handle`, so stamp timing here to keep the
        // "every turn-ending event carries timing" contract intact
        self.stamp_session_budget(&mut event);
        event
    }

    /// Recovers from a failure to write a turn-ending event.
    ///
    /// [`write_frame`] rejects an oversize frame *before* writing any bytes, so
    /// the stream stays synced and an oversize event (a large `Complete`, or a
    /// `DumpResult` while suspended) can be answered with a clean,
    /// session-preserving error. An oversize *suspension announcement* is
    /// unrecoverable — the worker is suspended but the parent never learned the
    /// resume point — so it propagates to the host loop's fatal handling, as
    /// does any genuine I/O break.
    fn recover_send_error(
        &mut self,
        failed: &pb::ChildEvent,
        err: FrameError,
        sink: &mut dyn EventSink,
    ) -> Result<(), FrameError> {
        let announces_suspension = matches!(
            failed.kind,
            Some(
                pb::child_event::Kind::FunctionCall(_)
                    | pb::child_event::Kind::OsCall(_)
                    | pb::child_event::Kind::NameLookup(_)
                    | pb::child_event::Kind::ResolveFutures(_)
            )
        );
        match err {
            FrameError::FrameTooLarge { len, max } if !announces_suspension => {
                let mut event = error_event(
                    ExcType::RuntimeError,
                    &format!("result frame of {len} bytes exceeds the maximum of {max} bytes"),
                );
                self.stamp_session_budget(&mut event);
                sink.send(&event)
            }
            other => Err(other),
        }
    }

    /// Stamps session timing and parent-enforced limits onto an event.
    ///
    /// Reported timing drives the parent's backstop. Fields are absent without
    /// a session.
    fn stamp_session_budget(&self, event: &mut pb::ChildEvent) {
        let tracker = match &self.state {
            SessionState::Ready(repl) => repl.tracker(),
            SessionState::Suspended(progress) => progress.tracker(),
            // no repl materialized yet → no tracker to report
            SessionState::Configured(_) => return,
        };
        stamp_budget(event, tracker);
    }

    /// Stores the session config; the repl is built lazily by [`ensure_repl`]
    /// on the first feed/dump (or restored by `Load` instead). Valid only on a
    /// not-yet-configured worker.
    fn handle_configure(&mut self, configure: pb::Configure) -> pb::ChildEvent {
        if matches!(self.state, SessionState::Configured(None)) {
            self.state = SessionState::Configured(Some(Box::new(configure)));
            ok_event()
        } else {
            protocol_violation("Configure while a session already exists")
        }
    }

    /// Materializes the repl from the stored config the first time the session
    /// runs (feed/dump), applying the config's script name, limits, and
    /// type-check setup. A no-op once the repl exists; errors only if the
    /// worker was never configured (which the pool's `Configure`-first checkout
    /// prevents in normal operation).
    fn ensure_repl(&mut self) -> Result<(), Box<pb::ChildEvent>> {
        let config = match &mut self.state {
            SessionState::Configured(config) => config.take(),
            // already materialized (or mid-feed) — nothing to do here
            SessionState::Ready(_) | SessionState::Suspended(_) => return Ok(()),
        };
        let Some(config) = config else {
            return Err(Box::new(protocol_violation("session has not been configured")));
        };
        let type_check_config = TypeCheckingConfig::from(config.as_ref());
        // Destructured exhaustively on purpose: a new `Configure` field must
        // fail to compile here until the child decides what to do with it.
        let pb::Configure {
            script_name,
            limits,
            type_check,
            type_check_stubs,
            assert_message_annotations,
            // read above, through the accessor that validates the enum number
            type_check_format: _,
            type_check_color: _,
            // range-checked when `Configure` arrived
            protocol_version: _,
            // informational only — never checked
            monty_version: _,
        } = *config;
        let limits = limits.unwrap_or_default().into();
        self.script_name = script_name;
        self.type_check = type_check.then(|| TypeCheckState {
            committed_stubs: type_check_stubs.unwrap_or_default(),
            pending_snippet: None,
            config: type_check_config,
        });
        // Missing field means an older parent; the feature defaults to on.
        let options = CompileOptions {
            assert_message_annotations: assert_message_annotations.map_or_else(
                AssertMessageAnnotations::default,
                AssertMessageAnnotations::from_max_bytes,
            ),
        };
        self.state = SessionState::Ready(Box::new(MontyRepl::new(
            &self.script_name,
            ResourceTracker::new(limits),
            options,
        )));
        Ok(())
    }

    /// Runs a `Feed` on the ready session: type-checks the snippet (unless
    /// skipped), injects inputs, and drives execution to the turn-ending event.
    fn handle_repl_feed(&mut self, feed: pb::Feed, sink: &mut dyn EventSink) -> pb::ChildEvent {
        if let Err(event) = self.ensure_repl() {
            return *event;
        }
        if !matches!(self.state, SessionState::Ready(_)) {
            // ensure_repl left it un-Ready only when mid-suspension
            return protocol_violation("Feed without a session ready for input");
        }
        if !feed.skip_type_check
            && let Some(event) = self.type_check_feed(&feed.code)
        {
            return event;
        }
        let inputs = match named_inputs(feed.inputs) {
            Ok(inputs) => inputs,
            Err(event) => return *event,
        };
        let SessionState::Ready(repl) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked Ready above");
        };
        // snippets fed with skip_type_check never become type-check context:
        // the caller explicitly excluded them from checking, so later snippets
        // must not be checked against their (unchecked) bindings either
        if !feed.skip_type_check
            && let Some(state) = &mut self.type_check
        {
            state.pending_snippet = Some(feed.code.clone());
        }
        let mut print = ProtoPrint::new(sink);
        let result = repl.feed_start(&feed.code, inputs, PrintWriter::Callback(&mut print));
        let event = self.drive(result, &mut print);
        print.drain();
        event
    }

    /// Answers a suspended external function or OS call with the parent's
    /// result, checking the `call_id` matches, then resumes execution.
    fn handle_resume_call(&mut self, resume: pb::ResumeCall, sink: &mut dyn EventSink) -> pb::ChildEvent {
        let expected_call_id = match &self.state {
            SessionState::Suspended(progress) => match progress.as_ref() {
                ReplProgress::FunctionCall(call) => Some(call.call_id),
                ReplProgress::OsCall(call) => Some(call.call_id),
                _ => None,
            },
            _ => None,
        };
        let Some(call_id) = expected_call_id else {
            return protocol_violation("ResumeCall without a suspended function/OS call");
        };
        if resume.call_id != call_id {
            return protocol_violation(&format!(
                "ResumeCall call_id {} does not match {call_id}",
                resume.call_id
            ));
        }
        let Some(wire_result) = resume.result else {
            return protocol_violation("ResumeCall has no result");
        };
        // NotHandled resolves against the suspended call itself — the child
        // owns the no-handler semantics (`OsFunctionCall::on_no_handler`), so
        // the parent never has to compute or echo the default exception.
        let result: ExtFunctionResult =
            if matches!(wire_result.kind, Some(pb::ext_function_result::Kind::NotHandled(_))) {
                let SessionState::Suspended(progress) = &self.state else {
                    unreachable!("checked above");
                };
                let ReplProgress::OsCall(call) = progress.as_ref() else {
                    return protocol_violation("NotHandled is only valid answering a suspended OS call");
                };
                ExtFunctionResult::Error(call.function_call.on_no_handler())
            } else {
                match wire_result.try_into() {
                    Ok(result) => result,
                    Err(err) => return protocol_violation(&format!("invalid result: {err}")),
                }
            };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(sink);
        let outcome = match *progress {
            ReplProgress::FunctionCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            ReplProgress::OsCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            _ => unreachable!("checked above"),
        };
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    /// Answers a suspended name lookup with the value (or absence) the parent
    /// resolved, then resumes execution.
    fn handle_resume_name_lookup(&mut self, resume: pb::ResumeNameLookup, sink: &mut dyn EventSink) -> pb::ChildEvent {
        let SessionState::Suspended(progress) = &self.state else {
            return protocol_violation("ResumeNameLookup without a suspended name lookup");
        };
        if !matches!(progress.as_ref(), ReplProgress::NameLookup(_)) {
            return protocol_violation("ResumeNameLookup without a suspended name lookup");
        }
        let result = match resume.try_into() {
            Ok(result) => result,
            Err(err) => return protocol_violation(&format!("invalid result: {err}")),
        };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let ReplProgress::NameLookup(lookup) = *progress else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(sink);
        let outcome = lookup.resume(result, PrintWriter::Callback(&mut print));
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    /// Raises the parent's exception uncatchably at any pending suspension.
    /// The `Error` reply returns the session to `Ready`.
    fn handle_abort_feed(&mut self, abort: pb::AbortFeed, sink: &mut dyn EventSink) -> pb::ChildEvent {
        // Guard against a corrupt `Complete` state instead of crashing.
        let suspended = matches!(&self.state, SessionState::Suspended(progress)
            if !matches!(progress.as_ref(), ReplProgress::Complete { .. }));
        if !suspended {
            return protocol_violation("AbortFeed without a suspended feed");
        }
        let Some(exception) = abort.exception else {
            return protocol_violation("AbortFeed has no exception");
        };
        let exc = match MontyException::try_from(exception) {
            Ok(exc) => exc,
            Err(err) => return protocol_violation(&format!("invalid exception: {err}")),
        };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(sink);
        let outcome = match *progress {
            ReplProgress::FunctionCall(call) => call.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::OsCall(call) => call.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::NameLookup(lookup) => lookup.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::ResolveFutures(state) => state.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::Complete { .. } => unreachable!("checked above"),
        };
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    /// Delivers the parent's resolved future results to a suspended
    /// `ResolveFutures` state, then resumes execution.
    fn handle_resume_futures(&mut self, resume: pb::ResumeFutures, sink: &mut dyn EventSink) -> pb::ChildEvent {
        let SessionState::Suspended(progress) = &self.state else {
            return protocol_violation("ResumeFutures without suspended futures");
        };
        if !matches!(progress.as_ref(), ReplProgress::ResolveFutures(_)) {
            return protocol_violation("ResumeFutures without suspended futures");
        }
        let results = match future_results_from_proto(resume.results) {
            Ok(results) => results,
            Err(err) => return protocol_violation(&format!("invalid results: {err}")),
        };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let ReplProgress::ResolveFutures(state) = *progress else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(sink);
        let outcome = state.resume(results, PrintWriter::Callback(&mut print));
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    /// Serializes the current session, and the metadata that lives outside it,
    /// into monty's dump format. The session stays live — dumping is read-only.
    fn handle_dump(&mut self) -> pb::ChildEvent {
        // a never-fed session is materialized into an empty repl so it can be
        // dumped; a never-configured worker has nothing to dump
        if let Err(event) = self.ensure_repl() {
            return *event;
        }
        let session = match &self.state {
            SessionState::Ready(repl) => SessionRef::Idle(repl),
            SessionState::Suspended(progress) => SessionRef::Suspended(progress),
            SessionState::Configured(_) => unreachable!("ensure_repl materialized the repl or errored"),
        };
        match dump(&self.script_name, self.type_check.as_ref(), session) {
            Ok(state) => event(pb::child_event::Kind::DumpResult(pb::DumpResult { state })),
            Err(err) => protocol_violation(&format!("dump failed: {err}")),
        }
    }

    /// Restores a dump produced by [`Self::handle_dump`] into this child. A
    /// restored suspension re-emits its suspension event so the parent learns
    /// the resume point.
    ///
    /// `Load` is valid only when no repl has been materialized yet — a freshly
    /// checked-out (`Configure`d, unfed) worker — so it initializes the session
    /// instead of feeding. Once a feed has run (or a prior `Load` restored a
    /// session), the repl exists and `Load` is rejected rather than silently
    /// discarding it.
    fn handle_load(&mut self, load: &pb::Load) -> pb::ChildEvent {
        if !matches!(self.state, SessionState::Configured(_)) {
            return protocol_violation("Load requires a session that has not started (a feed has already run)");
        }
        let restored = match Dump::load(&load.state) {
            Ok(restored) => restored,
            Err(err) => return protocol_violation(&format!("failed to load session: {err}")),
        };
        let Dump {
            script_name,
            type_check,
            state,
        } = restored;
        // the depth/oversize checks below can only fail on a forged or corrupted
        // dump — `drive` enforces them on every fresh suspension before it is
        // stored
        let mut event = match state {
            Session::Idle(repl) => {
                self.state = SessionState::Ready(repl);
                ok_event()
            }
            // the protocol only ever serves repl sessions; a `MontyRun`
            // execution has no way to accept further feeds
            Session::Running(_) => protocol_violation("dump holds a one-shot run, not a repl session"),
            Session::Suspended(progress) => match *progress {
                // a dump is never taken at Complete, but a forged one could
                // contain it; surface the value rather than fail
                ReplProgress::Complete { repl, value } => {
                    if exceeds_max_value_depth(&value) {
                        protocol_violation("dump value exceeds the maximum wire depth")
                    } else {
                        self.state = SessionState::Ready(Box::new(repl));
                        complete_event(value)
                    }
                }
                mut progress => {
                    if suspension_args_too_deep(&progress) {
                        protocol_violation("dump suspension arguments exceed the maximum wire depth")
                    } else {
                        let mut event = suspension_event(&mut progress);
                        // size-checked with the stamps `handle` sends it with
                        stamp_budget(&mut event, progress.tracker());
                        if let Some(message) = oversize_suspension_error_message(&event) {
                            protocol_violation(&message)
                        } else {
                            self.state = SessionState::Suspended(Box::new(progress));
                            event
                        }
                    }
                }
            },
        };
        // adopt the restored metadata only once the payload actually loaded
        // (state is now Ready/Suspended) — a failed load leaves the child in
        // its prior un-started state, re-loadable. Surface the adopted script
        // name so the parent can report it without parsing the opaque dump.
        if matches!(self.state, SessionState::Ready(_) | SessionState::Suspended(_)) {
            self.script_name = script_name;
            self.type_check = type_check;
            event.restored_script_name = Some(self.script_name.clone());
        }
        event
    }

    /// Drives execution until it needs the parent, returning the turn-ending
    /// event. Every OS call surfaces to the parent — the child performs no
    /// filesystem I/O (mounts are serviced parent-side).
    fn drive(
        &mut self,
        mut result: Result<ReplProgress, Box<ReplStartError>>,
        print: &mut ProtoPrint,
    ) -> pb::ChildEvent {
        loop {
            match result {
                Ok(ReplProgress::Complete { repl, value }) => {
                    self.state = SessionState::Ready(Box::new(repl));
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
                    return complete_event(value);
                }
                Ok(ReplProgress::OsCall(mut call)) => {
                    if os_call_args_too_deep(&call) {
                        let err =
                            MontyException::new(ExcType::RuntimeError, Some("Max argument depth exceeded".to_owned()));
                        result = call.resume(ExtFunctionResult::Error(err), PrintWriter::Callback(print));
                        continue;
                    }
                    let mut event = suspension_event_os_call(&mut call);
                    let progress = ReplProgress::OsCall(call);
                    // stamped before the size check, so the frame measured is
                    // the frame `handle` sends
                    stamp_budget(&mut event, progress.tracker());
                    if let Some(message) = oversize_suspension_error_message(&event) {
                        return self.abort_feed_with_runtime_error(progress.into_repl(), &message);
                    }
                    self.state = SessionState::Suspended(Box::new(progress));
                    return event;
                }
                Ok(ReplProgress::FunctionCall(mut call)) => {
                    // arguments too deep for the wire resume the call with a
                    // catchable error instead of corrupting the protocol
                    if function_call_args_too_deep(&call) {
                        let err =
                            MontyException::new(ExcType::RuntimeError, Some("Max argument depth exceeded".to_owned()));
                        result = call.resume(ExtFunctionResult::Error(err), PrintWriter::Callback(print));
                        continue;
                    }
                    let mut event = suspension_event_function_call(&mut call);
                    let progress = ReplProgress::FunctionCall(call);
                    stamp_budget(&mut event, progress.tracker());
                    if let Some(message) = oversize_suspension_error_message(&event) {
                        return self.abort_feed_with_runtime_error(progress.into_repl(), &message);
                    }
                    self.state = SessionState::Suspended(Box::new(progress));
                    return event;
                }
                Ok(mut progress) => {
                    let event = suspension_event(&mut progress);
                    self.state = SessionState::Suspended(Box::new(progress));
                    return event;
                }
                Err(err) => {
                    // Python-level failure: the session always survives
                    self.state = SessionState::Ready(Box::new(err.repl));
                    if let Some(state) = &mut self.type_check {
                        state.pending_snippet = None;
                    }
                    return event(pb::child_event::Kind::Error(pb::Error {
                        exception: Some((&err.error).into()),
                    }));
                }
            }
        }
    }

    /// Ends the current feed with a runtime error while keeping the REPL usable.
    fn abort_feed_with_runtime_error(&mut self, repl: MontyRepl, message: &str) -> pb::ChildEvent {
        self.state = SessionState::Ready(Box::new(repl));
        if let Some(state) = &mut self.type_check {
            state.pending_snippet = None;
        }
        error_event(ExcType::RuntimeError, message)
    }

    /// Type-checks a snippet against the accumulated session stubs. Returns
    /// the turn-ending event if the check fails (or errors), `None` to
    /// proceed with execution.
    fn type_check_feed(&mut self, code: &str) -> Option<pb::ChildEvent> {
        let state = self.type_check.as_ref()?;
        let stubs =
            (!state.committed_stubs.is_empty()).then(|| SourceFile::new(&state.committed_stubs, "repl_type_stubs.pyi"));
        match self
            .type_checker
            .run(&SourceFile::new(code, &self.script_name), stubs.as_ref(), state.config)
        {
            Ok(None) => None,
            Ok(Some(diagnostics)) => Some(event(pb::child_event::Kind::TypingError(pb::TypingError {
                diagnostics: diagnostics.to_string(),
            }))),
            Err(err) => Some(protocol_violation(&format!("type checker failed: {err}"))),
        }
    }

    /// Drops all session state, returning to the unconfigured state ready for
    /// the next `Configure` (or `Load`).
    ///
    /// `Err` means the type checker still holds this session's files, which is
    /// terminal for the worker — see the `Reset` arm in [`Self::handle`].
    fn reset(&mut self) -> Result<(), String> {
        self.state = SessionState::Configured(None);
        self.type_check = None;
        self.script_name = String::new();
        self.type_checker.reset()
    }
}

/// Wraps an event kind into a `ChildEvent` with zeroed timing fields;
/// [`Child::handle`] (and [`Child::fatal_event`]) stamps the timing fields onto
/// every turn-ending event just before it is sent.
fn event(kind: pb::child_event::Kind) -> pb::ChildEvent {
    pb::ChildEvent {
        kind: Some(kind),
        ..Default::default()
    }
}

/// Builds the turn-ending event for a recoverable protocol violation (wrong
/// state, bad call id, invalid payload). The child's state is unchanged.
///
/// Public so a host transport can answer a frame that decoded but is not a
/// valid request (e.g. a malformed parent message) without reaching into the
/// event-kind types.
#[must_use]
pub fn protocol_violation(message: &str) -> pb::ChildEvent {
    event(pb::child_event::Kind::Error(pb::Error {
        exception: Some(pb::RaisedException {
            exc_type: ExcType::RuntimeError.to_string(),
            message: Some(format!("protocol violation: {message}")),
            traceback: vec![],
            data: None,
        }),
    }))
}

/// Builds an *unstamped* `FatalError` event.
///
/// Public for hosts that cannot stamp timing because no [`Child`] is in scope —
/// notably a panic hook firing on a thread that no longer owns the child. When
/// a child is available, prefer [`Child::fatal_event`], which stamps timing.
#[must_use]
pub fn fatal_error_event(message: &str) -> pb::ChildEvent {
    event(pb::child_event::Kind::FatalError(pb::FatalError {
        message: message.to_owned(),
    }))
}

fn ok_event() -> pb::ChildEvent {
    event(pb::child_event::Kind::Ok(pb::Ok {}))
}

/// Builds a turn-ending `Error` event from an exception type and message.
fn error_event(exc_type: ExcType, message: &str) -> pb::ChildEvent {
    event(pb::child_event::Kind::Error(pb::Error {
        exception: Some(pb::RaisedException {
            exc_type: exc_type.to_string(),
            message: Some(message.to_owned()),
            traceback: vec![],
            data: None,
        }),
    }))
}

/// Stamps `tracker`'s timing and parent-enforced limits onto an event. Called
/// again by [`Child::handle`] just before sending, so a suspension announcement
/// is size-checked with the stamps it will carry.
fn stamp_budget(event: &mut pb::ChildEvent, tracker: &ResourceTracker) {
    event.total_execution_micros = u64::try_from(tracker.elapsed().as_micros()).unwrap_or(u64::MAX);
    event.max_duration_micros = tracker
        .max_duration()
        .map(|max| u64::try_from(max.as_micros()).unwrap_or(u64::MAX));
    event.max_suspensions = Some(tracker.max_suspensions() as u64);
}

/// Describes a suspension announcement that would exceed the wire frame limit.
///
/// The child turns this into a host-visible error before entering the
/// suspension, because the parent cannot resume a call it never received.
/// The event must already carry its session stamps (see [`stamp_budget`]).
fn oversize_suspension_error_message(event: &pb::ChildEvent) -> Option<String> {
    exceeds_max_frame_len(event)
        .map(|len| format!("argument frame of {len} bytes exceeds the maximum of {MAX_FRAME_LEN} bytes"))
}

/// Builds the suspension event for a `FunctionCall` (depth-checked by the
/// caller), **moving** the arguments into it.
///
/// The suspension keeps its args — a `Dump` of the suspended state (and its
/// replay on `Load`) needs them — so they come back via
/// [`Child::reclaim_suspension_payload`] once the event has been sent. They are
/// lent rather than copied because they are the largest thing a suspension
/// carries and are already live twice over here (the interpreter's own values,
/// plus this converted copy): a third copy for the announcement, on top of the
/// encode buffer, is what used to push a large host-call argument past the
/// session's memory limit.
fn suspension_event_function_call(call: &mut monty::ReplFunctionCall) -> pb::ChildEvent {
    event(pb::child_event::Kind::FunctionCall(WireFunctionCall {
        function_name: call.function_name.clone(),
        args: mem::take(&mut call.args),
        kwargs: mem::take(&mut call.kwargs),
        call_id: call.call_id,
        object_id: call.object_id,
    }))
}

/// Builds the suspension event for an `OsCall` (depth-checked by the caller),
/// **moving** the call payload into it — see
/// [`suspension_event_function_call`] for why, and
/// [`Child::reclaim_suspension_payload`] for how it comes back. `GetEnviron` is
/// the placeholder left behind: a unit variant, so the swap allocates nothing.
fn suspension_event_os_call(call: &mut monty::ReplOsCall) -> pb::ChildEvent {
    let function_call = mem::replace(&mut call.function_call, OsFunctionCall::GetEnviron);
    event(pb::child_event::Kind::OsCall(pb::OsCall {
        call_id: call.call_id,
        call: Some(function_call.into()),
    }))
}

fn complete_event(value: MontyObject) -> pb::ChildEvent {
    event(pb::child_event::Kind::Complete(pb::Complete {
        value: Some(value.into()),
    }))
}

/// Whether a suspension's argument payload nests too deeply for the wire —
/// used by `drive` (fresh) and `handle_load` (restored, i.e. forged dumps).
fn suspension_args_too_deep(progress: &ReplProgress) -> bool {
    match progress {
        ReplProgress::FunctionCall(call) => function_call_args_too_deep(call),
        ReplProgress::OsCall(call) => os_call_args_too_deep(call),
        // name lookups / future resolutions carry no sandbox values
        _ => false,
    }
}

/// Whether an external call's args/kwargs nest too deeply for the wire.
fn function_call_args_too_deep(call: &monty::ReplFunctionCall) -> bool {
    call.args.iter().any(exceeds_max_value_depth)
        || call
            .kwargs
            .iter()
            .any(|(k, v)| exceeds_max_value_depth(k) || exceeds_max_value_depth(v))
}

/// Whether an OS call's payload nests too deeply for the wire — only
/// `os.getenv`'s default carries an arbitrary (nestable) sandbox value.
fn os_call_args_too_deep(call: &monty::ReplOsCall) -> bool {
    match &call.function_call {
        OsFunctionCall::Getenv(args) => exceeds_max_value_depth(&args.default),
        _ => false,
    }
}

/// Builds the suspension event for a non-`Complete` progress state. Used on
/// `Load` to re-announce a restored suspension; fresh suspensions go through
/// `drive`, which adds depth/oversize checks before delegating to the same
/// per-variant builders.
fn suspension_event(progress: &mut ReplProgress) -> pb::ChildEvent {
    match progress {
        ReplProgress::FunctionCall(call) => suspension_event_function_call(call),
        ReplProgress::OsCall(call) => suspension_event_os_call(call),
        ReplProgress::NameLookup(lookup) => event(pb::child_event::Kind::NameLookup(pb::NameLookup {
            name: lookup.name.clone(),
            object_id: lookup.object_id().as_ref().map(uuid_to_pb),
        })),
        ReplProgress::ResolveFutures(state) => event(pb::child_event::Kind::ResolveFutures(pb::ResolveFutures {
            pending_call_ids: state.pending_call_ids().to_vec(),
        })),
        ReplProgress::Complete { .. } => unreachable!("Complete is handled before suspension_event"),
    }
}

/// Converts wire named inputs into `(name, value)` pairs for `feed_start`.
fn named_inputs(inputs: Vec<pb::NamedValue>) -> Result<Vec<(String, MontyObject)>, Box<pb::ChildEvent>> {
    inputs
        .into_iter()
        .map(|input| {
            let value = input
                .value
                .ok_or_else(|| Box::new(protocol_violation(&format!("input {:?} has no value", input.name))))?;
            let value = value
                .into_object()
                .map_err(|err| Box::new(protocol_violation(&format!("invalid input {:?}: {err}", input.name))))?;
            Ok((input.name, value))
        })
        .collect()
}

/// Streams sandbox `print()` output as `Print` events through an
/// [`EventSink`].
///
/// Line-buffered: a frame is written when the buffer ends with a newline or
/// exceeds [`Self::FLUSH_BYTES`], and [`Self::drain`] flushes any partial
/// line before the turn-ending event so ordering is exact.
struct ProtoPrint<'a> {
    buf: String,
    sink: &'a mut dyn EventSink,
}

impl<'a> ProtoPrint<'a> {
    /// Flush threshold for output that never produces a newline.
    const FLUSH_BYTES: usize = 8 * 1024;

    fn new(sink: &'a mut dyn EventSink) -> Self {
        Self {
            buf: String::new(),
            sink,
        }
    }

    /// Writes the buffer (if any) as one `Print` event.
    fn flush(&mut self) -> Result<(), MontyException> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let event = event(pb::child_event::Kind::Print(pb::Print {
            stream: pb::PrintStream::Stdout.into(),
            text: mem::take(&mut self.buf),
        }));
        self.sink.send(&event).map_err(|err| {
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
    /// event. Errors are ignored — if the sink is broken the turn-ending write
    /// fails anyway.
    fn drain(&mut self) {
        let _ = self.flush();
    }
}

impl PrintWriterCallback for ProtoPrint<'_> {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        // Append in pieces no larger than the flush threshold so a single huge
        // write cannot inflate the buffer (and the untracked copy it holds)
        // past `FLUSH_BYTES`: each filled chunk is flushed before the next is
        // appended.
        let mut rest = output.as_ref();
        while !rest.is_empty() {
            let take = floor_char_boundary(rest, Self::FLUSH_BYTES - self.buf.len());
            if take == 0 {
                // not even one char fits in the remaining room; flush to free
                // the whole threshold (far larger than any single char)
                self.flush()?;
                continue;
            }
            self.buf.push_str(&rest[..take]);
            rest = &rest[take..];
            self.maybe_flush()?;
        }
        Ok(())
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.buf.push(end);
        self.maybe_flush()
    }
}

/// Largest index `<= max` (capped at `s.len()`) that is a char boundary of
/// `s`, so `s[..idx]` is always valid UTF-8. A stable stand-in for the
/// unstable `str::floor_char_boundary`.
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        s.len()
    } else {
        let mut idx = max;
        // index 0 is always a boundary, so this terminates
        while !s.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }
}
