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

use std::{
    borrow::Cow,
    mem,
    time::{Duration, Instant},
};

use monty::{
    CheckedSource, Dump, MontyRepl, ReplProgress, ReplStartError, Session, SessionRef, dump,
    type_check_nesting_exception,
};
use monty_type_checking::{SourceFile, TypeCheckContext, TypeChecker, scan_imports};
use monty_types::{
    AssertMessageAnnotations, CHECKER_MODULE_NAMES, CompileOptions, ExcType, ExtFunctionResult, IMPORT_STUBS_CALL_ID,
    ModuleStub, MontyException, MontyObject, NamedValues, OsFunctionCall, OsPolicy, PrintStream, PrintWriter,
    PrintWriterCallback, ResourceLimits, ResourceTracker, SANDBOX_MODULE_NAMES, SOURCE_SCAN_THRESHOLD, SourceRange,
    TypeCheckState, TypeCheckingConfig,
};

use super::{
    BudgetVec, DEFAULT_PRINT_FLUSH_INTERVAL, FrameError, FrameReader, MAX_FRAME_LEN, ProtoConvertError,
    WireFunctionCall, check_protocol_version, exceeds_max_frame_len, ext_result_from_proto, future_results_from_proto,
    named_values_from_proto, os_call_from_proto, os_call_to_proto, pb, write_frame,
};
use crate::{
    admission::{ALLOCATION_MODEL_VERSION, DECODE_BUDGET_ERROR, canonical_event_decode_bytes, recognized_profile},
    convert::limits::micros_field,
    wire::uuid_to_pb,
};

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
    /// Host-enforced sleep budget; `None` when unlimited or no session exists.
    pub max_total_sleep: Option<Duration>,
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
    /// A feed awaiting host stubs before type checking.
    AwaitingStubs(Box<PendingFeed>),
}

/// A feed held until the parent answers `import_stubs`.
struct PendingFeed {
    repl: Box<MontyRepl>,
    code: String,
    inputs: NamedValues,
    /// Requested modules, in source order.
    asked: Vec<String>,
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
    /// Built by the first type-checked feed and dropped when the session resets.
    type_checker: Option<TypeChecker>,
    /// `Some` when the session was created with `type_check: true`.
    type_check: Option<TypeCheckState>,
    /// How long [`ProtoPrint`] may hold buffered output, from the session's
    /// `Configure`. `Duration::ZERO` means line buffering (see the field's
    /// documentation in the schema).
    print_flush_interval: Duration,
    /// The session's `OsPolicy` from `Configure`, applied when creating the REPL.
    os_policy: OsPolicy,
    /// Host module stubs received through `import_stubs` or restored by `Load`.
    module_stubs: Vec<ModuleStub>,
    /// Negotiated delivery policy belongs to this connection, never to a dump.
    receiver_allocation_profile: Option<pb::ReceiverAllocationProfile>,
    /// Retains a print sink failure until handle can return its original transport error.
    print_transport_error: Option<FrameError>,
}

impl Default for Child {
    fn default() -> Self {
        Self {
            state: SessionState::Configured(None),
            script_name: String::new(),
            type_checker: None,
            type_check: None,
            print_flush_interval: DEFAULT_PRINT_FLUSH_INTERVAL,
            os_policy: OsPolicy::default(),
            module_stubs: Vec::new(),
            receiver_allocation_profile: None,
            print_transport_error: None,
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
            pb::parent_request::Kind::Reset(_) => {
                self.reset();
                ok_event()
            }
            pb::parent_request::Kind::Shutdown(_) => {
                sink.send(&ok_event())?;
                return Ok(HandleOutcome::Shutdown);
            }
        };
        // These replies were stamped and admitted before their state was stored.
        let preadmitted = event.restored_script_name.is_some()
            || matches!(
                event.kind,
                Some(
                    pb::child_event::Kind::FunctionCall(_)
                        | pb::child_event::Kind::OsCall(_)
                        | pb::child_event::Kind::NameLookup(_)
                        | pb::child_event::Kind::ResolveFutures(_)
                )
            );
        let fatal = matches!(event.kind, Some(pb::child_event::Kind::FatalError(_)));
        if !preadmitted {
            self.stamp_session_budget(&mut event);
            if let Err(rejection) = admit_event(&event, self.receiver_allocation_profile.as_ref()) {
                event = if fatal {
                    self.fatal_event(DECODE_BUDGET_ERROR)
                } else {
                    self.rejection_event(rejection, false)
                };
            }
        }
        let sent = match self.print_transport_error.take() {
            Some(error) => Err(error),
            None => sink.send(&event),
        };
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
        Ok(if fatal {
            HandleOutcome::Fatal
        } else {
            HandleOutcome::Continue
        })
    }

    /// Builds one bounded replacement without copying the rejected payload.
    fn rejection_event(&self, rejection: EventRejection, suspension: bool) -> pb::ChildEvent {
        let mut event = error_event(ExcType::RuntimeError, &rejection.message(suspension));
        self.stamp_session_budget(&mut event);
        if admit_event(&event, self.receiver_allocation_profile.as_ref()).is_err() {
            event = error_event(ExcType::RuntimeError, DECODE_BUDGET_ERROR);
            self.stamp_session_budget(&mut event);
            admit_event(&event, self.receiver_allocation_profile.as_ref())
                .expect("validated profiles admit the fixed replacement error");
        }
        event
    }

