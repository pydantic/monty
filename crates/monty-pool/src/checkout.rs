//! A checked-out worker: one REPL session, driven turn by turn.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use monty::{
    AssertMessageAnnotations, ExcType, MontyException, MontyObject, OsFunctionCall, PrintStream, ResourceLimits,
};
use monty_fs::{MountCallOutcome, MountMode, MountTable, OverlayState};
use monty_proto::{FrameError, MONTY_VERSION, exceeds_max_value_depth, pb, validate_requirement};

use crate::{PoolError, pool::PoolInner, watchdog::DeadlineGuard, worker::Worker};

/// Arguments for the REPL session a checkout creates — mirrors
/// `MontyRepl`'s constructor surface.
#[derive(Debug, Clone)]
pub struct ReplConfig {
    /// Script name used in tracebacks and type-check diagnostics.
    pub script_name: String,
    /// Sandbox resource limits enforced inside the worker. `None` means
    /// unlimited (except monty's standard recursion-depth default).
    pub limits: Option<ResourceLimits>,
    /// Type-check every fed snippet before executing it.
    pub type_check: bool,
    /// Stub declarations made available to type checking.
    pub type_check_stubs: Option<String>,
    /// Give failed `assert` statements pytest-style introspected messages
    /// (see `limitations/assert.md`). On by default with a 120-byte
    /// operand-repr truncation; `MaxBytes` customizes the truncation.
    pub assert_message_annotations: AssertMessageAnnotations,
}

impl Default for ReplConfig {
    fn default() -> Self {
        Self {
            script_name: "main.py".to_owned(),
            limits: None,
            type_check: false,
            type_check_stubs: None,
            assert_message_annotations: AssertMessageAnnotations::default(),
        }
    }
}

/// A host directory mounted into the sandbox for one feed. Mounts are handled
/// entirely on the parent: the checkout services covered filesystem OS calls
/// from the host path itself (so mounts work even when the worker runs on a
/// remote machine), and OS calls the mounts don't cover surface as
/// [`TurnEvent::OsCall`].
#[derive(Debug, Clone)]
pub struct MountSpec {
    /// Absolute virtual POSIX path inside the sandbox, e.g. `/mnt/data`.
    pub virtual_path: String,
    /// Host directory to expose.
    pub host_path: PathBuf,
    /// Access mode.
    pub mode: MountSpecMode,
    /// Cap on total bytes written through this mount.
    pub write_bytes_limit: Option<u64>,
    /// Aggregate budget for retained overlay data and transient results.
    pub memory_usage_limit: u64,
}

impl MountSpec {
    /// Creates mount configuration with the default 100 MB memory budget and
    /// no cumulative write limit.
    #[must_use]
    pub fn new(virtual_path: String, host_path: PathBuf, mode: MountSpecMode) -> Self {
        Self {
            virtual_path,
            host_path,
            mode,
            write_bytes_limit: None,
            memory_usage_limit: monty_fs::DEFAULT_MEMORY_USAGE_LIMIT,
        }
    }
}

/// Access mode for a [`MountSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountSpecMode {
    ReadOnly,
    ReadWrite,
    /// Copy-on-write overlay in parent memory; writes are discarded when the
    /// feed ends.
    Overlay,
}

/// How a protocol turn ended: a suspension that needs an answer from the
/// caller, or completion of the fed snippet.
#[derive(Debug)]
pub enum TurnEvent {
    /// The sandbox called an external function — answer with
    /// [`Checkout::resume`]. When `method_call` is true this is a dataclass
    /// method call and the instance is the first argument.
    FunctionCall {
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        method_call: bool,
    },
    /// The sandbox performed an OS operation no mount handled (e.g.
    /// `"Path.read_text"`) — answer with [`Checkout::resume`].
    OsCall {
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
        /// The exception the sandbox would raise if nothing handles this
        /// call; a caller with no handler should resume with
        /// `ResumeValue::Error(not_handled_error)`. `None` only for calls
        /// re-announced after [`Checkout::restore`].
        not_handled_error: Option<MontyException>,
    },
    /// The sandbox read an undefined name — answer with
    /// [`Checkout::resume_name_lookup`].
    NameLookup { name: String },
    /// Every sandbox task is blocked on external futures — answer with
    /// [`Checkout::resume_futures`].
    ResolveFutures { pending_call_ids: Vec<u32> },
    /// The fed snippet completed with this value; the session is ready for
    /// the next [`Checkout::feed`].
    Complete(MontyObject),
}

/// The caller's answer to a [`TurnEvent::FunctionCall`] or
/// [`TurnEvent::OsCall`].
#[derive(Debug)]
pub enum ResumeValue {
    /// The call returned this value.
    Return(MontyObject),
    /// The call raised this exception.
    Error(MontyException),
    /// The call is asynchronous: register an external future and continue
    /// other tasks; resolve later via [`Checkout::resume_futures`].
    Future,
    /// No handler exists for the called name — the sandbox raises
    /// `NameError`.
    NotFound,
}

