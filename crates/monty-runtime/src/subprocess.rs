//! `monty subprocess`: protocol child mode.
//!
//! Reads framed [`pb::ParentRequest`]s from stdin and writes framed [`pb::ChildEvent`]s
//! to stdout (see `monty-proto` for the schema and protocol rules). The child
//! is strictly turn-based: one request in, zero or more streamed `Print`
//! events out, then exactly one turn-ending event.
//!
//! Crash isolation is the entire point of this mode: the parent must treat a
//! child that exits (or EOFs) *without* a `FatalError` event as crashed —
//! stack overflows and allocator aborts produce no final frame.
//!
//! In this mode stdout carries only protocol frames; diagnostics go to
//! stderr.

use std::{borrow::Cow, io, mem, panic, process::ExitCode};

use monty::{
    ExcType, ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRepl, PrintWriter,
    PrintWriterCallback, ReplProgress, ReplStartError, fs::MountTable,
};
use monty_proto::{
    FrameReader, MAX_FRAME_LEN, MONTY_VERSION, WireFunctionCall, WireOsCall, build_mount_table,
    exceeds_max_value_depth, future_results_from_proto, pb, write_frame,
};
use monty_type_checking::{SourceFile, type_check};
use prost::Message;

/// The child always runs with `LimitedTracker`: an absent/empty limits message
/// behaves like `ResourceLimits::new()`, and a single tracker type keeps the
/// session state enum free of generics.
type Tracker = LimitedTracker;

/// Version tag of the opaque dump envelope produced by `Dump`.
///
/// Wire layout: `[DUMP_VERSION u16 LE][tag u8][session meta][postcard
/// payload]` where tag 0 is a `MontyRepl` (idle session) and tag 1 a
/// `ReplProgress` (suspended). The session meta carries the child-side state
/// that lives *outside* the repl — script name, accumulated type-check stubs,
/// and the in-flight feed's mount requirements — so a `Load`ed session keeps
/// type-check enforcement and can validate its mounts instead of silently
/// dropping them:
///
/// - `[script_name str][type_check u8]` and, when `type_check` is 1,
///   `[committed_stubs str][has_pending u8][pending_snippet str?]`, where each
///   `str` is a `u32 LE` byte length followed by UTF-8 bytes.
/// - mount requirements: `[count u32 LE]` then per entry `[virtual_path
///   str][mode i32 LE][has_limit u8][write_bytes_limit u64 LE?]`, recording the
///   feed's mounts *without* host paths so `Load` can verify the re-supply. The
///   count is 0 for an idle dump.
///
/// The payload is monty's postcard format — only a monty child of the same
/// version can restore it.
const DUMP_VERSION: u16 = 2;