    /// Propagates final print failure after restoring a computed suspension's loan.
    fn finish_print(&mut self, mut event: pb::ChildEvent, print: &mut ProtoPrint<'_>) -> pb::ChildEvent {
        let drained = print.drain();
        if let Some(error) = print.transport_error.take() {
            self.print_transport_error = Some(error);
            return event;
        }
        if let Err(error) = drained {
            if let Err(error) = self.reclaim_suspension_payload(&mut event) {
                return self.fatal_event(&format!("suspension payload could not be restored: {error}"));
            }
            if matches!(self.state, SessionState::Suspended(_)) {
                let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None))
                else {
                    unreachable!("checked above");
                };
                self.state = SessionState::Ready(Box::new(progress.into_repl()));
                self.discard_pending_snippet();
            }
            error_event_from(&error)
        } else {
            event
        }
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
                call.args = mem::take(announced).into_call_args()?;
            }
            (ReplProgress::OsCall(call), Some(pb::child_event::Kind::OsCall(announced)))
                if announced.call.is_some() =>
            {
                call.function_call = os_call_from_proto(mem::take(announced))?.1;
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
    /// restored from a dump with limits capped by `Configure`, or ended by `Reset`.
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
                max_total_sleep: config
                    .limits
                    .as_ref()
                    .and_then(|limits| limits.max_total_sleep_micros)
                    .map(Duration::from_micros),
            },
            SessionState::Configured(None) => SessionBudget::default(),
            SessionState::Ready(repl) => self.tracker_budget(repl.tracker()),
            SessionState::Suspended(progress) => self.tracker_budget(progress.tracker()),
            SessionState::AwaitingStubs(pending) => self.tracker_budget(pending.repl.tracker()),
        }
    }

    /// The budget of a materialized session, whose limits live in its tracker.
    fn tracker_budget(&self, tracker: &ResourceTracker) -> SessionBudget {
        SessionBudget {
            max_memory: tracker.max_memory(),
            type_check: self.type_check.is_some(),
            max_suspensions: Some(tracker.max_suspensions()),
            max_total_sleep: tracker.max_total_sleep(),
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
        if admit_event(&event, self.receiver_allocation_profile.as_ref()).is_err() {
            event = fatal_error_event(DECODE_BUDGET_ERROR);
            self.stamp_session_budget(&mut event);
        }
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
                if let Err(rejection) = admit_event(&event, self.receiver_allocation_profile.as_ref()) {
                    event = self.rejection_event(rejection, false);
                }
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
            SessionState::AwaitingStubs(pending) => pending.repl.tracker(),
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
            // Applied on arrival rather than in `ensure_repl`, which a `Load`
            // never reaches: a dump restores the repl directly, and print
            // pacing is a delivery setting the dump does not carry.
            // Absent means an older parent, or one with no opinion; both get
            // the default rather than the line buffering that predates it.
            self.print_flush_interval = configure
                .print_flush_interval_ms
                .map_or(DEFAULT_PRINT_FLUSH_INTERVAL, |ms| Duration::from_millis(u64::from(ms)));
            // Reject invalid settings on the Configure turn.
            self.os_policy = match configure.os_policy.clone().map(OsPolicy::try_from) {
                None => OsPolicy::default(),
                Some(Ok(os_policy)) => os_policy,
                Some(Err(err)) => return protocol_violation(&format!("invalid os_policy: {err}")),
            };
            // ty walks the stubs with every feed, unguarded.
            if let Some(stubs) = &configure.type_check_stubs
                && type_check_nesting_exception(stubs, "repl_type_stubs.pyi", SOURCE_SCAN_THRESHOLD).is_err()
            {
                return protocol_violation("invalid type_check_stubs: Source is too deeply nested");
            }
            self.receiver_allocation_profile = configure
                .receiver_allocation_profile
                .filter(|profile| configure.protocol_version >= 7 && recognized_profile(profile));
            let mut event = ok_event();
            event.accepted_receiver_allocation_profile_version = self
                .receiver_allocation_profile
                .as_ref()
                .map(|_| ALLOCATION_MODEL_VERSION);
            self.state = SessionState::Configured(Some(Box::new(configure)));
            event
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
            SessionState::Ready(_) | SessionState::Suspended(_) | SessionState::AwaitingStubs(_) => return Ok(()),
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
            // applied when the `Configure` arrived, so a `Load` honors it too
            print_flush_interval_ms: _,
            // validated and stored when the `Configure` arrived
            os_policy: _,
            // a relay's concern; the child never stores sessions
            persistence: _,
            // a relay's concern; the child has no profiles
            profile: _,
            receiver_allocation_profile: _,
        } = *config;
        let limits = limits.unwrap_or_default().into();
        self.script_name = script_name;
        self.type_check = type_check.then(|| TypeCheckState {
            committed_stubs: type_check_stubs.unwrap_or_default(),
            pending_snippet: None,
            config: type_check_config,
            committed_imports: String::new(),
            pending_imports: String::new(),
            asked_modules: Vec::new(),
        });
        // Missing field means an older parent; the feature defaults to on.
        let options = CompileOptions {
            assert_message_annotations: assert_message_annotations.map_or_else(
                AssertMessageAnnotations::default,
                AssertMessageAnnotations::from_max_bytes,
            ),
            // Not on the wire: every host gets the default.
            source_scan_threshold: SOURCE_SCAN_THRESHOLD,
        };
        let repl = MontyRepl::new(&self.script_name, ResourceTracker::new(limits), options)
            .with_os_policy(self.os_policy.clone());
        self.state = SessionState::Ready(Box::new(repl));
        Ok(())
    }

    /// Checks source nesting and requests missing host stubs before starting a feed.
    fn handle_repl_feed(&mut self, feed: pb::Feed, sink: &mut dyn EventSink) -> pb::ChildEvent {
        if let Err(event) = self.ensure_repl() {
            return *event;
        }
        if !matches!(self.state, SessionState::Ready(_)) {
            return protocol_violation("Feed without a session ready for input");
        }
        let inputs = match named_values_from_proto(feed.inputs, feed.values) {
            Ok(inputs) => inputs,
            Err(err) => return protocol_violation(&format!("invalid inputs: {err}")),
        };
        let SessionState::Ready(mut repl) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked Ready above");
        };
        // Check nesting before the import scan, ty or the compiler parses the snippet.
        let code = feed.code;
        let checked = match repl.check_source(&code) {
            Ok(checked) => checked,
            Err(error) => {
                self.state = SessionState::Ready(repl);
                return error_event_from(&error);
            }
        };
        // The working directory persists in the REPL (including `os.chdir`); the parent
        // sends one only to switch it, and the switch holds whatever the feed's outcome.
        if !feed.cwd.is_empty() {
            repl.set_cwd(&feed.cwd);
        }
        // snippets fed with skip_type_check never become type-check context:
        // the caller explicitly excluded them from checking, so later snippets
        // must not be checked against their (unchecked) bindings either
        let type_checked = !feed.skip_type_check && self.type_check.is_some();
        if type_checked && let Some(state) = &mut self.type_check {
            let imports = scan_imports(&code);
            state.pending_snippet = Some(code.clone());
            state.pending_imports = imports.prelude;
            // Each answer is final for the session, even if it contains no stub.
            let unasked = |name: &str| {
                !SANDBOX_MODULE_NAMES.contains(&name)
                    && !CHECKER_MODULE_NAMES.contains(&name)
                    && !state.asked_modules.iter().any(|asked| asked == name)
            };
            if let Some(first) = imports.modules.iter().find(|module| unasked(&module.name)) {
                let asked: Vec<String> = imports
                    .modules
                    .iter()
                    .filter(|module| unasked(&module.name))
                    .map(|module| module.name.clone())
                    .collect();
                let position = SourceRange::new(
                    &repl.next_script_name(),
                    first.range.start().to_u32(),
                    first.range.end().to_u32(),
                );
                let ask = OsFunctionCall::ImportStubs(asked.clone());
                let mut event = event(pb::child_event::Kind::OsCall(os_call_to_proto(
                    IMPORT_STUBS_CALL_ID,
                    ask,
                    false,
                    &position,
                )));
                stamp_budget(&mut event, repl.tracker());
                if let Err(rejection) = admit_event(&event, self.receiver_allocation_profile.as_ref()) {
                    self.state = SessionState::Ready(repl);
                    self.discard_pending_snippet();
                    return self.rejection_event(rejection, true);
                }
                self.state = SessionState::AwaitingStubs(Box::new(PendingFeed {
                    repl,
                    code,
                    inputs,
                    asked,
                }));
                return event;
            }
        }
        self.start_feed(repl, checked, inputs, type_checked, sink)
    }

    /// Optionally type-checks, injects inputs and executes to the turn-ending event.
    fn start_feed(
        &mut self,
        repl: Box<MontyRepl>,
        checked: CheckedSource<'_>,
        inputs: NamedValues,
        type_checked: bool,
        sink: &mut dyn EventSink,
    ) -> pb::ChildEvent {
        if type_checked && let Some(event) = self.type_check_feed(checked.code()) {
            self.state = SessionState::Ready(repl);
            self.discard_pending_snippet();
            return event;
        }
        let mut print = ProtoPrint::new(sink, self.print_flush_interval, self.receiver_allocation_profile);
        let result = repl.feed_start_checked(checked, inputs, PrintWriter::Callback(&mut print));
        let event = self.drive(result);
        self.finish_print(event, &mut print)
    }

    /// Records the stubs and starts the pending feed; an exception answer ends it before execution.
    fn handle_resume_stubs(&mut self, resume: pb::ResumeCall, sink: &mut dyn EventSink) -> pb::ChildEvent {
        if resume.call_id != IMPORT_STUBS_CALL_ID {
            return protocol_violation(&format!(
                "ResumeCall call_id {} does not match {IMPORT_STUBS_CALL_ID}",
                resume.call_id
            ));
        }
        let Some(wire_result) = resume.result else {
            return protocol_violation("ResumeCall has no result");
        };
        let SessionState::AwaitingStubs(pending) = &self.state else {
            unreachable!("checked by the caller");
        };
        let not_handled = matches!(wire_result.kind, Some(pb::ext_function_result::Kind::NotHandled(_)));
        let result = if not_handled {
            ExtFunctionResult::NotFound(String::new())
        } else {
            match ext_result_from_proto(wire_result, resume.values) {
                Ok(result) => result,
                Err(err) => return protocol_violation(&format!("invalid result: {err}")),
            }
        };
        let stubs = match result {
            ExtFunctionResult::Return(answer) => match stubs_from_answer(&answer, &pending.asked) {
                Ok(stubs) => stubs,
                Err(message) => return protocol_violation(&message),
            },
            ExtFunctionResult::NotFound(_) => Vec::new(),
            ExtFunctionResult::Future(_) => {
                return protocol_violation("import_stubs cannot be answered with a future");
            }
            ExtFunctionResult::Error(exc) => {
                let SessionState::AwaitingStubs(pending) =
                    mem::replace(&mut self.state, SessionState::Configured(None))
                else {
                    unreachable!("checked above");
                };
                return self.fail_pending_feed(*pending, &exc);
            }
        };
        let SessionState::AwaitingStubs(pending) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        for stub in stubs {
            if !self.module_stubs.iter().any(|known| known.module() == stub.module()) {
                self.module_stubs.push(stub);
            }
        }
        let PendingFeed {
            repl,
            code,
            inputs,
            asked,
        } = *pending;
        if let Some(state) = &mut self.type_check {
            state.asked_modules.extend(asked);
        }
        // already found within the bound on arrival, so this cannot fail
        let checked = match repl.check_source(&code) {
            Ok(checked) => checked,
            Err(error) => {
                self.state = SessionState::Ready(repl);
                self.discard_pending_snippet();
                return error_event_from(&error);
            }
        };
        self.start_feed(repl, checked, inputs, true, sink)
    }

    /// Returns the exception and readies the session after a feed fails before execution.
    fn fail_pending_feed(&mut self, pending: PendingFeed, exc: &MontyException) -> pb::ChildEvent {
        self.state = SessionState::Ready(pending.repl);
        self.discard_pending_snippet();
        error_event_from(exc)
    }

    /// Excludes a failed feed from later type checks.
    fn discard_pending_snippet(&mut self) {
        if let Some(state) = &mut self.type_check {
            state.pending_snippet = None;
            state.pending_imports.clear();
        }
    }

    /// Answers a suspended external function or OS call with the parent's
    /// result, checking the `call_id` matches, then resumes execution.
    fn handle_resume_call(&mut self, resume: pb::ResumeCall, sink: &mut dyn EventSink) -> pb::ChildEvent {
        if matches!(self.state, SessionState::AwaitingStubs(_)) {
            return self.handle_resume_stubs(resume, sink);
        }
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
                match ext_result_from_proto(wire_result, resume.values) {
                    Ok(result) => result,
                    Err(err) => return protocol_violation(&format!("invalid result: {err}")),
                }
            };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(sink, self.print_flush_interval, self.receiver_allocation_profile);
        let outcome = match *progress {
            ReplProgress::FunctionCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            ReplProgress::OsCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            _ => unreachable!("checked above"),
        };
        let event = self.drive(outcome);
        self.finish_print(event, &mut print)
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
        let mut print = ProtoPrint::new(sink, self.print_flush_interval, self.receiver_allocation_profile);
        let outcome = lookup.resume(result, PrintWriter::Callback(&mut print));
        let event = self.drive(outcome);
        self.finish_print(event, &mut print)
    }

    /// Raises the parent's exception uncatchably at any pending suspension.
    /// The `Error` reply returns the session to `Ready`.
    fn handle_abort_feed(&mut self, abort: pb::AbortFeed, sink: &mut dyn EventSink) -> pb::ChildEvent {
        // Guard against a corrupt `Complete` state instead of crashing.
        let suspended = matches!(&self.state, SessionState::Suspended(progress)
            if !matches!(progress.as_ref(), ReplProgress::Complete { .. }));
        if !suspended && !matches!(self.state, SessionState::AwaitingStubs(_)) {
            return protocol_violation("AbortFeed without a suspended feed");
        }
        let Some(exception) = abort.exception else {
            return protocol_violation("AbortFeed has no exception");
        };
        let exc = match MontyException::try_from(exception) {
            Ok(exc) => exc,
            Err(err) => return protocol_violation(&format!("invalid exception: {err}")),
        };
        let progress = match mem::replace(&mut self.state, SessionState::Configured(None)) {
            SessionState::Suspended(progress) => progress,
            // a feed held for its stubs never started, so there is nothing to unwind
            SessionState::AwaitingStubs(pending) => return self.fail_pending_feed(*pending, &exc),
            _ => unreachable!("checked above"),
        };
        let mut print = ProtoPrint::new(sink, self.print_flush_interval, self.receiver_allocation_profile);
        let outcome = match *progress {
            ReplProgress::FunctionCall(call) => call.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::OsCall(call) => call.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::NameLookup(lookup) => lookup.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::ResolveFutures(state) => state.abort(exc, PrintWriter::Callback(&mut print)),
            ReplProgress::Complete { .. } => unreachable!("checked above"),
        };
        let event = self.drive(outcome);
        self.finish_print(event, &mut print)
    }

    /// Delivers settled futures to a `ResolveFutures` suspension, or one
    /// settled coroutine to the function call that allowed an eager reply.
    fn handle_resume_futures(&mut self, resume: pb::ResumeFutures, sink: &mut dyn EventSink) -> pb::ChildEvent {
        let results = match future_results_from_proto(resume.results, resume.values) {
            Ok(results) => results,
            Err(err) => return protocol_violation(&format!("invalid results: {err}")),
        };
        // Shaped against the borrowed state, so a rejected reply leaves the suspension intact.
        let SessionState::Suspended(progress) = &self.state else {
            return protocol_violation("ResumeFutures without suspended futures");
        };
        let reply = match progress.as_ref() {
            ReplProgress::FunctionCall(call) if call.allow_eager_await => match eager_result(results, call.call_id) {
                Ok(result) => FuturesReply::Eager(result),
                Err(message) => return protocol_violation(message),
            },
            ReplProgress::OsCall(call) if call.allow_eager_await => match eager_result(results, call.call_id) {
                Ok(result) => FuturesReply::Eager(result),
                Err(message) => return protocol_violation(message),
            },
            ReplProgress::ResolveFutures(_) => FuturesReply::Batch(results),
            _ => return protocol_violation("ResumeFutures without suspended futures"),
        };
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new(sink, self.print_flush_interval, self.receiver_allocation_profile);
        let outcome = match (*progress, reply) {
            (ReplProgress::FunctionCall(call), FuturesReply::Eager(result)) => {
                call.resume_eager(result, PrintWriter::Callback(&mut print))
            }
            (ReplProgress::OsCall(call), FuturesReply::Eager(result)) => {
                call.resume_eager(result, PrintWriter::Callback(&mut print))
            }
            (ReplProgress::ResolveFutures(state), FuturesReply::Batch(results)) => {
                state.resume(results, PrintWriter::Callback(&mut print))
            }
            _ => unreachable!("reply shaped by the suspension above"),
        };
        let event = self.drive(outcome);
        self.finish_print(event, &mut print)
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
            // the feed is not yet part of the session, so the dump could not carry it
            SessionState::AwaitingStubs(_) => {
                return error_event(
                    ExcType::RuntimeError,
                    "cannot dump while a feed awaits its module stubs",
                );
            }
            SessionState::Configured(_) => unreachable!("ensure_repl materialized the repl or errored"),
        };
        match dump(&self.script_name, self.type_check.as_ref(), &self.module_stubs, session) {
            Ok(state) => event(pb::child_event::Kind::DumpResult(pb::DumpResult {
                state: state.into(),
            })),
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
        let limits = match &self.state {
            SessionState::Configured(config) => {
                ResourceLimits::from(config.as_ref().and_then(|config| config.limits).unwrap_or_default())
            }
            _ => return protocol_violation("Load requires a session that has not started (a feed has already run)"),
        };
        let restored = match Dump::load(&load.state) {
            Ok(restored) => restored,
            Err(err) => return protocol_violation(&format!("failed to load session: {err}")),
        };
        let Dump {
            script_name,
            type_check,
            module_stubs,
            mut state,
        } = restored;
        // Retain snapshot clocks and stricter limits across both idle and suspended restores.
        match &mut state {
            Session::Idle(repl) => repl.tracker_mut().tighten_limits(&limits),
            Session::Suspended(progress) => progress.tracker_mut().tighten_limits(&limits),
            Session::Running(_) => {}
        }
        let (next_state, mut event) = match state {
            Session::Idle(repl) => (SessionState::Ready(repl), ok_event()),
            Session::Running(_) => return protocol_violation("dump holds a one-shot run, not a repl session"),
            Session::Suspended(progress) => match *progress {
                ReplProgress::Complete { repl, value } => (SessionState::Ready(Box::new(repl)), complete_event(value)),
                mut progress => {
                    let event = suspension_event(&mut progress);
                    (SessionState::Suspended(Box::new(progress)), event)
                }
            },
        };
        event.restored_script_name = Some(script_name.clone());
        match &next_state {
            SessionState::Ready(repl) => stamp_budget(&mut event, repl.tracker()),
            SessionState::Suspended(progress) => stamp_budget(&mut event, progress.tracker()),
            _ => unreachable!("load only creates ready or suspended sessions"),
        }
        if let Err(rejection) = admit_event(&event, self.receiver_allocation_profile.as_ref()) {
            return self.rejection_event(rejection, true);
        }
        self.state = next_state;
        self.script_name = script_name;
        self.module_stubs = module_stubs;
        self.type_check = type_check;
        event
    }

    /// Runs until a turn-ending event. OS calls not answered by `OsPolicy`
    /// go to the parent, including all filesystem I/O.
    fn drive(&mut self, result: Result<ReplProgress, Box<ReplStartError>>) -> pb::ChildEvent {
        match result {
            Ok(ReplProgress::Complete { repl, value }) => {
                self.state = SessionState::Ready(Box::new(repl));
                if let Some(state) = &mut self.type_check
                    && let Some(snippet) = state.pending_snippet.take()
                {
                    // The stubs' star import does not re-export imports. Keep them separately,
                    // deduplicated and ordered so the latest binding wins.
                    let imports = mem::take(&mut state.pending_imports);
                    let mut committed: Vec<&str> = state.committed_imports.lines().collect();
                    for import in imports.lines() {
                        committed.retain(|line| *line != import);
                        committed.push(import);
                    }
                    let mut joined = committed.join("\n");
                    if !joined.is_empty() {
                        joined.push('\n');
                    }
                    state.committed_imports = joined;
                    state.committed_stubs.push('\n');
                    state.committed_stubs.push_str(&snippet);
                }
                complete_event(value)
            }
            Ok(mut progress) => {
                let mut event = suspension_event(&mut progress);
                stamp_budget(&mut event, progress.tracker());
                if let Err(rejection) = admit_event(&event, self.receiver_allocation_profile.as_ref()) {
                    self.state = SessionState::Ready(Box::new(progress.into_repl()));
                    self.discard_pending_snippet();
                    self.rejection_event(rejection, true)
                } else {
                    self.state = SessionState::Suspended(Box::new(progress));
                    event
                }
            }
            Err(err) => {
                // Python-level failure: the session always survives
                self.state = SessionState::Ready(Box::new(err.repl));
                self.discard_pending_snippet();
                error_event_from(&err.error)
            }
        }
    }

    /// Checks against accumulated stubs and imports, returning an event on failure.
    fn type_check_feed(&mut self, code: &str) -> Option<pb::ChildEvent> {
        let state = self.type_check.as_ref()?;
        // `check_source` already ran the lexer scan, so only the AST walk is left.
        if let Err(error) = type_check_nesting_exception(code, &self.script_name, usize::MAX) {
            return Some(event(pb::child_event::Kind::Error(pb::Error {
                exception: Some((&error).into()),
            })));
        }
        let stubs =
            (!state.committed_stubs.is_empty()).then(|| SourceFile::new(&state.committed_stubs, "repl_type_stubs.pyi"));
        let context = TypeCheckContext {
            stubs: stubs.as_ref(),
            module_stubs: &self.module_stubs,
            prelude: &state.committed_imports,
        };
        let type_checker = self.type_checker.get_or_insert_with(TypeChecker::default);
        match type_checker.run_with(&SourceFile::new(code, &self.script_name), &context, state.config) {
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
        // Invalidating source files leaves Salsa caches and interned data allocated.
        self.type_checker = None;
        self.type_check = None;
        self.script_name = String::new();
        self.print_flush_interval = DEFAULT_PRINT_FLUSH_INTERVAL;
        self.os_policy = OsPolicy::default();
        self.module_stubs = Vec::new();
        self.receiver_allocation_profile = None;
        self.print_transport_error = None;
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
            traceback: BudgetVec::new(),
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
            traceback: BudgetVec::new(),
            data: None,
        }),
    }))
}