/// Callback receiving sandbox `print()` output streamed during a turn.
pub type OnPrint<'a> = &'a mut dyn FnMut(PrintStream, &str);

/// One worker dedicated to one REPL session.
///
/// Obtained from [`crate::Pool::checkout`]. [`Checkout::finish`] returns the
/// worker to the pool; dropping without finishing kills the worker instead —
/// mid-execution state cannot be trusted back into the pool.
pub struct Checkout {
    /// `None` after `finish()` or after the worker was discarded on error.
    worker: Option<Worker>,
    pool: Arc<PoolInner>,
    /// The suspension awaiting an answer, when mid-feed.
    pending: Option<Pending>,
    /// The session's `max_duration` budget for the parent-side backstop, when
    /// configured. Set from the config on `create`; for `restore`d sessions it
    /// is adopted from the timing fields on the worker's first reply (the limits
    /// travel inside the opaque dump).
    duration_budget: Option<Duration>,
    /// Cumulative sandbox execution time as last reported by the worker —
    /// the child's clock is the single source of truth (it runs only while
    /// the interpreter executes, never during suspensions or between feeds,
    /// and survives dump/load). Monotonic max across turns so a compromised
    /// worker cannot rewind the parent's view of its consumed budget.
    reported_execution: Duration,
    /// The deadline armed for the most recent turn, surfaced by
    /// [`PoolError::Timeout`] when the watchdog fires.
    armed_deadline: Option<Duration>,
    /// The script name a `restore` adopted, captured from the worker's `Load`
    /// reply (the name travels inside the opaque dump, so the parent learns it
    /// only by the worker echoing it). Reset at the start of each `restore` and
    /// taken by `restore` to return; unset for non-restore turns.
    restored_script_name: Option<String>,
    /// Parent-side mount table for the in-flight feed, built from the
    /// [`MountSpec`]s passed to [`Checkout::feed`] / [`Checkout::restore`].
    /// Covered filesystem `OsCall` events are serviced from it inside
    /// [`Checkout::request_turn`] without surfacing to the caller. Dropped
    /// when the feed ends so overlay writes never leak into the next feed.
    feed_mounts: Option<MountTable>,
}

/// Which kind of suspension is awaiting an answer.
enum Pending {
    /// FunctionCall or OsCall; carries the call id and name (the name feeds
    /// `ResumeValue::NotFound`'s NameError).
    Call {
        call_id: u32,
        function_name: String,
    },
    NameLookup,
    Futures,
}

