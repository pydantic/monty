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

use monty::{
    AssertMessageAnnotations, CompileOptions, ExcType, ExtFunctionResult, LimitedTracker, MontyException, MontyObject,
    MontyRepl, OsFunctionCall, PrintWriter, PrintWriterCallback, ReplProgress, ReplStartError,
};
use monty_type_checking::{SourceFile, type_check};
use prost::Message;

use super::{
    FrameError, FrameReader, MAX_FRAME_LEN, MONTY_VERSION, WireFunctionCall, exceeds_max_value_depth,
    future_results_from_proto, pb, write_frame,
};

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
/// stubs — so a `Load`ed session keeps type-check enforcement:
///
/// - `[script_name str][type_check u8]` and, when `type_check` is 1,
///   `[committed_stubs str][has_pending u8][pending_snippet str?]`, where each
///   `str` is a `u32 LE` byte length followed by UTF-8 bytes.
///
/// The payload is monty's postcard format — only a monty child of the same
/// version can restore it. Bumped to 5 because adding argument-name variants
/// changed the serialized `StaticStrings` discriminants.
const DUMP_VERSION: u16 = 5;

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

/// REPL session state of the child.
enum SessionState {
    /// No repl materialized yet. `Some` once `Configure` has stored the config
    /// (the repl is built lazily on the first `Feed` / `Dump`); `None` on a
    /// freshly spawned or just-`Reset` worker, before `Configure`. `Load` is
    /// valid only from here — it cannot clobber a started session.
    Configured(Option<Box<pb::Configure>>),
    /// Session ready for the next `Feed`.
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

/// The child-side session state that lives *outside* the repl/progress payload
/// and so must travel in the dump envelope explicitly: the script name and the
/// type-check state. Serialized by [`push_session_meta`] and parsed by
/// [`take_session_meta`].
struct SessionMeta {
    script_name: String,
    type_check: Option<TypeCheckState>,
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
    /// `Some` when the session was created with `type_check: true`.
    type_check: Option<TypeCheckState>,
}

impl Default for Child {
    fn default() -> Self {
        Self::new()
    }
}

impl Child {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: SessionState::Configured(None),
            script_name: String::new(),
            type_check: None,
        }
    }

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
                // Version skew is fatal. The protocol has no in-band
                // negotiation and assumes parent and child are deployed in
                // lockstep; a mismatched build can have a different frame
                // layout, so we fail fast with a `FatalError` rather than risk
                // a silent desync. Emit the fatal last gasp and stop the child.
                //
                // An *empty* version opts out of skew detection: it marks a
                // parent that ships in lockstep with the worker and so has no
                // independent build version to compare — the bundled wasm
                // worker, whose TypeScript driver is published together with the
                // `.wasm` in one package. A separately-deployed parent (the
                // subprocess pool) always sends its real version, so its skew
                // check is unaffected.
                if !configure.monty_version.is_empty() && configure.monty_version != MONTY_VERSION {
                    sink.send(&self.fatal_event(&format!(
                        "version skew: parent={:?} child={MONTY_VERSION:?}",
                        configure.monty_version
                    )))?;
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
            pb::parent_request::Kind::Dump(_) => self.handle_dump(),
            pb::parent_request::Kind::Load(load) => self.handle_load(load),
            pb::parent_request::Kind::Reset(_) => {
                self.reset();
                ok_event()
            }
            pb::parent_request::Kind::Shutdown(_) => {
                sink.send(&ok_event())?;
                return Ok(HandleOutcome::Shutdown);
            }
        };
        self.stamp_execution_time(&mut event);
        if let Err(err) = sink.send(&event) {
            self.recover_send_error(err, sink)?;
        }
        Ok(HandleOutcome::Continue)
    }

    /// Builds a timing-stamped `FatalError` event for an unrecoverable
    /// condition the host detected (frame desync, oversize request). The host
    /// sends it and exits right after; it is the child's parseable last gasp.
    #[must_use]
    pub fn fatal_event(&self, message: &str) -> pb::ChildEvent {
        let mut event = fatal_error_event(message);
        // fatal paths bypass `handle`, so stamp timing here to keep the
        // "every turn-ending event carries timing" contract intact
        self.stamp_execution_time(&mut event);
        event
    }

    /// Recovers from a failure to write a turn-ending event.
    ///
    /// [`write_frame`] rejects an oversize frame *before* writing any bytes, so
    /// the stream stays synced. When the session is not mid-suspension — e.g.
    /// a `Complete` result that is merely larger than the frame limit — we can
    /// answer with a clean, session-preserving error and keep serving instead
    /// of crashing the worker. An oversize *suspension* announcement is
    /// unrecoverable (the worker is already suspended but the parent never
    /// learned the resume point), so it propagates to the host loop's fatal
    /// handling, as does any genuine I/O break.
    fn recover_send_error(&mut self, err: FrameError, sink: &mut dyn EventSink) -> Result<(), FrameError> {
        match err {
            FrameError::FrameTooLarge { len, max } if !matches!(self.state, SessionState::Suspended(_)) => {
                let mut event = error_event(
                    ExcType::RuntimeError,
                    &format!("result frame of {len} bytes exceeds the maximum of {max} bytes"),
                );
                self.stamp_execution_time(&mut event);
                sink.send(&event)
            }
            other => Err(other),
        }
    }

    /// Stamps cumulative execution time and the `max_duration` budget onto a
    /// turn-ending event, making the child the single source of truth for
    /// timing (the parent's watchdog derives its backstop from these fields).
    /// Left zero/absent when no session exists.
    fn stamp_execution_time(&self, event: &mut pb::ChildEvent) {
        let tracker = match &self.state {
            SessionState::Ready(repl) => repl.tracker(),
            SessionState::Suspended(progress) => progress.tracker(),
            // no repl materialized yet → no tracker to report
            SessionState::Configured(_) => return,
        };
        event.total_execution_micros = u64::try_from(tracker.elapsed().as_micros()).unwrap_or(u64::MAX);
        event.max_duration_micros = tracker
            .max_duration()
            .map(|max| u64::try_from(max.as_micros()).unwrap_or(u64::MAX));
    }

    /// Stores the session config; the repl is built lazily by [`ensure_repl`]
    /// on the first feed/dump (or restored by `Load` instead). Valid only on a
    /// not-yet-configured worker.
    fn handle_configure(&mut self, configure: pb::Configure) -> pb::ChildEvent {
        if !matches!(self.state, SessionState::Configured(None)) {
            return protocol_violation("Configure while a session already exists");
        }
        self.state = SessionState::Configured(Some(Box::new(configure)));
        ok_event()
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
        let pb::Configure {
            script_name,
            limits,
            type_check,
            type_check_stubs,
            assert_message_annotations,
            // already validated against our own version when `Configure` arrived
            monty_version: _,
        } = *config;
        let limits = limits.unwrap_or_default().into();
        self.script_name = script_name;
        self.type_check = type_check.then(|| TypeCheckState {
            committed_stubs: type_check_stubs.unwrap_or_default(),
            pending_snippet: None,
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
            LimitedTracker::new(limits),
            options,
        )));
        Ok(())
    }

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
        let result: ExtFunctionResult = match resume.result {
            Some(result) => match result.try_into() {
                Ok(result) => result,
                Err(err) => return protocol_violation(&format!("invalid result: {err}")),
            },
            None => return protocol_violation("ResumeCall has no result"),
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

    /// Serializes the current session into the opaque dump envelope. The
    /// session stays live — dumping is read-only.
    fn handle_dump(&mut self) -> pb::ChildEvent {
        // a never-fed session is materialized into an empty repl so it can be
        // dumped; a never-configured worker has nothing to dump
        if let Err(event) = self.ensure_repl() {
            return *event;
        }
        let dumped = match &self.state {
            SessionState::Ready(repl) => repl.dump().map(|bytes| (0u8, bytes)),
            SessionState::Suspended(progress) => progress.dump().map(|bytes| (1u8, bytes)),
            SessionState::Configured(_) => unreachable!("ensure_repl materialized the repl or errored"),
        };
        match dumped {
            Ok((tag, payload)) => {
                let mut state = Vec::with_capacity(payload.len() + 64);
                state.extend_from_slice(&DUMP_VERSION.to_le_bytes());
                state.push(tag);
                push_session_meta(&mut state, &self.script_name, self.type_check.as_ref());
                state.extend_from_slice(&payload);
                event(pb::child_event::Kind::DumpResult(pb::DumpResult { state }))
            }
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
    fn handle_load(&mut self, load: pb::Load) -> pb::ChildEvent {
        if !matches!(self.state, SessionState::Configured(_)) {
            return protocol_violation("Load requires a session that has not started (a feed has already run)");
        }
        let pb::Load { state } = load;
        let Some((version_bytes, rest)) = state.split_at_checked(2) else {
            return protocol_violation("dump state too short");
        };
        let version = u16::from_le_bytes([version_bytes[0], version_bytes[1]]);
        if version != DUMP_VERSION {
            return protocol_violation(&format!("unsupported dump version {version} (expected {DUMP_VERSION})"));
        }
        let Some((&tag, rest)) = rest.split_first() else {
            return protocol_violation("dump state too short");
        };
        let Some((meta, payload)) = take_session_meta(rest) else {
            return protocol_violation("malformed dump session metadata");
        };
        let SessionMeta {
            script_name,
            type_check,
        } = meta;
        let mut event = match tag {
            0 => match MontyRepl::load(payload) {
                Ok(repl) => {
                    self.state = SessionState::Ready(Box::new(repl));
                    ok_event()
                }
                Err(err) => protocol_violation(&format!("failed to load session: {err}")),
            },
            1 => match ReplProgress::load(payload) {
                Ok(ReplProgress::Complete { repl, value }) => {
                    // a dump is never taken at Complete, but a forged/legacy
                    // one could contain it; surface the value rather than fail
                    self.state = SessionState::Ready(Box::new(repl));
                    complete_event(value)
                }
                Ok(progress) => {
                    let event = suspension_event(&progress);
                    self.state = SessionState::Suspended(Box::new(progress));
                    event
                }
                Err(err) => protocol_violation(&format!("failed to load suspended session: {err}")),
            },
            other => protocol_violation(&format!("unknown dump tag {other}")),
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
        mut result: Result<ReplProgress<Tracker>, Box<ReplStartError<Tracker>>>,
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
                    let function_call = call.take_function_call();
                    // only `os.getenv`'s default carries an arbitrary sandbox
                    // value — every other arm is flat strings/bytes/typed
                    // structs, which cannot nest
                    let too_deep = match &function_call {
                        OsFunctionCall::Getenv(args) => exceeds_max_value_depth(&args.default),
                        _ => false,
                    };
                    if too_deep {
                        let err =
                            MontyException::new(ExcType::RuntimeError, Some("Max argument depth exceeded".to_owned()));
                        result = call.resume(ExtFunctionResult::Error(err), PrintWriter::Callback(print));
                        continue;
                    }
                    let event = event(pb::child_event::Kind::OsCall(pb::OsCall {
                        call_id: call.call_id,
                        call: Some(function_call.into()),
                    }));
                    if let Some(message) = oversize_suspension_error_message(&event) {
                        return self.abort_feed_with_runtime_error(call.into_repl(), &message);
                    }
                    self.state = SessionState::Suspended(Box::new(ReplProgress::OsCall(call)));
                    return event;
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
                    if let Some(message) = oversize_suspension_error_message(&event) {
                        return self.abort_feed_with_runtime_error(call.into_repl(), &message);
                    }
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
    fn abort_feed_with_runtime_error(&mut self, repl: MontyRepl<Tracker>, message: &str) -> pb::ChildEvent {
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
        match type_check(&SourceFile::new(code, &self.script_name), stubs.as_ref()) {
            Ok(None) => None,
            Ok(Some(diagnostics)) => Some(event(pb::child_event::Kind::TypingError(pb::TypingError {
                diagnostics: diagnostics.to_string(),
            }))),
            Err(err) => Some(protocol_violation(&format!("type checker failed: {err}"))),
        }
    }

    /// Drops all session state, returning to the unconfigured state ready for
    /// the next `Configure` (or `Load`).
    fn reset(&mut self) {
        self.state = SessionState::Configured(None);
        self.type_check = None;
        self.script_name = String::new();
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

/// Describes a suspension announcement that would exceed the wire frame limit.
///
/// The child turns this into a host-visible error before entering the
/// suspension, because the parent cannot resume a call it never received.
fn oversize_suspension_error_message(event: &pb::ChildEvent) -> Option<String> {
    let len = u32::try_from(event.encoded_len()).unwrap_or(u32::MAX);
    (len > MAX_FRAME_LEN).then(|| format!("argument frame of {len} bytes exceeds the maximum of {MAX_FRAME_LEN} bytes"))
}

/// Builds the suspension event for a fresh `FunctionCall` (depth-checked by
/// the caller).
///
/// Clones the argument payload: the suspension keeps its args so a `Dump` of
/// the suspended state (and its replay on `Load`) stays complete.
fn suspension_event_function_call(call: &monty::ReplFunctionCall<Tracker>) -> pb::ChildEvent {
    event(pb::child_event::Kind::FunctionCall(WireFunctionCall {
        function_name: call.function_name.clone(),
        args: call.args.clone(),
        kwargs: call.kwargs.clone(),
        call_id: call.call_id,
        method_call: call.method_call,
    }))
}

fn complete_event(value: MontyObject) -> pb::ChildEvent {
    event(pb::child_event::Kind::Complete(pb::Complete {
        value: Some(value.into()),
    }))
}

/// Builds the suspension event for a non-`Complete`, non-`OsCall` progress
/// state (OS calls are special-cased in `drive` because emitting them consumes
/// the call's argument payload).
fn suspension_event(progress: &ReplProgress<Tracker>) -> pb::ChildEvent {
    let kind = match progress {
        ReplProgress::FunctionCall(call) => pb::child_event::Kind::FunctionCall(WireFunctionCall {
            function_name: call.function_name.clone(),
            args: call.args.clone(),
            kwargs: call.kwargs.clone(),
            call_id: call.call_id,
            method_call: call.method_call,
        }),
        ReplProgress::OsCall(call) => {
            // reached only on `Load` of a dumped OsCall suspension, where the
            // payload was already consumed by `take_function_call` (leaving
            // `Used`, announced as `Consumed` — the parent re-learns the call
            // from its own records); a fresh suspension goes through `drive`
            // instead, which moves the payload into the event
            let kind = match &call.function_call {
                OsFunctionCall::Used => pb::os_call::Call::Consumed(pb::Unit {}),
                function_call => function_call.clone().into(),
            };
            pb::child_event::Kind::OsCall(pb::OsCall {
                call_id: call.call_id,
                call: Some(kind),
            })
        }
        ReplProgress::NameLookup(lookup) => pb::child_event::Kind::NameLookup(pb::NameLookup {
            name: lookup.name.clone(),
        }),
        ReplProgress::ResolveFutures(state) => pb::child_event::Kind::ResolveFutures(pb::ResolveFutures {
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

/// Appends the session metadata (script name + type-check state, see
/// [`DUMP_VERSION`]) to a dump envelope.
fn push_session_meta(buf: &mut Vec<u8>, script_name: &str, type_check: Option<&TypeCheckState>) {
    push_str_field(buf, script_name);
    match type_check {
        Some(tc) => {
            buf.push(1);
            push_str_field(buf, &tc.committed_stubs);
            match &tc.pending_snippet {
                Some(snippet) => {
                    buf.push(1);
                    push_str_field(buf, snippet);
                }
                None => buf.push(0),
            }
        }
        None => buf.push(0),
    }
}

/// Splits the [`SessionMeta`] off the front of a dump envelope, returning it
/// together with the remaining postcard payload. `None` means the envelope is
/// malformed.
fn take_session_meta(bytes: &[u8]) -> Option<(SessionMeta, &[u8])> {
    let (script_name, rest) = take_str_field(bytes)?;
    let (&type_check_flag, rest) = rest.split_first()?;
    let (type_check, rest) = match type_check_flag {
        0 => (None, rest),
        1 => {
            let (committed_stubs, rest) = take_str_field(rest)?;
            let (&pending_flag, rest) = rest.split_first()?;
            let (pending_snippet, rest) = match pending_flag {
                0 => (None, rest),
                1 => take_str_field(rest).map(|(snippet, rest)| (Some(snippet), rest))?,
                _ => return None,
            };
            (
                Some(TypeCheckState {
                    committed_stubs,
                    pending_snippet,
                }),
                rest,
            )
        }
        _ => return None,
    };
    Some((
        SessionMeta {
            script_name,
            type_check,
        },
        rest,
    ))
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