/// Preserves the exception's traceback in the error event.
fn error_event_from(exc: &MontyException) -> pb::ChildEvent {
    event(pb::child_event::Kind::Error(pb::Error {
        exception: Some(exc.into()),
    }))
}

/// Validates a dict of dotted module paths to `.pyi` source or `None`.
/// Paths must belong to a requested module; errors are protocol-violation messages.
fn stubs_from_answer(answer: &MontyObject, asked: &[String]) -> Result<Vec<ModuleStub>, String> {
    let answer = answer.as_ref();
    let pairs = match answer.pairs() {
        Some(pairs) if answer.type_name() == "dict" => pairs,
        _ => return Err("import_stubs must be answered with a dict of module to stub source".to_owned()),
    };
    let mut stubs = Vec::new();
    for (key, value) in pairs {
        let Some(module) = key.as_str() else {
            return Err("import_stubs answer keys must be module names".to_owned());
        };
        let top_level = module.split('.').next().unwrap_or_default();
        if !asked.iter().any(|name| name == top_level) {
            return Err(format!("import_stubs answered for {module:?}, which was not asked for"));
        }
        if let Some(source) = value.as_str() {
            stubs.push(ModuleStub::new(module, source).map_err(|err| format!("import_stubs: {err}"))?);
        } else if value.type_name() != "NoneType" {
            return Err(format!("import_stubs answer for {module:?} must be a str or None"));
        }
    }
    Ok(stubs)
}