impl Checkout {
    /// Sends `Configure` on a fresh worker (the worker materializes the repl
    /// lazily on the first feed, or restores one via `load_snapshot` instead).
    pub(crate) fn create(worker: Worker, pool: Arc<PoolInner>, repl: &ReplConfig) -> Result<Self, PoolError> {
        let mut this = Self {
            worker: Some(worker),
            pool,
            pending: None,
            duration_budget: repl.limits.as_ref().and_then(|limits| limits.max_duration),
            reported_execution: Duration::ZERO,
            armed_deadline: None,
            restored_script_name: None,
            feed_mounts: None,
        };
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Configure(pb::Configure {
                script_name: repl.script_name.clone(),
                limits: repl.limits.as_ref().map(Into::into),
                type_check: repl.type_check,
                type_check_stubs: repl.type_check_stubs.clone(),
                assert_message_annotations: Some(repl.assert_message_annotations.max_bytes()),
                // This crate ships the matching `monty` binary, so our own
                // version is always what the child expects. The child rejects a
                // mismatch with a `FatalError` (relevant when a remote driver
                // built against a different version reuses the wire format).
                monty_version: MONTY_VERSION.to_owned(),
            })),
        };
        match this.request_turn(&request, this.pool.config.request_timeout, &mut |_, _| {})? {
            ControlEvent::Ok => Ok(this),
            other => Err(this.protocol_violation(&format!("unexpected reply to Configure: {other:?}"))),
        }
    }

    /// Restores a dumped session into this checkout's freshly configured (but
    /// not-yet-fed) worker, returning the re-announced suspension event when the
    /// dump was taken mid-feed (`None` for an idle, between-feeds dump).
    ///
    /// This is the low-level restore both `session.load` (idle dumps) and
    /// `session.load_snapshot` (suspended dumps) drive: the caller inspects the
    /// returned `Option` to tell which kind of dump it was and reject a
    /// mismatch. Only valid before the worker has been fed (the child rejects a
    /// `Load` once a repl exists).
    ///
    /// `mounts` re-establish a suspended feed's mounts, which are never part of
    /// the dump (they are host configuration the parent services itself). Pass
    /// the same mounts the original feed used — a mount silently omitted here
    /// degrades its filesystem calls into surfaced [`TurnEvent::OsCall`]s.
    /// The session's resource budget is taken from the dump, so the prior
    /// `Configure` limits are dropped here and re-adopted from the worker's
    /// reply.
    ///
    /// Returns the re-announced suspension (`Some` — a suspended dump) or `None`
    /// (an idle dump), paired with the worker's adopted script name (the dump's,
    /// not the `Configure` one), which the parent surfaces in restored snapshots.
    pub fn restore(
        &mut self,
        state: Vec<u8>,
        mounts: Vec<MountSpec>,
        on_print: OnPrint<'_>,
    ) -> Result<(Option<TurnEvent>, Option<String>), PoolError> {
        // the dump carries its own limits/consumed time/script name — forget
        // what the worker's Configure established and re-adopt from the reply
        self.pending = None;
        self.duration_budget = None;
        self.reported_execution = Duration::ZERO;
        self.restored_script_name = None;
        self.feed_mounts = build_mount_table(mounts)?;
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Load(pb::Load { state })),
        };
        let event = match self.request_turn(&request, self.pool.config.request_timeout, on_print)? {
            ControlEvent::Ok => None,
            ControlEvent::Turn(event) => Some(event),
            other @ ControlEvent::Dump(_) => {
                return Err(self.protocol_violation(&format!("unexpected reply to Load: {other:?}")));
            }
        };
        Ok((event, self.restored_script_name.take()))
    }

    /// Executes one snippet against the session. Inputs become sandbox
    /// globals; mounts apply to this feed only and are serviced by the parent
    /// (an invalid host path fails here, before any frame is sent, as a
    /// session-preserving [`PoolError::Runtime`]). Returns the first
    /// suspension (or completion); `print()` output streams to `on_print`
    /// throughout.
    ///
    /// # Errors
    /// [`PoolError::Runtime`] / [`PoolError::Typing`] leave the session
    /// usable; all other errors mean the worker was discarded.
    pub fn feed(
        &mut self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        mounts: Vec<MountSpec>,
        skip_type_check: bool,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        if self.pending.is_some() {
            return Err(PoolError::Protocol(
                "feed called while a suspension is awaiting an answer".to_owned(),
            ));
        }
        ensure_sendable(inputs.iter().map(|(_, value)| value))?;
        self.feed_mounts = build_mount_table(mounts)?;
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
                code: code.to_owned(),
                inputs: inputs
                    .into_iter()
                    .map(|(name, value)| pb::NamedValue {
                        name,
                        value: Some(value.into()),
                    })
                    .collect(),
                skip_type_check,
            })),
        };
        self.expect_turn(&request, on_print)
    }

    /// Answers a [`TurnEvent::FunctionCall`] or [`TurnEvent::OsCall`].
    pub fn resume(&mut self, value: ResumeValue, on_print: OnPrint<'_>) -> Result<TurnEvent, PoolError> {
        let Some(Pending::Call { call_id, function_name }) = &self.pending else {
            return Err(PoolError::Protocol("no suspended call to resume".to_owned()));
        };
        let (call_id, function_name) = (*call_id, function_name.clone());
        if let ResumeValue::Return(obj) = &value {
            ensure_sendable([obj])?;
        }
        let result = match value {
            ResumeValue::Return(obj) => pb::ext_function_result::Kind::ReturnValue(obj.into()),
            ResumeValue::Error(exc) => pb::ext_function_result::Kind::Error((&exc).into()),
            ResumeValue::Future => pb::ext_function_result::Kind::Future(call_id),
            ResumeValue::NotFound => pb::ext_function_result::Kind::NotFound(function_name),
        };
        self.pending = None;
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
                call_id,
                result: Some(pb::ExtFunctionResult { kind: Some(result) }),
            })),
        };
        self.expect_turn(&request, on_print)
    }

    /// Answers a [`TurnEvent::NameLookup`]: `Some(value)` resolves the name,
    /// `None` makes the sandbox raise `NameError`.
    pub fn resume_name_lookup(
        &mut self,
        value: Option<MontyObject>,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        if !matches!(self.pending, Some(Pending::NameLookup)) {
            return Err(PoolError::Protocol("no suspended name lookup to resume".to_owned()));
        }
        if let Some(obj) = &value {
            ensure_sendable([obj])?;
        }
        self.pending = None;
        let kind = match value {
            Some(obj) => pb::resume_name_lookup::Kind::Value(obj.into()),
            None => pb::resume_name_lookup::Kind::Undefined(pb::Unit {}),
        };
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
                kind: Some(kind),
            })),
        };
        self.expect_turn(&request, on_print)
    }

    /// Answers a [`TurnEvent::ResolveFutures`] with results for some or all
    /// pending call ids. Each result must be `Return` or `Error` — a future
    /// cannot resolve to another future or to "not found".
    pub fn resume_futures(
        &mut self,
        results: Vec<(u32, ResumeValue)>,
        on_print: OnPrint<'_>,
    ) -> Result<TurnEvent, PoolError> {
        if !matches!(self.pending, Some(Pending::Futures)) {
            return Err(PoolError::Protocol("no suspended futures to resume".to_owned()));
        }
        let results = results
            .into_iter()
            .map(|(call_id, value)| {
                if let ResumeValue::Return(obj) = &value {
                    ensure_sendable([obj])?;
                }
                let kind = match value {
                    ResumeValue::Return(obj) => pb::ext_function_result::Kind::ReturnValue(obj.into()),
                    ResumeValue::Error(exc) => pb::ext_function_result::Kind::Error((&exc).into()),
                    ResumeValue::Future | ResumeValue::NotFound => {
                        return Err(PoolError::Protocol(format!(
                            "future {call_id} must resolve to Return or Error"
                        )));
                    }
                };
                Ok(pb::FutureResult {
                    call_id,
                    result: Some(pb::ExtFunctionResult { kind: Some(kind) }),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.pending = None;
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::ResumeFutures(pb::ResumeFutures { results })),
        };
        self.expect_turn(&request, on_print)
    }

    /// Installs third-party Python packages into the session, making them
    /// importable by subsequent feeds. Session-scoped and repeatable; an empty
    /// `requirements` list is a no-op.
    ///
    /// Only an embedded-CPython worker honors this. The
    /// `monty` sandbox worker has no host interpreter to install for and a uv
    /// install failure both surface as [`PoolError::Runtime`] (the latter
    /// carrying uv's stderr); the session stays usable in either case. Bounded
    /// by the pool's `request_timeout`, so raise it for large dependency sets.
    ///
    /// Each requirement is validated here, at the pool boundary, before any
    /// frame is sent: a string that uv would parse as an option rather than a
    /// package specifier is rejected with [`PoolError::Runtime`] (a
    /// `ValueError`). See [`validate_requirement`] for the rationale.
    pub fn install_dependencies(&mut self, requirements: Vec<String>) -> Result<(), PoolError> {
        if self.pending.is_some() {
            return Err(PoolError::Protocol(
                "install_dependencies called while a suspension is awaiting an answer".to_owned(),
            ));
        }
        // Installing nothing trivially succeeds on any worker — including the
        // sandbox worker, which would otherwise reject the request outright.
        if requirements.is_empty() {
            return Ok(());
        }
        for requirement in &requirements {
            validate_requirement(requirement).map_err(invalid_requirement)?;
        }
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::InstallDependencies(pb::InstallDependencies {
                requirements,
            })),
        };
        match self.request_turn(&request, self.pool.config.request_timeout, &mut |_, _| {})? {
            ControlEvent::Ok => Ok(()),
            other => Err(self.protocol_violation(&format!("unexpected reply to InstallDependencies: {other:?}"))),
        }
    }

    /// Serializes the session (idle or suspended) into opaque bytes that
    /// [`Checkout::restore`] can restore — including into a
    /// different worker after this one crashes. The session stays live.
    pub fn dump(&mut self) -> Result<Vec<u8>, PoolError> {
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Dump(pb::Dump {})),
        };
        match self.request_turn(&request, self.pool.config.request_timeout, &mut |_, _| {})? {
            ControlEvent::Dump(state) => Ok(state),
            other => Err(self.protocol_violation(&format!("unexpected reply to Dump: {other:?}"))),
        }
    }

    /// Ends the session and returns the worker to the pool.
    ///
    /// Consumes the checkout. On error the worker is discarded (and the
    /// error reported), but the pool remains healthy either way.
    pub fn finish(mut self) -> Result<(), PoolError> {
        // A websocket worker is single-use — the pool discards it after every
        // checkout — so there is no point round-tripping a `Reset` to ready it
        // for reuse. Dropping it closes the socket, which the child reads as a
        // clean EOF and exits. Only subprocess workers are reset and returned to
        // the idle pool for the next checkout.
        if self.pool.config.transport.is_websocket() {
            if let Some(worker) = self.worker.take() {
                self.pool.release_worker(worker);
            }
            return Ok(());
        }
        let request = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Reset(pb::Reset {})),
        };
        match self.request_turn(&request, self.pool.config.request_timeout, &mut |_, _| {})? {
            ControlEvent::Ok => {
                if let Some(mut worker) = self.worker.take() {
                    worker.checkouts_served += 1;
                    self.pool.release_worker(worker);
                }
                Ok(())
            }
            other => Err(self.protocol_violation(&format!("unexpected reply to Reset: {other:?}"))),
        }
    }

    /// OS process id of the worker, when it is a local subprocess (`None` for a
    /// remote WebSocket worker, or a finished checkout). Diagnostics/tests.
    pub fn pid(&self) -> Option<u32> {
        self.worker.as_ref().and_then(Worker::pid)
    }

    /// Sends a request and requires the reply to be a [`TurnEvent`].
    ///
    /// This is the entry point for *execution* turns (feed/resume — the
    /// turns where the sandbox runs code), so the watchdog deadline includes
    /// [`Self::backstop_deadline`] on top of the configured request timeout.
    fn expect_turn(&mut self, request: &pb::ParentRequest, on_print: OnPrint<'_>) -> Result<TurnEvent, PoolError> {
        let deadline = min_deadline(self.pool.config.request_timeout, self.backstop_deadline());
        match self.request_turn(request, deadline, on_print)? {
            ControlEvent::Turn(event) => Ok(event),
            other => Err(self.protocol_violation(&format!("expected a turn event, got {other:?}"))),
        }
    }

    /// Parent-side kill deadline derived from the session's `max_duration`:
    /// the execution budget remaining after the time the worker has reported
    /// consuming so far, plus the configured grace. The child enforces the
    /// limit itself with a clean `TimeoutError`; this deadline only fires
    /// when that enforcement fails (e.g. a blocking syscall inside a mount
    /// that the sandbox's periodic time check never reaches).
    fn backstop_deadline(&self) -> Option<Duration> {
        let budget = self.duration_budget?;
        let grace = self.pool.config.duration_limit_grace?;
        Some(budget.saturating_sub(self.reported_execution) + grace)
    }

    /// Adopts the timing fields the worker stamps onto every turn-ending
    /// event. The reported total only ever ratchets up — a compromised worker
    /// must not rewind the parent's view of its consumed budget (it can still
    /// under-report, but each turn stays bounded by `budget + grace`). The
    /// budget itself is only adopted when the parent doesn't already know it,
    /// i.e. after [`Checkout::restore`].
    fn note_reported_time(&mut self, event: &pb::ChildEvent) {
        self.reported_execution = self
            .reported_execution
            .max(Duration::from_micros(event.total_execution_micros));
        if self.duration_budget.is_none() {
            self.duration_budget = event.max_duration_micros.map(Duration::from_micros);
        }
    }

    /// The core protocol turn: send one request, stream prints, classify the
    /// turn-ending event. The watchdog kills the worker if the turn outlives
    /// `deadline`. All failure paths discard the worker except `Runtime` /
    /// `Typing`, which are sandbox-level outcomes.
    fn request_turn(
        &mut self,
        request: &pb::ParentRequest,
        deadline: Option<Duration>,
        on_print: OnPrint<'_>,
    ) -> Result<ControlEvent, PoolError> {
        let Some(worker) = self.worker.as_mut() else {
            return Err(PoolError::Finished);
        };
        // Scope the watchdog's sticky kill flag to this turn's deadline so a
        // kill from a previous turn cannot misclassify this one (see
        // `Worker::reset_killed_for_timeout`).
        worker.reset_killed_for_timeout();
        self.armed_deadline = deadline;
        // The turn's total worker-time allowance: each mount exchange consumes
        // the interval the worker just ran, so a stream of covered OS calls
        // cannot extend one turn beyond `deadline` of cumulative execution.
        let mut remaining = deadline;
        let mut armed_at = Instant::now();
        let mut deadline_guard = self.pool.watchdog.arm(worker, deadline);

        if let Err(err) = worker.send(request) {
            // `write_frame` rejects an oversize frame *before* writing any
            // bytes, so the worker never saw the request and is still synced —
            // surface a clean, catchable error instead of discarding a healthy
            // worker as if it had crashed. Every other send failure is a real
            // I/O break (dead worker / closed pipe).
            return Err(match err {
                FrameError::FrameTooLarge { len, max } => PoolError::Runtime(MontyException::new(
                    ExcType::RuntimeError,
                    Some(format!(
                        "request frame of {len} bytes exceeds the maximum of {max} bytes"
                    )),
                )),
                _ => self.poison("sending a request"),
            });
        }
        let outcome = loop {
            let event = match self.worker.as_mut().expect("checked above").recv() {
                Ok(event) => event,
                // a decode failure means the frame arrived intact but its
                // payload was garbage (including values that fail semantic
                // validation, which happens during decode) — the worker
                // misbehaved, it didn't die
                Err(FrameError::Decode(err)) => {
                    return Err(self.protocol_violation(&format!("invalid payload from worker: {err}")));
                }
                Err(_) => return Err(self.poison("waiting for a reply")),
            };
            // Print events carry no timing (the fields are zero), so this is
            // a no-op for them thanks to the monotonic-max ratchet.
            self.note_reported_time(&event);
            // Only a `Load` reply carries this; it lets `restore` report the
            // dump's script name without parsing the opaque dump bytes.
            if let Some(name) = &event.restored_script_name {
                self.restored_script_name = Some(name.clone());
            }
            match event.kind {
                Some(pb::child_event::Kind::Print(print)) => {
                    let stream = match print.stream() {
                        pb::PrintStream::Stderr => PrintStream::Stderr,
                        pb::PrintStream::Stdout | pb::PrintStream::Unspecified => PrintStream::Stdout,
                    };
                    on_print(stream, &print.text);
                }
                Some(pb::child_event::Kind::FunctionCall(call)) => {
                    self.pending = Some(Pending::Call {
                        call_id: call.call_id,
                        function_name: call.function_name.clone(),
                    });
                    break self.convert_turn(|| {
                        Ok(TurnEvent::FunctionCall {
                            function_name: call.function_name,
                            args: call.args,
                            kwargs: call.kwargs,
                            call_id: call.call_id,
                            method_call: call.method_call,
                        })
                    });
                }
                Some(pb::child_event::Kind::OsCall(call)) => {
                    // Parent-side mounts: service covered filesystem calls
                    // here and resume the child directly — the caller never
                    // sees them (mirroring `Print` handling). The watchdog is
                    // disarmed around the local I/O (the child is idle,
                    // blocked on our reply — a slow host filesystem must not
                    // count against its deadline). Each exchange deducts the
                    // worker's just-elapsed interval from `remaining`, so
                    // issuing covered calls cannot reset the deadline: total
                    // worker execution per turn stays bounded by `deadline`.
                    //
                    // Deliberate trade-off: the host I/O window itself is
                    // deducted from nothing and runs with no watchdog, so a
                    // feed's *wall clock* is not bounded by the deadline — a
                    // stalled filesystem blocks it indefinitely, and a loop
                    // of covered calls gets its (per-call budget-bounded)
                    // host I/O for free. Only worker execution is a hard
                    // bound; see "Mount I/O is not covered by
                    // `request_timeout`" in limitations/pool-architecture.md.
                    deadline_guard = None;
                    remaining = remaining.map(|allowance| allowance.saturating_sub(armed_at.elapsed()));
                    let call_id = call.call_id;
                    // `Consumed` re-announcements (after `restore`) carry no
                    // payload and always surface — the caller re-learns the
                    // call from its own records. Everything else decodes into
                    // a typed `OsFunctionCall`; a payload the child could
                    // never legitimately produce is a protocol violation.
                    let function_call = match call.call {
                        None => break Err(self.protocol_violation("OsCall event with no call")),
                        Some(pb::os_call::Call::Consumed(_)) => None,
                        Some(kind) => match OsFunctionCall::try_from(kind) {
                            Ok(function_call) => Some(function_call),
                            Err(err) => {
                                break Err(self.protocol_violation(&format!("invalid OS call payload: {err}")));
                            }
                        },
                    };
                    let unhandled = match function_call {
                        Some(function_call) => match self.try_mount_call(function_call) {
                            MountCallOutcome::Handled(result) => {
                                let wire_result = match result {
                                    Ok(obj) => pb::ext_function_result::Kind::ReturnValue(obj.into()),
                                    Err(err) => pb::ext_function_result::Kind::Error((&err.into_exception()).into()),
                                };
                                // `armed_deadline` (the `Timeout` error's reported
                                // value) stays at the turn deadline: the allowance
                                // makes that the bound the whole turn is held to.
                                let next_deadline = min_deadline(remaining, self.backstop_deadline());
                                let worker = self.worker.as_mut().expect("checked above");
                                armed_at = Instant::now();
                                deadline_guard = self.pool.watchdog.arm(worker, next_deadline);
                                if let Err(err) = send_internal_resume(worker, call_id, wire_result) {
                                    // an oversize result frame is rejected before any
                                    // bytes are written, so the stream is intact and
                                    // the suspended child can be resumed with a small,
                                    // catchable error instead; anything else is a real
                                    // I/O break
                                    let FrameError::FrameTooLarge { len, max } = err else {
                                        break Err(self.poison("resuming a mount-covered OS call"));
                                    };
                                    let exc = MontyException::new(
                                        ExcType::RuntimeError,
                                        Some(format!(
                                            "OS call result frame of {len} bytes exceeds the maximum of {max} bytes"
                                        )),
                                    );
                                    let error_result = pb::ext_function_result::Kind::Error((&exc).into());
                                    if send_internal_resume(worker, call_id, error_result).is_err() {
                                        break Err(self.poison("resuming a mount-covered OS call"));
                                    }
                                }
                                // serviced internally — wait for the child's
                                // next event
                                continue;
                            }
                            MountCallOutcome::NotHandled(function_call) => Some(function_call),
                        },
                        None => None,
                    };
                    // Not mount-covered: surface to the caller in the
                    // `(name, args, kwargs)` host-callback shape, with the
                    // parent-computed no-handler error. A consumed
                    // re-announcement surfaces with an empty name and no
                    // error — the caller re-learns the call from its records.
                    let (function_name, args, kwargs, not_handled_error) = match unhandled {
                        Some(function_call) => {
                            let not_handled_error = function_call.on_no_handler();
                            let function_name = function_call.name().to_owned();
                            let (args, kwargs) = function_call.to_args();
                            (function_name, args, kwargs, Some(not_handled_error))
                        }
                        None => (String::new(), vec![], vec![], None),
                    };
                    self.pending = Some(Pending::Call {
                        call_id,
                        function_name: function_name.clone(),
                    });
                    break Ok(ControlEvent::Turn(TurnEvent::OsCall {
                        function_name,
                        args,
                        kwargs,
                        call_id,
                        not_handled_error,
                    }));
                }
                Some(pb::child_event::Kind::NameLookup(lookup)) => {
                    self.pending = Some(Pending::NameLookup);
                    break Ok(ControlEvent::Turn(TurnEvent::NameLookup { name: lookup.name }));
                }
                Some(pb::child_event::Kind::ResolveFutures(futures)) => {
                    self.pending = Some(Pending::Futures);
                    break Ok(ControlEvent::Turn(TurnEvent::ResolveFutures {
                        pending_call_ids: futures.pending_call_ids,
                    }));
                }
                Some(pb::child_event::Kind::Complete(complete)) => {
                    self.pending = None;
                    // the feed is over — drop its mounts so overlay writes
                    // cannot leak into the next feed
                    self.feed_mounts = None;
                    break self.convert_turn(|| {
                        let value = complete
                            .value
                            .ok_or(monty_proto::ProtoConvertError::MissingField("Complete.value"))?;
                        Ok(TurnEvent::Complete(value.into_object()?))
                    });
                }
                Some(pb::child_event::Kind::Error(error)) => {
                    self.pending = None;
                    self.feed_mounts = None;
                    let Some(exception) = error.exception else {
                        return Err(self.protocol_violation("error event with no exception"));
                    };
                    break match MontyException::try_from(exception) {
                        Ok(exc) => Err(PoolError::Runtime(exc)),
                        Err(err) => Err(self.protocol_violation(&format!("invalid exception payload: {err}"))),
                    };
                }
                Some(pb::child_event::Kind::TypingError(typing)) => {
                    self.pending = None;
                    self.feed_mounts = None;
                    break Err(PoolError::Typing(typing.diagnostics));
                }
                Some(pb::child_event::Kind::Ok(_)) => break Ok(ControlEvent::Ok),
                Some(pb::child_event::Kind::DumpResult(dump)) => break Ok(ControlEvent::Dump(dump.state)),
                Some(pb::child_event::Kind::FatalError(fatal)) => {
                    self.discard_worker();
                    break Err(PoolError::Protocol(format!(
                        "worker reported fatal error: {}",
                        fatal.message
                    )));
                }
                None => {
                    return Err(self.protocol_violation("unexpected event"));
                }
            }
        };
        self.finish_request_turn(deadline_guard, outcome)
    }

    /// Disarms the watchdog, then guards the narrow race where the deadline
    /// fired between reading the turn-ending event and removing the watchdog
    /// entry: if it did, the worker is already dead, so the apparent success
    /// is reported as this turn's timeout instead of handing back a dead
    /// worker.
    fn finish_request_turn(
        &mut self,
        deadline_guard: Option<DeadlineGuard>,
        outcome: Result<ControlEvent, PoolError>,
    ) -> Result<ControlEvent, PoolError> {
        drop(deadline_guard);
        if self.worker.as_ref().is_some_and(Worker::was_killed_for_timeout) {
            Err(self.poison("waiting for a reply"))
        } else {
            outcome
        }
    }

    /// Routes a decoded OS call to this feed's parent-side mount table,
    /// performing any covered host I/O here. The call is *moved* in so a
    /// covered write's payload reaches overlay storage without a copy;
    /// `NotHandled` hands it back for surfacing to the caller. With no
    /// mounts this feed, every call comes back `NotHandled`. Path
    /// containment inside covered calls is enforced by the `MountTable`.
    fn try_mount_call(&mut self, call: OsFunctionCall) -> MountCallOutcome {
        match self.feed_mounts.as_mut() {
            Some(mounts) => mounts.handle_os_call(call),
            None => MountCallOutcome::NotHandled(call),
        }
    }

    /// Runs a fallible payload conversion; conversion failures mean the
    /// worker sent garbage, which discards it.
    fn convert_turn(
        &mut self,
        convert: impl FnOnce() -> Result<TurnEvent, monty_proto::ProtoConvertError>,
    ) -> Result<ControlEvent, PoolError> {
        match convert() {
            Ok(event) => Ok(ControlEvent::Turn(event)),
            Err(err) => Err(self.protocol_violation(&format!("invalid payload from worker: {err}"))),
        }
    }

    /// Discards the worker after it violated the protocol on an intact stream
    /// (unexpected event kind, undecodable payload). Unlike [`Self::poison`]
    /// this is not a crash — the worker answered, just wrongly — so it maps
    /// to [`PoolError::Protocol`] rather than `Crashed`/`Timeout`.
    fn protocol_violation(&mut self, context: &str) -> PoolError {
        self.discard_worker();
        PoolError::Protocol(context.to_owned())
    }

    /// Discards the worker after an I/O failure and classifies it as a
    /// watchdog timeout or a crash.
    fn poison(&mut self, context: &str) -> PoolError {
        let Some(mut worker) = self.worker.take() else {
            return PoolError::Finished;
        };
        self.pending = None;
        self.feed_mounts = None;
        let timed_out = worker.was_killed_for_timeout();
        let status = worker.kill_and_reap();
        drop(worker);
        self.pool.release_capacity();
        if timed_out {
            PoolError::Timeout {
                timeout: self.armed_deadline.unwrap_or(Duration::ZERO),
            }
        } else {
            PoolError::Crashed {
                status,
                context: context.to_owned(),
            }
        }
    }

    /// Discards the worker without crash classification (fatal-error frames
    /// arrive on an intact stream, so this is a protocol failure, not a
    /// crash).
    fn discard_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            drop(worker);
            self.pool.release_capacity();
        }
        self.pending = None;
        self.feed_mounts = None;
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        // a checkout abandoned mid-session cannot be trusted back into the
        // pool: kill the worker and free its capacity
        if let Some(worker) = self.worker.take() {
            drop(worker);
            self.pool.release_capacity();
        }
    }
}