/// Runs the subprocess child loop until EOF, `Shutdown`, or a fatal error.
pub(crate) fn run() -> ExitCode {
    install_panic_hook();
    let mut reader = FrameReader::new(io::stdin().lock());
    let mut child = Child::new();

    loop {
        match reader.read::<pb::ParentRequest>() {
            Ok(Some(request)) => match child.handle(request) {
                Ok(None) => {}
                Ok(Some(code)) => return code,
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
            // the frame arrived intact but its payload didn't decode — this
            // includes values failing semantic validation (bad dates, unknown
            // enum names), which happens during decode. The stream is still
            // in sync, so answer with a turn-ending error and keep serving.
            Err(monty_proto::FrameError::Decode(err)) => {
                if send(&violation(&format!("malformed request: {err}"))).is_err() {
                    return ExitCode::from(3);
                }
            }
            Err(err) => {
                // the stream is desynchronized — unrecoverable by design
                child.fatal(&format!("malformed request frame: {err}"));
                return ExitCode::from(2);
            }
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
/// and so must travel in the dump envelope explicitly: the script name, the
/// type-check state, and the in-flight feed's mount requirements. Serialized by
/// [`push_session_meta`] and parsed by [`take_session_meta`].
struct SessionMeta {
    script_name: String,
    type_check: Option<TypeCheckState>,
    mount_requirements: Vec<MountRequirement>,
}

/// The host-independent shape of one mount: everything in a `pb::Mount`
/// except the machine-specific `host_path`. Recorded in a suspended feed's
/// dump so `Load` can verify the parent re-supplied the *same* mounts (by
/// virtual path, mode, and write cap) before resuming — host paths are never
/// dumped, so a malicious dump cannot inject a mount of an arbitrary host
/// directory.
#[derive(Clone, PartialEq)]
struct MountRequirement {
    virtual_path: String,
    mode: i32,
    write_bytes_limit: Option<u64>,
}

impl MountRequirement {
    fn of(mount: &pb::Mount) -> Self {
        Self {
            virtual_path: mount.virtual_path.clone(),
            mode: mount.mode,
            write_bytes_limit: mount.write_bytes_limit,
        }
    }
}

/// All child state.
struct Child {
    state: SessionState,
    /// Script name of the current session (used for error and type-check
    /// diagnostics).
    script_name: String,
    /// Mount table for the in-flight feed; rebuilt per feed, dropped when the
    /// feed completes. Not part of dumps (mounts are host configuration, not
    /// sandbox state) — instead a `Load` resuming a suspended feed carries the
    /// mounts the parent re-supplies, and `handle_load` rebuilds the table from
    /// them after checking they match `mount_requirements`.
    mounts: Option<MountTable>,
    /// Host-independent shape of the in-flight feed's mounts, dumped alongside
    /// a suspended session so `Load` can validate the re-supplied mounts.
    /// Empty between feeds and for idle sessions.
    mount_requirements: Vec<MountRequirement>,
    /// `Some` when the session was created with `type_check: true`.
    type_check: Option<TypeCheckState>,
}

impl Child {
    fn new() -> Self {
        Self {
            state: SessionState::Configured(None),
            script_name: String::new(),
            mounts: None,
            mount_requirements: Vec::new(),
            type_check: None,
        }
    }

    /// Drops the in-flight feed's mount table and its recorded requirements.
    /// Called when a feed ends (completion, error, reset) so a later idle dump
    /// records no mount requirements.
    fn clear_feed_mounts(&mut self) {
        self.mounts = None;
        self.mount_requirements.clear();
    }

    /// Handles one request: emits exactly one turn-ending event and returns
    /// what the main loop should do next. `Err` means stdout is broken.
    fn handle(&mut self, request: pb::ParentRequest) -> Result<Option<ExitCode>, monty_proto::FrameError> {
        let Some(kind) = request.kind else {
            send(&violation("request has no kind"))?;
            return Ok(None);
        };

        let mut event = match kind {
            pb::parent_request::Kind::Configure(configure) => {
                // Version skew is fatal. The protocol has no in-band
                // negotiation and assumes parent and child are deployed in
                // lockstep; a mismatched build can have a different frame
                // layout, so we fail fast with a `FatalError` rather than risk
                // a silent desync. `fatal` sends the event; returning an exit
                // code stops the loop so the parent sees a clean cause.
                if configure.monty_version != MONTY_VERSION {
                    self.fatal(&format!(
                        "version skew: parent={:?} child={MONTY_VERSION:?}",
                        configure.monty_version
                    ));
                    return Ok(Some(ExitCode::from(4)));
                }
                self.handle_configure(configure)
            }
            pb::parent_request::Kind::Feed(feed) => self.handle_repl_feed(feed),
            // The Monty sandbox has no host interpreter to install packages for;
            // dependency installation is only supported by the CPython worker.
            // Answer with a session-preserving error rather than a hard failure.
            pb::parent_request::Kind::InstallDependencies(_) => error_event(
                ExcType::RuntimeError,
                "dependency installation is only supported by the CPython worker",
            ),
            pb::parent_request::Kind::ResumeCall(resume) => self.handle_resume_call(resume),
            pb::parent_request::Kind::ResumeNameLookup(resume) => self.handle_resume_name_lookup(resume),
            pb::parent_request::Kind::ResumeFutures(resume) => self.handle_resume_futures(resume),
            pb::parent_request::Kind::Dump(_) => self.handle_dump(),
            pb::parent_request::Kind::Load(load) => self.handle_load(load),
            pb::parent_request::Kind::Reset(_) => {
                self.reset();
                ok_event()
            }
            pb::parent_request::Kind::Shutdown(_) => {
                send(&ok_event())?;
                return Ok(Some(ExitCode::SUCCESS));
            }
        };
        self.stamp_execution_time(&mut event);
        if let Err(err) = send(&event) {
            self.recover_send_error(err)?;
        }
        Ok(None)
    }

    /// Recovers from a failure to write a turn-ending event.
    ///
    /// `write_frame` rejects an oversize frame *before* writing any bytes, so
    /// the stream stays synced. When the session is not mid-suspension — e.g.
    /// a `Complete` result that is merely larger than the frame limit — we can
    /// answer with a clean, session-preserving error and keep serving instead
    /// of crashing the worker. An oversize *suspension* announcement is
    /// unrecoverable (the worker is already suspended but the parent never
    /// learned the resume point), so it propagates to the run loop's fatal
    /// handling, as does any genuine I/O break.
    fn recover_send_error(&mut self, err: monty_proto::FrameError) -> Result<(), monty_proto::FrameError> {
        match err {
            monty_proto::FrameError::FrameTooLarge { len, max }
                if !matches!(self.state, SessionState::Suspended(_)) =>
            {
                let mut event = error_event(
                    ExcType::RuntimeError,
                    &format!("result frame of {len} bytes exceeds the maximum of {max} bytes"),
                );
                self.stamp_execution_time(&mut event);
                send(&event)
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
            return violation("Configure while a session already exists");
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
            return Err(Box::new(violation("session has not been configured")));
        };
        let pb::Configure {
            script_name,
            limits,
            type_check,
            type_check_stubs,
            // already validated against our own version when `Configure` arrived
            monty_version: _,
        } = *config;
        let limits = limits.unwrap_or_default().into();
        self.script_name = script_name;
        self.type_check = type_check.then(|| TypeCheckState {
            committed_stubs: type_check_stubs.unwrap_or_default(),
            pending_snippet: None,
        });
        self.state = SessionState::Ready(Box::new(MontyRepl::new(&self.script_name, LimitedTracker::new(limits))));
        Ok(())
    }

    fn handle_repl_feed(&mut self, feed: pb::Feed) -> pb::ChildEvent {
        if let Err(event) = self.ensure_repl() {
            return *event;
        }
        if !matches!(self.state, SessionState::Ready(_)) {
            // ensure_repl left it un-Ready only when mid-suspension
            return violation("Feed without a session ready for input");
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
        // record the host-independent mount shape before consuming the specs,
        // so a dump taken mid-feed can make `Load` validate the re-supply
        let requirements = feed.mounts.iter().map(MountRequirement::of).collect();
        self.mounts = match build_mount_table(feed.mounts) {
            Ok(mounts) => mounts,
            Err(err) => return violation(&format!("invalid mounts: {err}")),
        };
        self.mount_requirements = requirements;
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
        let mut print = ProtoPrint::new();
        let result = repl.feed_start(&feed.code, inputs, PrintWriter::Callback(&mut print));
        let event = self.drive(result, &mut print);
        print.drain();
        event
    }

    fn handle_resume_call(&mut self, resume: pb::ResumeCall) -> pb::ChildEvent {
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
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new();
        let outcome = match *progress {
            ReplProgress::FunctionCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            ReplProgress::OsCall(call) => call.resume(result, PrintWriter::Callback(&mut print)),
            _ => unreachable!("checked above"),
        };
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    fn handle_resume_name_lookup(&mut self, resume: pb::ResumeNameLookup) -> pb::ChildEvent {
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
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let ReplProgress::NameLookup(lookup) = *progress else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new();
        let outcome = lookup.resume(result, PrintWriter::Callback(&mut print));
        let event = self.drive(outcome, &mut print);
        print.drain();
        event
    }

    fn handle_resume_futures(&mut self, resume: pb::ResumeFutures) -> pb::ChildEvent {
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
        let SessionState::Suspended(progress) = mem::replace(&mut self.state, SessionState::Configured(None)) else {
            unreachable!("checked above");
        };
        let ReplProgress::ResolveFutures(state) = *progress else {
            unreachable!("checked above");
        };
        let mut print = ProtoPrint::new();
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
                push_session_meta(
                    &mut state,
                    &self.script_name,
                    self.type_check.as_ref(),
                    &self.mount_requirements,
                );
                state.extend_from_slice(&payload);
                event(pb::child_event::Kind::DumpResult(pb::DumpResult { state }))
            }
            Err(err) => violation(&format!("dump failed: {err}")),
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
            return violation("Load requires a session that has not started (a feed has already run)");
        }
        let pb::Load { state, mounts } = load;
        let Some((version_bytes, rest)) = state.split_at_checked(2) else {
            return violation("dump state too short");
        };
        let version = u16::from_le_bytes([version_bytes[0], version_bytes[1]]);
        if version != DUMP_VERSION {
            return violation(&format!("unsupported dump version {version} (expected {DUMP_VERSION})"));
        }
        let Some((&tag, rest)) = rest.split_first() else {
            return violation("dump state too short");
        };
        let Some((meta, payload)) = take_session_meta(rest) else {
            return violation("malformed dump session metadata");
        };
        let SessionMeta {
            script_name,
            type_check,
            mount_requirements: requirements,
        } = meta;
        if let Err(message) = validate_supplied_mounts(&requirements, &mounts) {
            return error_event(ExcType::RuntimeError, &message);
        }
        let mount_table = match build_mount_table(mounts) {
            Ok(table) => table,
            Err(err) => return violation(&format!("invalid mounts: {err}")),
        };
        let mut event = match tag {
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
                    complete_event(value)
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
        // a resumed feed re-establishes the mounts the parent re-supplied; an
        // idle restore leaves `self.mounts` untouched (the next feed sets it)
        if matches!(self.state, SessionState::Suspended(_)) {
            self.mounts = mount_table;
            self.mount_requirements = requirements;
        }
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

    /// Drives execution until it needs the parent: handles mount-covered OS
    /// calls locally and returns the turn-ending event for everything else.
    fn drive(
        &mut self,
        mut result: Result<ReplProgress<Tracker>, Box<ReplStartError<Tracker>>>,
        print: &mut ProtoPrint,
    ) -> pb::ChildEvent {
        loop {
            match result {
                Ok(ReplProgress::Complete { repl, value }) => {
                    self.state = SessionState::Ready(Box::new(repl));
                    self.clear_feed_mounts();
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
                    let event = event(pb::child_event::Kind::OsCall(WireOsCall {
                        function_name: name.to_owned(),
                        args,
                        kwargs,
                        call_id,
                        not_handled_error: Some((&not_handled_error).into()),
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
                    self.clear_feed_mounts();
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
        self.clear_feed_mounts();
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
            Err(err) => Some(violation(&format!("type checker failed: {err}"))),
        }
    }

    /// Drops all session state, returning to the unconfigured state ready for
    /// the next `Configure` (or `Load`).
    fn reset(&mut self) {
        self.state = SessionState::Configured(None);
        self.clear_feed_mounts();
        self.type_check = None;
        self.script_name = String::new();
    }

    /// Best-effort `FatalError` event, duplicated to stderr. Used only for
    /// unrecoverable conditions — the child exits right after.
    fn fatal(&self, message: &str) {
        eprintln!("monty subprocess fatal error: {message}");
        let mut fatal_event = event(pb::child_event::Kind::FatalError(pb::FatalError {
            message: message.to_owned(),
        }));
        // fatal paths bypass `handle`, so stamp timing here to keep the
        // "every turn-ending event carries timing" contract intact
        self.stamp_execution_time(&mut fatal_event);
        let _ = send(&fatal_event);
    }
}

/// Writes one event frame to stdout.
///
/// Framing is stateless and `Stdout` handles share one global buffer, so a
/// fresh handle per write is safe.
fn send(event: &pb::ChildEvent) -> Result<(), monty_proto::FrameError> {
    write_frame(&mut io::stdout(), event)
}

/// Wraps an event kind into an `Event` with zeroed timing fields;
/// `Child::handle` (and `Child::fatal`) stamps the timing fields onto every
/// turn-ending event just before it is sent.
fn event(kind: pb::child_event::Kind) -> pb::ChildEvent {
    pb::ChildEvent {
        kind: Some(kind),
        ..Default::default()
    }
}

/// Builds the turn-ending event for a recoverable protocol violation (wrong
/// state, bad call id, invalid payload). The child's state is unchanged.
fn violation(message: &str) -> pb::ChildEvent {
    event(pb::child_event::Kind::Error(pb::Error {
        exception: Some(pb::RaisedException {
            exc_type: ExcType::RuntimeError.to_string(),
            message: Some(format!("protocol violation: {message}")),
            traceback: vec![],
            data: None,
        }),
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
            // `Used`, whose `name()` would panic) — the parent re-learns the
            // name/args from its own records; a fresh suspension goes through
            // `drive` instead
            let has_payload = !matches!(call.function_call, monty::OsFunctionCall::Used);
            pb::child_event::Kind::OsCall(WireOsCall {
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

/// Appends the session metadata (script name + type-check state + mount
/// requirements, see [`DUMP_VERSION`]) to a dump envelope.
fn push_session_meta(
    buf: &mut Vec<u8>,
    script_name: &str,
    type_check: Option<&TypeCheckState>,
    mount_requirements: &[MountRequirement],
) {
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
    push_mount_requirements(buf, mount_requirements);
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
    let (mount_requirements, rest) = take_mount_requirements(rest)?;
    Some((
        SessionMeta {
            script_name,
            type_check,
            mount_requirements,
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

/// Appends the in-flight feed's mount requirements to a dump envelope (see
/// [`DUMP_VERSION`]). Host paths are deliberately excluded.
fn push_mount_requirements(buf: &mut Vec<u8>, requirements: &[MountRequirement]) {
    let count = u32::try_from(requirements.len()).expect("mount count exceeds u32::MAX");
    buf.extend_from_slice(&count.to_le_bytes());
    for req in requirements {
        push_str_field(buf, &req.virtual_path);
        buf.extend_from_slice(&req.mode.to_le_bytes());
        match req.write_bytes_limit {
            Some(limit) => {
                buf.push(1);
                buf.extend_from_slice(&limit.to_le_bytes());
            }
            None => buf.push(0),
        }
    }
}

/// Splits the mount requirements off the front of a dump envelope, returning
/// them with the remaining postcard payload. `None` means the envelope is
/// malformed. The count comes from untrusted bytes, so entries are pushed
/// without pre-reserving capacity (a bogus count simply runs out of bytes).
fn take_mount_requirements(bytes: &[u8]) -> Option<(Vec<MountRequirement>, &[u8])> {
    let (count_bytes, mut rest) = bytes.split_at_checked(4)?;
    let count = u32::from_le_bytes(count_bytes.try_into().ok()?);
    let mut requirements = Vec::new();
    for _ in 0..count {
        let (virtual_path, after_path) = take_str_field(rest)?;
        let (mode_bytes, after_mode) = after_path.split_at_checked(4)?;
        let mode = i32::from_le_bytes(mode_bytes.try_into().ok()?);
        let (&has_limit, after_flag) = after_mode.split_first()?;
        let (write_bytes_limit, after_limit) = match has_limit {
            0 => (None, after_flag),
            1 => {
                let (limit_bytes, after) = after_flag.split_at_checked(8)?;
                (Some(u64::from_le_bytes(limit_bytes.try_into().ok()?)), after)
            }
            _ => return None,
        };
        requirements.push(MountRequirement {
            virtual_path,
            mode,
            write_bytes_limit,
        });
        rest = after_limit;
    }
    Some((requirements, rest))
}

/// Checks that the mounts the parent re-supplied to `Load` match the suspended
/// feed's recorded requirements exactly (by virtual path, mode, and write cap;
/// host paths may differ). Returns a human-readable error describing the first
/// discrepancy — a missing, extra, or altered mount — so a forgotten re-supply
/// fails loudly instead of silently dropping the feed's mounts.
fn validate_supplied_mounts(required: &[MountRequirement], supplied: &[pb::Mount]) -> Result<(), String> {
    for req in required {
        match supplied.iter().find(|m| m.virtual_path == req.virtual_path) {
            None => {
                return Err(format!(
                    "the dump was suspended with a mount at {:?} that was not re-supplied to load; \
                     pass the same mounts the original feed used",
                    req.virtual_path
                ));
            }
            Some(m) if m.mode != req.mode || m.write_bytes_limit != req.write_bytes_limit => {
                return Err(format!(
                    "the re-supplied mount at {:?} does not match the dump (mount mode or write limit differs)",
                    req.virtual_path
                ));
            }
            Some(_) => {}
        }
    }
    for mount in supplied {
        if !required.iter().any(|req| req.virtual_path == mount.virtual_path) {
            return Err(format!(
                "a mount at {:?} was supplied to load but the dump's feed had no such mount",
                mount.virtual_path
            ));
        }
    }
    Ok(())
}

/// Converts wire named inputs into `(name, value)` pairs for `feed_start`.
fn named_inputs(inputs: Vec<pb::NamedValue>) -> Result<Vec<(String, MontyObject)>, Box<pb::ChildEvent>> {
    inputs
        .into_iter()
        .map(|input| {
            let value = input
                .value
                .ok_or_else(|| Box::new(violation(&format!("input {:?} has no value", input.name))))?;
            let value = value
                .into_object()
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
    buf: String,
}

impl ProtoPrint {
    /// Flush threshold for output that never produces a newline.
    const FLUSH_BYTES: usize = 8 * 1024;

    fn new() -> Self {
        Self { buf: String::new() }
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
        send(&event).map_err(|err| {
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

/// Installs a panic hook that emits a best-effort `FatalError` frame before
/// the default unwind, giving the parent a parseable last gasp for ordinary
/// panics. Hard crashes (stack overflow, allocator abort) bypass this — the
/// parent's contract is "exit without FatalError == crash".
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // stdout's lock is reentrant on the same thread, and if the panic
        // interrupted a write its buffer may hold a partial frame we cannot
        // complete — a corrupt tail is fine, the parent already treats it as
        // a crash
        let _ = send(&event(pb::child_event::Kind::FatalError(pb::FatalError {
            message: format!("child panicked: {info}"),
        })));
        default_hook(info);
    }));
}