/// Stamps `tracker`'s timing and parent-enforced limits onto an event. Called
/// again by [`Child::handle`] just before sending, so a suspension announcement
/// is size-checked with the stamps it will carry.
fn stamp_budget(event: &mut pb::ChildEvent, tracker: &ResourceTracker) {
    event.total_execution_micros = u64::try_from(tracker.elapsed().as_micros()).unwrap_or(u64::MAX);
    event.feed_execution_micros = u64::try_from(tracker.feed_elapsed().as_micros()).unwrap_or(u64::MAX);
    event.max_feed_duration_micros = micros_field(tracker.max_feed_duration());
    event.max_turn_duration_micros = micros_field(tracker.max_turn_duration());
    event.max_total_sleep_micros = micros_field(tracker.max_total_sleep());
    event.max_suspensions = Some(tracker.max_suspensions() as u64);
}

/// Describes a suspension announcement that would exceed the wire frame limit.
///
/// The child turns this into a host-visible error before entering the
/// suspension, because the parent cannot resume a call it never received.
/// A local rejection is distinct from a failed transport write.
#[derive(Debug, Clone, Copy)]
enum EventRejection {
    DecodeBudget,
    FrameTooLarge { len: u32 },
}

impl EventRejection {
    /// Keeps the established wire-limit diagnostic when the receiver can admit it.
    fn message(self, suspension: bool) -> String {
        match self {
            Self::DecodeBudget => DECODE_BUDGET_ERROR.to_owned(),
            Self::FrameTooLarge { len } => {
                let payload = if suspension { "argument" } else { "result" };
                format!("{payload} frame of {len} bytes exceeds the maximum of {MAX_FRAME_LEN} bytes")
            }
        }
    }
}