/// Internal classification of a turn-ending event: real turn events for the
/// caller, plus the control acks (`Ok` / `DumpResult`) used by the checkout
/// lifecycle itself.
#[derive(Debug)]
enum ControlEvent {
    Turn(TurnEvent),
    Ok,
    Dump(Vec<u8>),
}

/// Rejects values too deeply nested for the wire (see
/// `monty_proto::MAX_VALUE_DEPTH`) with a session-preserving runtime error —
/// sending them would produce a frame the worker cannot decode.
fn ensure_sendable<'a>(values: impl IntoIterator<Item = &'a MontyObject>) -> Result<(), PoolError> {
    if values.into_iter().any(exceeds_max_value_depth) {
        Err(PoolError::Runtime(MontyException::new(
            ExcType::RuntimeError,
            Some("Max input depth exceeded".to_owned()),
        )))
    } else {
        Ok(())
    }
}

/// Converts a shared requirement-validation failure into a session-preserving
/// Python `ValueError`.
fn invalid_requirement(message: String) -> PoolError {
    PoolError::Runtime(MontyException::new(ExcType::ValueError, Some(message)))
}

/// The tighter of two optional deadlines (`None` means no deadline).
fn min_deadline(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (deadline, None) | (None, deadline) => deadline,
    }
}