/// Checks the finalized envelope before encoding or committing a suspension or load.
fn admit_event(event: &pb::ChildEvent, profile: Option<&pb::ReceiverAllocationProfile>) -> Result<(), EventRejection> {
    if let Some(profile) = profile
        && canonical_event_decode_bytes(event, profile).is_none_or(|cost| cost > profile.max_decode_bytes)
    {
        return Err(EventRejection::DecodeBudget);
    }
    if let Some(len) = exceeds_max_frame_len(event) {
        return Err(EventRejection::FrameTooLarge { len });
    }
    Ok(())
}

/// Builds the suspension event for a `FunctionCall`, **moving** the
/// arguments into it.
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
    event(pb::child_event::Kind::FunctionCall(WireFunctionCall::new(
        call.function_name.clone(),
        mem::take(&mut call.args),
        call.call_id,
        call.object_id,
        call.allow_eager_await,
        call.position.clone(),
    )))
}

/// Builds the suspension event for an `OsCall`, **moving** the call payload
/// into it — see
/// [`suspension_event_function_call`] for why, and
/// [`Child::reclaim_suspension_payload`] for how it comes back. `GetEnviron` is
/// the placeholder left behind: a unit variant, so the swap allocates nothing.
fn suspension_event_os_call(call: &mut monty::ReplOsCall) -> pb::ChildEvent {
    let function_call = mem::replace(&mut call.function_call, OsFunctionCall::GetEnviron);
    event(pb::child_event::Kind::OsCall(os_call_to_proto(
        call.call_id,
        function_call,
        call.allow_eager_await,
        &call.position,
    )))
}

fn complete_event(value: MontyObject) -> pb::ChildEvent {
    event(pb::child_event::Kind::Complete(value.into()))
}

/// Builds the suspension event for a non-`Complete` progress state. Used on
/// `Load` to re-announce a restored suspension; fresh suspensions go through
/// `drive`, which stamps and admits the event before storing the suspension.
fn suspension_event(progress: &mut ReplProgress) -> pb::ChildEvent {
    match progress {
        ReplProgress::FunctionCall(call) => suspension_event_function_call(call),
        ReplProgress::OsCall(call) => suspension_event_os_call(call),
        ReplProgress::NameLookup(lookup) => event(pb::child_event::Kind::NameLookup(pb::NameLookup {
            name: lookup.name.clone(),
            object_id: lookup.object_id().as_ref().map(uuid_to_pb),
            position: Some((&lookup.position).into()),
        })),
        ReplProgress::ResolveFutures(state) => event(pb::child_event::Kind::ResolveFutures(pb::ResolveFutures {
            pending_call_ids: state.pending_call_ids().to_vec().into(),
            position: Some(state.position().into()),
        })),
        ReplProgress::Complete { .. } => unreachable!("Complete is handled before suspension_event"),
    }
}

/// A validated `ResumeFutures` body, shaped for the suspension it answers.
enum FuturesReply {
    /// One settled coroutine for a call with `allow_eager_await`.
    Eager(Result<MontyObject, MontyException>),
    /// Results for a `ResolveFutures` suspension.
    Batch(Vec<(u32, ExtFunctionResult)>),
}