/// Builds the parent-side [`MountTable`] for one feed from its specs;
/// `Ok(None)` when `mounts` is empty (every OS call then surfaces to the
/// caller). An invalid mount (host path missing, not a directory, relative
/// virtual path, …) fails as a session-preserving [`PoolError::Runtime`] —
/// callers invoke this before sending any frame, so the worker never sees a
/// half-configured feed.
fn build_mount_table(mounts: Vec<MountSpec>) -> Result<Option<MountTable>, PoolError> {
    if mounts.is_empty() {
        return Ok(None);
    }
    let mut table = MountTable::new();
    for mount in mounts {
        let mode = match mount.mode {
            MountSpecMode::ReadOnly => MountMode::ReadOnly,
            MountSpecMode::ReadWrite => MountMode::ReadWrite,
            // Overlay state is created fresh per feed: writes live only as
            // long as the feed and are discarded with it.
            MountSpecMode::Overlay => MountMode::OverlayMemory(OverlayState::new()),
        };
        let mount = monty_fs::Mount::new(&mount.virtual_path, &mount.host_path, mode, mount.write_bytes_limit)
            .map_err(|err| PoolError::Runtime(err.into_exception()))?
            .with_memory_usage_limit(mount.memory_usage_limit);
        table.push_mount(mount);
    }
    Ok(Some(table))
}

/// Sends the `ResumeCall` answering a mount-serviced OS call. Free function
/// (not a method) so [`Checkout::request_turn`] can call it while the worker
/// is already mutably borrowed.
fn send_internal_resume(
    worker: &mut Worker,
    call_id: u32,
    result: pb::ext_function_result::Kind,
) -> Result<(), FrameError> {
    worker.send(&pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id,
            result: Some(pb::ExtFunctionResult { kind: Some(result) }),
        })),
    })
}