/// Checks an eager reply is exactly one settled result for `call_id`; the
/// error is the protocol-violation message.
fn eager_result(
    results: Vec<(u32, ExtFunctionResult)>,
    call_id: u32,
) -> Result<Result<MontyObject, MontyException>, &'static str> {
    match <[_; 1]>::try_from(results) {
        Ok([(id, ExtFunctionResult::Return(value))]) if id == call_id => Ok(Ok(value)),
        Ok([(id, ExtFunctionResult::Error(exc))]) if id == call_id => Ok(Err(exc)),
        Ok([(id, _)]) if id == call_id => Err("eager coroutine must resolve to a value or exception"),
        _ => Err("eager ResumeFutures must contain exactly the suspended call id"),
    }
}

/// Streams sandbox `print()` output as `Print` events through an
/// [`EventSink`].
///
/// Debounced: a frame is written once the buffer reaches
/// [`Self::FLUSH_BYTES`] or its oldest byte has waited out `interval`, so the
/// number of events a turn produces follows elapsed time and output volume
/// rather than how many times the program called `print()` — which is what
/// makes a loop of tiny prints cost the parent (and its telemetry) roughly
/// what one large print costs. [`Self::drain`] empties the buffer before every
/// turn-ending event, so ordering against suspensions stays exact.
///
/// Output on both streams shares the buffer, held as one segment per run, so
/// alternating between them batches like any other output and still reaches
/// the parent in the order the sandbox produced it.
///
/// A zero `interval` disables the timer and restores line buffering, one event
/// per completed line.
struct ProtoPrint<'a> {
    /// Buffered output, one segment per run on a single stream.
    segments: Vec<pb::PrintSegment>,
    /// Text bytes held across `segments`, so the size check stays O(1).
    buffered_bytes: usize,
    sink: &'a mut dyn EventSink,
    /// How long the oldest buffered byte may wait; zero means line buffering.
    interval: Duration,
    /// When the buffer stopped being empty; `None` while it is empty.
    buffered_since: Option<Instant>,
    receiver_allocation_profile: Option<pb::ReceiverAllocationProfile>,
    transport_error: Option<FrameError>,
}

impl<'a> ProtoPrint<'a> {
    /// Flush threshold for output that never reaches the interval.
    const FLUSH_BYTES: usize = 8 * 1024;

    fn new(
        sink: &'a mut dyn EventSink,
        interval: Duration,
        receiver_allocation_profile: Option<pb::ReceiverAllocationProfile>,
    ) -> Self {
        Self {
            segments: Vec::new(),
            buffered_bytes: 0,
            sink,
            interval,
            buffered_since: None,
            receiver_allocation_profile,
            transport_error: None,
        }
    }

    /// Writes everything buffered (if anything) as one `Print` event.
    fn flush(&mut self) -> Result<(), MontyException> {
        if self.segments.is_empty() {
            return Ok(());
        }
        self.buffered_since = None;
        self.buffered_bytes = 0;
        let segments = mem::take(&mut self.segments);
        self.send(segments)
    }

    /// Emits one `Print` event per completed line, leaving any trailing
    /// partial line buffered. With the timer off the contract is one event per
    /// completed line, so a single write carrying embedded newlines has to be
    /// split rather than shipped whole; a line spanning a stream switch keeps
    /// its runs together in one event.
    fn flush_lines(&mut self) -> Result<(), MontyException> {
        while let Some((index, end)) = self.first_line_end() {
            let mut line: Vec<pb::PrintSegment> = self.segments.drain(..index).collect();
            let rest = self.segments[0].text.split_off(end);
            line.push(pb::PrintSegment {
                stream: self.segments[0].stream,
                text: mem::replace(&mut self.segments[0].text, rest),
            });
            if self.segments[0].text.is_empty() {
                self.segments.remove(0);
            }
            self.buffered_bytes -= line.iter().map(|segment| segment.text.len()).sum::<usize>();
            if let Err(error) = self.send(line) {
                self.segments.clear();
                self.buffered_bytes = 0;
                self.buffered_since = None;
                return Err(error);
            }
        }
        if self.segments.is_empty() {
            self.buffered_since = None;
        }
        Ok(())
    }

    /// Where the first buffered line ends: the segment holding the `\n`, and
    /// the offset just past it. `None` while no line is complete.
    fn first_line_end(&self) -> Option<(usize, usize)> {
        self.segments
            .iter()
            .enumerate()
            .find_map(|(index, segment)| segment.text.find('\n').map(|at| (index, at + 1)))
    }

    /// Sends `segments` as one `Print` event.
    fn send(&mut self, segments: Vec<pb::PrintSegment>) -> Result<(), MontyException> {
        let event = event(pb::child_event::Kind::Print(pb::Print {
            segments: segments.into(),
        }));
        admit_event(&event, self.receiver_allocation_profile.as_ref())
            .map_err(|rejection| MontyException::new(ExcType::RuntimeError, Some(rejection.message(false))))?;
        self.sink.send(&event).map_err(|error| {
            let message = format!("failed to stream print output: {error}");
            if self.transport_error.is_none() {
                self.transport_error = Some(error);
            }
            MontyException::new(ExcType::RuntimeError, Some(message))
        })
    }

    /// Flushes whatever the buffer has earned: complete lines when the timer
    /// is off, then a frame if it has filled or its oldest byte has waited out
    /// `interval`.
    fn maybe_flush(&mut self) -> Result<(), MontyException> {
        // Lines leave first so the size threshold below cannot merge a
        // multi-line write back into one frame.
        if self.interval.is_zero() {
            self.flush_lines()?;
        }
        if self.buffered_bytes >= Self::FLUSH_BYTES || (!self.interval.is_zero() && self.interval_elapsed()) {
            self.flush()
        } else {
            Ok(())
        }
    }

    /// Whether buffered output has waited out `interval`. False when the
    /// buffer is empty, so an idle writer never reads the clock twice.
    fn interval_elapsed(&self) -> bool {
        self.buffered_since
            .is_some_and(|since| since.elapsed() >= self.interval)
    }

    /// Starts the interval clock when the buffer leaves the empty state.
    fn mark_buffered(&mut self) {
        if self.buffered_since.is_none() {
            self.buffered_since = Some(Instant::now());
        }
    }

    /// Flushes whatever is buffered; called before every turn-ending event.
    /// A local rejection must end the feed even when execution already completed.
    fn drain(&mut self) -> Result<(), MontyException> {
        self.flush()
    }

    /// Buffers `text`, extending the trailing segment while it is on the same
    /// stream and starting a new one when the stream changes.
    fn append(&mut self, stream: PrintStream, text: &str) {
        self.mark_buffered();
        self.buffered_bytes += text.len();
        let stream = wire_stream(stream);
        match self.segments.last_mut() {
            Some(segment) if segment.stream == stream => segment.text.push_str(text),
            _ => self.segments.push(pb::PrintSegment {
                stream,
                text: text.to_owned(),
            }),
        }
    }

    fn write(&mut self, stream: PrintStream, output: &str) -> Result<(), MontyException> {
        // Append in pieces no larger than the flush threshold so a single huge
        // write cannot inflate the buffer (and the untracked copy it holds)
        // past `FLUSH_BYTES`: each filled chunk is flushed before the next is
        // appended.
        let mut rest = output;
        while !rest.is_empty() {
            let take = floor_char_boundary(rest, Self::FLUSH_BYTES - self.buffered_bytes);
            if take == 0 {
                // not even one char fits in the remaining room; flush to free
                // the whole threshold (far larger than any single char)
                self.flush()?;
                continue;
            }
            self.append(stream, &rest[..take]);
            rest = &rest[take..];
            self.maybe_flush()?;
        }
        Ok(())
    }

    fn push(&mut self, stream: PrintStream, end: char) -> Result<(), MontyException> {
        self.append(stream, end.encode_utf8(&mut [0; 4]));
        self.maybe_flush()
    }
}

/// Maps the interpreter's stream tag onto its wire enum value.
fn wire_stream(stream: PrintStream) -> i32 {
    match stream {
        PrintStream::Stdout => i32::from(pb::PrintStream::Stdout),
        PrintStream::Stderr => i32::from(pb::PrintStream::Stderr),
    }
}

impl PrintWriterCallback for ProtoPrint<'_> {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.write(PrintStream::Stdout, &output)
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.push(PrintStream::Stdout, end)
    }

    fn stderr_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.write(PrintStream::Stderr, &output)
    }

    fn stderr_push(&mut self, end: char) -> Result<(), MontyException> {
        self.push(PrintStream::Stderr, end)
    }

    /// Releases output the program stopped writing to: without this a script
    /// that prints and then computes in silence would hold that line until its
    /// next print or the end of the turn. A zero `interval` has no timer to
    /// expire, so line buffering is left to decide flushes on its own.
    fn poll_flush(&mut self) -> Result<(), MontyException> {
        if !self.interval.is_zero() && self.interval_elapsed() {
            self.flush()
        } else {
            Ok(())
        }
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
